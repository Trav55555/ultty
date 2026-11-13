//! Pending approvals use the daemon's durable approval routes and confirmations.
//!
//! The caller supplies a client with authentication headers. Reading requires
//! `read` scope; decisions require `write` scope. A decision is sent once. A
//! transport failure can leave its outcome unknown. Never retry the same
//! incarnation; a replacement requires a fresh explicit inspection.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The pending-list envelope. `count` can exceed `items.len()` when paginated.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PendingApprovals {
    /// Total number of pending approvals on the daemon.
    pub count: u64,
    /// Requested maximum number of returned records.
    pub limit: u16,
    /// Last update across all pending records, absent for an empty list.
    pub updated_at: Option<String>,
    /// Records in the daemon's order, newest first.
    pub items: Vec<PendingApproval>,
}

/// A durable tool approval or heartbeat approval, without truncating its data.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PendingApproval {
    /// Identifier to use when submitting a decision.
    pub id: String,
    /// Server-issued immutable incarnation bound to the inspected action.
    pub decision_version: Option<String>,
    /// Durable approval scope; older heartbeat records omit it.
    pub scope: Option<String>,
    /// Origin of the proposed action, such as `tool:write_file`.
    pub source: String,
    /// The daemon's explanation of the proposed action.
    pub objective: String,
    /// Priority is preserved in the daemon's representation.
    pub priority: Value,
    /// Full tool arguments and run attribution supplied by the daemon.
    pub context: ApprovalContext,
    /// Creation timestamp supplied by the daemon.
    pub created_at: String,
    /// Last record update; older heartbeat records omit it.
    pub updated_at: Option<String>,
    /// Fields added by newer daemons remain available to the caller.
    #[serde(flatten)]
    pub additional: Map<String, Value>,
}

/// Tool-call details, with optional fields for non-tool heartbeat approvals.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ApprovalContext {
    /// Exact tool name whose execution is requested.
    pub tool_name: Option<String>,
    /// Complete argument object, including nested values and multiline strings.
    pub params: Option<Map<String, Value>>,
    /// Durable run awaiting the decision.
    pub run_id: Option<String>,
    /// Originating request, when supplied by the daemon.
    pub request_id: Option<String>,
    /// Tool invocation within the run.
    pub tool_call_id: Option<String>,
    /// Principal to which the requested execution is bound.
    pub principal_id: Option<String>,
    /// Requesting client surface.
    pub surface: Option<String>,
    /// Hash binding the server-side approval to its exact payload.
    pub call_hash: Option<String>,
    /// Hash version, retained as JSON because legacy records can use strings.
    pub call_hash_version: Option<Value>,
    /// Server-side execution environment bound to the approval.
    pub execution_context: Option<Value>,
    /// Expiry timestamp; the daemon remains responsible for expiry validation.
    pub approval_expires_at: Option<String>,
    /// Additional context is retained without interpretation or truncation.
    #[serde(flatten)]
    pub additional: Map<String, Value>,
}

/// A single decision on one pending record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Authorize the action bound to this record.
    Approve,
    /// Deny the action bound to this record.
    Deny,
}

impl ApprovalDecision {
    fn route(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
        }
    }

    fn expected_status(self) -> ApprovalDecisionStatus {
        match self {
            Self::Approve => ApprovalDecisionStatus::Approved,
            Self::Deny => ApprovalDecisionStatus::Denied,
        }
    }
}

/// Confirmed terminal decision statuses accepted by this client.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionStatus {
    /// The daemon accepted the approval; execution may still be queued.
    Approved,
    /// The daemon accepted the denial.
    Denied,
}

/// A server-confirmed decision for the exact requested approval.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalDecisionResult {
    /// Confirmed decision, checked against the requested action.
    pub status: ApprovalDecisionStatus,
    /// Confirmed identifier, checked against the requested record.
    pub approval_id: String,
    /// Whether the pending record was removed, when the daemon reports it.
    pub removed: Option<bool>,
    /// Original action source, when supplied.
    pub source: Option<String>,
    /// Execution state, such as `queued_for_managed_resume`.
    pub execution: Option<String>,
    /// Full server result. Approval does not itself prove execution completed.
    pub result: Option<Value>,
    /// Additional confirmation fields supplied by newer daemons.
    #[serde(flatten)]
    pub additional: Map<String, Value>,
}

/// Fetch the real pending envelope from the daemon base URL.
///
/// An empty successful envelope is distinct from an HTTP, authentication, or
/// malformed-response failure. `limit` must be between 1 and 200 inclusive.
pub async fn fetch_pending(
    client: &Client,
    base_url: &Url,
    limit: u16,
) -> Result<PendingApprovals> {
    if !(1..=200).contains(&limit) {
        bail!("Pending approval limit must be between 1 and 200");
    }
    let mut url = pending_url(base_url)?;
    url.query_pairs_mut()
        .append_pair("limit", &limit.to_string());
    let response = client
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .context("Could not fetch pending approvals")?;
    if !response.status().is_success() {
        bail!(
            "Daemon rejected pending approval request: HTTP {}",
            response.status()
        );
    }
    let payload: Value = response
        .json()
        .await
        .context("Invalid pending approval response")?;
    reject_application_error(&payload)?;
    if let Some(status) = payload.get("status") {
        if status.as_str() != Some("ok") {
            bail!("Pending approval request was not confirmed: {status}");
        }
    }
    let pending: PendingApprovals =
        serde_json::from_value(payload).context("Invalid pending approval envelope")?;
    if pending
        .items
        .iter()
        .any(|record| record.id.trim().is_empty())
    {
        bail!("Pending approval response contains an empty record identifier");
    }
    Ok(pending)
}

/// The server rejected a decision because its inspected incarnation changed.
#[derive(Debug)]
pub struct StaleApproval(String);

impl std::fmt::Display for StaleApproval {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}
impl std::error::Error for StaleApproval {}

/// Read-only reconciliation of the previously inspected incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalRefresh {
    Same,
    Changed,
    Missing,
    Unknown,
}

pub fn reconcile_pending(
    queue: &PendingApprovals,
    id: &str,
    version: Option<&str>,
    scope: Option<&str>,
) -> ApprovalRefresh {
    if let Some(item) = queue.items.iter().find(|item| item.id == id) {
        if item.decision_version.as_deref() == version && item.scope.as_deref() == scope {
            ApprovalRefresh::Same
        } else {
            ApprovalRefresh::Changed
        }
    } else if queue.count > queue.items.len() as u64 {
        ApprovalRefresh::Unknown
    } else {
        ApprovalRefresh::Missing
    }
}

/// Submit one decision to the daemon's `pending/{id}/approve` or `deny` route.
///
/// The caller supplies the daemon base URL and an authenticated
/// client. Only a matching identifier and terminal decision count as success.
/// This function never retries. If confirmation cannot be read, the action may
/// already have happened. Do not retry that incarnation; inspect a replacement
/// explicitly before submitting a new decision.
pub async fn submit_decision(
    client: &Client,
    base_url: &Url,
    approval_id: &str,
    expected_version: &str,
    decision: ApprovalDecision,
) -> Result<ApprovalDecisionResult> {
    if approval_id.trim().is_empty() || matches!(approval_id, "." | "..") {
        bail!("A nonempty approval identifier is required");
    }
    if expected_version.trim().is_empty() {
        bail!("A decision version from the inspected tool approval is required; upgrade the daemon and reinspect");
    }
    let mut url = pending_url(base_url)?;
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("Daemon URL cannot contain path segments"))?
        .push(approval_id)
        .push(decision.route());
    let response = client
        .post(url)
        .json(&serde_json::json!({"expected_version": expected_version}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| {
            format!(
                "No confirmation for {} of approval {approval_id}; outcome may be unknown, refresh before resubmitting",
                decision.route()
            )
        })?;
    if !response.status().is_success() {
        bail!(
            "Daemon rejected {} of approval {approval_id}: HTTP {}",
            decision.route(),
            response.status()
        );
    }
    let payload: Value = response.json().await.with_context(|| {
        format!(
            "Invalid confirmation for approval {approval_id}; outcome may be unknown, refresh before resubmitting"
        )
    })?;
    if let Err(error) = reject_application_error(&payload) {
        let message = format!("Decision failed for approval {approval_id}: {error}");
        return Err(error.context(message));
    }
    let result: ApprovalDecisionResult = serde_json::from_value(payload).with_context(|| {
        format!(
            "Daemon did not confirm {} of approval {approval_id}",
            decision.route()
        )
    })?;
    if result.approval_id != approval_id || result.status != decision.expected_status() {
        bail!(
            "Daemon confirmation does not match {} of approval {approval_id}",
            decision.route()
        );
    }
    Ok(result)
}

fn pending_url(base_url: &Url) -> Result<Url> {
    if !matches!(base_url.scheme(), "http" | "https") {
        bail!("Daemon URL must use HTTP or HTTPS");
    }
    let mut url = base_url.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("Daemon URL cannot contain path segments"))?
        .pop_if_empty()
        .extend(["approvals", "pending"]);
    Ok(url)
}

fn reject_application_error(payload: &Value) -> Result<()> {
    if payload.get("consumed").and_then(Value::as_bool) == Some(true) {
        bail!(
            "Approval consumed; execution outcome may be unknown. Do not retry this decision: {}",
            payload.get("error").unwrap_or(&Value::Null)
        );
    }
    if payload.get("status").and_then(Value::as_str) == Some("stale") {
        return Err(StaleApproval(
            payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Approval changed or was already consumed; refresh before deciding")
                .to_owned(),
        )
        .into());
    }
    if let Some(error) = payload.get("error").filter(|error| !error.is_null()) {
        bail!("Daemon approval error: {error}");
    }
    if let Some(status @ ("error" | "not_found" | "stale")) =
        payload.get("status").and_then(Value::as_str)
    {
        bail!("Daemon approval outcome: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;

    // The listener handles one request and closes. A retry cannot get a second
    // response, and the caller has a bounded timeout if the wire contract breaks.
    async fn mock_response(status: u16, body: &str) -> (Url, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let body = body.to_owned();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "client closed before finishing request headers");
                request.extend_from_slice(&buffer[..count]);
            }
            let headers_end = request
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .unwrap()
                + 4;
            let headers = String::from_utf8_lossy(&request[..headers_end]).to_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .and_then(|length| length.parse::<usize>().ok())
                .unwrap_or(0);
            while request.len() < headers_end + length {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            let response = format!(
                "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, server)
    }

    fn authenticated_client() -> Client {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_static("Bearer mock-approval-token"),
        );
        Client::builder()
            .default_headers(headers)
            .no_proxy()
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn test_fetch_pending_real_envelope_and_full_context() {
        let body = serde_json::json!({
            "count": 3, "limit": 2, "updated_at": "2026-11-12T10:00:00Z",
            "items": [{
                "id": "approval-1", "scope": "tool_call", "decision_version":"inspected-version", "source": "tool:write_file",
                "objective": "Write a file", "priority": "HIGH",
                "created_at": "2026-11-12T09:00:00Z", "updated_at": null,
                "context": {
                    "tool_name": "write_file", "run_id": "run-1", "request_id": "req-1",
                    "tool_call_id": "call-1", "principal_id": "principal-1",
                    "params": {"path": "file.txt", "content": "first\nsecond", "nested": [1, {"flag": true}]},
                    "call_hash_version": "1", "project_path": "/project"
                }
            }, {
                "id": "heartbeat-1", "source": "heartbeat", "objective": "Check backlog",
                "priority": "LOW", "created_at": "2026-11-12T08:00:00Z",
                "context": {"backlog": ["full context"]}
            }]
        });
        let (url, server) = mock_response(200, &body.to_string()).await;
        let pending = fetch_pending(&authenticated_client(), &url, 2)
            .await
            .unwrap();
        assert_eq!(pending.count, 3);
        assert_eq!(pending.items.len(), 2);
        assert_eq!(
            pending.items[0].decision_version.as_deref(),
            Some("inspected-version")
        );
        let context = &pending.items[0].context;
        assert_eq!(context.run_id.as_deref(), Some("run-1"));
        assert_eq!(context.request_id.as_deref(), Some("req-1"));
        assert_eq!(context.params.as_ref().unwrap()["content"], "first\nsecond");
        assert_eq!(context.params.as_ref().unwrap()["nested"][1]["flag"], true);
        assert_eq!(context.additional["project_path"], "/project");
        assert_eq!(pending.items[1].scope, None);
        let request = server.await.unwrap().to_lowercase();
        assert!(request.starts_with("get /approvals/pending?limit=2 http/1.1\r\n"));
        assert!(request.contains("authorization: bearer mock-approval-token\r\n"));
    }

    #[tokio::test]
    async fn test_fetch_empty_envelope_is_success() {
        let (url, server) = mock_response(
            200,
            r#"{"count":0,"limit":20,"updated_at":null,"items":[]}"#,
        )
        .await;
        let pending = fetch_pending(&authenticated_client(), &url, 20)
            .await
            .unwrap();
        assert!(pending.items.is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_fetch_http_and_application_failures_are_not_empty_success() {
        for (status, body) in [
            (
                302,
                r#"{"count":0,"limit":20,"updated_at":null,"items":[]}"#,
            ),
            (401, r#"{"detail":"Authentication required"}"#),
            (404, r#"{"detail":"Not Found"}"#),
            (500, r#"{"error":"database failed"}"#),
            (200, r#"{"status":"error","error":"database failed"}"#),
            (200, r#"{"status":"stale"}"#),
            (200, r#"{"id":"old-flat-shape"}"#),
            (204, ""),
        ] {
            let (url, server) = mock_response(status, body).await;
            assert!(fetch_pending(&authenticated_client(), &url, 20)
                .await
                .is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_submit_approve_confirms_identifier_and_preserves_queued_execution() {
        let (url, server) = mock_response(
            200,
            r#"{"status":"approved","approval_id":"approval/one?","removed":true,"execution":"queued_for_managed_resume","result":"One execution authorized; tool has not run yet."}"#,
        )
        .await;
        let result = submit_decision(
            &authenticated_client(),
            &url,
            "approval/one?",
            "inspected-version",
            ApprovalDecision::Approve,
        )
        .await
        .unwrap();
        assert_eq!(result.status, ApprovalDecisionStatus::Approved);
        assert_eq!(
            result.execution.as_deref(),
            Some("queued_for_managed_resume")
        );
        let request = server.await.unwrap().to_lowercase();
        assert!(
            request.starts_with("post /approvals/pending/approval%2fone%3f/approve http/1.1\r\n")
        );
        assert!(request.contains("authorization: bearer mock-approval-token\r\n"));
        assert_eq!(
            serde_json::from_str::<Value>(request.split_once("\r\n\r\n").unwrap().1).unwrap(),
            serde_json::json!({"expected_version":"inspected-version"})
        );
    }

    #[tokio::test]
    async fn test_submit_deny_uses_real_route() {
        let (url, server) =
            mock_response(200, r#"{"status":"denied","approval_id":"approval-1"}"#).await;
        let result = submit_decision(
            &authenticated_client(),
            &url,
            "approval-1",
            "inspected-version",
            ApprovalDecision::Deny,
        )
        .await
        .unwrap();
        assert_eq!(result.status, ApprovalDecisionStatus::Denied);
        assert!(server
            .await
            .unwrap()
            .starts_with("POST /approvals/pending/approval-1/deny HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn test_submit_rejects_every_unconfirmed_outcome() {
        for (status, body) in [
            (302, r#"{"status":"approved","approval_id":"approval-1"}"#),
            (403, r#"{"detail":"Scope write required"}"#),
            (500, r#"{"status":"approved","approval_id":"approval-1"}"#),
            (
                200,
                r#"{"status":"error","approval_id":"approval-1","error":"Approval expired"}"#,
            ),
            (200, r#"{"status":"not_found","approval_id":"approval-1"}"#),
            (200, r#"{"status":"stale","approval_id":"approval-1"}"#),
            (200, r#"{"status":"denied","approval_id":"approval-1"}"#),
            (200, r#"{"status":"approved","approval_id":"other"}"#),
            (200, r#"{"status":"approved"}"#),
            (
                200,
                r#"{"status":"approved","approval_id":"approval-1","error":"failed"}"#,
            ),
            (200, "not json"),
            (204, ""),
        ] {
            let (url, server) = mock_response(status, body).await;
            let result = submit_decision(
                &authenticated_client(),
                &url,
                "approval-1",
                "inspected-version",
                ApprovalDecision::Approve,
            )
            .await;
            assert!(result.is_err(), "accepted HTTP {status} response {body}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_submit_disconnect_is_uncertain_and_is_not_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            drop(socket);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err(),
                "a decision must not be retried after a disconnect"
            );
        });
        let error = submit_decision(
            &authenticated_client(),
            &url,
            "approval-1",
            "inspected-version",
            ApprovalDecision::Approve,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("outcome may be unknown"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn test_submit_binds_exact_inspected_version_and_rejects_reused_id_stale() {
        let version = "Version-A/\"\n?=";
        let (url, server) = mock_response(200, r#"{"status":"stale","approval_id":"same-id","error":"Approval changed or was already consumed; refresh before deciding"}"#).await;
        let error = submit_decision(
            &authenticated_client(),
            &url,
            "same-id",
            version,
            ApprovalDecision::Approve,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Approval changed"));
        assert!(error.is::<StaleApproval>());
        let request = server.await.unwrap();
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body, serde_json::json!({"expected_version":version}));
    }

    #[tokio::test]
    async fn test_consumed_failure_is_explicit_and_missing_version_never_posts() {
        let (url, server) = mock_response(200, r#"{"status":"error","approval_id":"same-id","error":"claimed execution failed","consumed":true}"#).await;
        let error = submit_decision(
            &authenticated_client(),
            &url,
            "same-id",
            "inspected",
            ApprovalDecision::Approve,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("consumed"));
        assert!(error.to_string().contains("Do not retry"));
        server.await.unwrap();
        let unused = Url::parse("http://127.0.0.1:1").unwrap();
        let error = submit_decision(
            &authenticated_client(),
            &unused,
            "same-id",
            "",
            ApprovalDecision::Deny,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("decision version"));
    }
}
