//! Event loop for the terminal emulator.
//!
//! This module provides the core event loop that coordinates:
//! - Reading from the PTY (shell output)
//! - Parsing ANSI sequences
//! - Updating terminal state
//! - Handling user input

use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a foreground-process-name lookup stays valid before `/proc` is
/// scanned again. Resize storms and the periodic tab-title refresh both go
/// through this cache.
const FOREGROUND_NAME_TTL: Duration = Duration::from_millis(500);

/// Maximum PTY bytes drained per `process()` call so rendering and input
/// handling are never starved by a process that floods its terminal.
pub const PTY_DRAIN_BUDGET: usize = 1024 * 1024;

use polling::{Event, Events, Poller};

use crate::grid::{Dimensions, Terminal, TerminalHandler};
use crate::protocol::ansi::AnsiParser;
use crate::pty::{Child, PtySize};

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
    /// Cached foreground process name and when it was looked up
    foreground_name_cache: RefCell<Option<(Instant, Option<String>)>>,
    /// Bytes drained by the most recent `process()` call
    last_drain_bytes: usize,
    /// The last `process()` stopped at the drain budget with data left
    drain_pending: bool,
}

impl EventLoop {
    /// Create a new event loop with a shell.
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        let child = Child::spawn_shell(PtySize::new(cols, rows), &[]).map_err(io::Error::other)?;
        Self::from_child(child, cols, rows)
    }

    /// Create a new event loop with a shell in a specific working directory.
    pub fn new_with_dir(cols: u16, rows: u16, working_dir: &str) -> io::Result<Self> {
        let child = Child::spawn_shell_in_dir(PtySize::new(cols, rows), working_dir, &[])
            .map_err(io::Error::other)?;
        Self::from_child(child, cols, rows)
    }

    /// Create with a specific command.
    pub fn with_command(program: &str, args: &[&str], cols: u16, rows: u16) -> io::Result<Self> {
        let working_dir = crate::pty::current_dir();
        let child = Child::spawn(program, args, &working_dir, PtySize::new(cols, rows), &[])
            .map_err(io::Error::other)?;
        Self::from_child(child, cols, rows)
    }

    /// Drive an already spawned child whose PTY is `cols` x `rows`.
    ///
    /// Use this when the child needs spawn options the convenience
    /// constructors do not take, such as per-child environment variables.
    pub fn from_child(child: Child, cols: u16, rows: u16) -> io::Result<Self> {
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
            foreground_name_cache: RefCell::new(None),
            last_drain_bytes: 0,
            drain_pending: false,
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

    /// Get the PTY master file descriptor for readiness wakeups.
    pub fn pty_fd(&self) -> RawFd {
        self.child.pty_fd()
    }

    /// Get the child process ID.
    pub fn child_pid(&self) -> u32 {
        self.child.pid().as_raw() as u32
    }

    /// Best-effort name of the foreground process attached to this PTY.
    ///
    /// The answer is cached for [`FOREGROUND_NAME_TTL`]: it needs a full
    /// `/proc` scan, which must not run on every resize frame.
    pub fn foreground_process_name(&self) -> Option<String> {
        let now = Instant::now();
        if let Some((looked_up, name)) = self.foreground_name_cache.borrow().as_ref() {
            if now.duration_since(*looked_up) < FOREGROUND_NAME_TTL {
                return name.clone();
            }
        }
        let name = foreground_process_name(self.child.pty_fd());
        *self.foreground_name_cache.borrow_mut() = Some((now, name.clone()));
        name
    }

    /// Bytes drained from the PTY by the most recent [`EventLoop::process`].
    pub fn last_drain_bytes(&self) -> usize {
        self.last_drain_bytes
    }

    /// Whether the most recent [`EventLoop::process`] stopped at
    /// [`PTY_DRAIN_BUDGET`] with more output still queued in the PTY.
    pub fn drain_pending(&self) -> bool {
        self.drain_pending
    }

    /// Best-effort current working directory of the PTY child.
    pub fn child_working_directory(&self) -> Option<String> {
        child_working_directory(self.child_pid())
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
        self.process_with_budget(PTY_DRAIN_BUDGET)
    }

    /// Process pending PTY output, draining at most `budget` bytes.
    ///
    /// When the budget is exhausted [`EventLoop::drain_pending`] is set and
    /// the next [`EventLoop::wait`] resumes draining without blocking.
    pub fn process_with_budget(&mut self, budget: usize) -> io::Result<Vec<TerminalEvent>> {
        self.events.clear();
        let trace = pipeline_trace_enabled();
        let process_start = trace.then(Instant::now);
        let mut read_batches = 0usize;
        let mut read_bytes = 0usize;

        // Read available data from PTY FIRST (before checking child exit)
        // This ensures we don't miss output from fast-exiting processes like `echo`.
        // Stop after PTY_DRAIN_BUDGET bytes; the fd stays readable so the
        // next tick picks up the rest after a render/input pass.
        self.drain_pending = false;
        loop {
            let remaining = budget.saturating_sub(read_bytes);
            if remaining == 0 {
                self.drain_pending = true;
                break;
            }
            let read_len = remaining.min(self.read_buf.len());
            match self.child.read(&mut self.read_buf[..read_len]) {
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

        self.last_drain_bytes = read_bytes;

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
        if self.drain_pending {
            // The previous tick hit the drain budget; data is still queued.
            return self.process();
        }

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

#[cfg(target_os = "linux")]
fn foreground_process_name(pty_fd: RawFd) -> Option<String> {
    // SAFETY: `pty_fd` is an open PTY master file descriptor owned by the child
    // wrapper. `tcgetpgrp` only queries kernel metadata for that terminal.
    let pgrp = unsafe { nix::libc::tcgetpgrp(pty_fd) };
    if pgrp <= 0 {
        return None;
    }

    process_name_for_group(pgrp)
}

#[cfg(not(target_os = "linux"))]
fn foreground_process_name(_pty_fd: RawFd) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn child_working_directory(pid: u32) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| !path.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn child_working_directory(_pid: u32) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn process_name_for_group(pgrp: i32) -> Option<String> {
    let mut fallback = None;

    for entry in fs::read_dir("/proc").ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.file_name().to_string_lossy().parse::<i32>().is_err() {
            continue;
        }
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some(process) = parse_proc_stat(&stat) else {
            continue;
        };

        if process.pgrp != pgrp || is_shell_process(&process.name) {
            continue;
        }

        if process.pid == pgrp {
            return Some(process.name);
        }

        fallback.get_or_insert(process.name);
    }

    fallback
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct ProcStat {
    pid: i32,
    pgrp: i32,
    name: String,
}

#[cfg(target_os = "linux")]
fn parse_proc_stat(stat: &str) -> Option<ProcStat> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let pid = stat[..open].trim().parse().ok()?;
    let name = stat[open + 1..close].to_string();
    let mut fields = stat[close + 1..].split_whitespace();
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    let pgrp = fields.next()?.parse().ok()?;

    Some(ProcStat { pid, pgrp, name })
}

#[cfg(target_os = "linux")]
fn is_shell_process(name: &str) -> bool {
    matches!(
        name.rsplit('/').next().unwrap_or(name),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish"
    )
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

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_proc_stat_handles_process_names_with_spaces() {
        let stat = "1234 (name with space) S 1 5678 5678 0 -1 4194304";

        assert_eq!(
            parse_proc_stat(stat),
            Some(ProcStat {
                pid: 1234,
                pgrp: 5678,
                name: "name with space".to_string(),
            })
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shell_process_filter_recognizes_common_shells() {
        assert!(is_shell_process("zsh"));
        assert!(is_shell_process("/bin/bash"));
        assert!(!is_shell_process("nvim"));
    }

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

    #[test]
    fn process_drains_at_most_the_byte_budget_per_call() {
        let mut event_loop = EventLoop::with_command(
            "/bin/sh",
            &[
                "/bin/sh",
                "-c",
                "printf 'abcdefghijklmnopqrstuvwxyz0123456789'",
            ],
            80,
            24,
        )
        .expect("Failed to spawn printf");

        // Let the 36 bytes land in the PTY before draining with a tiny budget.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut first = None;
        while Instant::now() < deadline && first.is_none() {
            let mut poll_events = Events::new();
            event_loop
                .poller
                .wait(&mut poll_events, Some(Duration::from_millis(50)))
                .unwrap();
            if !poll_events.is_empty() {
                event_loop.process_with_budget(10).unwrap();
                first = Some(event_loop.last_drain_bytes());
            }
        }

        assert_eq!(
            first,
            Some(10),
            "first call must stop exactly at the budget"
        );
        assert!(event_loop.drain_pending());

        let mut total = 10;
        while event_loop.drain_pending() {
            event_loop.process_with_budget(10).unwrap();
            assert!(event_loop.last_drain_bytes() <= 10);
            total += event_loop.last_drain_bytes();
        }
        assert_eq!(total, 36);

        let text = event_loop.terminal().grid().row(0).unwrap().text_trimmed();
        assert_eq!(text, "abcdefghijklmnopqrstuvwxyz0123456789");
    }
}
