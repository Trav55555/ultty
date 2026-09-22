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
    Text(String),
    Data(ControlData),
    Error { code: u16, message: String },
}

/// Structured payload for successful control responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlData {
    Info(TerminalInfo),
    Tabs(Vec<TabInfo>),
    Selection(Option<String>),
    Version(VersionInfo),
    Pong,
    ScreenshotSaved(String),
    ScreenshotData(Vec<u8>),
    SearchResults(Vec<SearchMatch>),
    BatchResults(Vec<ControlResponse>),
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
