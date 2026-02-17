//! Native tmux integration support.
//!
//! This module provides support for running tmux inside Ultty with proper
//! escape sequence passthrough and control mode integration.

use std::collections::VecDeque;
use std::env;
use std::io::{self, Write};
use std::process::{Child, Command, Stdio};

/// Tmux integration mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxMode {
    /// Not using tmux integration.
    None,
    /// Running inside an existing tmux session (nested).
    Nested,
    /// Tmux control mode (-CC) for native pane/window management.
    ControlMode,
    /// Standard tmux session started by terminal.
    Standard,
}

/// Tmux passthrough state for parsing DCS sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PassthroughState {
    /// Not in a passthrough sequence.
    Normal,
    /// Saw ESC
    Escape,
    /// Saw ESC P (DCS start)
    DcsStart,
    /// Inside tmux passthrough, collecting inner content.
    InPassthrough,
    /// Saw inner ESC, might be end of passthrough.
    PassthroughEscape,
}

/// Tmux session information.
#[derive(Debug, Clone)]
pub struct TmuxSession {
    /// Session ID
    pub id: String,
    /// Session name
    pub name: String,
    /// Number of windows
    pub windows: usize,
    /// Whether this session is attached
    pub attached: bool,
}

/// Tmux window information.
#[derive(Debug, Clone)]
pub struct TmuxWindow {
    /// Window index
    pub index: usize,
    /// Window name
    pub name: String,
    /// Whether this is the active window
    pub active: bool,
    /// Number of panes
    pub panes: usize,
}

/// Tmux pane information.
#[derive(Debug, Clone)]
pub struct TmuxPane {
    /// Pane ID (%N format)
    pub id: String,
    /// Pane index within window
    pub index: usize,
    /// Whether this is the active pane
    pub active: bool,
    /// Pane width in columns
    pub width: u16,
    /// Pane height in rows
    pub height: u16,
    /// Current command running in pane
    pub command: String,
}

/// Control mode notification from tmux.
#[derive(Debug, Clone)]
pub enum TmuxNotification {
    /// Output from a pane: %output <pane_id> <data>
    Output { pane_id: String, data: Vec<u8> },
    /// Pane changed size: %layout-change <window_id> <layout>
    LayoutChange { window_id: String, layout: String },
    /// Session changed: %session-changed <session_id> <session_name>
    SessionChanged { id: String, name: String },
    /// Window added: %window-add <window_id>
    WindowAdd { window_id: String },
    /// Window closed: %window-close <window_id>
    WindowClose { window_id: String },
    /// Window renamed: %window-renamed <window_id> <name>
    WindowRenamed { window_id: String, name: String },
    /// Pane mode changed (copy mode, etc)
    PaneModeChanged { pane_id: String, mode: String },
    /// Client detached
    ClientDetached,
    /// Exit notification
    Exit { reason: String },
    /// Unknown notification
    Unknown(String),
}

/// Tmux control mode connection.
pub struct TmuxController {
    /// The tmux process in control mode.
    process: Option<Child>,
    /// Current mode
    mode: TmuxMode,
    /// Passthrough parser state
    passthrough_state: PassthroughState,
    /// Buffer for passthrough content
    passthrough_buffer: Vec<u8>,
    /// Pending notifications
    notifications: VecDeque<TmuxNotification>,
    /// Current session info (populated by list_sessions response)
    #[allow(dead_code)]
    session: Option<TmuxSession>,
    /// Current window list (populated by list_windows response)
    #[allow(dead_code)]
    windows: Vec<TmuxWindow>,
    /// Current pane list (populated by list_panes response)
    #[allow(dead_code)]
    panes: Vec<TmuxPane>,
}

impl TmuxController {
    /// Create a new tmux controller.
    pub fn new() -> Self {
        let mode = Self::detect_mode();

        Self {
            process: None,
            mode,
            passthrough_state: PassthroughState::Normal,
            passthrough_buffer: Vec::new(),
            notifications: VecDeque::new(),
            session: None,
            windows: Vec::new(),
            panes: Vec::new(),
        }
    }

    /// Detect the current tmux mode based on environment.
    fn detect_mode() -> TmuxMode {
        // Check if we're inside tmux
        if env::var("TMUX").is_ok() {
            TmuxMode::Nested
        } else {
            TmuxMode::None
        }
    }

    /// Get the current tmux mode.
    pub fn mode(&self) -> TmuxMode {
        self.mode
    }

    /// Check if we're running inside tmux.
    pub fn is_nested(&self) -> bool {
        self.mode == TmuxMode::Nested
    }

    /// Check if control mode is active.
    pub fn is_control_mode(&self) -> bool {
        self.mode == TmuxMode::ControlMode
    }

    /// Start a new tmux session in control mode.
    ///
    /// Control mode (-CC) allows the terminal to act as a native tmux client,
    /// managing windows and panes through the terminal's own UI.
    pub fn start_control_mode(&mut self, session_name: Option<&str>) -> io::Result<()> {
        let mut cmd = Command::new("tmux");
        cmd.arg("-CC"); // Control mode

        if let Some(name) = session_name {
            cmd.args(["-L", name]);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());

        let child = cmd.spawn()?;
        self.process = Some(child);
        self.mode = TmuxMode::ControlMode;

        tracing::info!("Started tmux control mode");
        Ok(())
    }

    /// Attach to an existing tmux session in control mode.
    pub fn attach_control_mode(&mut self, session: &str) -> io::Result<()> {
        let mut cmd = Command::new("tmux");
        cmd.args(["-CC", "attach-session", "-t", session]);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());

        let child = cmd.spawn()?;
        self.process = Some(child);
        self.mode = TmuxMode::ControlMode;

        tracing::info!("Attached to tmux session '{}' in control mode", session);
        Ok(())
    }

    /// Send a command to tmux in control mode.
    pub fn send_command(&mut self, command: &str) -> io::Result<()> {
        if let Some(ref mut child) = self.process {
            if let Some(ref mut stdin) = child.stdin {
                writeln!(stdin, "{}", command)?;
                stdin.flush()?;
            }
        }
        Ok(())
    }

    /// Send keys to a specific pane.
    pub fn send_keys(&mut self, pane_id: &str, keys: &str) -> io::Result<()> {
        self.send_command(&format!("send-keys -t {} {}", pane_id, keys))
    }

    /// Create a new window.
    pub fn new_window(&mut self, name: Option<&str>) -> io::Result<()> {
        let cmd = if let Some(n) = name {
            format!("new-window -n {}", n)
        } else {
            "new-window".to_string()
        };
        self.send_command(&cmd)
    }

    /// Split the current pane horizontally.
    pub fn split_horizontal(&mut self) -> io::Result<()> {
        self.send_command("split-window -h")
    }

    /// Split the current pane vertically.
    pub fn split_vertical(&mut self) -> io::Result<()> {
        self.send_command("split-window -v")
    }

    /// Select a specific pane.
    pub fn select_pane(&mut self, pane_id: &str) -> io::Result<()> {
        self.send_command(&format!("select-pane -t {}", pane_id))
    }

    /// Resize a pane.
    pub fn resize_pane(&mut self, pane_id: &str, width: u16, height: u16) -> io::Result<()> {
        self.send_command(&format!(
            "resize-pane -t {} -x {} -y {}",
            pane_id, width, height
        ))
    }

    /// Kill a pane.
    pub fn kill_pane(&mut self, pane_id: &str) -> io::Result<()> {
        self.send_command(&format!("kill-pane -t {}", pane_id))
    }

    /// List all sessions.
    pub fn list_sessions(&mut self) -> io::Result<()> {
        self.send_command("list-sessions -F '#{session_id}:#{session_name}:#{session_windows}:#{session_attached}'")
    }

    /// List all windows in current session.
    pub fn list_windows(&mut self) -> io::Result<()> {
        self.send_command(
            "list-windows -F '#{window_index}:#{window_name}:#{window_active}:#{window_panes}'",
        )
    }

    /// List all panes in current window.
    pub fn list_panes(&mut self) -> io::Result<()> {
        self.send_command("list-panes -F '#{pane_id}:#{pane_index}:#{pane_active}:#{pane_width}:#{pane_height}:#{pane_current_command}'")
    }

    /// Process output from tmux control mode.
    ///
    /// Returns parsed notifications.
    pub fn process_output(&mut self, data: &[u8]) -> Vec<TmuxNotification> {
        let mut notifications = Vec::new();

        // Parse line by line
        let text = String::from_utf8_lossy(data);
        for line in text.lines() {
            if let Some(notification) = self.parse_notification(line) {
                notifications.push(notification);
            }
        }

        notifications
    }

    /// Parse a single notification line from tmux.
    fn parse_notification(&self, line: &str) -> Option<TmuxNotification> {
        if line.starts_with('%') {
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            match parts.first().copied() {
                Some("%output") => {
                    let pane_id = parts.get(1)?.to_string();
                    let data = parts.get(2).unwrap_or(&"").as_bytes().to_vec();
                    Some(TmuxNotification::Output { pane_id, data })
                }
                Some("%layout-change") => {
                    let window_id = parts.get(1)?.to_string();
                    let layout = parts.get(2).unwrap_or(&"").to_string();
                    Some(TmuxNotification::LayoutChange { window_id, layout })
                }
                Some("%session-changed") => {
                    let id = parts.get(1)?.to_string();
                    let name = parts.get(2).unwrap_or(&"").to_string();
                    Some(TmuxNotification::SessionChanged { id, name })
                }
                Some("%window-add") => {
                    let window_id = parts.get(1)?.to_string();
                    Some(TmuxNotification::WindowAdd { window_id })
                }
                Some("%window-close") => {
                    let window_id = parts.get(1)?.to_string();
                    Some(TmuxNotification::WindowClose { window_id })
                }
                Some("%window-renamed") => {
                    let window_id = parts.get(1)?.to_string();
                    let name = parts.get(2).unwrap_or(&"").to_string();
                    Some(TmuxNotification::WindowRenamed { window_id, name })
                }
                Some("%client-detached") => Some(TmuxNotification::ClientDetached),
                Some("%exit") => {
                    let reason = parts.get(1).unwrap_or(&"").to_string();
                    Some(TmuxNotification::Exit { reason })
                }
                _ => Some(TmuxNotification::Unknown(line.to_string())),
            }
        } else {
            None
        }
    }

    /// Poll for pending notifications.
    pub fn poll_notifications(&mut self) -> Option<TmuxNotification> {
        self.notifications.pop_front()
    }

    /// Process data for passthrough sequences.
    ///
    /// When running inside tmux (nested mode), certain escape sequences need
    /// to be wrapped in DCS passthrough sequences to reach the outer terminal.
    ///
    /// Returns (processed_data, extracted_passthrough_content).
    pub fn process_passthrough(&mut self, data: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut output = Vec::with_capacity(data.len());
        let mut extracted = Vec::new();

        for &byte in data {
            match self.passthrough_state {
                PassthroughState::Normal => {
                    if byte == 0x1b {
                        self.passthrough_state = PassthroughState::Escape;
                    } else {
                        output.push(byte);
                    }
                }
                PassthroughState::Escape => {
                    if byte == b'P' {
                        self.passthrough_state = PassthroughState::DcsStart;
                    } else {
                        // Not a DCS, emit the ESC and this byte
                        output.push(0x1b);
                        output.push(byte);
                        self.passthrough_state = PassthroughState::Normal;
                    }
                }
                PassthroughState::DcsStart => {
                    // Check for "tmux;" prefix
                    self.passthrough_buffer.push(byte);
                    if self.passthrough_buffer.len() >= 5 {
                        if &self.passthrough_buffer[..5] == b"tmux;" {
                            self.passthrough_buffer.clear();
                            self.passthrough_state = PassthroughState::InPassthrough;
                        } else {
                            // Not a tmux passthrough, emit what we have
                            output.push(0x1b);
                            output.push(b'P');
                            output.extend_from_slice(&self.passthrough_buffer);
                            self.passthrough_buffer.clear();
                            self.passthrough_state = PassthroughState::Normal;
                        }
                    }
                }
                PassthroughState::InPassthrough => {
                    if byte == 0x1b {
                        self.passthrough_state = PassthroughState::PassthroughEscape;
                    } else {
                        self.passthrough_buffer.push(byte);
                    }
                }
                PassthroughState::PassthroughEscape => {
                    if byte == b'\\' {
                        // End of passthrough (ST = ESC \)
                        // The passthrough content has doubled ESCs, so unescape
                        let content = self.unescape_passthrough(&self.passthrough_buffer);
                        extracted.push(content);
                        self.passthrough_buffer.clear();
                        self.passthrough_state = PassthroughState::Normal;
                    } else if byte == 0x1b {
                        // Doubled ESC inside passthrough, emit one
                        self.passthrough_buffer.push(0x1b);
                    } else {
                        // ESC followed by something else
                        self.passthrough_buffer.push(0x1b);
                        self.passthrough_buffer.push(byte);
                        self.passthrough_state = PassthroughState::InPassthrough;
                    }
                }
            }
        }

        (output, extracted)
    }

    /// Unescape passthrough content (doubled ESCs become single).
    fn unescape_passthrough(&self, data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len());
        let mut i = 0;

        while i < data.len() {
            if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == 0x1b {
                result.push(0x1b);
                i += 2;
            } else {
                result.push(data[i]);
                i += 1;
            }
        }

        result
    }

    /// Wrap data in tmux passthrough sequence for sending to outer terminal.
    ///
    /// Use this when running inside tmux and need to send sequences that
    /// should reach the terminal outside tmux (e.g., clipboard OSC sequences).
    pub fn wrap_passthrough(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len() * 2 + 10);

        // DCS tmux;
        result.extend_from_slice(b"\x1bPtmux;");

        // Double all ESC characters
        for &byte in data {
            if byte == 0x1b {
                result.push(0x1b);
                result.push(0x1b);
            } else {
                result.push(byte);
            }
        }

        // ST (String Terminator)
        result.extend_from_slice(b"\x1b\\");

        result
    }

    /// Check if tmux is available on the system.
    pub fn is_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get tmux version.
    pub fn version() -> Option<String> {
        Command::new("tmux")
            .arg("-V")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    }

    /// Detach from control mode.
    pub fn detach(&mut self) -> io::Result<()> {
        if self.mode == TmuxMode::ControlMode {
            self.send_command("detach-client")?;
        }
        Ok(())
    }

    /// Kill the tmux session and clean up.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.mode = TmuxMode::None;
    }
}

impl Default for TmuxController {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TmuxController {
    fn drop(&mut self) {
        // Gracefully detach if in control mode
        if self.mode == TmuxMode::ControlMode {
            let _ = self.detach();
        }
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── TmuxMode enum ──────────────────────────────────────────────

    #[test]
    fn tmux_mode_clone_and_eq() {
        let modes = [
            TmuxMode::None,
            TmuxMode::Nested,
            TmuxMode::ControlMode,
            TmuxMode::Standard,
        ];
        for mode in &modes {
            let cloned = *mode;
            assert_eq!(*mode, cloned);
        }
        assert_ne!(TmuxMode::None, TmuxMode::Nested);
        assert_ne!(TmuxMode::ControlMode, TmuxMode::Standard);
    }

    #[test]
    fn tmux_mode_debug() {
        // Ensure Debug is implemented and produces non-empty output.
        let s = format!("{:?}", TmuxMode::ControlMode);
        assert!(s.contains("ControlMode"));
    }

    // ── TmuxController construction & defaults ─────────────────────

    #[test]
    fn controller_new_has_no_process() {
        let c = TmuxController::new();
        assert!(c.process.is_none());
        assert!(matches!(c.mode(), TmuxMode::None | TmuxMode::Nested));
        assert!(c.notifications.is_empty());
        assert!(c.windows.is_empty());
        assert!(c.panes.is_empty());
        assert!(c.session.is_none());
    }

    #[test]
    fn controller_default_same_as_new() {
        let a = TmuxController::new();
        let b = TmuxController::default();
        assert_eq!(a.mode(), b.mode());
        assert!(a.process.is_none());
        assert!(b.process.is_none());
    }

    // ── Mode helper methods ────────────────────────────────────────

    #[test]
    fn is_nested_and_is_control_mode() {
        let mut c = TmuxController::new();
        c.mode = TmuxMode::Nested;
        assert!(c.is_nested());
        assert!(!c.is_control_mode());

        c.mode = TmuxMode::ControlMode;
        assert!(!c.is_nested());
        assert!(c.is_control_mode());

        c.mode = TmuxMode::None;
        assert!(!c.is_nested());
        assert!(!c.is_control_mode());

        c.mode = TmuxMode::Standard;
        assert!(!c.is_nested());
        assert!(!c.is_control_mode());
    }

    // ── kill() resets mode ─────────────────────────────────────────

    #[test]
    fn kill_sets_mode_to_none() {
        let mut c = TmuxController::new();
        c.mode = TmuxMode::ControlMode;
        c.kill();
        assert_eq!(c.mode(), TmuxMode::None);
        assert!(c.process.is_none());
    }

    // ── poll_notifications ─────────────────────────────────────────

    #[test]
    fn poll_notifications_fifo() {
        let mut c = TmuxController::new();
        c.notifications.push_back(TmuxNotification::ClientDetached);
        c.notifications.push_back(TmuxNotification::Exit {
            reason: "done".into(),
        });

        assert!(matches!(
            c.poll_notifications(),
            Some(TmuxNotification::ClientDetached)
        ));
        assert!(matches!(
            c.poll_notifications(),
            Some(TmuxNotification::Exit { .. })
        ));
        assert!(c.poll_notifications().is_none());
    }

    // ── parse_notification – exhaustive ────────────────────────────

    #[test]
    fn parse_output_notification() {
        let c = TmuxController::new();
        match c.parse_notification("%output %5 some data here") {
            Some(TmuxNotification::Output { pane_id, data }) => {
                assert_eq!(pane_id, "%5");
                assert_eq!(data, b"some data here");
            }
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn parse_output_with_no_data() {
        let c = TmuxController::new();
        match c.parse_notification("%output %0") {
            Some(TmuxNotification::Output { pane_id, data }) => {
                assert_eq!(pane_id, "%0");
                assert!(data.is_empty());
            }
            other => panic!("Expected Output with empty data, got {:?}", other),
        }
    }

    #[test]
    fn parse_layout_change() {
        let c = TmuxController::new();
        match c.parse_notification("%layout-change @1 abc,80x24,0,0") {
            Some(TmuxNotification::LayoutChange { window_id, layout }) => {
                assert_eq!(window_id, "@1");
                assert_eq!(layout, "abc,80x24,0,0");
            }
            other => panic!("Expected LayoutChange, got {:?}", other),
        }
    }

    #[test]
    fn parse_layout_change_no_layout() {
        let c = TmuxController::new();
        match c.parse_notification("%layout-change @2") {
            Some(TmuxNotification::LayoutChange { window_id, layout }) => {
                assert_eq!(window_id, "@2");
                assert!(layout.is_empty());
            }
            other => panic!("Expected LayoutChange, got {:?}", other),
        }
    }

    #[test]
    fn parse_session_changed() {
        let c = TmuxController::new();
        match c.parse_notification("%session-changed $3 my-session") {
            Some(TmuxNotification::SessionChanged { id, name }) => {
                assert_eq!(id, "$3");
                assert_eq!(name, "my-session");
            }
            other => panic!("Expected SessionChanged, got {:?}", other),
        }
    }

    #[test]
    fn parse_window_add() {
        let c = TmuxController::new();
        match c.parse_notification("%window-add @7") {
            Some(TmuxNotification::WindowAdd { window_id }) => {
                assert_eq!(window_id, "@7");
            }
            other => panic!("Expected WindowAdd, got {:?}", other),
        }
    }

    #[test]
    fn parse_window_close() {
        let c = TmuxController::new();
        match c.parse_notification("%window-close @9") {
            Some(TmuxNotification::WindowClose { window_id }) => {
                assert_eq!(window_id, "@9");
            }
            other => panic!("Expected WindowClose, got {:?}", other),
        }
    }

    #[test]
    fn parse_window_renamed() {
        let c = TmuxController::new();
        match c.parse_notification("%window-renamed @1 new-name") {
            Some(TmuxNotification::WindowRenamed { window_id, name }) => {
                assert_eq!(window_id, "@1");
                assert_eq!(name, "new-name");
            }
            other => panic!("Expected WindowRenamed, got {:?}", other),
        }
    }

    #[test]
    fn parse_window_renamed_name_with_spaces() {
        let c = TmuxController::new();
        match c.parse_notification("%window-renamed @1 my window name") {
            Some(TmuxNotification::WindowRenamed { window_id, name }) => {
                assert_eq!(window_id, "@1");
                // splitn(3, ' ') means "my window name" is the third part
                assert_eq!(name, "my window name");
            }
            other => panic!("Expected WindowRenamed, got {:?}", other),
        }
    }

    #[test]
    fn parse_client_detached() {
        let c = TmuxController::new();
        match c.parse_notification("%client-detached") {
            Some(TmuxNotification::ClientDetached) => {}
            other => panic!("Expected ClientDetached, got {:?}", other),
        }
    }

    #[test]
    fn parse_exit_with_reason() {
        let c = TmuxController::new();
        match c.parse_notification("%exit server exited") {
            Some(TmuxNotification::Exit { reason }) => {
                // splitn(3, ' ') with only 2 parts: "%exit" and "server exited"
                assert_eq!(reason, "server");
            }
            other => panic!("Expected Exit, got {:?}", other),
        }
    }

    #[test]
    fn parse_exit_no_reason() {
        let c = TmuxController::new();
        match c.parse_notification("%exit") {
            Some(TmuxNotification::Exit { reason }) => {
                assert!(reason.is_empty());
            }
            other => panic!("Expected Exit, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_percent_notification() {
        let c = TmuxController::new();
        match c.parse_notification("%something-new foo") {
            Some(TmuxNotification::Unknown(line)) => {
                assert_eq!(line, "%something-new foo");
            }
            other => panic!("Expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn parse_non_notification_returns_none() {
        let c = TmuxController::new();
        assert!(c.parse_notification("").is_none());
        assert!(c.parse_notification("hello world").is_none());
        assert!(c.parse_notification("no-percent-prefix").is_none());
    }

    // ── process_output ─────────────────────────────────────────────

    #[test]
    fn process_output_multiple_notifications() {
        let mut c = TmuxController::new();
        let data = b"%window-add @1\n%window-add @2\n%exit\n";
        let notifs = c.process_output(data);
        assert_eq!(notifs.len(), 3);
        assert!(
            matches!(&notifs[0], TmuxNotification::WindowAdd { window_id } if window_id == "@1")
        );
        assert!(
            matches!(&notifs[1], TmuxNotification::WindowAdd { window_id } if window_id == "@2")
        );
        assert!(matches!(&notifs[2], TmuxNotification::Exit { .. }));
    }

    #[test]
    fn process_output_skips_non_notification_lines() {
        let mut c = TmuxController::new();
        let data = b"some regular output\n%window-add @1\nmore output\n";
        let notifs = c.process_output(data);
        assert_eq!(notifs.len(), 1);
        assert!(matches!(&notifs[0], TmuxNotification::WindowAdd { .. }));
    }

    #[test]
    fn process_output_empty() {
        let mut c = TmuxController::new();
        let notifs = c.process_output(b"");
        assert!(notifs.is_empty());
    }

    // ── wrap_passthrough ───────────────────────────────────────────

    #[test]
    fn wrap_passthrough_empty_data() {
        let wrapped = TmuxController::wrap_passthrough(b"");
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\\");
    }

    #[test]
    fn wrap_passthrough_no_esc() {
        let wrapped = TmuxController::wrap_passthrough(b"Hello");
        assert_eq!(wrapped, b"\x1bPtmux;Hello\x1b\\");
    }

    #[test]
    fn wrap_passthrough_doubles_esc() {
        let data = b"\x1b[31mRed\x1b[0m";
        let wrapped = TmuxController::wrap_passthrough(data);

        // Manually construct expected:
        //   ESC P t m u x ;  ESC ESC [ 3 1 m R e d ESC ESC [ 0 m  ESC \
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1bPtmux;");
        expected.push(0x1b);
        expected.push(0x1b);
        expected.extend_from_slice(b"[31mRed");
        expected.push(0x1b);
        expected.push(0x1b);
        expected.extend_from_slice(b"[0m");
        expected.extend_from_slice(b"\x1b\\");

        assert_eq!(wrapped, expected);
    }

    #[test]
    fn wrap_passthrough_all_esc_bytes() {
        // Data consisting only of ESC characters
        let data = b"\x1b\x1b\x1b";
        let wrapped = TmuxController::wrap_passthrough(data);
        // Each ESC doubled, plus header and trailer
        let expected = b"\x1bPtmux;\x1b\x1b\x1b\x1b\x1b\x1b\x1b\\";
        assert_eq!(wrapped, expected.as_ref());
    }

    // ── unescape_passthrough ───────────────────────────────────────

    #[test]
    fn unescape_passthrough_no_esc() {
        let c = TmuxController::new();
        let result = c.unescape_passthrough(b"Hello World");
        assert_eq!(result, b"Hello World");
    }

    #[test]
    fn unescape_passthrough_doubled_esc() {
        let c = TmuxController::new();
        let data = [0x1b, 0x1b, b'[', b'3', b'1', b'm'];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[0x1b, b'[', b'3', b'1', b'm']);
    }

    #[test]
    fn unescape_passthrough_single_esc_kept() {
        let c = TmuxController::new();
        // A single ESC followed by non-ESC should be kept as-is
        let data = [0x1b, b'['];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[0x1b, b'[']);
    }

    #[test]
    fn unescape_passthrough_empty() {
        let c = TmuxController::new();
        let result = c.unescape_passthrough(b"");
        assert!(result.is_empty());
    }

    #[test]
    fn unescape_passthrough_trailing_single_esc() {
        let c = TmuxController::new();
        // Trailing single ESC
        let data = [b'A', 0x1b];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[b'A', 0x1b]);
    }

    #[test]
    fn unescape_passthrough_multiple_doubled() {
        let c = TmuxController::new();
        // Two doubled ESC pairs with content between them
        let data = [0x1b, 0x1b, b'X', 0x1b, 0x1b, b'Y'];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[0x1b, b'X', 0x1b, b'Y']);
    }

    // ── process_passthrough – state machine ────────────────────────

    #[test]
    fn process_passthrough_normal_data() {
        let mut c = TmuxController::new();
        let (out, ext) = c.process_passthrough(b"plain text 123");
        assert_eq!(out, b"plain text 123");
        assert!(ext.is_empty());
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_esc_non_dcs() {
        let mut c = TmuxController::new();
        // ESC followed by something other than 'P' => both emitted
        let (out, ext) = c.process_passthrough(b"\x1b[31m");
        assert_eq!(out, b"\x1b[31m");
        assert!(ext.is_empty());
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_dcs_not_tmux() {
        let mut c = TmuxController::new();
        // ESC P followed by non-"tmux;" prefix
        let (out, ext) = c.process_passthrough(b"\x1bPotherXYZ");
        // Should emit ESC, P, and then the buffer contents
        assert!(out.starts_with(&[0x1b, b'P']));
        assert!(ext.is_empty());
    }

    #[test]
    fn process_passthrough_full_sequence() {
        let mut c = TmuxController::new();
        // Build: ESC P t m u x ; <doubled-esc>[31m ESC \
        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        // Doubled ESC inside passthrough
        input.push(0x1b);
        input.push(0x1b);
        input.extend_from_slice(b"[31m");
        // ST = ESC backslash
        input.push(0x1b);
        input.push(b'\\');

        let (out, ext) = c.process_passthrough(&input);
        assert!(out.is_empty(), "No output bytes expected");
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], b"\x1b[31m");
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_data_before_and_after() {
        let mut c = TmuxController::new();
        let mut input = Vec::new();
        input.extend_from_slice(b"before");
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"inner");
        input.push(0x1b);
        input.push(b'\\');
        input.extend_from_slice(b"after");

        let (out, ext) = c.process_passthrough(&input);
        assert_eq!(&out, b"beforeafter");
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], b"inner");
    }

    #[test]
    fn process_passthrough_two_sequences() {
        let mut c = TmuxController::new();

        // First passthrough
        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"seq1");
        input.push(0x1b);
        input.push(b'\\');
        // Second passthrough
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"seq2");
        input.push(0x1b);
        input.push(b'\\');

        let (out, ext) = c.process_passthrough(&input);
        assert!(out.is_empty());
        assert_eq!(ext.len(), 2);
        assert_eq!(ext[0], b"seq1");
        assert_eq!(ext[1], b"seq2");
    }

    #[test]
    fn process_passthrough_incremental() {
        // Feed the passthrough byte-by-byte to test state persistence
        let mut c = TmuxController::new();

        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"hello");
        input.push(0x1b);
        input.push(b'\\');

        let mut all_out = Vec::new();
        let mut all_ext = Vec::new();

        for &byte in &input {
            let (out, ext) = c.process_passthrough(&[byte]);
            all_out.extend_from_slice(&out);
            all_ext.extend(ext);
        }

        assert!(all_out.is_empty());
        assert_eq!(all_ext.len(), 1);
        assert_eq!(all_ext[0], b"hello");
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_esc_esc_in_content() {
        // Doubled ESC inside passthrough means a single ESC in the extracted content
        let mut c = TmuxController::new();

        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        // Doubled ESC (the passthrough_state machine handles this:
        // first ESC -> PassthroughEscape, second ESC -> push one ESC, back to InPassthrough)
        input.push(0x1b);
        input.push(0x1b);
        input.push(b'X');
        input.push(0x1b);
        input.push(b'\\');

        let (_out, ext) = c.process_passthrough(&input);
        assert_eq!(ext.len(), 1);
        // The state machine pushes one ESC for doubled ESC, then 'X'
        assert_eq!(ext[0], &[0x1b, b'X']);
    }

    #[test]
    fn process_passthrough_esc_other_in_content() {
        // ESC followed by non-ESC, non-backslash inside passthrough
        let mut c = TmuxController::new();

        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.push(b'A');
        // ESC followed by 'Z' (not ESC or backslash)
        input.push(0x1b);
        input.push(b'Z');
        input.push(b'B');
        // End
        input.push(0x1b);
        input.push(b'\\');

        let (_out, ext) = c.process_passthrough(&input);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], &[b'A', 0x1b, b'Z', b'B']);
    }

    #[test]
    fn process_passthrough_partial_dcs_prefix() {
        // ESC P followed by only 3 bytes (not enough for "tmux;")
        // then more data that makes it not "tmux;"
        let mut c = TmuxController::new();
        let (out, ext) = c.process_passthrough(b"\x1bPabcde");
        // "abcde" != "tmux;", so should emit ESC P + buffer
        assert_eq!(out, b"\x1bPabcde");
        assert!(ext.is_empty());
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    // ── wrap then process roundtrip ────────────────────────────────

    #[test]
    fn wrap_then_process_roundtrip() {
        let original = b"\x1b[31mHello\x1b[0m World";
        let wrapped = TmuxController::wrap_passthrough(original);

        let mut c = TmuxController::new();
        let (_out, ext) = c.process_passthrough(&wrapped);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], original.as_ref());
    }

    #[test]
    fn wrap_then_process_roundtrip_no_esc() {
        let original = b"simple text";
        let wrapped = TmuxController::wrap_passthrough(original);

        let mut c = TmuxController::new();
        let (_out, ext) = c.process_passthrough(&wrapped);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], original.as_ref());
    }

    #[test]
    fn wrap_then_process_roundtrip_only_esc() {
        let original = b"\x1b";
        let wrapped = TmuxController::wrap_passthrough(original);

        let mut c = TmuxController::new();
        let (_out, ext) = c.process_passthrough(&wrapped);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], original.as_ref());
    }

    // ── Struct construction ────────────────────────────────────────

    #[test]
    fn tmux_session_clone_and_debug() {
        let session = TmuxSession {
            id: "$0".into(),
            name: "main".into(),
            windows: 3,
            attached: true,
        };
        let cloned = session.clone();
        assert_eq!(cloned.id, "$0");
        assert_eq!(cloned.name, "main");
        assert_eq!(cloned.windows, 3);
        assert!(cloned.attached);
        let dbg = format!("{:?}", session);
        assert!(dbg.contains("main"));
    }

    #[test]
    fn tmux_window_clone_and_debug() {
        let window = TmuxWindow {
            index: 0,
            name: "editor".into(),
            active: true,
            panes: 2,
        };
        let cloned = window.clone();
        assert_eq!(cloned.index, 0);
        assert_eq!(cloned.name, "editor");
        assert!(cloned.active);
        assert_eq!(cloned.panes, 2);
        let dbg = format!("{:?}", window);
        assert!(dbg.contains("editor"));
    }

    #[test]
    fn tmux_pane_clone_and_debug() {
        let pane = TmuxPane {
            id: "%0".into(),
            index: 0,
            active: false,
            width: 80,
            height: 24,
            command: "bash".into(),
        };
        let cloned = pane.clone();
        assert_eq!(cloned.id, "%0");
        assert_eq!(cloned.index, 0);
        assert!(!cloned.active);
        assert_eq!(cloned.width, 80);
        assert_eq!(cloned.height, 24);
        assert_eq!(cloned.command, "bash");
        let dbg = format!("{:?}", pane);
        assert!(dbg.contains("bash"));
    }

    // ── TmuxNotification debug & clone ─────────────────────────────

    #[test]
    fn notification_variants_debug_and_clone() {
        let variants: Vec<TmuxNotification> = vec![
            TmuxNotification::Output {
                pane_id: "%0".into(),
                data: vec![65],
            },
            TmuxNotification::LayoutChange {
                window_id: "@1".into(),
                layout: "layout".into(),
            },
            TmuxNotification::SessionChanged {
                id: "$0".into(),
                name: "s".into(),
            },
            TmuxNotification::WindowAdd {
                window_id: "@1".into(),
            },
            TmuxNotification::WindowClose {
                window_id: "@1".into(),
            },
            TmuxNotification::WindowRenamed {
                window_id: "@1".into(),
                name: "n".into(),
            },
            TmuxNotification::PaneModeChanged {
                pane_id: "%0".into(),
                mode: "copy".into(),
            },
            TmuxNotification::ClientDetached,
            TmuxNotification::Exit {
                reason: "done".into(),
            },
            TmuxNotification::Unknown("???".into()),
        ];
        for v in &variants {
            let cloned = v.clone();
            let dbg = format!("{:?}", cloned);
            assert!(!dbg.is_empty());
        }
    }

    // ── send_command without process ───────────────────────────────

    #[test]
    fn send_command_no_process_does_not_error() {
        let mut c = TmuxController::new();
        // No process attached => send_command should silently succeed
        assert!(c.send_command("list-sessions").is_ok());
    }

    #[test]
    fn send_keys_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.send_keys("%0", "Enter").is_ok());
    }

    #[test]
    fn new_window_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.new_window(Some("test")).is_ok());
        assert!(c.new_window(None).is_ok());
    }

    #[test]
    fn split_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.split_horizontal().is_ok());
        assert!(c.split_vertical().is_ok());
    }

    #[test]
    fn select_pane_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.select_pane("%0").is_ok());
    }

    #[test]
    fn resize_pane_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.resize_pane("%0", 120, 40).is_ok());
    }

    #[test]
    fn kill_pane_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.kill_pane("%0").is_ok());
    }

    #[test]
    fn list_commands_no_process_does_not_error() {
        let mut c = TmuxController::new();
        assert!(c.list_sessions().is_ok());
        assert!(c.list_windows().is_ok());
        assert!(c.list_panes().is_ok());
    }

    #[test]
    fn detach_not_control_mode_does_nothing() {
        let mut c = TmuxController::new();
        c.mode = TmuxMode::None;
        assert!(c.detach().is_ok());
        c.mode = TmuxMode::Nested;
        assert!(c.detach().is_ok());
        c.mode = TmuxMode::Standard;
        assert!(c.detach().is_ok());
    }

    // ── detect_mode depends on env ─────────────────────────────────

    #[test]
    fn detect_mode_returns_valid_variant() {
        let mode = TmuxController::detect_mode();
        assert!(matches!(mode, TmuxMode::None | TmuxMode::Nested));
    }

    // ── is_available and version don't panic ───────────────────────

    #[test]
    fn is_available_does_not_panic() {
        let _ = TmuxController::is_available();
    }

    #[test]
    fn version_does_not_panic() {
        let _ = TmuxController::version();
    }

    // ── PassthroughState enum ──────────────────────────────────────

    #[test]
    fn passthrough_state_eq_and_clone() {
        let states = [
            PassthroughState::Normal,
            PassthroughState::Escape,
            PassthroughState::DcsStart,
            PassthroughState::InPassthrough,
            PassthroughState::PassthroughEscape,
        ];
        for s in &states {
            assert_eq!(s, &s.clone());
        }
        assert_ne!(PassthroughState::Normal, PassthroughState::Escape);
    }

    // ── Edge case: notification with only the command, no args ─────

    #[test]
    fn parse_window_add_no_id_returns_none() {
        let c = TmuxController::new();
        // "%window-add" with no window_id should return None (parts.get(1) is None)
        let result = c.parse_notification("%window-add");
        assert!(result.is_none());
    }

    #[test]
    fn parse_window_close_no_id_returns_none() {
        let c = TmuxController::new();
        let result = c.parse_notification("%window-close");
        assert!(result.is_none());
    }

    #[test]
    fn parse_output_no_pane_id_returns_none() {
        let c = TmuxController::new();
        let result = c.parse_notification("%output");
        assert!(result.is_none());
    }

    #[test]
    fn parse_layout_change_no_window_id_returns_none() {
        let c = TmuxController::new();
        let result = c.parse_notification("%layout-change");
        assert!(result.is_none());
    }

    #[test]
    fn parse_session_changed_no_id_returns_none() {
        let c = TmuxController::new();
        let result = c.parse_notification("%session-changed");
        assert!(result.is_none());
    }

    #[test]
    fn parse_window_renamed_no_id_returns_none() {
        let c = TmuxController::new();
        let result = c.parse_notification("%window-renamed");
        assert!(result.is_none());
    }

    // ── process_output with non-UTF8 data ──────────────────────────

    #[test]
    fn process_output_invalid_utf8() {
        let mut c = TmuxController::new();
        // Invalid UTF-8 should still be handled gracefully via lossy conversion
        let data: &[u8] = &[0xFF, 0xFE, b'\n', b'%', b'e', b'x', b'i', b't', b'\n'];
        let notifs = c.process_output(data);
        // Should parse the %exit line
        assert!(notifs
            .iter()
            .any(|n| matches!(n, TmuxNotification::Exit { .. })));
    }
}
