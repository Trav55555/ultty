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
    pub detail_scroll: usize,
    pub generation: u64,
    pub notice: Option<usize>,
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
            detail_scroll: 0,
            generation: 0,
            notice: None,
            detail_id: None,
            reports: Vec::new(),
            error: None,
            loading: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttentionItem {
    Stream,
    Approval(usize),
    Run(usize),
}

pub(crate) fn attention(state: &WorkState, stream_id: Option<&str>) -> Vec<AttentionItem> {
    let mut items = Vec::new();
    if stream_id.is_some() {
        items.push(AttentionItem::Stream);
    }
    for (i, item) in state.approvals.iter().enumerate() {
        if Some(item.id.as_str()) != stream_id {
            items.push(AttentionItem::Approval(i));
        }
    }
    for (i, run) in state.runs.iter().enumerate() {
        if matches!(category(&run.status), "waiting" | "blocked") && run.status != "idle" {
            items.push(AttentionItem::Run(i));
        }
    }
    items
}

pub(crate) fn attention_key(
    state: &WorkState,
    item: &AttentionItem,
    stream_id: Option<&str>,
) -> String {
    match item {
        AttentionItem::Stream => format!("approval:{}", stream_id.unwrap_or_default()),
        AttentionItem::Approval(i) => format!("approval:{}", state.approvals[*i].id),
        AttentionItem::Run(i) => format!("run:{}", state.runs[*i].id),
    }
}

pub(crate) async fn detail(
    connection: Connection,
    id: String,
) -> Result<(WorkRun, Vec<Report>), String> {
    let url = endpoint(&connection, &["runs", &id])?;
    let response = connection
        .client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let run: WorkRun = response
        .json()
        .await
        .map_err(|e| format!("Invalid run response: {e}"))?;
    if run.id != id {
        return Err("Daemon returned a different run id".into());
    }
    let reports = reports(connection, id).await?;
    Ok((run, reports))
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
        app.handle_update(crate::Msg::WorkRuns(
            app.work.generation,
            Ok(vec![run("b"), run("a")]),
        ));
        assert_eq!(app.work.runs[app.work.selected].id, "b");
    }

    #[test]
    fn test_stale_report_does_not_replace_selected_run_evidence() {
        let mut app = crate::PorygonTui::new();
        app.work.page = Page::Detail;
        app.work.detail_id = Some("current".into());
        app.work.loading = true;
        app.handle_update(crate::Msg::WorkDetail(
            "previous".into(),
            0,
            Err("stale".into()),
        ));
        assert!(app.work.error.is_none());
        assert!(app.work.loading);
    }

    #[test]
    fn test_same_run_old_generation_does_not_replace_new_detail() {
        let mut app = crate::PorygonTui::new();
        app.work.page = Page::Detail;
        app.work.detail_id = Some("current".into());
        app.work.generation = 2;
        app.work.loading = true;
        app.handle_update(crate::Msg::WorkDetail(
            "current".into(),
            1,
            Err("stale".into()),
        ));
        assert!(app.work.loading);
        assert!(app.work.error.is_none());
    }

    #[test]
    fn test_old_list_refresh_cannot_replace_newer_state() {
        let mut app = crate::PorygonTui::new();
        app.work.page = Page::Work;
        app.work.generation = 3;
        app.work.loading = true;
        app.handle_update(crate::Msg::WorkRuns(2, Err("stale failure".into())));
        assert!(app.work.error.is_none());
        assert!(app.work.loading);
        app.work.page = Page::Attention;
        app.handle_update(crate::Msg::WorkAttention(2, Err("stale failure".into())));
        assert!(app.work.error.is_none());
        assert!(app.work.loading);
    }

    #[test]
    fn test_refresh_failure_is_visible_even_below_the_first_page() {
        use ulttui::prelude::View;
        let mut app = crate::PorygonTui::new();
        app.terminal_width = 80;
        app.terminal_height = 12;
        app.work.page = Page::Work;
        app.work.runs = (0..100)
            .map(|index| WorkRun {
                id: format!("r{index}"),
                objective: format!("objective {index}"),
                status: "executing".into(),
                result: None,
                error: None,
            })
            .collect();
        app.work.selected = 99;
        app.work.error = Some("request timed out".into());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| crate::work_views::view(&app).render_ansi(frame, frame.area()))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Refresh failed"), "{screen}");
        assert!(screen.contains("request timed out"), "{screen}");
        assert!(screen.contains("objective 99"), "{screen}");
    }

    #[test]
    fn test_stream_approval_does_not_open_modal() {
        let mut app = crate::PorygonTui::new();
        app.handle_update(crate::Msg::ApprovalReceived(
            crate::types::ApprovalRequest {
                scope: Some("tool_call".into()),
                decision_version: Some("fixture-version".into()),
                decision_stale: false,
                id: "stream".into(),
                tool_name: "shell".into(),
                params: serde_json::json!({}),
                risk_score: -1.0,
                reasoning: "approval".into(),
                blast_radius: "unknown".into(),
                reversible: None,
            },
            true,
        ));
        assert!(!app.show_approval_modal);
        assert_eq!(
            attention(&app.work, Some("stream")),
            vec![AttentionItem::Stream]
        );
    }

    #[test]
    fn test_detail_long_result_is_reachable_in_narrow_terminal() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use ulttui::prelude::View;
        let mut app = crate::PorygonTui::new();
        app.terminal_width = 40;
        app.terminal_height = 10;
        app.work.page = Page::Detail;
        app.work.detail_id = Some("r".into());
        app.work.runs.push(WorkRun {
            id: "r".into(),
            objective: "objective first".into(),
            status: "completed".into(),
            result: Some(format!("{}\nEND-MARKER", "word ".repeat(80))),
            error: None,
        });
        app.work.detail_scroll = 1000;
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("END-MARKER"), "{screen}");
    }

    #[test]
    fn test_status_categories_keep_finished_distinct_from_success() {
        assert_eq!(category("executing"), "running");
        assert_eq!(category("awaiting_input"), "waiting");
        assert_eq!(category("paused_needs_review"), "blocked");
        assert_eq!(category("failed"), "finished");
    }
}
