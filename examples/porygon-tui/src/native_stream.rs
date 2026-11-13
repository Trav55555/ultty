//! One submission per socket. Transport loss never resubmits the user's work.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::connection::Connection;
use crate::Msg;

#[derive(Debug)]
pub(crate) enum Control {
    Approval { id: String, approved: bool },
    Cancel,
}

#[derive(Debug)]
pub(crate) enum Event {
    Submitted,
    NotSubmitted {
        message: String,
        error: String,
    },
    Token(String),
    Reset,
    Approval {
        id: String,
        tool: String,
        arguments: Value,
    },
    DecisionSent(String),
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        id: String,
        result: String,
        success: bool,
    },
    Done(Option<String>),
    Error(String),
    Disconnected,
}

pub(crate) struct Submission {
    pub connection: Connection,
    pub epoch: u64,
    pub run_id: String,
    pub conversation_id: Option<String>,
    pub message: String,
}

pub(crate) fn stream(
    submission: Submission,
    controls: mpsc::Receiver<Control>,
) -> impl futures_util::Stream<Item = Msg> + Send + 'static {
    // Start lazily so constructing an application command needs no Tokio runtime.
    futures_util::stream::unfold(
        (Some((submission, controls)), None::<mpsc::Receiver<Msg>>),
        |(start, mut receiver)| async move {
            if let Some((submission, controls)) = start {
                let (sender, incoming) = mpsc::channel(64);
                receiver = Some(incoming);
                tokio::spawn(async move {
                    let run_id = submission.run_id.clone();
                    let epoch = submission.epoch;
                    let result = drive(submission, controls, &sender).await;
                    if let Err(error) = result {
                        let _ = sender
                            .send(Msg::NativeEvent(run_id.clone(), epoch, Event::Error(error)))
                            .await;
                    }
                    let _ = sender
                        .send(Msg::NativeEvent(run_id, epoch, Event::Disconnected))
                        .await;
                });
            }
            let mut receiver = receiver?;
            receiver
                .recv()
                .await
                .map(|message| (message, (None, Some(receiver))))
        },
    )
}

async fn drive(
    submission: Submission,
    mut controls: mpsc::Receiver<Control>,
    sender: &mpsc::Sender<Msg>,
) -> Result<(), String> {
    let epoch = submission.epoch;
    let url = submission.connection.stream_url()?;
    let connect = tokio_tungstenite::connect_async(url.as_str());
    let connection_result = tokio::select! {
        biased;
        _ = controls.recv() => {
            let _ = sender.send(Msg::NativeEvent(submission.run_id, epoch, Event::NotSubmitted { message:submission.message, error:"Stopped before submission; draft preserved.".into() })).await;
            return Ok(());
        }
        result = tokio::time::timeout(Duration::from_secs(5), connect) => result,
    };
    let (mut socket, _) = match connection_result {
        Ok(Ok(socket)) => socket,
        _ => {
            let _ = sender
                .send(Msg::NativeEvent(
                    submission.run_id,
                    epoch,
                    Event::NotSubmitted {
                        message: submission.message,
                        error: "Could not connect. Nothing was submitted; draft preserved.".into(),
                    },
                ))
                .await;
            return Ok(());
        }
    };
    if matches!(controls.try_recv(), Ok(Control::Cancel)) {
        let _ = sender
            .send(Msg::NativeEvent(
                submission.run_id,
                epoch,
                Event::NotSubmitted {
                    message: submission.message,
                    error: "Stopped before submission; draft preserved.".into(),
                },
            ))
            .await;
        return Ok(());
    }
    let mut payload = json!({
        "auth": submission.connection.auth_token,
        "message": submission.message,
        "run_id": submission.run_id,
        "surface": "tui",
    });
    if let Some(id) = submission.conversation_id {
        payload["conversation_id"] = Value::String(id);
    }
    socket
        .send(Message::Text(payload.to_string()))
        .await
        .map_err(|_| {
            "Submission delivery is uncertain. Inspect this run before starting more work."
                .to_string()
        })?;
    let run_id = submission.run_id;
    sender
        .send(Msg::NativeEvent(run_id.clone(), epoch, Event::Submitted))
        .await
        .map_err(|_| "Client closed".to_string())?;
    loop {
        tokio::select! {
            _ = sender.closed() => return Ok(()),
            control = controls.recv() => {
                let Some(control) = control else { return Ok(()); };
                let (payload, decision) = match control {
                    Control::Approval { id, approved } => (
                        json!({"type":"approval_response", "approval_id":id, "approved":approved}),
                        Some(id),
                    ),
                    Control::Cancel => (json!({"type":"cancel"}), None),
                };
                socket.send(Message::Text(payload.to_string())).await
                    .map_err(|_| "Control delivery is uncertain; reconnect to inspect the run.".to_string())?;
                if let Some(id) = decision {
                    if sender.send(Msg::NativeEvent(run_id.clone(), epoch, Event::DecisionSent(id))).await.is_err() {
                        return Ok(());
                    }
                }
            }
            frame = socket.next() => {
                let Some(frame) = frame else { return Ok(()); };
                match frame.map_err(|_| "Stream disconnected. The prompt was not resent.".to_string())? {
                    Message::Close(_) => return Ok(()),
                    Message::Ping(bytes) => {
                        socket.send(Message::Pong(bytes)).await.map_err(|_| "Stream disconnected".to_string())?;
                    }
                    Message::Text(text) => {
                        let event = decode(&text)?;
                        if let Some(event) = event {
                            let done = matches!(event, Event::Done(_));
                            if sender.send(Msg::NativeEvent(run_id.clone(), epoch, event)).await.is_err() || done {
                                return Ok(());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn decode(text: &str) -> Result<Option<Event>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| "Invalid daemon event".to_string())?;
    let field = |key: &str| value[key].as_str().unwrap_or_default().to_string();
    Ok(match value["type"].as_str() {
        Some("token") => Some(Event::Token(field("text"))),
        Some("stream_reset") => Some(Event::Reset),
        Some("approval_required") => {
            let id = field("approval_id");
            if id.is_empty() {
                return Err("Approval event has no identity".into());
            }
            Some(Event::Approval {
                id,
                tool: field("tool_name"),
                arguments: value["arguments"].clone(),
            })
        }
        Some("tool_call") => Some(Event::ToolCall {
            id: field("call_id"),
            name: field("name"),
            arguments: value["arguments"].clone(),
        }),
        Some("tool_result") => Some(Event::ToolResult {
            id: field("call_id"),
            result: value["result"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value["result"].to_string()),
            success: value["success"].as_bool().unwrap_or(false),
        }),
        Some("done") => Some(Event::Done(
            value["conversation_id"].as_str().map(str::to_string),
        )),
        Some("error") => Some(Event::Error(field("message"))),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_current_daemon_token_and_reset() {
        assert!(
            matches!(decode(r#"{"type":"token","text":"hello"}"#).unwrap(), Some(Event::Token(t)) if t == "hello")
        );
        assert!(matches!(
            decode(r#"{"type":"stream_reset"}"#).unwrap(),
            Some(Event::Reset)
        ));
        assert!(decode(r#"{"type":"future_event"}"#).unwrap().is_none());
    }

    #[tokio::test]
    async fn real_socket_sends_auth_identity_and_approval_without_retry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
            let request: Value =
                serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(request["auth"], "fixture-token");
            assert_eq!(request["message"], "hello");
            assert_eq!(request["run_id"], "fixture-run");
            assert!(request.get("prompt").is_none());
            ws.send(Message::Text(json!({"type":"approval_required","approval_id":"a1","tool_name":"shell","arguments":{"command":"true"}}).to_string())).await.unwrap();
            let response: Value =
                serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(
                response,
                json!({"type":"approval_response","approval_id":"a1","approved":false})
            );
            ws.close(None).await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let (tx, rx) = mpsc::channel(4);
        let connection =
            Connection::from_values(&format!("http://{address}"), "fixture-token").unwrap();
        let mut events = Box::pin(stream(
            Submission {
                epoch: 0,
                connection,
                run_id: "fixture-run".into(),
                conversation_id: None,
                message: "hello".into(),
            },
            rx,
        ));
        let mut decision_sent = false;
        while let Some(message) = tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
        {
            match message {
                Msg::NativeEvent(_, 0, Event::Approval { id, .. }) => tx
                    .send(Control::Approval {
                        id,
                        approved: false,
                    })
                    .await
                    .unwrap(),
                Msg::NativeEvent(_, 0, Event::DecisionSent(_)) => decision_sent = true,
                _ => {}
            }
        }
        assert!(decision_sent);
        server.await.unwrap();
    }
}
