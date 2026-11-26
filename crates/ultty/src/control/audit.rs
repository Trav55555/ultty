//! The control-plane audit trail: who connected to a control socket, whether
//! they authenticated, and what they asked for.
//!
//! Every line goes to the `ultty::audit` tracing target, so it lands in the
//! log file with everything else and can be filtered on its own
//! (`RUST_LOG=ultty::audit=debug`). Requests that change something or read
//! what is on screen are recorded at INFO; status queries
//! ([`RequestSummary::routine`]) at DEBUG. Text a request carries (typed
//! text, commands, patterns, override sources) is recorded only as a byte
//! count, and tokens never.

use std::fmt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use ultty_protocol::control::ControlRequest;

/// Which control socket a request arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A terminal window's own control socket.
    Local,
    /// A persistent session host's control socket (`ultty-session`).
    SessionHost,
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Local => "local",
            Self::SessionHost => "session-host",
        })
    }
}

/// The process on the other end of a control connection, as the kernel
/// reports it (`SO_PEERCRED`). Fields the platform cannot report are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerIdentity {
    /// The peer's process id when it connected.
    pub pid: Option<i32>,
    /// The peer's user id.
    pub uid: Option<u32>,
    /// The peer's group id.
    pub gid: Option<u32>,
    /// The peer's executable, from `/proc/<pid>/exe`; best effort.
    pub exe: Option<PathBuf>,
}

impl PeerIdentity {
    /// Ask the kernel who is on the other end of `stream`.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn of(stream: &UnixStream) -> Self {
        use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};

        match getsockopt(stream, PeerCredentials) {
            Ok(credentials) => Self {
                pid: Some(credentials.pid()),
                uid: Some(credentials.uid()),
                gid: Some(credentials.gid()),
                exe: std::fs::read_link(format!("/proc/{}/exe", credentials.pid())).ok(),
            },
            Err(error) => {
                tracing::debug!(target: "ultty::audit", %error, "No peer credentials");
                Self::default()
            }
        }
    }

    /// Ask the kernel who is on the other end of `stream`.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn of(_stream: &UnixStream) -> Self {
        Self::default()
    }
}

/// `Some(value)` as `value`, `None` as `-`, for log fields.
struct OrDash<T>(Option<T>);

impl<T: fmt::Display> fmt::Display for OrDash<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("-"),
        }
    }
}

/// What a request is, stripped of anything secret: its kind, what it acts
/// on, and how many bytes of text it carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestSummary {
    /// The request's operation name, as in `describe`'s list.
    pub kind: &'static str,
    /// The pane it targets, when it names one.
    pub pane: Option<u64>,
    /// The window it targets, when it names one.
    pub window: Option<u64>,
    /// The tab index it names.
    pub tab: Option<usize>,
    /// Bytes of text the request carries; never the text itself.
    pub bytes: Option<usize>,
    /// Commands in a batch.
    pub commands: Option<usize>,
    /// Whether it arrived with a token (`with_token`).
    pub with_token: bool,
    /// A status query, recorded at DEBUG rather than INFO.
    pub routine: bool,
}

impl fmt::Display for RequestSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kind={}", self.kind)?;
        for (name, value) in [
            ("pane", self.pane),
            ("window", self.window),
            ("tab", self.tab.map(|tab| tab as u64)),
            ("bytes", self.bytes.map(|bytes| bytes as u64)),
            ("commands", self.commands.map(|count| count as u64)),
        ] {
            if let Some(value) = value {
                write!(f, " {name}={value}")?;
            }
        }
        if self.with_token {
            f.write_str(" with_token=true")?;
        }
        Ok(())
    }
}

impl RequestSummary {
    /// Summarise `request`, looking through `with_token` and `with_owner`.
    pub fn of(request: &ControlRequest) -> Self {
        let (inner, with_token) = unwrap_envelopes(request);
        let mut summary = Self {
            with_token,
            ..Self::default()
        };
        let text = |parts: &[&str]| Some(parts.iter().map(|part| part.len()).sum());
        let program = |command: Option<&str>, args: &[String]| {
            Some(command.map_or(0, str::len) + args.iter().map(String::len).sum::<usize>())
        };
        let (kind, routine) = match inner {
            ControlRequest::WithToken { .. } => ("with_token", true),
            ControlRequest::WithOwner { .. } => ("with_owner", true),
            ControlRequest::Describe => ("describe", true),
            ControlRequest::Ping => ("ping", true),
            ControlRequest::Version => ("version", true),
            ControlRequest::GetInfo => ("get_info", true),
            ControlRequest::ListWindows => ("list_windows", true),
            ControlRequest::ListTabs => ("list_tabs", true),
            ControlRequest::ListPanes => ("list_panes", true),
            ControlRequest::WorkspaceList => ("workspace_list", true),
            ControlRequest::Authenticate { .. } => ("authenticate", true),
            ControlRequest::ProgramStatus { target } => {
                summary.pane = *target;
                ("program_status", true)
            }
            ControlRequest::Images { target } => {
                summary.pane = *target;
                ("images", true)
            }
            ControlRequest::JobStatus { job } => {
                summary.pane = Some(*job);
                ("job_status", true)
            }
            ControlRequest::WaitJob { job, .. } => {
                summary.pane = Some(*job);
                ("wait_job", true)
            }
            ControlRequest::SubscribeV2 { target, .. } => {
                summary.pane = *target;
                ("subscribe_v2", false)
            }
            ControlRequest::Subscribe { target } => {
                summary.pane = *target;
                ("subscribe", false)
            }
            ControlRequest::SendText { text: sent, target } => {
                summary.pane = *target;
                summary.bytes = text(&[sent]);
                ("send_text", false)
            }
            ControlRequest::SendKey { target, .. } => {
                summary.pane = *target;
                ("send_key", false)
            }
            ControlRequest::SendRaw { data, target } => {
                summary.pane = *target;
                summary.bytes = Some(data.len());
                ("send_raw", false)
            }
            ControlRequest::NewTab {
                command,
                args,
                window,
                pane,
                ..
            } => {
                summary.pane = *pane;
                summary.window = *window;
                summary.bytes = program(command.as_deref(), args);
                ("new_tab", false)
            }
            ControlRequest::NewWindow { command, args, .. } => {
                summary.bytes = program(command.as_deref(), args);
                ("new_window", false)
            }
            ControlRequest::CloseWindow { window } => {
                summary.window = Some(*window);
                ("close_window", false)
            }
            ControlRequest::MoveTab { tab, window, .. } => {
                summary.tab = Some(*tab);
                summary.window = *window;
                ("move_tab", false)
            }
            ControlRequest::ToggleQuick { .. } => ("toggle_quick", false),
            ControlRequest::ShowQuick { .. } => ("show_quick", false),
            ControlRequest::HideQuick => ("hide_quick", false),
            ControlRequest::RunCommand {
                command,
                args,
                target,
            } => {
                summary.pane = *target;
                summary.bytes = program(Some(command), args);
                ("run_command", false)
            }
            ControlRequest::RunCommandAndWait {
                command, target, ..
            } => {
                summary.pane = *target;
                summary.bytes = text(&[command]);
                ("run_command_and_wait", false)
            }
            ControlRequest::SplitPane {
                command,
                args,
                target,
                ..
            } => {
                summary.pane = *target;
                summary.bytes = program(command.as_deref(), args);
                ("split_pane", false)
            }
            ControlRequest::CloseTab { index } => {
                summary.tab = *index;
                ("close_tab", false)
            }
            ControlRequest::GetText { target, .. } => {
                summary.pane = *target;
                ("get_text", false)
            }
            ControlRequest::GetSelection { target } => {
                summary.pane = *target;
                ("get_selection", false)
            }
            ControlRequest::WorkspaceSave { name } => {
                summary.bytes = text(&[name]);
                ("workspace_save", false)
            }
            ControlRequest::Regions { target } => {
                summary.pane = *target;
                ("regions", false)
            }
            ControlRequest::PushOverride { source, target, .. } => {
                summary.pane = *target;
                summary.bytes = text(&[source]);
                ("push_override", false)
            }
            ControlRequest::ClearOverride { target } => {
                summary.pane = *target;
                ("clear_override", false)
            }
            ControlRequest::InvokeAction { target, .. } => {
                summary.pane = Some(*target);
                ("invoke_action", false)
            }
            ControlRequest::LastCommand { target, .. } => {
                summary.pane = *target;
                ("last_command", false)
            }
            ControlRequest::Search {
                pattern, target, ..
            } => {
                summary.pane = *target;
                summary.bytes = text(&[pattern]);
                ("search", false)
            }
            ControlRequest::WaitFor {
                pattern, target, ..
            } => {
                summary.pane = *target;
                summary.bytes = text(&[pattern]);
                ("wait_for", false)
            }
            ControlRequest::FocusWindow { window, pane } => {
                summary.window = *window;
                summary.pane = *pane;
                ("focus_window", false)
            }
            ControlRequest::SwitchTab { index } => {
                summary.tab = Some(*index);
                ("switch_tab", false)
            }
            ControlRequest::FocusPane { target } => {
                summary.pane = Some(*target);
                ("focus_pane", false)
            }
            ControlRequest::ZoomPane { target, .. } => {
                summary.pane = Some(*target);
                ("zoom_pane", false)
            }
            ControlRequest::SwapPanes { first, .. } => {
                summary.pane = Some(*first);
                ("swap_panes", false)
            }
            ControlRequest::ToggleFullscreen { window, pane } => {
                summary.window = *window;
                summary.pane = *pane;
                ("toggle_fullscreen", false)
            }
            ControlRequest::Resize { window, pane, .. } => {
                summary.window = *window;
                summary.pane = *pane;
                ("resize", false)
            }
            ControlRequest::SetOpacity { window, pane, .. } => {
                summary.window = *window;
                summary.pane = *pane;
                ("set_opacity", false)
            }
            ControlRequest::SetPaneStatus { target, .. } => {
                summary.pane = *target;
                ("set_pane_status", false)
            }
            ControlRequest::Screenshot { window, pane, .. } => {
                summary.window = *window;
                summary.pane = *pane;
                ("screenshot", false)
            }
            ControlRequest::Batch { commands } => {
                summary.commands = Some(commands.len());
                ("batch", false)
            }
        };
        summary.kind = kind;
        summary.routine = routine;
        summary
    }
}

/// The requests to record for `request`: itself, then each command of a
/// batch (recursively), each summarised.
pub fn summaries(request: &ControlRequest) -> Vec<RequestSummary> {
    let mut out = vec![RequestSummary::of(request)];
    if let (ControlRequest::Batch { commands }, _) = unwrap_envelopes(request) {
        out.extend(commands.iter().flat_map(summaries));
    }
    out
}

/// The request inside any `with_token` and `with_owner` wrappers, and
/// whether a token wrapper was among them.
fn unwrap_envelopes(mut request: &ControlRequest) -> (&ControlRequest, bool) {
    let mut with_token = false;
    loop {
        match request {
            ControlRequest::WithToken { request: inner, .. } => {
                with_token = true;
                request = inner;
            }
            ControlRequest::WithOwner { request: inner, .. } => request = inner,
            _ => return (request, with_token),
        }
    }
}

/// Record the first request on a connection: who sent it. At DEBUG when
/// the connection asks only status queries (`routine`), so a poller does
/// not fill the log.
pub fn connection(transport: Transport, conn: usize, peer: &PeerIdentity, routine: bool) {
    let line = format!(
        "control connection conn={conn} transport={transport} peer_pid={} peer_uid={} peer_gid={} peer_exe={}",
        OrDash(peer.pid),
        OrDash(peer.uid),
        OrDash(peer.gid),
        OrDash(peer.exe.as_ref().map(|exe| exe.display())),
    );
    if routine {
        tracing::debug!(target: "ultty::audit", "{line}");
    } else {
        tracing::info!(target: "ultty::audit", "{line}");
    }
}

/// Record a connection that closed without sending a request, such as an
/// instance probe.
pub fn connection_without_request(transport: Transport, conn: usize, peer: &PeerIdentity) {
    tracing::debug!(
        target: "ultty::audit",
        "control connection closed without a request conn={conn} transport={transport} peer_pid={}",
        OrDash(peer.pid),
    );
}

/// Record a request; see [`RequestSummary::routine`] for its level.
pub fn request(transport: Transport, conn: usize, peer: &PeerIdentity, summary: &RequestSummary) {
    if summary.routine {
        tracing::debug!(
            target: "ultty::audit",
            "control request {summary} conn={conn} transport={transport} peer_pid={}",
            OrDash(peer.pid),
        );
    } else {
        tracing::info!(
            target: "ultty::audit",
            "control request {summary} conn={conn} transport={transport} peer_pid={}",
            OrDash(peer.pid),
        );
    }
}

/// Record a request the server could not read (malformed or too large).
pub fn unreadable_request(transport: Transport, conn: usize, peer: &PeerIdentity, bytes: usize) {
    tracing::warn!(
        target: "ultty::audit",
        "control request unreadable bytes={bytes} conn={conn} transport={transport} peer_pid={}",
        OrDash(peer.pid),
    );
}

/// How an authentication attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The token matched.
    Accepted,
    /// The token did not match.
    WrongToken,
    /// The expected token could not be read, so nothing could match.
    TokenUnavailable,
}

/// Record an authentication attempt: WARN when it failed; when it
/// succeeded INFO, or DEBUG for a connection asking only status queries.
/// The presented token is never recorded.
pub fn authentication(
    transport: Transport,
    conn: usize,
    peer: &PeerIdentity,
    outcome: AuthOutcome,
    routine: bool,
) {
    let pid = OrDash(peer.pid);
    match outcome {
        AuthOutcome::Accepted if routine => tracing::debug!(
            target: "ultty::audit",
            "control authentication succeeded conn={conn} transport={transport} peer_pid={pid}"
        ),
        AuthOutcome::Accepted => tracing::info!(
            target: "ultty::audit",
            "control authentication succeeded conn={conn} transport={transport} peer_pid={pid}"
        ),
        AuthOutcome::WrongToken => tracing::warn!(
            target: "ultty::audit",
            "control authentication failed reason=wrong_token conn={conn} transport={transport} peer_pid={pid}"
        ),
        AuthOutcome::TokenUnavailable => tracing::warn!(
            target: "ultty::audit",
            "control authentication failed reason=token_unavailable conn={conn} transport={transport} peer_pid={pid}"
        ),
    }
}

/// Record an error reply. Refusals for missing or wrong credentials (403)
/// are WARN, other errors INFO. The error message is not recorded: some
/// echo what the request named.
pub fn refusal(transport: Transport, conn: usize, peer: &PeerIdentity, code: u16) {
    let pid = OrDash(peer.pid);
    if code == 403 {
        tracing::warn!(
            target: "ultty::audit",
            "control request refused code={code} conn={conn} transport={transport} peer_pid={pid}"
        );
    } else {
        tracing::info!(
            target: "ultty::audit",
            "control request refused code={code} conn={conn} transport={transport} peer_pid={pid}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_summary_records_text_lengths_not_text() {
        let summary = RequestSummary::of(&ControlRequest::WithToken {
            token: "abcdef".into(),
            request: Box::new(ControlRequest::SendText {
                text: "hunter2".into(),
                target: Some(3),
            }),
        });
        assert_eq!(
            summary.to_string(),
            "kind=send_text pane=3 bytes=7 with_token=true"
        );
        assert!(!summary.routine);
        let run = RequestSummary::of(&ControlRequest::RunCommand {
            command: "echo".into(),
            args: vec!["secret".into()],
            target: None,
        });
        assert_eq!(run.to_string(), "kind=run_command bytes=10");
    }

    #[test]
    fn test_request_summary_marks_status_queries_routine() {
        for request in [
            ControlRequest::Ping,
            ControlRequest::ListPanes,
            ControlRequest::GetInfo,
            ControlRequest::JobStatus { job: 1 },
        ] {
            assert!(RequestSummary::of(&request).routine, "{request:?}");
        }
        for request in [
            ControlRequest::GetText {
                include_scrollback: false,
                scrollback_lines: None,
                target: None,
            },
            ControlRequest::GetSelection { target: None },
            ControlRequest::Screenshot {
                path: None,
                window: None,
                pane: None,
            },
            ControlRequest::CloseTab { index: Some(1) },
        ] {
            assert!(!RequestSummary::of(&request).routine, "{request:?}");
        }
    }

    #[test]
    fn test_summaries_list_each_batch_command() {
        let batch = ControlRequest::Batch {
            commands: vec![
                ControlRequest::Ping,
                ControlRequest::SendRaw {
                    data: vec![1, 2, 3],
                    target: None,
                },
            ],
        };
        let kinds: Vec<_> = summaries(&batch)
            .into_iter()
            .map(|summary| summary.to_string())
            .collect();
        assert_eq!(
            kinds,
            [
                "kind=batch commands=2",
                "kind=ping",
                "kind=send_raw bytes=3"
            ]
        );
    }

    #[test]
    fn test_peer_identity_reports_this_process() {
        let (left, _right) = UnixStream::pair().unwrap();
        let peer = PeerIdentity::of(&left);
        assert_eq!(peer.pid, Some(std::process::id() as i32));
        let process = std::fs::metadata("/proc/self").unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(peer.uid, Some(process.uid()));
        assert_eq!(peer.gid, Some(process.gid()));
        assert_eq!(peer.exe, std::env::current_exe().ok());
    }
}
