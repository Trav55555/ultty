//! Explicit, bounded verification against a synthetic daemon fixture.

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::connection::{self, Connection, RunSnapshot};
use crate::native_stream::{self, Control, Event, Submission};
use crate::Msg;

/// Verify denial, cancellation, and read-only reconnect through real transports.
///
/// This command requires explicit fixture consent and connection settings. It
/// emits one JSON receipt only after every gate passes, without credentials.
pub(crate) fn run() -> Result<(), String> {
    if std::env::var("PORYGON_CONTRACT_FIXTURE").as_deref() != Ok("1") {
        return Err("native contract verification requires PORYGON_CONTRACT_FIXTURE=1".into());
    }
    let url = std::env::var("PORYGON_URL")
        .map_err(|_| "native contract verification requires explicit PORYGON_URL".to_string())?;
    let token = std::env::var("PORYGON_TOKEN")
        .map_err(|_| "native contract verification requires explicit PORYGON_TOKEN".to_string())?;
    let connection = Connection::from_values(&url, &token)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not create contract verification runtime: {error}"))?;
    let receipt = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), verify(connection, &url, &token))
            .await
            .map_err(|_| "native contract verification exceeded its 30-second limit".to_string())?
    })?;
    println!("{receipt}");
    Ok(())
}

async fn verify(connection: Connection, url: &str, token: &str) -> Result<Value, String> {
    connection.check().await?;
    let approval_run_id = connection::new_run_id()?;
    let (controls, receiver) = mpsc::channel(8);
    let mut stream = Box::pin(native_stream::stream(
        submission(&connection, &approval_run_id, "approval"),
        receiver,
    ));
    let mut approval_id = None;
    let mut decision_sent = false;
    let mut response = String::new();
    loop {
        match next_event(&mut stream, &approval_run_id).await? {
            Event::Approval { id, .. } => {
                if approval_id.is_some() {
                    return Err("fixture requested more than one approval".into());
                }
                controls
                    .send(Control::Approval {
                        id: id.clone(),
                        approved: false,
                    })
                    .await
                    .map_err(|_| "fixture approval channel closed before denial".to_string())?;
                approval_id = Some(id);
            }
            Event::DecisionSent(id) => {
                if approval_id.as_deref() != Some(&id) {
                    return Err("fixture denial was sent for a different approval".into());
                }
                decision_sent = true;
            }
            Event::Token(text) => {
                response.push_str(&text);
            }
            Event::Done(_) => break,
            Event::Reset => return Err("fixture reset the denial response".into()),
            _ => {}
        }
    }
    if !decision_sent || response != "beforedenied" {
        return Err("fixture did not confirm denial with 'denied' before Done".into());
    }
    drop(stream);
    drop(controls);
    let approval_run = connection.fetch_run(&approval_run_id).await?;
    if approval_run.status != "completed" || approval_run.result.as_deref() != Some("beforedenied")
    {
        return Err("HTTP run inspection did not confirm the completed denial result".into());
    }

    let cancel_run_id = connection::new_run_id()?;
    let (controls, receiver) = mpsc::channel(8);
    let mut stream = Box::pin(native_stream::stream(
        submission(&connection, &cancel_run_id, "wait"),
        receiver,
    ));
    loop {
        match next_event(&mut stream, &cancel_run_id).await? {
            Event::Token(text) if !text.is_empty() => break,
            Event::Done(_) => return Err("wait fixture completed before cancellation".into()),
            Event::Approval { .. } => {
                return Err("wait fixture unexpectedly requested approval".into())
            }
            _ => {}
        }
    }
    // Submit cancellation once. Its response can be uncertain; only GET polls
    // follow it, and success requires the durable run to become cancelled.
    let cancellation = connection.cancel_run(&cancel_run_id).await;
    let cancelled = observe_cancelled(&connection, &cancel_run_id, cancellation).await?;
    drop(stream);
    drop(controls);

    let reconnected = Connection::from_values(url, token)?;
    let observed = reconnected.fetch_run(&cancel_run_id).await?;
    if observed.status != "cancelled" {
        return Err("read-only reconnect did not preserve the cancelled run disposition".into());
    }
    Ok(json!({
        "approval_run_id": approval_run_id,
        "approval_id": approval_id,
        "decision": "denied",
        "approval_run_status": approval_run.status,
        "cancel_run_id": cancel_run_id,
        "cancel_run_status": cancelled.status,
        "reconnected_run_status": observed.status,
    }))
}

fn submission(connection: &Connection, run_id: &str, message: &str) -> Submission {
    Submission {
        connection: connection.clone(),
        epoch: 0,
        run_id: run_id.to_string(),
        conversation_id: None,
        message: message.to_string(),
    }
}

async fn next_event<S: Stream<Item = Msg> + Unpin>(
    stream: &mut S,
    run_id: &str,
) -> Result<Event, String> {
    match stream.next().await {
        Some(Msg::NativeEvent(id, 0, event)) if id == run_id => match event {
            Event::Error(error) | Event::NotSubmitted { error, .. } => Err(error),
            Event::Disconnected => Err("fixture stream disconnected before confirmation".into()),
            event => Ok(event),
        },
        Some(_) => Err("fixture stream delivered an event for a different run".into()),
        None => Err("fixture stream ended before confirmation".into()),
    }
}

async fn observe_cancelled(
    connection: &Connection,
    run_id: &str,
    initial: Result<RunSnapshot, String>,
) -> Result<RunSnapshot, String> {
    let mut snapshot = initial.ok();
    for _ in 0..20 {
        if let Some(run) = snapshot.take() {
            match run.status.as_str() {
                "cancelled" => return Ok(run),
                "completed" | "failed" | "insufficient_oracle" => {
                    return Err(format!(
                        "wait fixture ended as {} instead of cancelled",
                        run.status
                    ));
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        snapshot = Some(connection.fetch_run(run_id).await?);
    }
    if let Some(run) = snapshot.filter(|run| run.status == "cancelled") {
        return Ok(run);
    }
    Err("cancellation was not confirmed within 20 read-only observations".into())
}
