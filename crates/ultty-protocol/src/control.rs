//! Control protocol for CLI automation (ulttyctl).
//!
//! This module defines messages for external control of ultty,
//! separate from the TUI app protocol.

use crate::{Key, Modifiers};

/// Control messages from ulttyctl to terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlRequest {
    // === Input Injection ===
    /// Send text as if typed.
    SendText {
        text: String,
    },

    /// Send a key press.
    SendKey {
        key: Key,
        modifiers: Modifiers,
    },

    /// Send raw bytes to PTY.
    SendRaw {
        data: Vec<u8>,
    },

    // === Command Execution ===
    /// Create a new tab with optional command.
    NewTab {
        title: Option<String>,
        command: Option<String>,
        args: Vec<String>,
        working_dir: Option<String>,
    },

    /// Run command in current tab.
    RunCommand {
        command: String,
        args: Vec<String>,
    },

    /// Close a tab by index (None = current).
    CloseTab {
        index: Option<usize>,
    },

    // === State Queries ===
    /// Get visible text from terminal.
    GetText {
        /// Include scrollback buffer.
        include_scrollback: bool,
        /// Number of scrollback lines (None = all).
        scrollback_lines: Option<usize>,
    },

    /// Get current selection.
    GetSelection,

    /// List all tabs.
    ListTabs,

    /// Get terminal info (size, cursor pos, etc.).
    GetInfo,

    // === Window Control ===
    /// Focus the ultty window.
    FocusWindow,

    /// Switch to tab by index.
    SwitchTab {
        index: usize,
    },

    /// Toggle fullscreen.
    ToggleFullscreen,

    /// Resize window.
    Resize {
        width: u32,
        height: u32,
    },

    /// Set window opacity.
    SetOpacity {
        opacity: f32,
    },

    // === Screenshot ===
    Screenshot {
        path: Option<String>,
    },

    // === Misc ===
    Ping,

    Version,
}

/// Control responses from terminal to ulttyctl.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlResponse {
    /// Success with no data.
    Ok,

    /// Success with text data.
    Text(String),

    /// Success with structured data.
    Data(ControlData),

    /// Error response.
    Error { code: u16, message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlData {
    Info(TerminalInfo),
    Tabs(Vec<TabInfo>),
    Selection(Option<String>),
    Version(VersionInfo),
    Pong,
    ScreenshotSaved(String),
    ScreenshotData(Vec<u8>),
}

/// Terminal information.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalInfo {
    /// Terminal width in columns.
    pub cols: u16,
    /// Terminal height in rows.
    pub rows: u16,
    /// Cursor column.
    pub cursor_col: u16,
    /// Cursor row.
    pub cursor_row: u16,
    /// Current tab index.
    pub current_tab: usize,
    /// Total number of tabs.
    pub tab_count: usize,
    /// Window is focused.
    pub focused: bool,
    /// Current working directory.
    pub cwd: Option<String>,
}

/// Tab information.
#[derive(Debug, Clone, PartialEq)]
pub struct TabInfo {
    /// Tab index.
    pub index: usize,
    /// Tab title.
    pub title: String,
    /// Tab is active.
    pub active: bool,
    /// Process ID.
    pub pid: Option<u32>,
    /// Current working directory.
    pub cwd: Option<String>,
}

/// Version information.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionInfo {
    /// ultty version.
    pub version: String,
    /// Protocol version.
    pub protocol_version: u16,
    /// Git commit hash.
    pub git_hash: Option<String>,
}

/// Control socket path.
pub fn control_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(runtime_dir).join("ultty-ctl.sock")
}

/// Magic bytes for control protocol.
pub const CONTROL_MAGIC: [u8; 2] = [0x55, 0x43]; // "UC"

/// Control protocol version.
pub const CONTROL_VERSION: u16 = 1;
