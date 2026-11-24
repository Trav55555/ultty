//! Single-instance CLI handoff (ADR 0027 Slice 6).
//!
//! When `behavior.single_instance = true`, a newly launched `ultty` connects to
//! the newest running instance's control socket, sends `ControlRequest::NewWindow`
//! carrying its working directory and optional `-e` command, and exits once the
//! existing process confirms the window opened. If no instance answers within a
//! short timeout, the caller starts normally as a new process.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ultty_protocol::control::{
    ControlData, ControlEnvelope, ControlRequest, ControlResponse, OutputFormat,
};

use super::auth::read_auth_token;
use super::paths::{control_dir, running_instance_candidates, ControlEndpoint};

/// Maximum time to wait for a running instance to open a window before falling
/// back to starting a new process.
pub const DEFAULT_HANDOFF_TIMEOUT: Duration = Duration::from_secs(2);

/// Upper bound on a control reply read during single-instance handoff.
const MAX_HANDOFF_RESPONSE_BYTES: u64 = 64 * 1024;

/// Parameters forwarded to a running Ultty instance when opening a window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewWindowLaunch {
    /// Optional `-e` / `--` program to run instead of the default shell.
    pub command: Option<String>,
    /// Arguments for `command`.
    pub args: Vec<String>,
    /// Working directory of the invoking `ultty` process.
    pub working_dir: Option<String>,
}

/// Result of attempting to hand off a launch to an existing Ultty process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffOutcome {
    /// The existing process `pid` opened window `window`; this launcher can exit 0.
    Opened { pid: u32, window: u64 },
    /// No responsive instance accepted the launch; start a new process normally.
    StartNewProcess,
}

/// Result of attempting to toggle the quick terminal in an existing Ultty process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickHandoffOutcome {
    /// The existing process `pid` toggled its quick terminal.
    Toggled {
        pid: u32,
        window: Option<u64>,
        visible: bool,
    },
    /// No responsive instance accepted the toggle; start a quick-only process.
    StartNewProcess,
}

/// Read the caller's activation token from `XDG_ACTIVATION_TOKEN` (Wayland) or
/// `DESKTOP_STARTUP_ID` (X11), if non-empty.
pub fn current_activation_token() -> Option<String> {
    for var in ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"] {
        if let Ok(token) = std::env::var(var) {
            let trimmed = token.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// Try to toggle the quick terminal in the newest running Ultty instance in the
/// default control directory.
pub fn try_toggle_quick_in_existing_instance(
    activation_token: Option<String>,
) -> QuickHandoffOutcome {
    let Ok(dir) = control_dir() else {
        return QuickHandoffOutcome::StartNewProcess;
    };
    try_toggle_quick_in_existing_instance_in(&dir, activation_token, DEFAULT_HANDOFF_TIMEOUT)
}

/// Try to toggle the quick terminal in the newest running Ultty instance in
/// `dir`, giving up after `timeout` per candidate.
pub fn try_toggle_quick_in_existing_instance_in(
    dir: &Path,
    activation_token: Option<String>,
    timeout: Duration,
) -> QuickHandoffOutcome {
    let self_pid = std::process::id();
    let request = ControlRequest::ToggleQuick { activation_token };
    for endpoint in running_instance_candidates(dir) {
        let Some(pid) = endpoint.pid() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        match send_authenticated_request(&endpoint, &request, timeout) {
            Ok(ControlResponse::Data(ControlData::QuickToggled { window, visible })) => {
                return QuickHandoffOutcome::Toggled {
                    pid,
                    window,
                    visible,
                };
            }
            Ok(ControlResponse::Error { code, message }) => {
                tracing::debug!(
                    pid,
                    code,
                    %message,
                    "quick-terminal handoff rejected by candidate"
                );
            }
            Ok(other) => {
                tracing::debug!(
                    pid,
                    ?other,
                    "quick-terminal handoff received unexpected response"
                );
            }
            Err(error) => {
                tracing::debug!(
                    pid,
                    socket = %endpoint.socket.display(),
                    %error,
                    "quick-terminal handoff skipped candidate"
                );
            }
        }
    }
    QuickHandoffOutcome::StartNewProcess
}

/// Try to open a window in the newest running Ultty instance in the default
/// control directory.
pub fn try_open_in_existing_instance(launch: &NewWindowLaunch) -> HandoffOutcome {
    let Ok(dir) = control_dir() else {
        return HandoffOutcome::StartNewProcess;
    };
    try_open_in_existing_instance_in(&dir, launch, DEFAULT_HANDOFF_TIMEOUT)
}

/// Try to open a window in the newest running Ultty instance in `dir`, giving
/// up after `timeout` per candidate.
pub fn try_open_in_existing_instance_in(
    dir: &Path,
    launch: &NewWindowLaunch,
    timeout: Duration,
) -> HandoffOutcome {
    let self_pid = std::process::id();
    for endpoint in running_instance_candidates(dir) {
        let Some(pid) = endpoint.pid() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        match handoff_to_endpoint(&endpoint, launch, timeout) {
            Ok(window) => return HandoffOutcome::Opened { pid, window },
            Err(error) => {
                tracing::debug!(
                    pid,
                    socket = %endpoint.socket.display(),
                    %error,
                    "single-instance handoff skipped candidate"
                );
            }
        }
    }
    HandoffOutcome::StartNewProcess
}

fn handoff_to_endpoint(
    endpoint: &ControlEndpoint,
    launch: &NewWindowLaunch,
    timeout: Duration,
) -> Result<u64, String> {
    let inner = ControlRequest::NewWindow {
        title: None,
        command: launch.command.clone(),
        args: launch.args.clone(),
        working_dir: launch.working_dir.clone(),
    };
    match send_authenticated_request(endpoint, &inner, timeout)? {
        ControlResponse::Data(ControlData::WindowOpened { window, .. }) => Ok(window),
        ControlResponse::Error { code, message } => {
            Err(format!("instance rejected new-window ({code}): {message}"))
        }
        other => Err(format!("unexpected response: {other:?}")),
    }
}

fn send_authenticated_request(
    endpoint: &ControlEndpoint,
    inner: &ControlRequest,
    timeout: Duration,
) -> Result<ControlResponse, String> {
    let mut stream =
        UnixStream::connect(&endpoint.socket).map_err(|e| format!("connect failed: {e}"))?;
    let token =
        read_auth_token(&endpoint.token).map_err(|e| format!("auth token unavailable: {e}"))?;

    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("set write timeout failed: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("set read timeout failed: {e}"))?;

    let envelope = ControlEnvelope {
        output_format: OutputFormat::Json,
        request: ControlRequest::WithToken {
            token,
            request: Box::new(inner.clone()),
        },
    };
    let mut payload =
        serde_json::to_string(&envelope).map_err(|e| format!("serialize failed: {e}"))?;
    payload.push('\n');

    stream
        .write_all(payload.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    let _ = stream.shutdown(Shutdown::Write);

    let raw = read_reply(&mut stream, timeout)?;
    if raw.trim().is_empty() {
        return Err("empty reply".to_string());
    }
    serde_json::from_str(raw.trim()).map_err(|e| format!("invalid response: {e}"))
}

fn read_reply(stream: &mut UnixStream, timeout: Duration) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    let mut buf = Vec::new();
    let mut limited = stream.take(MAX_HANDOFF_RESPONSE_BYTES);
    let mut chunk = [0u8; 1024];
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err("timed out waiting for reply".to_string());
        };
        let _ = limited.get_ref().set_read_timeout(Some(remaining));
        match limited.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }
    String::from_utf8(buf).map_err(|e| format!("non-utf8 reply: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    use crate::control::auth::write_auth_token;

    fn spawn_mock_instance(
        dir: &Path,
        pid: u32,
        response: ControlResponse,
    ) -> (
        String,
        std::sync::mpsc::Receiver<ControlEnvelope>,
        std::thread::JoinHandle<()>,
    ) {
        let endpoint = ControlEndpoint::for_pid(dir, pid);
        let token = write_auth_token(&endpoint.token).unwrap();
        let listener = UnixListener::bind(&endpoint.socket).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = String::new();
            stream.read_to_string(&mut raw).unwrap();
            let envelope: ControlEnvelope = serde_json::from_str(raw.trim()).unwrap();
            tx.send(envelope).unwrap();
            let mut reply = serde_json::to_string(&response).unwrap();
            reply.push('\n');
            stream.write_all(reply.as_bytes()).unwrap();
        });
        (token, rx, handle)
    }

    #[test]
    fn handoff_opens_window_in_newest_running_instance_with_cwd_and_command() {
        let dir = tempfile::tempdir().unwrap();
        let _older = UnixListener::bind(ControlEndpoint::for_pid(dir.path(), 2001).socket).unwrap();
        std::thread::sleep(Duration::from_millis(20));

        let (token, rx, server) = spawn_mock_instance(
            dir.path(),
            2002,
            ControlResponse::Data(ControlData::WindowOpened {
                window: 3,
                tab_index: 2,
                pane: 7,
                warning: None,
            }),
        );

        let launch = NewWindowLaunch {
            command: Some("htop".to_string()),
            args: vec!["-d".to_string(), "5".to_string()],
            working_dir: Some("/tmp/project".to_string()),
        };
        let outcome = try_open_in_existing_instance_in(dir.path(), &launch, Duration::from_secs(2));
        assert_eq!(
            outcome,
            HandoffOutcome::Opened {
                pid: 2002,
                window: 3
            }
        );

        let envelope = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        assert_eq!(envelope.output_format, OutputFormat::Json);
        match envelope.request {
            ControlRequest::WithToken {
                token: sent_token,
                request,
            } => {
                assert_eq!(sent_token, token);
                assert_eq!(
                    *request,
                    ControlRequest::NewWindow {
                        title: None,
                        command: Some("htop".to_string()),
                        args: vec!["-d".to_string(), "5".to_string()],
                        working_dir: Some("/tmp/project".to_string()),
                    }
                );
            }
            other => panic!("expected authenticated NewWindow, got {other:?}"),
        }
    }

    #[test]
    fn handoff_falls_back_when_no_instance_is_running() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = try_open_in_existing_instance_in(
            dir.path(),
            &NewWindowLaunch::default(),
            Duration::from_millis(100),
        );
        assert_eq!(outcome, HandoffOutcome::StartNewProcess);
    }

    #[test]
    fn handoff_skips_stale_socket_and_uses_live_instance() {
        let dir = tempfile::tempdir().unwrap();
        let (_token, _rx, server) = spawn_mock_instance(
            dir.path(),
            3001,
            ControlResponse::Data(ControlData::WindowOpened {
                window: 1,
                tab_index: 1,
                pane: 2,
                warning: None,
            }),
        );
        std::thread::sleep(Duration::from_millis(20));
        let stale_endpoint = ControlEndpoint::for_pid(dir.path(), 3002);
        let stale_listener = UnixListener::bind(&stale_endpoint.socket).unwrap();
        drop(stale_listener);

        let outcome = try_open_in_existing_instance_in(
            dir.path(),
            &NewWindowLaunch::default(),
            Duration::from_secs(2),
        );
        server.join().unwrap();
        assert_eq!(
            outcome,
            HandoffOutcome::Opened {
                pid: 3001,
                window: 1
            }
        );
    }

    #[test]
    fn handoff_falls_back_after_short_timeout_on_unresponsive_instance() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = ControlEndpoint::for_pid(dir.path(), 4001);
        write_auth_token(&endpoint.token).unwrap();
        let listener = UnixListener::bind(&endpoint.socket).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            let _ = done_rx.recv_timeout(Duration::from_secs(5));
        });

        let start = Instant::now();
        let outcome = try_open_in_existing_instance_in(
            dir.path(),
            &NewWindowLaunch::default(),
            Duration::from_millis(120),
        );
        let elapsed = start.elapsed();
        let _ = done_tx.send(());
        server.join().unwrap();

        assert_eq!(outcome, HandoffOutcome::StartNewProcess);
        assert!(
            elapsed < Duration::from_secs(2),
            "handoff should time out promptly, took {elapsed:?}"
        );
    }

    #[test]
    fn handoff_falls_back_on_error_response() {
        let dir = tempfile::tempdir().unwrap();
        let (_token, _rx, server) = spawn_mock_instance(
            dir.path(),
            5001,
            ControlResponse::Error {
                code: 2,
                message: "cannot open window".to_string(),
            },
        );
        let outcome = try_open_in_existing_instance_in(
            dir.path(),
            &NewWindowLaunch::default(),
            Duration::from_secs(2),
        );
        server.join().unwrap();
        assert_eq!(outcome, HandoffOutcome::StartNewProcess);
    }

    #[test]
    fn quick_handoff_forwards_activation_token_and_reports_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let (token, rx, server) = spawn_mock_instance(
            dir.path(),
            6001,
            ControlResponse::Data(ControlData::QuickToggled {
                window: Some(2),
                visible: true,
            }),
        );

        let outcome = try_toggle_quick_in_existing_instance_in(
            dir.path(),
            Some("xdg-tok-42".to_string()),
            Duration::from_secs(2),
        );
        assert_eq!(
            outcome,
            QuickHandoffOutcome::Toggled {
                pid: 6001,
                window: Some(2),
                visible: true,
            }
        );

        let envelope = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        match envelope.request {
            ControlRequest::WithToken {
                token: sent_token,
                request,
            } => {
                assert_eq!(sent_token, token);
                assert_eq!(
                    *request,
                    ControlRequest::ToggleQuick {
                        activation_token: Some("xdg-tok-42".to_string()),
                    }
                );
            }
            other => panic!("expected authenticated ToggleQuick, got {other:?}"),
        }
    }
}
