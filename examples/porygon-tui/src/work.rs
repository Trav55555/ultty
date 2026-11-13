use serde::Deserialize;
use serde_json::Value;

use crate::approval_client::PendingApproval;
use crate::connection::Connection;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorkRun {
    pub id: String,
    pub objective: String,
    pub status: String,
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Runs {
    runs: Vec<WorkRun>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Report {
    pub run_id: String,
    pub report_type: String,
    pub passed: bool,
    pub summary: String,
    pub checks: Vec<Value>,
    pub artifacts: Value,
}

#[derive(Deserialize)]
struct Reports {
    verification_reports: Vec<Report>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Closed,
    Work,
    Attention,
    Detail,
}

pub(crate) struct WorkState {
    pub page: Page,
    pub runs: Vec<WorkRun>,
    pub approvals: Vec<PendingApproval>,
    pub selected: usize,
    pub detail_scroll: f32,
    pub detail_id: Option<String>,
    pub reports: Vec<Report>,
    pub error: Option<String>,
    pub loading: bool,
}
impl Default for WorkState {
    fn default() -> Self {
        Self {
            page: Page::Closed,
            runs: Vec::new(),
            approvals: Vec::new(),
            selected: 0,
            detail_scroll: 0.0,
            detail_id: None,
            reports: Vec::new(),
            error: None,
            loading: false,
        }
    }
}

fn endpoint(connection: &Connection, segments: &[&str]) -> Result<reqwest::Url, String> {
    let mut url = connection.base_url.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| "Invalid daemon URL".to_string())?;
        path.pop_if_empty();
        path.extend(segments.iter().copied());
    }
    Ok(url)
}

pub(crate) async fn runs(connection: Connection) -> Result<Vec<WorkRun>, String> {
    let url = endpoint(&connection, &["runs"])?;
    let response = connection
        .client
        .get(url)
        .query(&[("limit", "200")])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let result: Runs = response
        .json()
        .await
        .map_err(|e| format!("Invalid run list: {e}"))?;
    if result.runs.iter().any(|run| run.id.trim().is_empty()) {
        return Err("Run list contains an empty id".into());
    }
    Ok(result.runs)
}

pub(crate) async fn reports(connection: Connection, id: String) -> Result<Vec<Report>, String> {
    let url = endpoint(&connection, &["runs", &id, "verification"])?;
    let response = connection
        .client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let result: Reports = response
        .json()
        .await
        .map_err(|e| format!("Invalid verification response: {e}"))?;
    if result
        .verification_reports
        .iter()
        .any(|report| report.run_id != id)
    {
        return Err("Verification report belongs to another run".into());
    }
    Ok(result.verification_reports)
}

pub(crate) fn category(status: &str) -> &'static str {
    match status {
        "created" | "planned" | "executing" | "verifying" => "running",
        "awaiting_input" | "idle" => "waiting",
        "paused_needs_review" | "insufficient_oracle" => "blocked",
        "completed" | "failed" | "cancelled" => "finished",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    async fn fixture(
        status: u16,
        body: &'static str,
    ) -> (Connection, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connection = Connection::from_values(
            &format!("http://{}/", listener.local_addr().unwrap()),
            "test-token",
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            String::from_utf8(buffer[..n].to_vec()).unwrap()
        });
        (connection, server)
    }

    #[tokio::test]
    async fn test_work_list_uses_authenticated_run_route_and_preserves_unknown_status() {
        let (connection, server) = fixture(200, r#"{"runs":[{"id":"r1","objective":"test","status":"future","result":null,"error":null}],"count":1}"#).await;
        let runs = runs(connection).await.unwrap();
        assert_eq!(category(&runs[0].status), "unknown");
        let request = server.await.unwrap().to_lowercase();
        assert!(request.starts_with("get /runs?limit=200 "));
        assert!(request.contains("authorization: bearer test-token"));
    }

    #[tokio::test]
    async fn test_report_rejects_other_run_and_http_failure() {
        let (connection, server) = fixture(200, r#"{"verification_reports":[{"run_id":"other","report_type":"tests","passed":true,"summary":"ok","checks":[],"artifacts":{}}]}"#).await;
        assert!(reports(connection, "r1".into())
            .await
            .unwrap_err()
            .contains("another run"));
        assert!(server
            .await
            .unwrap()
            .starts_with("GET /runs/r1/verification "));
        let (connection, server) = fixture(403, r#"{"detail":"forbidden"}"#).await;
        assert!(reports(connection, "r1".into()).await.is_err());
        server.await.unwrap();
    }

    #[test]
    fn test_refresh_keeps_selection_by_run_id() {
        let run = |id: &str| WorkRun {
            id: id.into(),
            objective: id.into(),
            status: "executing".into(),
            result: None,
            error: None,
        };
        let mut app = crate::PorygonTui::new();
        app.work.page = Page::Work;
        app.work.runs = vec![run("a"), run("b")];
        app.work.selected = 1;
        app.handle_update(crate::Msg::WorkRuns(Ok(vec![run("b"), run("a")])));
        assert_eq!(app.work.runs[app.work.selected].id, "b");
    }

    #[test]
    fn test_stale_report_does_not_replace_selected_run_evidence() {
        let mut app = crate::PorygonTui::new();
        app.work.page = Page::Detail;
        app.work.detail_id = Some("current".into());
        app.work.loading = true;
        app.handle_update(crate::Msg::WorkReports(
            "previous".into(),
            Err("stale".into()),
        ));
        assert!(app.work.error.is_none());
        assert!(app.work.loading);
    }

    #[test]
    fn test_status_categories_keep_finished_distinct_from_success() {
        assert_eq!(category("executing"), "running");
        assert_eq!(category("awaiting_input"), "waiting");
        assert_eq!(category("paused_needs_review"), "blocked");
        assert_eq!(category("failed"), "finished");
    }
}
