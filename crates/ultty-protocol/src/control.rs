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
    },

    SendKey {
        key: Key,
        modifiers: Modifiers,
    },

    SendRaw {
        data: Vec<u8>,
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
    },

    CloseTab {
        index: Option<usize>,
    },

    // === State Queries ===
    GetText {
        include_scrollback: bool,
        scrollback_lines: Option<usize>,
    },

    GetSelection,

    ListTabs,

    GetInfo,

    // === Search ===
    Search {
        pattern: String,
        regex: bool,
        case_sensitive: bool,
        include_scrollback: bool,
    },

    // === Wait Operations ===
    WaitFor {
        pattern: String,
        regex: bool,
        timeout_ms: u64,
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
    /// (ulttyctl) wrap their request with the token from
    /// [`auth_token_path`]. The response is the inner request's response.
    WithToken {
        token: String,
        request: Box<ControlRequest>,
    },

    // === Misc ===
    Ping,

    Version,
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
    Tabs { tabs: Vec<TabInfo> },
    Selection { text: Option<String> },
    Version(VersionInfo),
    Pong,
    ScreenshotSaved { path: String },
    ScreenshotData { data: Vec<u8> },
    SearchResults { matches: Vec<SearchMatch> },
    BatchResults { results: Vec<ControlResponse> },
    Authenticated,
    WaitMatched { line: usize, col: usize },
    WaitTimeout,
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

/// Path to the Unix domain socket used by the control server.
pub fn control_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(runtime_dir).join("ultty-ctl.sock")
}

/// Path to the control auth token file.
pub fn auth_token_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(runtime_dir).join("ultty-auth.token")
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
