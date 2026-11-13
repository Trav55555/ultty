//! Authenticated HTTP and WebSocket configuration for the Porygon daemon.

use std::fs::File;
use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:18789/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A configured authenticated connection to a Porygon daemon.
#[derive(Clone)]
pub(crate) struct Connection {
    pub(crate) base_url: Url,
    pub(crate) auth_token: String,
    pub(crate) client: reqwest::Client,
}

/// The daemon fields consumed by run list/detail views.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RunSnapshot {
    pub(crate) id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) result: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) metadata: Value,
}

#[derive(Deserialize)]
struct StatusPayload {
    uptime_seconds: f64,
    subsystems: Value,
}

impl Connection {
    pub(crate) fn from_values(url: &str, token: &str) -> Result<Self, String> {
        let base_url = validate_base_url(url)?;
        if token.trim().is_empty() {
            return Err("PORYGON_TOKEN is required; set it to the daemon API token".into());
        }
        if token.trim() != token {
            return Err("PORYGON_TOKEN must not contain leading or trailing whitespace".into());
        }

        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            "PORYGON_TOKEN contains characters invalid in an HTTP header".to_string()
        })?;
        let mut headers = HeaderMap::new();
        let mut bearer = bearer;
        bearer.set_sensitive(true);
        headers.insert(AUTHORIZATION, bearer);

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| format!("could not create daemon HTTP client: {err}"))?;

        Ok(Self {
            base_url,
            auth_token: token.to_owned(),
            client,
        })
    }

    pub(crate) fn from_env() -> Result<Self, String> {
        let url = std::env::var("PORYGON_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let token = std::env::var("PORYGON_TOKEN")
            .map_err(|_| "PORYGON_TOKEN is missing; set it to the daemon API token".to_string())?;
        Self::from_values(&url, &token)
    }

    pub(crate) fn stream_url(&self) -> Result<Url, String> {
        let mut url = self.endpoint(&["agent", "stream"])?;
        let scheme = match self.base_url.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => return Err("daemon URL must use http or https".into()),
        };
        url.set_scheme(scheme)
            .map_err(|_| "could not convert daemon URL to WebSocket URL".to_string())?;
        Ok(url)
    }

    pub(crate) async fn check(&self) -> Result<(), String> {
        let url = self.endpoint(&["status"])?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| format!("daemon status request failed: {err}"))?;
        require_success(response.status(), "daemon status request")?;
        let status: StatusPayload = response
            .json()
            .await
            .map_err(|err| format!("daemon returned an invalid status payload: {err}"))?;
        if !status.uptime_seconds.is_finite() || status.uptime_seconds < 0.0 {
            return Err("daemon returned an invalid status payload: uptime_seconds".into());
        }
        if !status.subsystems.is_object() {
            return Err("daemon returned an invalid status payload: subsystems".into());
        }
        Ok(())
    }

    pub(crate) async fn fetch_run(&self, run_id: &str) -> Result<RunSnapshot, String> {
        self.run_request(Method::GET, run_id, false).await
    }

    pub(crate) async fn cancel_run(&self, run_id: &str) -> Result<RunSnapshot, String> {
        self.run_request(Method::POST, run_id, true).await
    }

    async fn run_request(
        &self,
        method: Method,
        run_id: &str,
        cancel: bool,
    ) -> Result<RunSnapshot, String> {
        let url = if cancel {
            self.endpoint(&["runs", run_id, "cancel"])?
        } else {
            self.endpoint(&["runs", run_id])?
        };
        let operation = if cancel {
            "run cancellation"
        } else {
            "run request"
        };
        let response = self
            .client
            .request(method, url)
            .send()
            .await
            .map_err(|err| format!("{operation} failed: {err}"))?;
        require_success(response.status(), operation)?;
        let snapshot: RunSnapshot = response
            .json()
            .await
            .map_err(|err| format!("daemon returned an invalid run payload: {err}"))?;
        if snapshot.id != run_id {
            return Err("daemon returned a run with a different id".into());
        }
        Ok(snapshot)
    }

    pub(crate) fn endpoint(&self, segments: &[&str]) -> Result<Url, String> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| "daemon URL cannot be used as a hierarchical URL".to_string())?;
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }
}

fn validate_base_url(value: &str) -> Result<Url, String> {
    let mut url =
        Url::parse(value).map_err(|_| "PORYGON_URL must be a valid absolute URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("PORYGON_URL must use http or https".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("PORYGON_URL must not contain a query or fragment".into());
    }
    let authority = value
        .split_once("://")
        .map(|(_, remainder)| remainder.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    if authority.contains('@') || !url.username().is_empty() || url.password().is_some() {
        return Err("PORYGON_URL must not contain user information".into());
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(
            "plain HTTP is allowed only for a loopback daemon; use HTTPS for remote hosts".into(),
        );
    }

    let normalized_path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&normalized_path);
    Ok(url)
}

fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(host)
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

fn require_success(status: StatusCode, operation: &str) -> Result<(), String> {
    if status.is_success() {
        Ok(())
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        Err(format!(
            "{operation} was rejected ({}); check PORYGON_TOKEN",
            status
        ))
    } else {
        Err(format!("{operation} returned {status}"))
    }
}

pub(crate) fn new_run_id() -> Result<String, String> {
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut random))
        .map_err(|err| format!("could not read secure randomness for run id: {err}"))?;
    let mut id = String::with_capacity("native-".len() + random.len() * 2);
    id.push_str("native-");
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").map_err(|_| "could not format run id".to_string())?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    async fn mock_response(
        status: StatusCode,
        body: &'static str,
        location: Option<String>,
    ) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (request, mut stream) = read_request(stream).await;
            let mut response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            if let Some(location) = location {
                response.push_str(&format!("Location: {location}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(body);
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{address}/"), task)
    }

    async fn read_request(stream: TcpStream) -> (String, TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let done = line == "\r\n" || line.is_empty();
            request.push_str(&line);
            if done {
                break;
            }
        }
        (request, reader.into_inner())
    }

    #[test]
    fn test_endpoint_paths_preserve_prefix_and_encode_run_id() {
        let connection = Connection::from_values("https://example.test/api/v1", "token").unwrap();
        assert_eq!(
            connection.endpoint(&["status"]).unwrap().as_str(),
            "https://example.test/api/v1/status"
        );
        assert_eq!(
            connection.endpoint(&["runs", "a/b ?"]).unwrap().as_str(),
            "https://example.test/api/v1/runs/a%2Fb%20%3F"
        );
        assert_eq!(
            connection
                .endpoint(&["runs", "a/b", "cancel"])
                .unwrap()
                .as_str(),
            "https://example.test/api/v1/runs/a%2Fb/cancel"
        );
    }

    #[test]
    fn test_connection_rejects_unsafe_urls_and_tokens() {
        assert!(Connection::from_values("http://remote.example", "token").is_err());
        assert!(Connection::from_values("https://user@example.test", "token").is_err());
        assert!(Connection::from_values("https://example.test/?secret=1", "token").is_err());
        assert!(Connection::from_values("http://127.0.0.1:18789", " ").is_err());
        assert!(Connection::from_values("http://[::1]:18789", "token").is_ok());
    }

    #[tokio::test]
    async fn test_status_sends_bearer_and_parses_daemon_payload() {
        let (base_url, server) = mock_response(
            StatusCode::OK,
            r#"{"uptime_seconds":12.5,"subsystems":{"healthy":true}}"#,
            None,
        )
        .await;
        let connection = Connection::from_values(&base_url, "wire-secret").unwrap();

        connection.check().await.unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /status HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("\r\nauthorization: bearer wire-secret\r\n"));
    }

    #[tokio::test]
    async fn test_status_rejects_auth_failure_without_claiming_connected() {
        let (base_url, server) = mock_response(StatusCode::UNAUTHORIZED, "{}", None).await;
        let connection = Connection::from_values(&base_url, "wrong-token").unwrap();

        let error = connection.check().await.unwrap_err();
        assert!(error.contains("check PORYGON_TOKEN"));
        let request = server.await.unwrap();
        assert!(request
            .to_ascii_lowercase()
            .contains("\r\nauthorization: bearer wrong-token\r\n"));
    }

    #[tokio::test]
    async fn test_status_does_not_follow_redirects() {
        let (base_url, server) = mock_response(
            StatusCode::FOUND,
            "",
            Some("http://127.0.0.1:9/redirect-target".to_string()),
        )
        .await;
        let connection = Connection::from_values(&base_url, "token").unwrap();

        let error = connection.check().await.unwrap_err();
        assert!(error.contains("returned 302"));
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /status HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn test_fetch_and_cancel_use_expected_paths_and_snapshots() {
        let (base_url, server) = mock_response(
            StatusCode::OK,
            r#"{"id":"run-1","status":"running","metadata":{}}"#,
            None,
        )
        .await;
        let connection = Connection::from_values(&base_url, "token").unwrap();
        let snapshot = connection.fetch_run("run-1").await.unwrap();
        assert_eq!(snapshot.id, "run-1");
        assert_eq!(snapshot.status, "running");

        // The mock server handles one request; use a second server for cancellation.
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /runs/run-1 HTTP/1.1\r\n"));

        let (cancel_url, cancel_server) = mock_response(
            StatusCode::OK,
            r#"{"id":"run-1","status":"cancelled","result":null,"error":null,"metadata":{}}"#,
            None,
        )
        .await;
        let cancel_connection = Connection::from_values(&cancel_url, "token").unwrap();
        let cancelled = cancel_connection.cancel_run("run-1").await.unwrap();
        assert_eq!(cancelled.id, "run-1");
        assert_eq!(cancelled.status, "cancelled");
        let request = cancel_server.await.unwrap();
        assert!(request.starts_with("POST /runs/run-1/cancel HTTP/1.1\r\n"));
    }

    #[test]
    fn test_stream_url_uses_secure_scheme_and_daemon_prefix() {
        let connection = Connection::from_values("https://example.test/root", "token").unwrap();
        assert_eq!(
            connection.stream_url().unwrap().as_str(),
            "wss://example.test/root/agent/stream"
        );
    }
}
