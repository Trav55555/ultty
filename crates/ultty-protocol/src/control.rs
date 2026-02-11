//! Control protocol for CLI automation (ulttyctl).
//!
//! This module defines messages for external control of ultty,
//! separate from the TUI app protocol.

use crate::{Key, Modifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

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

    // === Misc ===
    Ping,

    Version,
}

/// Agent status update for terminal status bar display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusUpdate {
    pub cost_usd: f64,
    pub budget_usd: f64,
    pub pressure: String,
    pub model: String,
    pub context_quality: String,
    pub step: String,
    pub agent_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlResponse {
    Ok,
    Text(String),
    Data(ControlData),
    Error { code: u16, message: String },
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub line: usize,
    pub col: usize,
    pub text: String,
    pub context_before: String,
    pub context_after: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalInfo {
    pub cols: u16,
    pub rows: u16,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub current_tab: usize,
    pub tab_count: usize,
    pub focused: bool,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    pub index: usize,
    pub title: String,
    pub active: bool,
    pub pid: Option<u32>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub protocol_version: u16,
    pub git_hash: Option<String>,
}

pub fn control_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(runtime_dir).join("ultty-ctl.sock")
}

pub fn auth_token_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(runtime_dir).join("ultty-auth.token")
}

pub const CONTROL_MAGIC: [u8; 2] = [0x55, 0x43]; // "UC"

pub const CONTROL_VERSION: u16 = 1;
