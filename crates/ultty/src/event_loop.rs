//! Event loop for the terminal emulator.
//!
//! This module provides the core event loop that coordinates:
//! - Reading from the PTY (shell output)
//! - Parsing ANSI sequences
//! - Updating terminal state
//! - Handling user input

use crate::grid::{Dimensions, Terminal, TerminalHandler};
use crate::protocol::ansi::AnsiParser;
use crate::pty::{Child, PtySize};
use polling::{Event, Events, Poller};
use std::io;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Events produced by the terminal event loop.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// Terminal content has changed and needs redraw.
    Redraw,
    /// Child process has exited.
    ChildExited { status: i32 },
    /// Bell character received.
    Bell,
    /// Window title changed.
    TitleChanged(String),
    /// Set clipboard content (OSC 52).
    SetClipboard {
        /// Clipboard target: 'c' for clipboard, 's' for selection
        clipboard: char,
        /// Decoded text content
        data: String,
    },
    /// Desktop notification (OSC 9/777).
    Notification {
        /// Notification title (may be empty for OSC 9)
        title: String,
        /// Notification body/message
        body: String,
    },
}

/// The terminal event loop.
///
/// Coordinates PTY I/O, parsing, and terminal state updates.
pub struct EventLoop {
    /// Child process with PTY
    child: Child,
    /// Terminal state
    terminal: Terminal,
    /// ANSI parser
    parser: AnsiParser,
    /// Read buffer
    read_buf: Vec<u8>,
    /// Pending events
    events: Vec<TerminalEvent>,
    /// Poller for I/O events
    poller: Poller,
}

impl EventLoop {
    /// Create a new event loop with a shell.
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        let size = PtySize::new(cols, rows);
        let child = Child::spawn_shell(size).map_err(io::Error::other)?;

        let terminal = Terminal::new(Dimensions::new(cols, rows));
        let parser = AnsiParser::new();

        let poller = Poller::new()?;

        // Register PTY fd for reading
        // SAFETY: The PTY file descriptor is valid and owned by `child` for the
        // lifetime of this EventLoop. The poller only uses it for edge-triggered
        // notifications, not for reading/writing.
        unsafe {
            poller.add(child.pty().as_raw_fd(), Event::readable(0))?;
        }

        Ok(Self {
            child,
            terminal,
            parser,
            read_buf: vec![0u8; 65536], // 64KB read buffer
            events: Vec::new(),
            poller,
        })
    }

    /// Create a new event loop with a shell in a specific working directory.
    pub fn new_with_dir(cols: u16, rows: u16, working_dir: &str) -> io::Result<Self> {
        let size = PtySize::new(cols, rows);
        let child = Child::spawn_shell_in_dir(size, working_dir).map_err(io::Error::other)?;

        let terminal = Terminal::new(Dimensions::new(cols, rows));
        let parser = AnsiParser::new();

        let poller = Poller::new()?;

        // Register PTY fd for reading
        // SAFETY: The PTY file descriptor is valid and owned by `child` for the
        // lifetime of this EventLoop. The poller only uses it for edge-triggered
        // notifications, not for reading/writing.
        unsafe {
            poller.add(child.pty().as_raw_fd(), Event::readable(0))?;
        }

        Ok(Self {
            child,
            terminal,
            parser,
            read_buf: vec![0u8; 65536],
            events: Vec::new(),
            poller,
        })
    }

    /// Create with a specific command.
    pub fn with_command(program: &str, args: &[&str], cols: u16, rows: u16) -> io::Result<Self> {
        let size = PtySize::new(cols, rows);
        let working_dir = crate::pty::current_dir();
        let child = Child::spawn(program, args, &working_dir, size).map_err(io::Error::other)?;

        let terminal = Terminal::new(Dimensions::new(cols, rows));
        let parser = AnsiParser::new();

        let poller = Poller::new()?;

        // SAFETY: The PTY file descriptor is valid and owned by `child` for the
        // lifetime of this EventLoop. The poller only uses it for edge-triggered
        // notifications, not for reading/writing.
        unsafe {
            poller.add(child.pty().as_raw_fd(), Event::readable(0))?;
        }

        Ok(Self {
            child,
            terminal,
            parser,
            read_buf: vec![0u8; 65536],
            events: Vec::new(),
            poller,
        })
    }

    /// Get the terminal state.
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// Get mutable terminal state.
    pub fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    /// Check if the child process has exited.
    pub fn has_exited(&self) -> bool {
        self.child.has_exited()
    }

    /// Get exit status if child has exited.
    pub fn exit_status(&self) -> Option<i32> {
        self.child.exit_status()
    }

    /// Resize the terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let size = PtySize::new(cols, rows);
        self.child.resize(size).map_err(io::Error::other)?;
        self.terminal.resize(Dimensions::new(cols, rows));
        Ok(())
    }

    /// Send input to the shell.
    pub fn send_input(&mut self, data: &[u8]) -> io::Result<()> {
        self.child.write_all(data).map_err(io::Error::other)
    }

    /// Send a key press to the shell.
    pub fn send_key(&mut self, key: &str) -> io::Result<()> {
        self.send_input(key.as_bytes())
    }

    /// Process pending PTY output (non-blocking).
    ///
    /// Returns events that occurred during processing.
    pub fn process(&mut self) -> io::Result<Vec<TerminalEvent>> {
        self.events.clear();
        let trace = pipeline_trace_enabled();
        let process_start = trace.then(Instant::now);
        let mut read_batches = 0usize;
        let mut read_bytes = 0usize;

        // Read available data from PTY FIRST (before checking child exit)
        // This ensures we don't miss output from fast-exiting processes like `echo`
        loop {
            match self.child.read(&mut self.read_buf) {
                Ok(0) => break,
                Ok(n) => {
                    read_batches += 1;
                    read_bytes += n;
                    let buf = std::mem::take(&mut self.read_buf);
                    self.process_bytes(&buf[..n]);
                    self.read_buf = buf;
                    self.events.push(TerminalEvent::Redraw);
                }
                Err(crate::pty::PtyError::Closed) => {
                    self.child.try_wait().ok();
                    if let Some(status) = self.child.exit_status() {
                        self.events.push(TerminalEvent::ChildExited { status });
                    }
                    break;
                }
                Err(e) => {
                    return Err(io::Error::other(e));
                }
            }
        }

        if let Some(start) = process_start {
            if read_bytes > 0 {
                tracing::info!(
                    target: "ultty::perf",
                    read_batches,
                    read_bytes,
                    elapsed_us = start.elapsed().as_micros(),
                    "pipeline_pty_drain"
                );
            }
        }

        // Check for child exit AFTER reading all available data
        if self.child.try_wait().map_err(io::Error::other)? {
            if let Some(status) = self.child.exit_status() {
                if !self
                    .events
                    .iter()
                    .any(|e| matches!(e, TerminalEvent::ChildExited { .. }))
                {
                    self.events.push(TerminalEvent::ChildExited { status });
                }
            }
        }

        Ok(std::mem::take(&mut self.events))
    }

    /// Wait for events with timeout.
    ///
    /// Returns events that occurred, or empty vec on timeout.
    pub fn wait(&mut self, timeout: Duration) -> io::Result<Vec<TerminalEvent>> {
        let mut poll_events = Events::new();

        // Wait for PTY to be readable
        self.poller.wait(&mut poll_events, Some(timeout))?;

        if poll_events.is_empty() {
            // Timeout - still check for child exit
            if self.child.try_wait().unwrap_or(false) {
                if let Some(status) = self.child.exit_status() {
                    return Ok(vec![TerminalEvent::ChildExited { status }]);
                }
            }
            return Ok(Vec::new());
        }

        // Process available data
        self.process()
    }

    /// Process a chunk of bytes from the PTY.
    fn process_bytes(&mut self, data: &[u8]) {
        use crate::protocol::ansi::{Action, AnsiOutput};

        let mut handler = TerminalHandler::new(&mut self.terminal);
        let events = &mut self.events;
        let trace = pipeline_trace_enabled();
        let start = trace.then(Instant::now);
        let mut actions = 0usize;
        let mut print_actions = 0usize;

        self.parser
            .feed_with_ascii_spans(data, |output| match output {
                AnsiOutput::Action(action) => {
                    if trace {
                        actions += 1;
                        if matches!(action, Action::Print(_)) {
                            print_actions += 1;
                        }
                    }
                    // Check for actions that produce events before handling
                    match &action {
                        Action::Bell => {
                            events.push(TerminalEvent::Bell);
                        }
                        Action::SetTitle(title) => {
                            events.push(TerminalEvent::TitleChanged(title.clone()));
                        }
                        Action::SetClipboard { clipboard, data } => {
                            events.push(TerminalEvent::SetClipboard {
                                clipboard: *clipboard,
                                data: data.clone(),
                            });
                        }
                        Action::Notification { title, body } => {
                            events.push(TerminalEvent::Notification {
                                title: title.clone(),
                                body: body.clone(),
                            });
                        }
                        _ => {}
                    }
                    handler.handle(action);
                }
                AnsiOutput::PrintAscii(bytes) => {
                    if trace {
                        actions += 1;
                        print_actions += bytes.len();
                    }
                    handler.handle_ascii_span(bytes);
                }
            });

        if let Some(start) = start {
            tracing::info!(
                target: "ultty::perf",
                bytes = data.len(),
                actions,
                print_actions,
                elapsed_us = start.elapsed().as_micros(),
                "pipeline_parse_grid"
            );
        }

        // Send any pending responses back to the PTY
        self.send_pending_responses();
    }

    /// Send any pending terminal responses to the PTY.
    fn send_pending_responses(&mut self) {
        let responses = self.terminal.take_responses();
        for response in responses {
            if let Err(e) = self.child.write_all(&response) {
                tracing::warn!("Failed to send terminal response: {}", e);
            }
        }
    }

    /// Run the event loop with a callback.
    ///
    /// The callback receives events and returns `true` to continue or `false` to stop.
    pub fn run<F>(&mut self, mut callback: F) -> io::Result<i32>
    where
        F: FnMut(&mut Self, TerminalEvent) -> bool,
    {
        loop {
            let events = self.wait(Duration::from_millis(16))?; // ~60fps

            for event in events {
                let should_continue = callback(self, event.clone());
                if !should_continue {
                    return Ok(self.child.exit_status().unwrap_or(0));
                }

                if let TerminalEvent::ChildExited { status } = event {
                    return Ok(status);
                }
            }
        }
    }
}

fn pipeline_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("ULTTY_TRACE_PIPELINE").is_ok_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_event_loop() {
        let result = EventLoop::new(80, 24);
        assert!(result.is_ok());

        let event_loop = result.unwrap();
        assert!(!event_loop.has_exited());
        assert_eq!(event_loop.terminal().dimensions().cols, 80);
        assert_eq!(event_loop.terminal().dimensions().rows, 24);
    }

    #[test]
    fn run_echo() {
        let mut event_loop = EventLoop::with_command("/bin/echo", &["/bin/echo", "test"], 80, 24)
            .expect("Failed to create event loop");

        // Process until child exits
        let mut redraw_count = 0;
        let status = event_loop
            .run(|_el, event| {
                match event {
                    TerminalEvent::Redraw => redraw_count += 1,
                    TerminalEvent::ChildExited { .. } => return false,
                    _ => {}
                }
                true
            })
            .expect("Event loop failed");

        assert_eq!(status, 0);
        assert!(redraw_count > 0);
    }

    #[test]
    fn resize_terminal() {
        let mut event_loop = EventLoop::new(80, 24).expect("Failed to create event loop");

        assert!(event_loop.resize(120, 40).is_ok());
        assert_eq!(event_loop.terminal().dimensions().cols, 120);
        assert_eq!(event_loop.terminal().dimensions().rows, 40);
    }

    #[test]
    fn send_input_to_cat() {
        let mut event_loop = EventLoop::with_command("/bin/cat", &["/bin/cat"], 80, 24)
            .expect("Failed to spawn cat");

        // Send input
        event_loop.send_input(b"hello\n").expect("Failed to send");

        // Wait for echo
        std::thread::sleep(Duration::from_millis(100));

        // Process output
        let events = event_loop.process().expect("Process failed");
        assert!(!events.is_empty());

        // Check terminal has content
        let row = event_loop.terminal().grid().row(0).unwrap();
        let text = row.text_trimmed();
        assert!(text.contains("hello"), "Expected 'hello', got: {}", text);
    }
}
