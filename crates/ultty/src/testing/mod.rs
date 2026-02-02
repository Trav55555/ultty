//! Testing infrastructure for autonomous terminal testing.
//! Headless terminal testing utilities.
//!
//! This module provides hooks for testing the terminal emulator without a GUI.
//! It allows spawning a headless terminal, sending input, and capturing output.
//!
//! # Example
//!
//! ```no_run
//! use ultty::testing::TestTerminal;
//! use std::time::Duration;
//!
//! let mut term = TestTerminal::new().unwrap();
//!
//! // Send a command and wait for output
//! term.send("echo hello\n").unwrap();
//! term.wait_for_output(Duration::from_millis(100)).unwrap();
//!
//! // Check output contains expected text
//! assert!(term.capture_text().contains("hello"));
//! ```

pub mod snapshot;

pub use snapshot::{SnapshotDiff, TerminalSnapshot, TerminalVerifier};

use crate::event_loop::EventLoop;
use crate::grid::{MouseMode, Terminal};
use std::time::{Duration, Instant};

/// A headless terminal instance for testing.
pub struct TestTerminal {
    event_loop: EventLoop,
    cols: u16,
    rows: u16,
}

impl TestTerminal {
    /// Create a new test terminal with default dimensions (80x24).
    pub fn new() -> std::io::Result<Self> {
        Self::with_size(80, 24)
    }

    /// Create a new test terminal with specified dimensions.
    pub fn with_size(cols: u16, rows: u16) -> std::io::Result<Self> {
        let event_loop = EventLoop::new(cols, rows)?;
        Ok(Self {
            event_loop,
            cols,
            rows,
        })
    }

    /// Create a test terminal in a specific working directory.
    pub fn with_dir(cols: u16, rows: u16, working_dir: &str) -> std::io::Result<Self> {
        let event_loop = EventLoop::new_with_dir(cols, rows, working_dir)?;
        Ok(Self {
            event_loop,
            cols,
            rows,
        })
    }

    /// Send input to the terminal.
    pub fn send(&mut self, input: &str) -> std::io::Result<()> {
        self.event_loop.send_input(input.as_bytes())
    }

    /// Send raw bytes to the terminal.
    pub fn send_bytes(&mut self, input: &[u8]) -> std::io::Result<()> {
        self.event_loop.send_input(input)
    }

    /// Send a key press (with optional modifiers).
    pub fn send_key(&mut self, key: &str) -> std::io::Result<()> {
        match key {
            "Enter" | "Return" => self.send_bytes(b"\r"),
            "Tab" => self.send_bytes(b"\t"),
            "Escape" | "Esc" => self.send_bytes(b"\x1b"),
            "Backspace" => self.send_bytes(b"\x7f"),
            "Up" => self.send_bytes(b"\x1b[A"),
            "Down" => self.send_bytes(b"\x1b[B"),
            "Right" => self.send_bytes(b"\x1b[C"),
            "Left" => self.send_bytes(b"\x1b[D"),
            "Home" => self.send_bytes(b"\x1b[H"),
            "End" => self.send_bytes(b"\x1b[F"),
            "PageUp" => self.send_bytes(b"\x1b[5~"),
            "PageDown" => self.send_bytes(b"\x1b[6~"),
            "Delete" => self.send_bytes(b"\x1b[3~"),
            "Insert" => self.send_bytes(b"\x1b[2~"),
            "F1" => self.send_bytes(b"\x1bOP"),
            "F2" => self.send_bytes(b"\x1bOQ"),
            "F3" => self.send_bytes(b"\x1bOR"),
            "F4" => self.send_bytes(b"\x1bOS"),
            "F5" => self.send_bytes(b"\x1b[15~"),
            "F6" => self.send_bytes(b"\x1b[17~"),
            "F7" => self.send_bytes(b"\x1b[18~"),
            "F8" => self.send_bytes(b"\x1b[19~"),
            "F9" => self.send_bytes(b"\x1b[20~"),
            "F10" => self.send_bytes(b"\x1b[21~"),
            "F11" => self.send_bytes(b"\x1b[23~"),
            "F12" => self.send_bytes(b"\x1b[24~"),
            // Ctrl+key
            s if s.starts_with("Ctrl+") => {
                let ch = s.chars().last().unwrap_or('?');
                let ctrl_char = (ch.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                self.send_bytes(&[ctrl_char])
            }
            // Regular character
            _ => self.send(key),
        }
    }

    /// Process pending terminal output.
    pub fn process(&mut self) -> std::io::Result<()> {
        self.event_loop.process()?;
        Ok(())
    }

    /// Wait for output with timeout.
    pub fn wait_for_output(&mut self, timeout: Duration) -> std::io::Result<bool> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.process()?;
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(true)
    }

    /// Wait until terminal output stabilizes.
    pub fn wait_until_stable(
        &mut self,
        stable_duration: Duration,
        max_wait: Duration,
    ) -> std::io::Result<()> {
        let start = Instant::now();
        let mut last_content = self.capture_text();
        let mut stable_since = Instant::now();

        while start.elapsed() < max_wait {
            self.process()?;
            std::thread::sleep(Duration::from_millis(50));

            let current = self.capture_text();
            if current != last_content {
                last_content = current;
                stable_since = Instant::now();
            } else if stable_since.elapsed() >= stable_duration {
                return Ok(());
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Terminal output did not stabilize",
        ))
    }

    /// Get the terminal state.
    pub fn terminal(&self) -> &Terminal {
        self.event_loop.terminal()
    }

    /// Capture the current terminal text content.
    pub fn capture_text(&self) -> String {
        let grid = self.terminal().grid();
        let dims = grid.dimensions();
        let mut output = String::new();

        for row_idx in 0..dims.rows {
            if let Some(row) = grid.row(row_idx) {
                for col_idx in 0..dims.cols {
                    if let Some(cell) = row.get(col_idx) {
                        output.push(cell.char());
                    }
                }
            }
            output.push('\n');
        }
        output
    }

    /// Capture terminal content as a grid representation.
    pub fn capture_grid(&self) -> Vec<String> {
        let grid = self.terminal().grid();
        let dims = grid.dimensions();
        let mut lines = Vec::new();

        for row_idx in 0..dims.rows {
            let mut line = String::new();
            if let Some(row) = grid.row(row_idx) {
                for col_idx in 0..dims.cols {
                    if let Some(cell) = row.get(col_idx) {
                        line.push(cell.char());
                    }
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    /// Check if the terminal output contains a string.
    pub fn contains(&self, needle: &str) -> bool {
        self.capture_text().contains(needle)
    }

    /// Check if a specific row contains text.
    pub fn row_contains(&self, row: u16, needle: &str) -> bool {
        let grid = self.terminal().grid();
        if let Some(r) = grid.row(row) {
            let mut line = String::new();
            for col in 0..grid.dimensions().cols {
                if let Some(cell) = r.get(col) {
                    line.push(cell.char());
                }
            }
            return line.contains(needle);
        }
        false
    }

    /// Get cursor position.
    pub fn cursor_position(&self) -> (u16, u16) {
        let cursor = self.terminal().grid().cursor();
        (cursor.position.col, cursor.position.row)
    }

    /// Check if terminal is in alternate screen mode.
    pub fn is_alternate_screen(&self) -> bool {
        self.terminal().is_alternate_screen()
    }

    /// Check if mouse tracking is enabled.
    pub fn mouse_tracking_enabled(&self) -> bool {
        self.terminal().modes().mouse_mode != MouseMode::None
    }

    /// Get terminal dimensions.
    pub fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Resize the terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.cols = cols;
        self.rows = rows;
        self.event_loop.resize(cols, rows)
    }

    /// Run a command and wait for it to complete.
    pub fn run_command(&mut self, cmd: &str, timeout: Duration) -> std::io::Result<String> {
        // Clear any pending output
        self.wait_for_output(Duration::from_millis(100))?;
        let before = self.capture_text();

        // Send command
        self.send(cmd)?;
        self.send_key("Enter")?;

        // Wait for output to stabilize
        self.wait_until_stable(Duration::from_millis(200), timeout)?;

        let after = self.capture_text();

        // Return new content
        Ok(after.strip_prefix(&before).unwrap_or(&after).to_string())
    }

    /// Capture a snapshot of the current terminal state.
    pub fn snapshot(&self) -> TerminalSnapshot {
        TerminalSnapshot::capture(self.terminal())
    }

    /// Get a verifier for making assertions about terminal state.
    pub fn verify(&self) -> TerminalVerifier<'_> {
        TerminalVerifier::new(self.terminal())
    }

    /// Print debug info about current terminal state.
    pub fn debug(&self) -> String {
        self.snapshot().to_debug_string()
    }

    /// Run an app and verify its behavior.
    pub fn test_app(&mut self, cmd: &str, checks: &[AppCheck]) -> AppTestResult {
        let mut result = AppTestResult {
            app: cmd.to_string(),
            passed: Vec::new(),
            failed: Vec::new(),
        };

        // Launch the app
        if let Err(e) = self.send(&format!("{}\r", cmd)) {
            result.failed.push(format!("Failed to launch: {}", e));
            return result;
        }

        // Wait for app to start
        if let Err(e) = self.wait_until_stable(Duration::from_millis(500), Duration::from_secs(10))
        {
            result.failed.push(format!("App didn't stabilize: {}", e));
            return result;
        }

        // Run checks
        let snapshot = self.snapshot();
        for check in checks {
            match check {
                AppCheck::AlternateScreen(expected) => {
                    if snapshot.alternate_screen == *expected {
                        result.passed.push("Alternate screen".to_string());
                    } else {
                        result.failed.push(format!(
                            "Alternate screen: expected {}, got {}",
                            expected, snapshot.alternate_screen
                        ));
                    }
                }
                AppCheck::MouseTracking(expected) => {
                    if snapshot.mouse_tracking == *expected {
                        result.passed.push("Mouse tracking".to_string());
                    } else {
                        result.failed.push(format!(
                            "Mouse tracking: expected {}, got {}",
                            expected, snapshot.mouse_tracking
                        ));
                    }
                }
                AppCheck::ContainsText(text) => {
                    if snapshot.contains(text) {
                        result.passed.push(format!("Contains '{}'", text));
                    } else {
                        result.failed.push(format!("Missing text: '{}'", text));
                    }
                }
                AppCheck::HasColors => {
                    if snapshot.has_colors() {
                        result.passed.push("Has colors".to_string());
                    } else {
                        result.failed.push("No colors detected".to_string());
                    }
                }
                AppCheck::MinRows(min) => {
                    let actual = snapshot.non_empty_rows();
                    if actual >= *min {
                        result
                            .passed
                            .push(format!("Has {} rows (min {})", actual, min));
                    } else {
                        result
                            .failed
                            .push(format!("Only {} rows (need {})", actual, min));
                    }
                }
            }
        }

        result
    }
}

/// Checks to perform on an app.
#[derive(Debug, Clone)]
pub enum AppCheck {
    AlternateScreen(bool),
    MouseTracking(bool),
    ContainsText(&'static str),
    HasColors,
    MinRows(usize),
}

/// Result of testing an app.
#[derive(Debug)]
pub struct AppTestResult {
    pub app: String,
    pub passed: Vec<String>,
    pub failed: Vec<String>,
}

impl AppTestResult {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "{}: {} passed, {} failed",
            self.app,
            self.passed.len(),
            self.failed.len()
        )
    }
}

impl Default for TestTerminal {
    fn default() -> Self {
        Self::new().expect("Failed to create test terminal")
    }
}

/// Test assertion helpers.
pub mod assertions {
    use super::*;

    /// Assert that terminal contains text.
    pub fn assert_contains(term: &TestTerminal, needle: &str) {
        assert!(
            term.contains(needle),
            "Expected terminal to contain '{}', but it didn't.\nContent:\n{}",
            needle,
            term.capture_text()
        );
    }

    /// Assert that terminal is in alternate screen.
    pub fn assert_alternate_screen(term: &TestTerminal) {
        assert!(
            term.is_alternate_screen(),
            "Expected terminal to be in alternate screen mode"
        );
    }

    /// Assert that mouse tracking is enabled.
    pub fn assert_mouse_tracking(term: &TestTerminal) {
        assert!(
            term.mouse_tracking_enabled(),
            "Expected mouse tracking to be enabled"
        );
    }

    /// Assert cursor position.
    pub fn assert_cursor_at(term: &TestTerminal, col: u16, row: u16) {
        let (actual_col, actual_row) = term.cursor_position();
        assert_eq!(
            (actual_col, actual_row),
            (col, row),
            "Expected cursor at ({}, {}), but was at ({}, {})",
            col,
            row,
            actual_col,
            actual_row
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_creation() {
        let term = TestTerminal::new();
        assert!(term.is_ok());
    }

    #[test]
    fn test_send_and_capture() {
        let mut term = TestTerminal::new().unwrap();
        term.wait_for_output(Duration::from_millis(500)).unwrap();

        term.send("echo hello\r").unwrap();
        term.wait_for_output(Duration::from_millis(500)).unwrap();

        // Should contain our echo output
        let text = term.capture_text();
        assert!(text.contains("echo hello") || text.contains("hello"));
    }

    #[test]
    fn test_dimensions() {
        let term = TestTerminal::with_size(120, 40).unwrap();
        assert_eq!(term.dimensions(), (120, 40));
    }
}
