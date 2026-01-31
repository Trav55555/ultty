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
        self.send_command("list-windows -F '#{window_index}:#{window_name}:#{window_active}:#{window_panes}'")
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
            match parts.get(0).copied() {
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

    #[test]
    fn test_mode_detection() {
        let controller = TmuxController::new();
        // Mode depends on environment
        assert!(matches!(
            controller.mode(),
            TmuxMode::None | TmuxMode::Nested
        ));
    }

    #[test]
    fn test_passthrough_wrap() {
        let data = b"\x1b[31mHello\x1b[0m";
        let wrapped = TmuxController::wrap_passthrough(data);

        // Should start with DCS tmux;
        assert!(wrapped.starts_with(b"\x1bPtmux;"));
        // Should end with ST
        assert!(wrapped.ends_with(b"\x1b\\"));
        // ESCs should be doubled
        assert!(wrapped.windows(2).any(|w| w == [0x1b, 0x1b]));
    }

    #[test]
    fn test_passthrough_parsing() {
        let mut controller = TmuxController::new();

        // Normal data should pass through
        let (output, extracted) = controller.process_passthrough(b"Hello World");
        assert_eq!(output, b"Hello World");
        assert!(extracted.is_empty());

        // Reset state
        controller.passthrough_state = PassthroughState::Normal;
        controller.passthrough_buffer.clear();

        // Passthrough sequence should be extracted
        let passthrough = b"\x1bPtmux;\x1b\x1b[31mRed\x1b\\Normal";
        let (output, extracted) = controller.process_passthrough(passthrough);

        // "Normal" should be in output after the passthrough ends
        assert!(output.ends_with(b"Normal"));
        // Should have extracted the inner content
        assert_eq!(extracted.len(), 1);
        // Inner content should have ESCs unescaped
        assert_eq!(&extracted[0], b"\x1b[31mRed");
    }

    #[test]
    fn test_notification_parsing() {
        let controller = TmuxController::new();

        // Output notification
        let notif = controller.parse_notification("%output %0 Hello");
        assert!(matches!(notif, Some(TmuxNotification::Output { .. })));

        // Window add
        let notif = controller.parse_notification("%window-add @1");
        assert!(matches!(notif, Some(TmuxNotification::WindowAdd { .. })));

        // Session changed
        let notif = controller.parse_notification("%session-changed $0 main");
        assert!(matches!(
            notif,
            Some(TmuxNotification::SessionChanged { .. })
        ));

        // Exit
        let notif = controller.parse_notification("%exit");
        assert!(matches!(notif, Some(TmuxNotification::Exit { .. })));

        // Normal line (not a notification)
        let notif = controller.parse_notification("some output");
        assert!(notif.is_none());
    }

    #[test]
    fn test_tmux_available() {
        // This test just checks the function runs without panic
        let _ = TmuxController::is_available();
        let _ = TmuxController::version();
    }
}
