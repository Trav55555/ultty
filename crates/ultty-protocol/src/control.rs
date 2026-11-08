//! Control protocol for CLI automation (ulttyctl).
//!
//! This module defines messages for external control of ultty,
//! separate from the TUI app protocol.

use crate::{Key, Modifiers};
use serde::{Deserialize, Serialize};

/// Output format used by `ulttyctl` responses.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Human-readable plain text output.
    #[default]
    Text,
    /// Machine-readable JSON output.
    Json,
}

/// Complete control socket request envelope.
///
/// `ulttyctl` sends this structure so automation can preserve argument
/// boundaries and request machine-readable responses without relying on
/// shell-like command parsing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlEnvelope {
    /// Preferred response format for this client.
    #[serde(default)]
    pub output_format: OutputFormat,
    /// Typed request to execute.
    pub request: ControlRequest,
}

/// Request message sent over the control socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    // === Input Injection ===
    SendText {
        text: String,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    SendKey {
        key: Key,
        modifiers: Modifiers,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    SendRaw {
        data: Vec<u8>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Command Execution ===
    NewTab {
        title: Option<String>,
        command: Option<String>,
        args: Vec<String>,
        working_dir: Option<String>,
    },

    RunCommand {
        command: String,
        args: Vec<String>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    CloseTab {
        index: Option<usize>,
    },

    // === State Queries ===
    GetText {
        include_scrollback: bool,
        scrollback_lines: Option<usize>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    GetSelection {
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    ListTabs,

    /// Every pane in every tab, tabs in order and panes in layout order.
    ListPanes,

    GetInfo,

    // === Search ===
    Search {
        pattern: String,
        regex: bool,
        case_sensitive: bool,
        include_scrollback: bool,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Wait Operations ===
    /// Wait until something happens in a pane: `pattern` appears (the
    /// default), or the event `until` names. The pane is fixed when the
    /// request arrives: `target`, or the pane focused at that moment.
    WaitFor {
        /// Text to wait for; ignored unless `until` is `Text`.
        #[serde(default)]
        pattern: String,
        #[serde(default)]
        regex: bool,
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "WaitCondition::is_text")]
        until: WaitCondition,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Events ===
    /// Turn this connection into an event stream: after the `Subscribed`
    /// reply, one [`ControlEvent`] per line until the client disconnects.
    Subscribe {
        /// Only events about this pane; every pane's events when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Window Control ===
    FocusWindow,

    SwitchTab {
        index: usize,
    },

    ToggleFullscreen,

    Resize {
        width: u32,
        height: u32,
    },

    SetOpacity {
        opacity: f32,
    },

    SetAgentStatus {
        status: AgentStatusUpdate,
    },

    // === Screenshot ===
    Screenshot {
        path: Option<String>,
    },

    // === Batch Mode ===
    Batch {
        commands: Vec<ControlRequest>,
    },

    // === Auth ===
    Authenticate {
        token: String,
    },

    /// Authenticate and execute `request` in one round trip.
    ///
    /// The control server handles one request per connection, so clients
    /// (ulttyctl) wrap their request with the token the
    /// instance wrote at start. The response is the inner request's response.
    WithToken {
        token: String,
        request: Box<ControlRequest>,
    },

    // === Misc ===
    Ping,

    Version,
}

/// What a `WaitFor` request waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitCondition {
    /// The pattern appears on screen or in scrollback.
    #[default]
    Text,
    /// The next command finishes (OSC 133;D; needs shell integration).
    CommandFinished,
    /// The bell rings.
    Bell,
    /// No output for `ms` milliseconds.
    Quiet { ms: u64 },
}

impl WaitCondition {
    pub fn is_text(&self) -> bool {
        *self == WaitCondition::Text
    }
}

/// Agent status update for terminal status bar display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusUpdate {
    /// Current cumulative spend in USD.
    pub cost_usd: f64,
    /// Configured budget ceiling in USD.
    pub budget_usd: f64,
    /// Pressure indicator (for example: low/normal/high).
    pub pressure: String,
    /// Active model name.
    pub model: String,
    /// Context quality summary shown in the status bar.
    pub context_quality: String,
    /// Current workflow step.
    pub step: String,
    /// Agent display name.
    pub agent_name: String,
}

impl AgentStatusUpdate {
    /// Validate that numeric fields are finite (not NaN or Inf).
    pub fn is_valid(&self) -> bool {
        self.cost_usd.is_finite() && self.budget_usd.is_finite()
    }

    /// Clamp cost and budget to finite values, replacing NaN/Inf with 0.0.
    pub fn sanitized(mut self) -> Self {
        if !self.cost_usd.is_finite() {
            self.cost_usd = 0.0;
        }
        if !self.budget_usd.is_finite() {
            self.budget_usd = 0.0;
        }
        self
    }
}

/// Response envelope returned by the control server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlResponse {
    Ok,
    Text { text: String },
    Data(ControlData),
    Error { code: u16, message: String },
}

/// Structured payload for successful control responses.
///
/// Variants carry named fields: serde's internal `type` tag cannot wrap a
/// bare string, list or option, so such a variant would fail to serialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlData {
    Info(TerminalInfo),
    Tabs {
        tabs: Vec<TabInfo>,
    },
    Panes {
        panes: Vec<PaneInfo>,
    },
    Selection {
        text: Option<String>,
    },
    Version(VersionInfo),
    Pong,
    ScreenshotSaved {
        path: String,
    },
    ScreenshotData {
        data: Vec<u8>,
    },
    SearchResults {
        matches: Vec<SearchMatch>,
    },
    BatchResults {
        results: Vec<ControlResponse>,
    },
    Authenticated,
    WaitMatched {
        line: usize,
        col: usize,
    },
    /// A `WaitFor` for an event saw it.
    WaitEvent {
        event: ControlEvent,
    },
    /// A `WaitFor` for quiet saw no output for the whole period.
    WaitQuiet,
    WaitTimeout,
    /// The connection is now an event stream.
    Subscribed,
}

/// Something that happened in a pane, sent to subscribers one per line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ControlEvent {
    /// A pane was opened (a new tab or a split).
    PaneOpened { pane: u64, tab_index: usize },
    /// A pane was closed.
    PaneClosed { pane: u64 },
    /// The shell started running a command (OSC 133;C).
    CommandStarted { pane: u64 },
    /// The command finished (OSC 133;D), with its exit code when the shell
    /// reported one.
    CommandFinished { pane: u64, exit_code: Option<i32> },
    /// The bell rang.
    Bell { pane: u64 },
    /// A program asked for a desktop notification (OSC 9/777).
    Notification {
        pane: u64,
        title: String,
        body: String,
    },
    /// A program set the pane title (OSC 0/2).
    TitleChanged { pane: u64, title: String },
    /// The pane's shell process exited.
    ProcessExited { pane: u64, status: i32 },
}

impl ControlEvent {
    /// The pane the event is about.
    pub fn pane(&self) -> u64 {
        match self {
            ControlEvent::PaneOpened { pane, .. }
            | ControlEvent::PaneClosed { pane }
            | ControlEvent::CommandStarted { pane }
            | ControlEvent::CommandFinished { pane, .. }
            | ControlEvent::Bell { pane }
            | ControlEvent::Notification { pane, .. }
            | ControlEvent::TitleChanged { pane, .. }
            | ControlEvent::ProcessExited { pane, .. } => *pane,
        }
    }
}

/// A single text search match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    /// Zero-based line number of the match.
    pub line: usize,
    /// Zero-based column of the match.
    pub col: usize,
    /// Matched text.
    pub text: String,
    /// Context preceding the match.
    pub context_before: String,
    /// Context following the match.
    pub context_after: String,
}

/// Snapshot of terminal state returned by `get_info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalInfo {
    /// Terminal width in cells.
    pub cols: u16,
    /// Terminal height in cells.
    pub rows: u16,
    /// Cursor column in cells.
    pub cursor_col: u16,
    /// Cursor row in cells.
    pub cursor_row: u16,
    /// Currently selected tab index.
    pub current_tab: usize,
    /// Total number of tabs.
    pub tab_count: usize,
    /// Whether this window has focus.
    pub focused: bool,
    /// Working directory for the active tab, if known.
    pub cwd: Option<String>,
}

/// Metadata describing a single tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    /// Tab index.
    pub index: usize,
    /// User-visible tab title.
    pub title: String,
    /// Whether this tab is currently active.
    pub active: bool,
    /// Foreground process ID when available.
    pub pid: Option<u32>,
    /// Working directory for the tab, if known.
    pub cwd: Option<String>,
}

/// Metadata describing a single pane (one terminal in a tab's layout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneInfo {
    /// Pane id. Unique within one Ultty process and never reused, so it
    /// stays valid across splits, focus changes and tab switches until the
    /// pane closes. Treat it as opaque.
    pub id: u64,
    /// Index of the tab holding the pane.
    pub tab_index: usize,
    /// Whether this is the focused pane of its tab.
    pub active: bool,
    /// Whether this pane has keyboard focus: the active pane of the active tab.
    pub focused: bool,
    /// Title the program set (OSC 0/2); empty when none was set.
    pub title: String,
    /// Name of the foreground process, when it can be read.
    pub foreground: Option<String>,
    /// Shell process ID.
    pub pid: Option<u32>,
    /// Working directory, if known.
    pub cwd: Option<String>,
    /// Pane width in cells.
    pub cols: u16,
    /// Pane height in cells.
    pub rows: u16,
}

/// Build and protocol version information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Semantic version string.
    pub version: String,
    /// Control protocol version.
    pub protocol_version: u16,
    /// Optional VCS revision hash.
    pub git_hash: Option<String>,
}

/// Control protocol frame magic (`"UC"`).
pub const CONTROL_MAGIC: [u8; 2] = [0x55, 0x43]; // "UC"

/// Current control protocol version.
pub const CONTROL_VERSION: u16 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    fn every_response() -> Vec<ControlResponse> {
        let data = vec![
            ControlData::Info(TerminalInfo {
                cols: 80,
                rows: 24,
                cursor_col: 1,
                cursor_row: 2,
                current_tab: 0,
                tab_count: 1,
                focused: true,
                cwd: Some("/home".to_string()),
            }),
            ControlData::Tabs {
                tabs: vec![TabInfo {
                    index: 0,
                    title: "zsh".to_string(),
                    active: true,
                    pid: Some(42),
                    cwd: None,
                }],
            },
            ControlData::Panes {
                panes: vec![PaneInfo {
                    id: 7,
                    tab_index: 0,
                    active: true,
                    focused: false,
                    title: "vim".to_string(),
                    foreground: Some("vim".to_string()),
                    pid: Some(42),
                    cwd: None,
                    cols: 40,
                    rows: 24,
                }],
            },
            ControlData::Selection {
                text: Some("picked".to_string()),
            },
            ControlData::Selection { text: None },
            ControlData::Version(VersionInfo {
                version: "0.1.0".to_string(),
                protocol_version: 1,
                git_hash: None,
            }),
            ControlData::Pong,
            ControlData::ScreenshotSaved {
                path: "/tmp/shot.png".to_string(),
            },
            ControlData::ScreenshotData {
                data: vec![1, 2, 3],
            },
            ControlData::SearchResults {
                matches: vec![SearchMatch {
                    line: 3,
                    col: 4,
                    text: "needle".to_string(),
                    context_before: "a ".to_string(),
                    context_after: " b".to_string(),
                }],
            },
            ControlData::BatchResults {
                results: vec![ControlResponse::Ok],
            },
            ControlData::Authenticated,
            ControlData::WaitMatched { line: 1, col: 2 },
            ControlData::WaitTimeout,
            ControlData::Subscribed,
            ControlData::WaitEvent {
                event: ControlEvent::CommandFinished {
                    pane: 1,
                    exit_code: Some(2),
                },
            },
            ControlData::WaitQuiet,
        ];
        let mut responses = vec![
            ControlResponse::Ok,
            ControlResponse::Text {
                text: "screen".to_string(),
            },
            ControlResponse::Error {
                code: 4,
                message: "nope".to_string(),
            },
        ];
        responses.extend(data.into_iter().map(ControlResponse::Data));
        responses
    }

    #[test]
    fn events_are_flat_json_objects_named_by_event() {
        let event = ControlEvent::CommandFinished {
            pane: 3,
            exit_code: Some(1),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"event":"command_finished","pane":3,"exit_code":1}"#
        );
        assert_eq!(serde_json::from_str::<ControlEvent>(&json).unwrap(), event);
        assert_eq!(event.pane(), 3);
    }

    #[test]
    fn wait_condition_defaults_to_text_and_round_trips() {
        let old: ControlRequest = serde_json::from_str(
            r#"{"type":"wait_for","pattern":"ok","regex":false,"timeout_ms":5}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            ControlRequest::WaitFor {
                until: WaitCondition::Text,
                ..
            }
        ));
        for until in [
            WaitCondition::CommandFinished,
            WaitCondition::Bell,
            WaitCondition::Quiet { ms: 500 },
        ] {
            let request = ControlRequest::WaitFor {
                pattern: String::new(),
                regex: false,
                timeout_ms: 1000,
                until,
                target: None,
            };
            let json = serde_json::to_string(&request).unwrap();
            assert_eq!(
                serde_json::from_str::<ControlRequest>(&json).unwrap(),
                request
            );
        }
    }

    /// Clients written before pane targets keep working: an omitted `target`
    /// means the focused pane, and an unset one is not sent.
    #[test]
    fn target_is_optional_on_the_wire() {
        let old: ControlRequest =
            serde_json::from_str(r#"{"type":"send_text","text":"hi"}"#).unwrap();
        assert_eq!(
            old,
            ControlRequest::SendText {
                text: "hi".to_string(),
                target: None
            }
        );
        let old: ControlRequest = serde_json::from_str(r#"{"type":"get_selection"}"#).unwrap();
        assert_eq!(old, ControlRequest::GetSelection { target: None });

        let untargeted = serde_json::to_string(&ControlRequest::GetSelection { target: None });
        assert_eq!(untargeted.unwrap(), r#"{"type":"get_selection"}"#);
        let targeted: ControlRequest =
            serde_json::from_str(r#"{"type":"get_selection","target":4}"#).unwrap();
        assert_eq!(targeted, ControlRequest::GetSelection { target: Some(4) });
    }

    /// ulttyctl reads every reply as JSON, so each one must survive the trip.
    #[test]
    fn every_response_round_trips_through_json() {
        for response in every_response() {
            let json = serde_json::to_string(&response)
                .unwrap_or_else(|e| panic!("{response:?} does not serialize: {e}"));
            let back: ControlResponse = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{json} does not parse back: {e}"));
            assert_eq!(back, response);
        }
    }
}
