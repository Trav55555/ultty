//! Event loop for the terminal emulator.
//!
//! Each [`EventLoop`] drives one PTY child. A dedicated I/O thread reads the
//! child's output as fast as it arrives, parses it into the shared
//! [`Terminal`], and answers terminal queries (DSR, DA, DECRQM). The UI
//! thread never parses: it locks the terminal to render, queues input,
//! resizes, and collects [`TerminalEvent`]s with [`EventLoop::process`].
//! Parsing is therefore never paced by frames, and a flood of output cannot
//! stall input or rendering.
//!
//! Every write to the PTY goes through one ordered queue that the I/O thread
//! drains without blocking: keyboard input, query replies, and in-band
//! protocol strings. Two writers on one fd could splice bytes into the
//! middle of an in-band string, and a blocking write to a child that is not
//! reading would freeze whichever thread made it.
//!
//! In-band protocol data (ADR 0018) arrives in the child's output, is decoded
//! here, and is handed to the UI with [`EventLoop::take_in_band`]. Query
//! replies produced after such data are held until the UI answers it with
//! [`EventLoop::finish_in_band`], so a client sees replies in the order it
//! asked. That ordering is what the in-band probe relies on: a client sends
//! `ClientHello` and then DA1, and treats `ServerHello` arriving first as
//! support.

use std::cell::RefCell;
use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
use std::ops::{Deref, DerefMut};
use std::os::fd::{AsFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a foreground-process-name lookup stays valid before `/proc` is
/// scanned again. Resize storms and the periodic tab-title refresh both go
/// through this cache.
const FOREGROUND_NAME_TTL: Duration = Duration::from_millis(500);

/// Bytes the I/O thread reads (and parses under one lock) at a time. Small
/// enough that the renderer never waits long for the lock, large enough to
/// amortize the syscall and lock.
const READ_CHUNK: usize = 64 * 1024;

/// Events kept for the UI between two [`EventLoop::process`] calls. Bells,
/// titles, and notifications beyond this are dropped rather than queued
/// without bound while the UI is not collecting them.
const MAX_PENDING_EVENTS: usize = 1024;

/// Minimum spacing between wakes for plain output. The UI renders at most
/// once per frame, so waking it for every chunk of a flood only makes it
/// contend for the terminal lock. Events (bell, title, exit) wake at once,
/// and output whose wake was skipped is always flushed by a trailing wake.
const OUTPUT_WAKE_INTERVAL: Duration = Duration::from_millis(4);

/// Most decoded in-band bytes kept for the UI. More than this means the UI
/// has stopped collecting; the data is dropped and reported as broken.
const MAX_IN_BAND_PENDING: usize = 16 * 1024 * 1024;

/// Most bytes queued for the PTY. A child that stops reading its input fills
/// this, after which [`EventLoop::send_input`] fails instead of growing the
/// queue without bound.
const MAX_OUTBOUND: usize = 16 * 1024 * 1024;

/// After the PTY closes, how often and how many times the I/O thread pokes
/// the UI so it can reap the child once the kernel reports the exit.
const EXIT_POKE_INTERVAL: Duration = Duration::from_millis(20);
const EXIT_POKES: usize = 250;

/// How long after the child's exit to wait for the I/O thread to drain and
/// close the PTY before reporting [`TerminalEvent::ChildExited`] anyway. It
/// usually closes within milliseconds; it stays open only while a background
/// process the child left behind still holds it.
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(200);

/// The longest the exit report waits for unread output to be drained when
/// the PTY stays open. Past the grace the exit is held back only while the
/// kernel still has bytes for the I/O thread to read, which a loaded machine
/// can delay; this bounds that wait.
const EXIT_DRAIN_CAP: Duration = Duration::from_secs(2);

/// Whether to report the child's exit now: once the I/O thread has closed
/// the PTY; after [`EXIT_DRAIN_GRACE`] if the kernel holds no unread output
/// (`unread` is only asked then); and after [`EXIT_DRAIN_CAP`] regardless.
fn exit_report_due(closed: bool, waited: Duration, unread: impl FnOnce() -> usize) -> bool {
    closed || waited >= EXIT_DRAIN_CAP || (waited >= EXIT_DRAIN_GRACE && unread() == 0)
}

/// Where the child's exit cannot be watched (no pidfd), how often the I/O
/// thread pokes the UI to check for it.
const EXIT_CHECK_FALLBACK_INTERVAL: Duration = Duration::from_millis(250);

/// Poller keys in the I/O thread.
const PTY_KEY: usize = 0;
const EXIT_KEY: usize = 1;

use bytes::{Buf, BytesMut};
use polling::{Event, Events, Poller};

use crate::grid::{Dimensions, Terminal, TerminalHandler};
use crate::protocol::ansi::AnsiParser;
use crate::pty::{Child, Pty, PtyError, PtySize};

/// Called by the I/O thread when new terminal state or events are ready.
///
/// It runs on the I/O thread, so it must only schedule work (for example,
/// post a user event to the windowing event loop).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

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
    /// The program reported its progress (OSC 9;4).
    Progress(crate::protocol::ansi::Progress),
    /// Authoritative program records changed. Read the current snapshot;
    /// intermediate reports can be coalesced between calls to `process`.
    ProgramStatusChanged {
        /// Current generation in the terminal-owned status store.
        generation: u64,
        /// Cause of the latest change, not a complete mutation history.
        reason: ultty_vt::program_status::StatusChangeReason,
    },
    /// The shell started running a command (OSC 133;C).
    CommandStarted,
    /// The command finished (OSC 133;D), with the exit code the shell
    /// reported, if any.
    CommandFinished { exit_code: Option<i32> },
    /// A command submitted through the fenced shell API completed.
    ShellCommandFinished {
        /// Submission identity, independent of shell row-mark counters.
        sequence: u64,
        /// Exit code from its matched OSC 133 C/D lifecycle.
        exit_code: Option<i32>,
    },
}

/// Authoritative result of one fenced shell submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommandOutcome {
    /// A matching start and finish were observed after the reservation.
    Finished {
        /// The shell's reported exit code, if supplied.
        exit_code: Option<i32>,
    },
    /// Prompt/reset/process exit invalidated the pending reservation.
    Cancelled,
}

#[derive(Debug)]
struct ShellReservation {
    sequence: u64,
    started: bool,
}

#[derive(Debug, Default)]
struct ShellCommands {
    next_sequence: u64,
    prompt_ready: bool,
    pending: Option<ShellReservation>,
    results: VecDeque<(u64, ShellCommandOutcome)>,
}

impl ShellCommands {
    fn finish(&mut self, outcome: ShellCommandOutcome) {
        if let Some(pending) = self.pending.take() {
            if self.results.len() == 64 {
                self.results.pop_front();
            }
            self.results.push_back((pending.sequence, outcome));
        }
    }

    fn cancel(&mut self) {
        self.prompt_ready = false;
        self.finish(ShellCommandOutcome::Cancelled);
    }

    fn observe(&mut self, action: &crate::protocol::ansi::Action) {
        use crate::protocol::ansi::Action;
        match action {
            Action::PromptStart | Action::FullReset | Action::SoftReset => self.cancel(),
            Action::CommandStart => {
                // A fresh editable prompt cannot inherit an older reservation.
                self.finish(ShellCommandOutcome::Cancelled);
                self.prompt_ready = true;
            }
            Action::CommandExecuted => {
                self.prompt_ready = false;
                if self.pending.as_ref().is_some_and(|pending| pending.started) {
                    self.finish(ShellCommandOutcome::Cancelled);
                } else if let Some(pending) = self.pending.as_mut() {
                    pending.started = true;
                }
            }
            Action::CommandFinished(exit_code) => {
                if self.pending.as_ref().is_some_and(|pending| pending.started) {
                    self.finish(ShellCommandOutcome::Finished {
                        exit_code: *exit_code,
                    });
                }
            }
            Action::Print(_) => self.prompt_ready = false,
            _ => {}
        }
    }
}

/// State shared between the UI thread and a terminal's I/O thread.
struct State {
    shell_commands: ShellCommands,
    attached_exited: bool,
    terminal: Terminal,
    events: Vec<TerminalEvent>,
    /// Output was parsed since the UI last collected events.
    output_pending: bool,
    /// Status changed without necessarily changing any grid pixels.
    status_pending: bool,
    bytes_since_collect: usize,
    /// The PTY reported end of file or an error; the reader has stopped.
    closed: bool,
    /// Bumped on every parsed chunk and on close, for [`EventLoop::wait`].
    generation: u64,
    /// Decoded in-band frame bytes the UI has not taken yet.
    in_band: Vec<u8>,
    /// Since the UI last took in-band input, a string was not valid base64
    /// or `in_band` overflowed. The stream is no longer trustworthy.
    in_band_broken: bool,
    /// The UI took in-band input and has not answered it yet.
    in_band_open: bool,
    /// Query replies held until the UI answers pending in-band input.
    held_responses: Vec<u8>,
    /// Bytes for the PTY, written in order by the I/O thread.
    outbound: BytesMut,
}

impl State {
    /// Whether query replies must wait for the UI's in-band answer.
    fn holds_responses(&self) -> bool {
        self.in_band_open || self.in_band_broken || !self.in_band.is_empty()
    }

    fn has_in_band(&self) -> bool {
        self.in_band_broken || !self.in_band.is_empty()
    }

    /// Whether the UI has anything to collect.
    fn ready(&self) -> bool {
        self.output_pending
            || self.status_pending
            || !self.events.is_empty()
            || self.closed
            || self.has_in_band()
    }
}

/// In-band protocol data taken from the PTY output by
/// [`EventLoop::take_in_band`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InBandInput {
    /// Decoded frame bytes, in order. They may end mid-frame.
    pub bytes: Vec<u8>,
    /// Some in-band data was malformed or dropped since the last take, so
    /// any session reading this stream should end.
    pub broken: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Threads inside [`EventLoop::wait`]. `Condvar::notify_all` is a futex
    /// system call even with nobody waiting, and the app never waits, so the
    /// I/O thread skips it at zero.
    waiters: AtomicUsize,
    /// Set when the I/O thread wakes the UI and cleared when the UI collects
    /// events, so at most one wake is outstanding per terminal.
    wake_pending: AtomicBool,
    waker: Mutex<Option<Waker>>,
    stop: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // The I/O thread never panics while holding the lock in practice; if
        // it did, the terminal state is still the best we have.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wake threads blocked in [`EventLoop::wait`], if there are any. Call it
    /// after the state change is unlocked: a waiter registers while holding
    /// the lock and rechecks `ready` before sleeping, so one that registers
    /// after the load here sees the change and does not sleep.
    fn notify_waiters(&self) {
        if self.waiters.load(Ordering::SeqCst) > 0 {
            self.changed.notify_all();
        }
    }

    /// Tell waiters and the UI that something changed.
    fn notify(&self) {
        self.notify_waiters();
        if !self.wake_pending.swap(true, Ordering::AcqRel) {
            let waker = self
                .waker
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(waker) = waker {
                waker();
            }
        }
    }
}

/// Locked access to a terminal driven by an [`EventLoop`].
///
/// While a guard is alive the I/O thread cannot parse into this terminal,
/// so hold it only as long as needed (one frame's render, one query). Taking
/// a second guard on the same terminal from the same thread would deadlock;
/// debug builds panic instead.
pub struct TerminalGuard<'a> {
    state: MutexGuard<'a, State>,
    #[cfg(debug_assertions)]
    _held: reentry::Held,
}

impl Deref for TerminalGuard<'_> {
    type Target = Terminal;

    fn deref(&self) -> &Terminal {
        &self.state.terminal
    }
}

impl DerefMut for TerminalGuard<'_> {
    fn deref_mut(&mut self) -> &mut Terminal {
        // Conservative invalidation: even a mutable borrow which makes no
        // change is cheaper than missing a viewer-visible local mutation.
        self.state.generation = self.state.generation.wrapping_add(1);
        &mut self.state.terminal
    }
}

impl TerminalGuard<'_> {
    /// Authoritative observation generation at this locked snapshot boundary.
    /// Parsing, stream closure and local mutable access advance it, independently
    /// of coalesced/lost display events. Scoped to this terminal's lifetime.
    pub fn snapshot_generation(&self) -> u64 {
        self.state.generation
    }
}

/// Debug-only detection of a thread locking the same terminal twice.
#[cfg(debug_assertions)]
mod reentry {
    use std::cell::RefCell;

    thread_local! {
        static HELD: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) struct Held(usize);

    impl Held {
        pub(super) fn acquire(id: usize) -> Self {
            HELD.with(|held| {
                let mut held = held.borrow_mut();
                assert!(
                    !held.contains(&id),
                    "terminal locked twice on one thread; this deadlocks in release builds"
                );
                held.push(id);
            });
            Held(id)
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            HELD.with(|held| held.borrow_mut().retain(|&id| id != self.0));
        }
    }
}

/// The terminal event loop.
///
/// Owns the PTY child and the I/O thread that parses its output.
pub struct EventLoop {
    /// Child process with PTY
    child: Child,
    shared: Arc<Shared>,
    /// Wakes the I/O thread's poll so it can observe `stop`.
    reader_poller: Arc<Poller>,
    reader: Option<JoinHandle<()>>,
    /// Cached foreground process name and when it was looked up
    foreground_name_cache: RefCell<Option<(Instant, Option<String>)>>,
    /// Bytes parsed between the two most recent [`EventLoop::process`] calls
    last_drain_bytes: usize,
    /// When `waitpid` first reported the child's exit.
    exit_seen_at: Option<Instant>,
    /// Whether `ChildExited` has gone out. It goes out once.
    exit_reported: bool,
    /// Last status generation delivered, independent of the lossy event queue.
    status_generation: u64,
    /// Last fenced result announced; the bounded result store is authoritative.
    shell_result_sequence: u64,
    /// [`Terminal::uid`] of the terminal this loop drives; readable without
    /// taking the terminal lock.
    terminal_uid: u64,
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
        let mut terminal = Terminal::new(Dimensions::new(cols, rows));
        let terminal_uid = terminal.uid();
        terminal.set_terminal_name(child.term());
        // The UI thread pairs these anchors with native sessions' inline
        // surfaces (ADR 0020); see `ProtocolServer::pair_inline_surfaces`.
        terminal.set_inline_anchors(true);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                shell_commands: ShellCommands::default(),
                attached_exited: false,
                terminal,
                events: Vec::new(),
                output_pending: false,
                status_pending: false,
                bytes_since_collect: 0,
                closed: false,
                generation: 0,
                in_band: Vec::new(),
                in_band_broken: false,
                in_band_open: false,
                held_responses: Vec::new(),
                outbound: BytesMut::new(),
            }),
            changed: Condvar::new(),
            waiters: AtomicUsize::new(0),
            wake_pending: AtomicBool::new(false),
            waker: Mutex::new(None),
            stop: AtomicBool::new(false),
        });

        let reader_pty = child.pty().try_clone().map_err(io::Error::other)?;
        let poller = Arc::new(Poller::new()?);
        // SAFETY: `reader_pty` owns the registered fd and is moved into the
        // I/O thread, which deletes the registration before dropping it.
        unsafe {
            poller.add(&reader_pty, Event::readable(PTY_KEY))?;
        }
        let exit_watch = ExitWatch::new(child.pid().as_raw(), &poller);

        let reader = std::thread::Builder::new()
            .name(format!("ultty-pty-{}", child.pid()))
            .spawn({
                let shared = Arc::clone(&shared);
                let poller = Arc::clone(&poller);
                move || run_reader(reader_pty, &shared, &poller, exit_watch)
            })?;

        Ok(Self {
            child,
            shared,
            reader_poller: poller,
            reader: Some(reader),
            foreground_name_cache: RefCell::new(None),
            last_drain_bytes: 0,
            exit_seen_at: None,
            exit_reported: false,
            status_generation: 0,
            shell_result_sequence: 0,
            terminal_uid,
        })
    }

    /// [`Terminal::uid`] of the driven terminal, without locking it.
    pub fn terminal_uid(&self) -> u64 {
        self.terminal_uid
    }

    /// Register the callback the I/O thread uses to wake the UI.
    ///
    /// If output is already waiting, the waker is called right away.
    pub fn set_waker(&self, waker: Waker) {
        *self
            .shared
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(waker);
        // Drop any wake that fired before a waker existed so this call
        // re-delivers it.
        self.shared.wake_pending.store(false, Ordering::Release);
        let pending = self.shared.lock().ready();
        if pending {
            self.shared.notify();
        }
    }

    /// Lock and return the terminal state.
    pub fn terminal(&self) -> TerminalGuard<'_> {
        TerminalGuard {
            #[cfg(debug_assertions)]
            _held: reentry::Held::acquire(Arc::as_ptr(&self.shared) as usize),
            state: self.shared.lock(),
        }
    }

    /// Lock and return the terminal state for mutation.
    pub fn terminal_mut(&mut self) -> TerminalGuard<'_> {
        self.terminal()
    }

    /// Observe whether a snapshot may have changed without consuming events.
    pub fn snapshot_generation(&self) -> u64 {
        self.shared.lock().generation
    }

    /// Check if the child process has exited.
    pub fn has_exited(&self) -> bool {
        self.child.has_exited()
    }

    /// Whether the child exit was reported after the bounded final-output drain.
    /// Owners must keep the PTY and its events until this boundary.
    pub fn exit_drained(&self) -> bool {
        self.exit_reported
    }

    /// Get the PTY master file descriptor.
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

    /// Bytes the I/O thread parsed between the two most recent
    /// [`EventLoop::process`] calls.
    pub fn last_drain_bytes(&self) -> usize {
        self.last_drain_bytes
    }

    /// Best-effort current working directory of the PTY child.
    pub fn child_working_directory(&self) -> Option<String> {
        child_working_directory(self.child_pid())
    }

    /// Best-effort working directory of the pane's foreground process (what
    /// Alacritty opens new windows in), else of the shell itself.
    pub fn foreground_working_directory(&self) -> Option<String> {
        foreground_working_directory(self.child.pty_fd())
            .or_else(|| child_working_directory(self.child_pid()))
    }

    /// Get exit status if child has exited.
    pub fn exit_status(&self) -> Option<i32> {
        self.child.exit_status()
    }

    /// Resize the terminal.
    ///
    /// The grid is resized before the PTY, so output the child produces in
    /// response to SIGWINCH is always parsed at the new size.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let cell = {
            let mut terminal = self.terminal();
            if terminal.dimensions() != Dimensions::new(cols, rows) {
                terminal.resize(Dimensions::new(cols, rows));
            }
            terminal.cell_pixel_size()
        };
        self.child
            .resize(PtySize::for_cells(cols, rows, cell))
            .map_err(io::Error::other)
    }

    /// Queue input for the shell. The I/O thread writes it in order.
    ///
    /// Fails once the PTY has closed, or when the child has stopped reading
    /// and [`MAX_OUTBOUND`] bytes are already waiting.
    pub fn send_input(&mut self, data: &[u8]) -> io::Result<()> {
        self.enqueue(
            |state| {
                if !data.is_empty() {
                    // Further typing may edit the submitted line or interrupt
                    // its program. Its eventual C/D is no longer attributable.
                    state.shell_commands.cancel();
                }
                state.outbound.extend_from_slice(data);
            },
            data.len(),
        )
    }

    /// Atomically submit one printable shell line at an observed empty prompt.
    ///
    /// Requires OSC 133 B, no subsequent editable input/output, an idle shell,
    /// an empty input queue and no pending submission. Acceptance only means
    /// queued; query `shell_command_result` for the matched completion.
    pub fn submit_shell_command(&mut self, command: &str) -> io::Result<u64> {
        if command.trim().is_empty() || command.chars().any(char::is_control) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "command must be one nonempty printable shell line",
            ));
        }
        let len = command
            .len()
            .checked_add(1)
            .filter(|len| *len <= MAX_OUTBOUND)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "command exceeds PTY input budget",
                )
            })?;
        let sequence;
        {
            let mut state = self.shared.lock();
            if state.closed || state.attached_exited || self.child.has_exited() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "PTY closed"));
            }
            if !state.shell_commands.prompt_ready
                || state.shell_commands.pending.is_some()
                || !state.outbound.is_empty()
                || state.terminal.is_alternate_screen()
                || state.terminal.shell_integration().command_executing
                || self.child.pty().unread_bytes().unwrap_or(1) != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "no observed empty shell prompt",
                ));
            }
            sequence = state
                .shell_commands
                .next_sequence
                .checked_add(1)
                .ok_or_else(|| io::Error::other("shell submission sequence exhausted"))?;
            // Capacity and every precondition are checked before reservation.
            state.outbound.reserve(len);
            state.outbound.extend_from_slice(command.as_bytes());
            state.outbound.extend_from_slice(b"\n");
            state.shell_commands.next_sequence = sequence;
            state.shell_commands.prompt_ready = false;
            state.shell_commands.pending = Some(ShellReservation {
                sequence,
                started: false,
            });
        }
        if let Err(error) = self.reader_poller.notify() {
            // It is already accepted. Returning a retryable error here could
            // duplicate a command the reader consumed before notify failed.
            tracing::warn!("accepted shell submission wake failed: {error}");
        }
        Ok(sequence)
    }

    /// Result for a submitted sequence, retained for the most recent 64 results.
    /// `None` means pending, unknown, or older than the retained window.
    pub fn shell_command_result(&self, sequence: u64) -> Option<ShellCommandOutcome> {
        self.shared
            .lock()
            .shell_commands
            .results
            .iter()
            .find_map(|(id, outcome)| (*id == sequence).then_some(*outcome))
    }

    /// Take the in-band protocol data parsed since the last call, if any.
    ///
    /// Query replies stay held until [`EventLoop::finish_in_band`] is
    /// called, so callers must always call it after a `Some`.
    pub fn take_in_band(&self) -> Option<InBandInput> {
        let mut state = self.shared.lock();
        if !state.has_in_band() {
            return None;
        }
        state.in_band_open = true;
        Some(InBandInput {
            bytes: std::mem::take(&mut state.in_band),
            broken: std::mem::take(&mut state.in_band_broken),
        })
    }

    /// Answer in-band input taken with [`EventLoop::take_in_band`]: queue
    /// `frames` as in-band strings, then the query replies held behind them.
    pub fn finish_in_band(&self, frames: &[u8]) -> io::Result<()> {
        let wire = ultty_protocol::in_band::encode_to_vec(frames);
        // Clearing `in_band_open`, choosing the held replies, and queueing
        // them must happen under one lock. Otherwise the I/O thread could
        // see nothing held in between and send a newer reply first.
        self.enqueue(
            |state| {
                state.in_band_open = false;
                state.outbound.extend_from_slice(&wire);
                // If more in-band data arrived after the take, the held
                // replies may include ones it caused, which must follow the
                // next answer. Holding them all is safe; sending any early
                // is not.
                if !state.holds_responses() {
                    let held = std::mem::take(&mut state.held_responses);
                    state.outbound.extend_from_slice(&held);
                }
            },
            wire.len(),
        )
    }

    /// Queue protocol `frames` for the child as in-band strings.
    pub fn send_in_band(&self, frames: &[u8]) -> io::Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        let wire = ultty_protocol::in_band::encode_to_vec(frames);
        self.enqueue(|state| state.outbound.extend_from_slice(&wire), wire.len())
    }

    /// Process group in the foreground of this PTY.
    pub fn foreground_pgrp(&self) -> Option<i32> {
        // SAFETY: `pty_fd` is the open PTY master owned by `self.child`;
        // `tcgetpgrp` only reads kernel metadata for that terminal.
        let pgrp = unsafe { libc::tcgetpgrp(self.child.pty_fd()) };
        (pgrp > 0).then_some(pgrp)
    }

    /// Processes in this PTY's foreground process group other than shells:
    /// the candidates for the program running there. Empty when it cannot
    /// be determined (no `/proc`).
    pub fn foreground_pids(&self) -> Vec<i32> {
        self.foreground_pgrp()
            .map(pids_in_group)
            .unwrap_or_default()
    }

    fn enqueue(&self, push: impl FnOnce(&mut State), len: usize) -> io::Result<()> {
        {
            let mut state = self.shared.lock();
            if state.closed {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "PTY closed"));
            }
            if state.outbound.len() + len > MAX_OUTBOUND {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "PTY input queue full: child is not reading its input",
                ));
            }
            push(&mut state);
        }
        self.reader_poller.notify()
    }

    /// Send a key press to the shell.
    pub fn send_key(&mut self, key: &str) -> io::Result<()> {
        self.send_input(key.as_bytes())
    }

    /// Collect what happened since the last call (non-blocking).
    ///
    /// Returns one [`TerminalEvent::Redraw`] if any output was parsed,
    /// followed by bells, titles, clipboard, and notification events in the
    /// order they arrived, and, once, [`TerminalEvent::ChildExited`] after
    /// the child has exited.
    pub fn process(&mut self) -> io::Result<Vec<TerminalEvent>> {
        self.shared.wake_pending.store(false, Ordering::Release);
        let mut events = Vec::new();
        let closed = {
            let mut state = self.shared.lock();
            if state.output_pending {
                state.output_pending = false;
                events.push(TerminalEvent::Redraw);
            }
            state.status_pending = false;
            events.append(&mut state.events);
            self.last_drain_bytes = std::mem::take(&mut state.bytes_since_collect);
            state.closed
        };

        // Report the exit only after the child's last output: once the I/O
        // thread has drained and closed the PTY, or, if something else keeps
        // the PTY open, after a short grace once nothing is left unread (a
        // starved I/O thread may not have read the last chunk yet), and in
        // any case after EXIT_DRAIN_CAP. waitpid alone sees the exit while
        // the final chunk can still be unread.
        let mut exited_status = None;
        if self.child.try_wait().map_err(io::Error::other)? {
            let seen_at = *self.exit_seen_at.get_or_insert_with(Instant::now);
            let unread = || self.child.pty().unread_bytes().unwrap_or(0);
            let report = !self.exit_reported && exit_report_due(closed, seen_at.elapsed(), unread);
            if report {
                self.exit_reported = true;
                let mut state = self.shared.lock();
                state.attached_exited = true;
                state.shell_commands.cancel();
                state.terminal.process_exited();
                state.generation = state.generation.wrapping_add(1);
                exited_status = self.child.exit_status();
            }
        }
        let status = {
            let state = self.shared.lock();
            let status = state.terminal.program_status();
            (status.generation(), status.reason())
        };
        if status.0 != self.status_generation {
            self.status_generation = status.0;
            events.push(TerminalEvent::ProgramStatusChanged {
                generation: status.0,
                reason: status.1,
            });
        }
        if let Some(status) = exited_status {
            events.push(TerminalEvent::ChildExited { status });
        }
        {
            let state = self.shared.lock();
            for (sequence, outcome) in &state.shell_commands.results {
                if *sequence > self.shell_result_sequence {
                    if let ShellCommandOutcome::Finished { exit_code } = outcome {
                        events.push(TerminalEvent::ShellCommandFinished {
                            sequence: *sequence,
                            exit_code: *exit_code,
                        });
                    }
                    self.shell_result_sequence = *sequence;
                }
            }
        }
        Ok(events)
    }

    /// Draw `bytes` as if the program had written them, for notes from the
    /// terminal itself such as "process exited". Replies the bytes would
    /// cause are dropped, not sent to the program.
    pub fn write_local(&self, bytes: &[u8]) {
        let mut state = self.shared.lock();
        parse_into(&mut state, &mut AnsiParser::new(), bytes);
        state.terminal.take_responses();
        state.output_pending = true;
    }

    /// Wait up to `timeout` for output or an exit, then [`EventLoop::process`].
    pub fn wait(&mut self, timeout: Duration) -> io::Result<Vec<TerminalEvent>> {
        {
            let state = self.shared.lock();
            if !state.ready() {
                self.shared.waiters.fetch_add(1, Ordering::SeqCst);
                let _ = self
                    .shared
                    .changed
                    .wait_timeout_while(state, timeout, |s| !s.ready())
                    .unwrap_or_else(PoisonError::into_inner);
                self.shared.waiters.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.process()
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
                                                                // No protocol server here: answer in-band data with nothing so
                                                                // held query replies go out.
            if self.take_in_band().is_some() {
                self.finish_in_band(&[])?;
            }

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

impl Drop for EventLoop {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.reader_poller.notify();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The I/O thread: read, parse, answer queries, wake the UI, repeat.
fn run_reader(pty: Pty, shared: &Shared, poller: &Poller, mut exit_watch: ExitWatch) {
    let mut parser = AnsiParser::new();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut poll_events = Events::new();
    let mut wakes = WakeThrottle::default();

    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        match drain(&pty, &mut parser, &mut buf, shared, &mut wakes)
            .and_then(|()| flush_outbound(&pty, shared))
        {
            Ok(()) => {}
            Err(err) => {
                if !matches!(err, PtyError::Closed) {
                    tracing::warn!("PTY read failed: {err}");
                }
                close(shared);
                break;
            }
        }

        // Readiness is one-shot; re-arm before sleeping. If an output wake
        // was skipped, sleep no longer than it takes to become due.
        poll_events.clear();
        let timeout = match (wakes.flush_due_in(), exit_watch.poke_due_in()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let interest = if shared.lock().outbound.is_empty() {
            Event::readable(PTY_KEY)
        } else {
            Event::all(PTY_KEY)
        };
        if let Err(err) = poller
            .modify(pty.as_fd(), interest)
            .and_then(|()| poller.wait(&mut poll_events, timeout))
        {
            tracing::warn!("PTY poll failed: {err}");
            close(shared);
            break;
        }
        if poll_events.iter().any(|event| event.key == EXIT_KEY) {
            exit_watch.child_exited(poller);
        }
        if exit_watch.poke_if_due() {
            shared.notify();
        }
        // Readable with nothing to read means the other side hung up on
        // platforms that report EOF as a zero-length read.
        if poll_events.iter().any(|event| event.key == PTY_KEY)
            && hung_up(&pty)
            && !has_pending_input(&pty)
        {
            close(shared);
            break;
        }
        wakes.flush_if_due(shared);
    }
    let _ = poller.delete(pty.as_fd());
    exit_watch.stop(poller);

    // The UI reaps the child; keep nudging it until it has, or we are told
    // to stop. A stopped UI drops the EventLoop, which sets `stop`.
    for _ in 0..EXIT_POKES {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(EXIT_POKE_INTERVAL);
        shared.notify();
    }
}

/// Tells the UI when the child exits while its PTY stays open.
///
/// The PTY usually closes when the child exits, which ends the I/O loop and
/// starts the exit pokes. A background process the child left behind can
/// keep the PTY open, though, and the UI only reaps the child and reports
/// the exit when something wakes it. On Linux a pidfd makes the exit itself
/// that wakeup; the UI is then poked every [`EXIT_POKE_INTERVAL`] until it
/// has reported the exit (after [`EXIT_DRAIN_GRACE`]) and dropped or
/// stopped the loop. Without a pidfd the UI is poked every
/// [`EXIT_CHECK_FALLBACK_INTERVAL`] for as long as the PTY is open.
struct ExitWatch {
    pidfd: Option<std::os::fd::OwnedFd>,
    /// When to poke the UI next, and how many exit pokes are left.
    next_poke: Option<Instant>,
    pokes_left: usize,
}

impl ExitWatch {
    fn new(pid: i32, poller: &Poller) -> Self {
        let pidfd = open_pidfd(pid).filter(|pidfd| {
            // SAFETY: the watch owns `pidfd` and deletes it from the poller
            // before dropping it (`child_exited` or `stop`).
            unsafe { poller.add(pidfd, Event::readable(EXIT_KEY)) }.is_ok()
        });
        let next_poke = pidfd
            .is_none()
            .then(|| Instant::now() + EXIT_CHECK_FALLBACK_INTERVAL);
        Self {
            pidfd,
            next_poke,
            pokes_left: 0,
        }
    }

    /// The pidfd reported the exit: poke the UI now, then until it is done.
    fn child_exited(&mut self, poller: &Poller) {
        self.stop(poller);
        self.next_poke = Some(Instant::now());
        self.pokes_left = EXIT_POKES;
    }

    fn poke_due_in(&self) -> Option<Duration> {
        self.next_poke
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    /// Whether to poke the UI now; schedules the next poke if so.
    fn poke_if_due(&mut self) -> bool {
        let now = Instant::now();
        if self.next_poke.is_none_or(|at| at > now) {
            return false;
        }
        self.next_poke = if self.pidfd.is_none() && self.pokes_left == 0 {
            Some(now + EXIT_CHECK_FALLBACK_INTERVAL)
        } else {
            self.pokes_left = self.pokes_left.saturating_sub(1);
            (self.pokes_left > 0).then(|| now + EXIT_POKE_INTERVAL)
        };
        true
    }

    /// Stop watching the pidfd.
    fn stop(&mut self, poller: &Poller) {
        if let Some(pidfd) = self.pidfd.take() {
            let _ = poller.delete(&pidfd);
        }
    }
}

/// A pidfd for `pid`, readable once the process exits.
#[cfg(target_os = "linux")]
fn open_pidfd(pid: i32) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        tracing::debug!(
            "pidfd_open({pid}) failed: {}; polling for the child's exit",
            io::Error::last_os_error()
        );
        return None;
    }
    // SAFETY: the syscall returned a new fd that nothing else owns.
    Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as RawFd) })
}

#[cfg(not(target_os = "linux"))]
fn open_pidfd(_pid: i32) -> Option<std::os::fd::OwnedFd> {
    None
}

/// Read and parse everything currently available.
fn drain(
    pty: &Pty,
    parser: &mut AnsiParser,
    buf: &mut [u8],
    shared: &Shared,
    wakes: &mut WakeThrottle,
) -> Result<(), PtyError> {
    let trace = pipeline_trace_enabled();
    let start = trace.then(Instant::now);
    let mut read_batches = 0usize;
    let mut read_bytes = 0usize;
    loop {
        // The master is nonblocking. Reading and applying its bytes share
        // submit's lock, so drained-but-unparsed old C/D markers cannot be
        // mistaken for a newly reserved command.
        let (n, urgent) = read_and_parse(shared, parser, buf, |buf| pty.read(buf))?;
        if n == 0 {
            if let Some(start) = start.filter(|_| read_bytes > 0) {
                tracing::info!(
                    target: "ultty::perf",
                    read_batches,
                    read_bytes,
                    elapsed_us = start.elapsed().as_micros(),
                    "pipeline_pty_drain"
                );
            }
            return Ok(());
        }
        read_batches += 1;
        read_bytes += n;
        flush_outbound(pty, shared)?;
        wakes.output(shared, urgent);
    }
}

/// Read once and apply under the input reservation lock. Production readers
/// must be nonblocking; extraction permits deterministic read/parse race tests.
fn read_and_parse(
    shared: &Shared,
    parser: &mut AnsiParser,
    buf: &mut [u8],
    read: impl FnOnce(&mut [u8]) -> Result<usize, PtyError>,
) -> Result<(usize, bool), PtyError> {
    let mut state = shared.lock();
    let n = read(buf)?;
    if n == 0 {
        return Ok((0, false));
    }
    parse_into(&mut state, parser, &buf[..n]);
    let hold = state.holds_responses();
    for response in state.terminal.take_responses() {
        if hold {
            state.held_responses.extend_from_slice(&response);
        } else {
            state.outbound.extend_from_slice(&response);
        }
    }
    let urgent = state.status_pending || !state.events.is_empty() || state.has_in_band();
    Ok((n, urgent))
}

/// Rate limit for output wakes; see [`OUTPUT_WAKE_INTERVAL`].
#[derive(Default)]
struct WakeThrottle {
    last_wake: Option<Instant>,
    /// Output arrived whose wake was skipped.
    pending: bool,
}

impl WakeThrottle {
    /// New output was parsed; `urgent` if it also queued events.
    fn output(&mut self, shared: &Shared, urgent: bool) {
        let due = self
            .last_wake
            .is_none_or(|at| at.elapsed() >= OUTPUT_WAKE_INTERVAL);
        if urgent || due {
            self.wake(shared);
        } else {
            // Waiters in `EventLoop::wait` still see progress immediately.
            shared.notify_waiters();
            self.pending = true;
        }
    }

    fn wake(&mut self, shared: &Shared) {
        shared.notify();
        self.last_wake = Some(Instant::now());
        self.pending = false;
    }

    /// How long the reader may sleep before a skipped wake must go out.
    fn flush_due_in(&self) -> Option<Duration> {
        self.pending.then(|| {
            self.last_wake.map_or(Duration::ZERO, |at| {
                OUTPUT_WAKE_INTERVAL.saturating_sub(at.elapsed())
            })
        })
    }

    fn flush_if_due(&mut self, shared: &Shared) {
        if self.flush_due_in() == Some(Duration::ZERO) {
            self.wake(shared);
        }
    }
}

/// Write queued bytes until the queue is empty or the PTY would block.
fn flush_outbound(pty: &Pty, shared: &Shared) -> Result<(), PtyError> {
    let mut state = shared.lock();
    while !state.outbound.is_empty() {
        match pty.write(&state.outbound) {
            Ok(0) => break,
            Ok(n) => state.outbound.advance(n),
            Err(PtyError::Closed) => return Err(PtyError::Closed),
            Err(PtyError::WriteFailed(err)) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => {
                tracing::warn!("PTY write failed, dropping queued input: {err}");
                state.outbound.clear();
                break;
            }
        }
    }
    Ok(())
}

fn close(shared: &Shared) {
    {
        let mut state = shared.lock();
        state.closed = true;
        state.generation = state.generation.wrapping_add(1);
    }
    shared.notify();
}

/// Whether the PTY master reports a hangup.
fn hung_up(pty: &Pty) -> bool {
    poll_revents(pty, libc::POLLIN) & libc::POLLHUP != 0
}

/// Whether bytes are still queued (a HUP can arrive with output pending).
fn has_pending_input(pty: &Pty) -> bool {
    let mut pending: libc::c_int = 0;
    // SAFETY: FIONREAD writes one c_int to `pending`; the fd is open for the
    // duration of the call because `pty` is borrowed.
    let rc = unsafe {
        libc::ioctl(
            std::os::fd::AsRawFd::as_raw_fd(pty),
            libc::FIONREAD,
            &mut pending,
        )
    };
    rc == 0 && pending > 0
}

fn poll_revents(pty: &Pty, events: libc::c_short) -> libc::c_short {
    let mut pollfd = libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(pty),
        events,
        revents: 0,
    };
    // SAFETY: one valid pollfd entry, zero timeout; `pty` keeps the fd open.
    let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
    if ready <= 0 {
        0
    } else {
        pollfd.revents
    }
}

/// Parse one chunk of PTY output into the shared state.
fn parse_into(state: &mut State, parser: &mut AnsiParser, data: &[u8]) {
    use crate::protocol::ansi::{Action, AnsiOutput, WindowOp};

    let status_before = state.terminal.program_status().generation();
    let State {
        shell_commands,
        terminal,
        events,
        in_band,
        in_band_broken,
        ..
    } = state;
    let mut handler = TerminalHandler::new(terminal);
    let trace = pipeline_trace_enabled();
    let start = trace.then(Instant::now);
    let mut actions = 0usize;
    let mut print_actions = 0usize;
    let mut grid_output = false;

    parser.feed_with_ascii_spans(data, |output| match output {
        AnsiOutput::Action(action) => {
            shell_commands.observe(&action);
            grid_output |= !matches!(
                action,
                Action::ProgramStatus(_) | Action::ProgramStatusQuery
            );
            if trace {
                actions += 1;
                if matches!(action, Action::Print(_)) {
                    print_actions += 1;
                }
            }
            let event = match &action {
                Action::Bell => Some(TerminalEvent::Bell),
                Action::SetTitle(title) => Some(TerminalEvent::TitleChanged(title.clone())),
                Action::SetClipboard { clipboard, data } => Some(TerminalEvent::SetClipboard {
                    clipboard: *clipboard,
                    data: data.clone(),
                }),
                Action::Notification { title, body } => Some(TerminalEvent::Notification {
                    title: title.clone(),
                    body: body.clone(),
                }),
                Action::Progress(progress) => Some(TerminalEvent::Progress(*progress)),
                Action::CommandExecuted => Some(TerminalEvent::CommandStarted),
                Action::CommandFinished(exit_code) => Some(TerminalEvent::CommandFinished {
                    exit_code: *exit_code,
                }),
                Action::InBand(text) => {
                    if in_band.len() + text.len() > MAX_IN_BAND_PENDING
                        || ultty_protocol::in_band::decode_payload(text, in_band).is_err()
                    {
                        *in_band_broken = true;
                    }
                    None
                }
                _ => None,
            };
            let pops_title = matches!(action, Action::WindowOp(WindowOp::PopTitle(_)));
            handler.handle(action);
            // A title popped off the title stack is only known once applied.
            let event = event.or_else(|| {
                pops_title
                    .then(|| TerminalEvent::TitleChanged(handler.terminal().title().to_owned()))
            });
            if let Some(event) = event {
                let repeat_bell = matches!(event, TerminalEvent::Bell)
                    && matches!(events.last(), Some(TerminalEvent::Bell));
                // Only the latest of a run of progress reports matters.
                if matches!(event, TerminalEvent::Progress(_))
                    && matches!(events.last(), Some(TerminalEvent::Progress(_)))
                {
                    events.pop();
                }
                if !repeat_bell && events.len() < MAX_PENDING_EVENTS {
                    events.push(event);
                }
            }
        }
        AnsiOutput::PrintAscii(bytes) => {
            grid_output |= !bytes.is_empty();
            if !bytes.is_empty() {
                shell_commands.prompt_ready = false;
            }
            if trace {
                actions += 1;
                print_actions += bytes.len();
            }
            handler.handle_ascii_span(bytes);
        }
    });

    if state.attached_exited {
        state.shell_commands.cancel();
        state.terminal.process_exited();
    }

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

    state.output_pending |= grid_output;
    state.status_pending |= state.terminal.program_status().generation() != status_before;
    state.bytes_since_collect += data.len();
    state.generation = state.generation.wrapping_add(1);
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

#[cfg(target_os = "linux")]
fn foreground_working_directory(pty_fd: RawFd) -> Option<String> {
    // SAFETY: `pty_fd` is an open PTY master file descriptor owned by the child
    // wrapper. `tcgetpgrp` only queries kernel metadata for that terminal.
    let pgrp = unsafe { nix::libc::tcgetpgrp(pty_fd) };
    if pgrp <= 0 {
        return None;
    }
    // The group leader is the pid that names the group.
    child_working_directory(pgrp as u32)
}

#[cfg(not(target_os = "linux"))]
fn foreground_working_directory(_pty_fd: RawFd) -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
fn child_working_directory(_pid: u32) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn process_name_for_group(pgrp: i32) -> Option<String> {
    // The group leader is usually the program itself: one read.
    if let Some(leader) = read_proc_stat(pgrp) {
        if leader.pgrp == pgrp && !is_shell_process(&leader.name) {
            return Some(leader.name);
        }
    }
    group_members(pgrp)
        .into_iter()
        .find(|process| !is_shell_process(&process.name))
        .map(|process| process.name)
}

#[cfg(target_os = "linux")]
fn pids_in_group(pgrp: i32) -> Vec<i32> {
    group_members(pgrp)
        .into_iter()
        .filter(|process| !is_shell_process(&process.name))
        .map(|process| process.pid)
        .collect()
}

/// Processes in group `pgrp`, lowest pid first.
///
/// A job's processes are its leader with the leader's siblings (a
/// pipeline) and descendants (a subshell), so only the processes under
/// the leader's parent that are in the group are read. Reading every
/// process on the machine is the fallback, for when that walk cannot be
/// made: the leader is gone, its parent is init, or the kernel has no
/// `/proc/<pid>/task/<tid>/children`.
#[cfg(target_os = "linux")]
fn group_members(pgrp: i32) -> Vec<ProcStat> {
    let mut members = group_members_from_tree(pgrp).unwrap_or_else(|| {
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
            .filter_map(read_proc_stat)
            .filter(|process| process.pgrp == pgrp)
            .collect()
    });
    members.sort_by_key(|process| process.pid);
    members
}

#[cfg(target_os = "linux")]
fn group_members_from_tree(pgrp: i32) -> Option<Vec<ProcStat>> {
    let leader = read_proc_stat(pgrp).filter(|leader| leader.pgrp == pgrp)?;
    if leader.ppid <= 1 {
        return None;
    }
    let mut members = Vec::new();
    let mut parents = vec![leader.ppid];
    while let Some(parent) = parents.pop() {
        for child in child_pids(parent)? {
            let Some(process) = read_proc_stat(child) else {
                continue;
            };
            if process.pgrp == pgrp {
                parents.push(process.pid);
                members.push(process);
            }
        }
    }
    Some(members)
}

/// Children of every thread of `pid`, or `None` when the kernel does not
/// list them.
#[cfg(target_os = "linux")]
fn child_pids(pid: i32) -> Option<Vec<i32>> {
    let mut children: Vec<i32> = Vec::new();
    for task in fs::read_dir(format!("/proc/{pid}/task")).ok()? {
        let Ok(task) = task else {
            continue;
        };
        match fs::read_to_string(task.path().join("children")) {
            Ok(list) => children.extend(
                list.split_whitespace()
                    .filter_map(|c| c.parse::<i32>().ok()),
            ),
            // The process exited while being read; it has no children left.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    Some(children)
}

#[cfg(target_os = "linux")]
fn read_proc_stat(pid: i32) -> Option<ProcStat> {
    parse_proc_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

#[cfg(not(target_os = "linux"))]
fn pids_in_group(_pgrp: i32) -> Vec<i32> {
    Vec::new()
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct ProcStat {
    pid: i32,
    ppid: i32,
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
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;

    Some(ProcStat {
        pid,
        ppid,
        pgrp,
        name,
    })
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
mod status_and_submission_tests {
    use super::*;
    use ultty_vt::program_status::{ProgramState, StatusChangeReason};

    fn quiet() -> EventLoop {
        EventLoop::with_command("/bin/sleep", &["/bin/sleep", "10"], 80, 24).unwrap()
    }

    fn output(event_loop: &EventLoop, bytes: &[u8]) {
        parse_into(&mut event_loop.shared.lock(), &mut AnsiParser::new(), bytes);
    }

    fn prompt(event_loop: &EventLoop) {
        output(event_loop, b"\x1b]133;A\x07$ \x1b]133;B\x07");
    }

    #[test]
    fn test_program_status_generation_survives_event_overflow() {
        let mut event_loop = quiet();
        let mut bytes = Vec::new();
        for _ in 0..MAX_PENDING_EVENTS + 20 {
            bytes.extend_from_slice(b"\x1b]2;title\x07");
        }
        bytes.extend_from_slice(b"\x1b]7501;state=working\x07\x1b]133;A\x07");
        output(&event_loop, &bytes);
        let events = event_loop.process().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TerminalEvent::ProgramStatusChanged {
                generation: 2,
                reason: StatusChangeReason::Prompt
            }
        )));
        assert!(event_loop.terminal().program_status().records().is_empty());
        assert!(!event_loop
            .process()
            .unwrap()
            .iter()
            .any(|event| matches!(event, TerminalEvent::ProgramStatusChanged { .. })));
    }

    #[test]
    fn test_program_status_only_wakes_without_grid_redraw() {
        let mut event_loop = quiet();
        output(&event_loop, b"\x1b]7501;state=working\x07");
        assert!(event_loop.shared.lock().ready());
        let events = event_loop.process().unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event, TerminalEvent::ProgramStatusChanged { .. })));
        assert!(!events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Redraw)));
        assert!(!event_loop.shared.lock().ready());
    }

    #[test]
    fn test_program_status_final_output_precedes_exit_cleanup_on_real_pty() {
        let script = r#"printf '\033]7501;state=working:id=work\007\033]7501;state=done:id=done\007\033]7501;state=error:id=error\007'"#;
        let mut event_loop =
            EventLoop::with_command("/bin/sh", &["/bin/sh", "-c", script], 80, 24).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut exited = false;
        while Instant::now() < deadline {
            let events = event_loop.wait(Duration::from_millis(20)).unwrap();
            if events
                .iter()
                .any(|event| matches!(event, TerminalEvent::ChildExited { .. }))
            {
                exited = true;
                break;
            }
        }
        assert!(exited);
        let snapshot = event_loop.terminal().program_status_snapshot();
        assert_eq!(snapshot.records.len(), 2);
        assert!(snapshot
            .records
            .iter()
            .all(|record| matches!(record.state, ProgramState::Done | ProgramState::Error)));
        assert_eq!(snapshot.reason, StatusChangeReason::ProcessExit);
    }

    #[test]
    fn test_shell_submission_stale_finish_and_concurrent_refusal() {
        let mut event_loop = quiet();
        output(&event_loop, b"\x1b]133;D;7\x07");
        prompt(&event_loop);
        let sequence = event_loop.submit_shell_command("true").unwrap();
        assert!(event_loop.submit_shell_command("false").is_err());
        assert_eq!(event_loop.shell_command_result(sequence), None);
        let events = event_loop.process().unwrap();
        assert!(!events
            .iter()
            .any(|event| matches!(event, TerminalEvent::ShellCommandFinished { .. })));
        output(&event_loop, b"\x1b]133;D;9\x07");
        assert_eq!(event_loop.shell_command_result(sequence), None);
        let mut flood = b"\x1b]2;title\x07".repeat(MAX_PENDING_EVENTS + 1);
        flood.extend_from_slice(b"\x1b]133;C\x07\x1b]133;D;0\x07");
        output(&event_loop, &flood);
        assert_eq!(
            event_loop.shell_command_result(sequence),
            Some(ShellCommandOutcome::Finished { exit_code: Some(0) })
        );
        let events = event_loop.process().unwrap();
        assert!(events.iter().any(|event| matches!(event, TerminalEvent::ShellCommandFinished { sequence: id, exit_code: Some(0) } if *id == sequence)));
    }

    #[test]
    fn test_shell_submission_cannot_reserve_between_read_and_parse() {
        let mut event_loop = quiet();
        prompt(&event_loop);
        let shared = Arc::clone(&event_loop.shared);
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            read_and_parse(&shared, &mut AnsiParser::new(), &mut [0; 128], |buf| {
                let stale = b"\x1b]133;C\x07\x1b]133;D;0\x07";
                buf[..stale.len()].copy_from_slice(stale);
                read_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(stale.len())
            })
            .unwrap();
        });
        read_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // submit_shell_command must acquire this exact lock to reserve or
        // enqueue. The old bytes are already read but not yet applied.
        assert!(matches!(
            event_loop.shared.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        release_tx.send(()).unwrap();
        reader.join().unwrap();
        assert!(event_loop.submit_shell_command("true").is_err());
        assert!(event_loop.shared.lock().shell_commands.pending.is_none());
        assert_eq!(event_loop.shared.lock().shell_commands.next_sequence, 0);
    }

    #[test]
    fn test_shell_submission_overflow_and_input_refusal_leave_no_reservation() {
        let mut event_loop = quiet();
        prompt(&event_loop);
        assert!(event_loop
            .submit_shell_command(&"x".repeat(MAX_OUTBOUND))
            .is_err());
        assert!(event_loop.shared.lock().shell_commands.pending.is_none());
        assert_eq!(event_loop.shared.lock().shell_commands.next_sequence, 0);
        for command in ["", "   ", "a\nb", "a\rb", "a\0b", "a\tb", "a\x1bb"] {
            assert!(event_loop.submit_shell_command(command).is_err());
        }
        event_loop.send_input(b"editable").unwrap();
        assert!(event_loop.submit_shell_command("true").is_err());
        assert!(event_loop.shared.lock().shell_commands.pending.is_none());
    }

    #[test]
    fn test_shell_submission_extra_input_cancels_before_start() {
        let mut event_loop = quiet();
        prompt(&event_loop);
        let sequence = event_loop.submit_shell_command("true").unwrap();
        event_loop.send_input(b"x").unwrap();
        output(&event_loop, b"\x1b]133;C\x07\x1b]133;D;0\x07");
        assert_eq!(
            event_loop.shell_command_result(sequence),
            Some(ShellCommandOutcome::Cancelled)
        );
        assert!(!event_loop
            .process()
            .unwrap()
            .iter()
            .any(|event| matches!(event, TerminalEvent::ShellCommandFinished { .. })));
    }

    #[test]
    fn test_shell_submission_prompt_reset_cancel_and_sequence_survives_ris() {
        let mut event_loop = quiet();
        for cancel in [b"\x1b]133;A\x07".as_slice(), b"\x1bc".as_slice()] {
            // The fake child is not a shell; clear its test input queue.
            event_loop.shared.lock().outbound.clear();
            prompt(&event_loop);
            let sequence = event_loop.submit_shell_command("true").unwrap();
            output(&event_loop, cancel);
            assert_eq!(
                event_loop.shell_command_result(sequence),
                Some(ShellCommandOutcome::Cancelled)
            );
            assert_eq!(
                event_loop.shared.lock().shell_commands.next_sequence,
                sequence
            );
        }
    }

    #[test]
    fn test_shell_submission_instant_completion_on_real_pty() {
        let script = r#"printf '\033]133;A\007$ \033]133;B\007'; while IFS= read -r line; do printf '\033]133;C\007'; eval "$line"; result=$?; printf '\033]133;D;%s\007\033]133;A\007$ \033]133;B\007' "$result"; done"#;
        let mut event_loop =
            EventLoop::with_command("/bin/sh", &["/bin/sh", "-c", script], 80, 24).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let sequence = loop {
            event_loop.wait(Duration::from_millis(10)).unwrap();
            if let Ok(sequence) = event_loop.submit_shell_command("true") {
                break sequence;
            }
            assert!(Instant::now() < deadline, "prompt never ready");
        };
        while event_loop.shell_command_result(sequence).is_none() {
            event_loop.wait(Duration::from_millis(10)).unwrap();
            assert!(Instant::now() < deadline, "completion missing");
        }
        assert_eq!(
            event_loop.shell_command_result(sequence),
            Some(ShellCommandOutcome::Finished { exit_code: Some(0) })
        );
    }

    #[test]
    fn test_shell_submission_exit_without_finish_cancels_on_real_pty() {
        let script = r#"printf '\033]133;A\007$ \033]133;B\007'; IFS= read -r line; printf '\033]133;C\007'; eval "$line""#;
        let mut event_loop =
            EventLoop::with_command("/bin/sh", &["/bin/sh", "-c", script], 80, 24).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let sequence = loop {
            event_loop.wait(Duration::from_millis(10)).unwrap();
            if let Ok(sequence) = event_loop.submit_shell_command("exit 7") {
                break sequence;
            }
            assert!(Instant::now() < deadline, "prompt never ready");
        };
        while event_loop.shell_command_result(sequence).is_none() {
            event_loop.wait(Duration::from_millis(10)).unwrap();
            assert!(Instant::now() < deadline, "exit cancellation missing");
        }
        assert_eq!(
            event_loop.shell_command_result(sequence),
            Some(ShellCommandOutcome::Cancelled)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_revision_tracks_mutation_without_consuming_events() {
        let event_loop =
            EventLoop::with_command("/bin/sleep", &["/bin/sleep", "5"], 20, 3).expect("create");
        let before = event_loop.snapshot_generation();
        {
            let terminal = event_loop.terminal();
            assert_eq!(terminal.snapshot_generation(), before);
            assert_eq!(terminal.dimensions().cols, 20);
        }
        assert_eq!(event_loop.snapshot_generation(), before);
        {
            let mut terminal = event_loop.terminal();
            terminal.set_title("changed locally".to_owned());
            assert_ne!(terminal.snapshot_generation(), before);
        }
        assert_ne!(event_loop.snapshot_generation(), before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_proc_stat_handles_process_names_with_spaces() {
        let stat = "1234 (name with space) S 1 5678 5678 0 -1 4194304";

        assert_eq!(
            parse_proc_stat(stat),
            Some(ProcStat {
                pid: 1234,
                ppid: 1,
                pgrp: 5678,
                name: "name with space".to_string(),
            })
        );
    }

    /// A child of this test in its own process group, killed on drop.
    #[cfg(target_os = "linux")]
    struct GroupChild(std::process::Child);

    #[cfg(target_os = "linux")]
    impl GroupChild {
        fn spawn(program: &str, args: &[&str], group: i32) -> Self {
            use std::os::unix::process::CommandExt;
            let child = std::process::Command::new(program)
                .args(args)
                .process_group(group)
                .spawn()
                .expect("spawn group child");
            GroupChild(child)
        }

        fn pid(&self) -> i32 {
            self.0.id() as i32
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for GroupChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_name_is_the_group_leader_when_it_is_not_a_shell() {
        let leader = GroupChild::spawn("sleep", &["30"], 0);
        assert_eq!(
            process_name_for_group(leader.pid()).as_deref(),
            Some("sleep")
        );
        assert_eq!(pids_in_group(leader.pid()), vec![leader.pid()]);
    }

    /// `bash script | less`: the shell leads the group and the program the
    /// user sees is its sibling.
    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_name_finds_a_sibling_when_a_shell_leads_the_group() {
        // `; :` keeps sh from exec'ing its command, so a shell leads.
        let leader = GroupChild::spawn("sh", &["-c", "read never; :"], 0);
        let sibling = GroupChild::spawn("sleep", &["31"], leader.pid());

        let walked = group_members_from_tree(leader.pid()).expect("tree walk");
        let walked: Vec<i32> = walked.iter().map(|process| process.pid).collect();
        assert!(walked.contains(&sibling.pid()), "{walked:?}");

        assert_eq!(
            process_name_for_group(leader.pid()).as_deref(),
            Some("sleep")
        );
        assert_eq!(pids_in_group(leader.pid()), vec![sibling.pid()]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_shell_alone_in_its_group_names_no_program() {
        let shell = GroupChild::spawn("sh", &["-c", "read never; :"], 0);
        assert_eq!(process_name_for_group(shell.pid()), None);
        assert!(pids_in_group(shell.pid()).is_empty());
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
    fn child_exit_is_reported_once() {
        let mut event_loop =
            EventLoop::with_command("/bin/true", &["/bin/true"], 80, 24).expect("create");
        event_loop
            .run(|_, event| !matches!(event, TerminalEvent::ChildExited { .. }))
            .expect("run");
        for _ in 0..5 {
            let events = event_loop.wait(Duration::from_millis(20)).expect("wait");
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, TerminalEvent::ChildExited { .. })),
                "ChildExited reported again: {events:?}"
            );
        }
    }

    #[test]
    fn wait_wakes_on_output_without_running_out_its_timeout() {
        let mut event_loop = EventLoop::with_command(
            "/bin/sh",
            &["/bin/sh", "-c", "sleep 0.2; printf hi; sleep 5"],
            20,
            3,
        )
        .expect("create");
        let start = Instant::now();
        event_loop.wait(Duration::from_secs(10)).expect("wait");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "wait slept through the output: {:?}",
            start.elapsed()
        );
        assert_eq!(
            event_loop.terminal().grid().row(0).unwrap().text_trimmed(),
            "hi"
        );
        assert_eq!(event_loop.shared.waiters.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn write_local_draws_without_replying_to_the_program() {
        let mut event_loop =
            EventLoop::with_command("/bin/sleep", &["/bin/sleep", "5"], 20, 3).expect("create");
        // DA1 would make a real program's output send a reply.
        event_loop.write_local(b"NOTE\x1b[c");
        {
            let mut terminal = event_loop.terminal();
            assert_eq!(terminal.grid().row(0).unwrap().text_trimmed(), "NOTE");
            assert!(terminal.take_responses().is_empty(), "a reply is queued");
        }
        let events = event_loop.process().expect("process");
        assert!(events.iter().any(|e| matches!(e, TerminalEvent::Redraw)));
    }

    /// XTGETTCAP's `TN` must name the `TERM` the program actually got.
    #[test]
    fn terminal_reports_the_term_its_child_was_started_with() {
        let env = [("TERM".into(), "xterm-ultty".into())];
        let child = Child::spawn(
            "/bin/sh",
            &["/bin/sh", "-c", "sleep 1"],
            &crate::pty::current_dir(),
            PtySize::new(80, 24),
            &env,
        )
        .expect("spawn");
        let event_loop = EventLoop::from_child(child, 80, 24).expect("create event loop");
        assert_eq!(event_loop.terminal().terminal_name(), "xterm-ultty");
    }

    #[test]
    fn exit_waits_for_unread_output_past_the_grace_but_not_past_the_cap() {
        let ms = Duration::from_millis;
        // A closed PTY means everything was read.
        assert!(exit_report_due(true, ms(0), || 4096));
        // Within the grace: wait for the I/O thread to close the PTY.
        assert!(!exit_report_due(false, ms(100), || 0));
        // Past the grace with nothing unread: something else holds the PTY.
        assert!(exit_report_due(false, ms(250), || 0));
        // Past the grace with output still unread: a starved I/O thread has
        // not caught up yet, so hold the exit back.
        assert!(!exit_report_due(false, ms(250), || 512));
        // But never past the cap.
        assert!(exit_report_due(false, EXIT_DRAIN_CAP, || 512));
    }

    /// A child that prints and exits at once must not have its last output
    /// lost: `ChildExited` used to be reported as soon as `waitpid` saw the
    /// exit, while the final chunk was still unread in the PTY.
    #[test]
    fn child_exit_is_reported_after_its_final_output() {
        for attempt in 0..20 {
            let mut event_loop = EventLoop::with_command(
                "/bin/sh",
                &["/bin/sh", "-c", "seq 1 3000; printf FINAL_OUTPUT_MARK"],
                80,
                24,
            )
            .expect("create event loop");
            event_loop
                .run(|_, event| !matches!(event, TerminalEvent::ChildExited { .. }))
                .expect("run");
            let terminal = event_loop.terminal();
            let shown = terminal
                .grid()
                .rows()
                .any(|row| row.text().contains("FINAL_OUTPUT_MARK"));
            assert!(shown, "attempt {attempt}: final output missing at exit");
        }
    }

    /// A background process can keep the PTY open after the child exits;
    /// the exit must still be reported, once the drain grace runs out.
    #[test]
    fn child_exit_is_reported_while_a_background_process_holds_the_pty() {
        let mut event_loop = EventLoop::with_command(
            "/bin/sh",
            &["/bin/sh", "-c", "sleep 5 & echo STARTED"],
            80,
            24,
        )
        .expect("create event loop");
        let start = Instant::now();
        let status = event_loop
            .run(|_, event| !matches!(event, TerminalEvent::ChildExited { .. }))
            .expect("run");
        assert_eq!(status, 0);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "exit took {:?}; it waited for the background sleep",
            start.elapsed()
        );
    }

    /// The UI sleeps until a terminal wakes it. A child that exits while a
    /// background process holds its PTY produces no output and no hangup,
    /// so the exit itself must wake the UI, or the pane never closes. The
    /// sleep ignores SIGHUP, which the kernel sends the foreground group
    /// when sh exits, so it really does hold the PTY open.
    #[test]
    fn exit_behind_a_held_pty_wakes_the_ui() {
        let mut event_loop = EventLoop::with_command(
            "/bin/sh",
            &["/bin/sh", "-c", "trap '' HUP; sleep 5 & echo STARTED"],
            80,
            24,
        )
        .expect("create event loop");
        let (woke_tx, woke_rx) = std::sync::mpsc::channel();
        let woke_tx = Mutex::new(woke_tx);
        event_loop.set_waker(Arc::new(move || {
            let _ = woke_tx.lock().unwrap().send(());
        }));

        let start = Instant::now();
        loop {
            // Like the UI: only look at the terminal when woken.
            woke_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("the exit never woke the UI");
            let events = event_loop.process().expect("process");
            if events
                .iter()
                .any(|e| matches!(e, TerminalEvent::ChildExited { .. }))
            {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(4), "no exit reported");
        }
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

    /// vim and less push the title on entry and pop it on exit; the
    /// restored title must reach the UI like any other title change.
    #[test]
    fn popping_the_title_stack_reports_the_restored_title() {
        let mut event_loop = EventLoop::with_command(
            "/bin/sh",
            &[
                "/bin/sh",
                "-c",
                r"printf '\033]2;shell\007\033[22;0t\033]2;vim\007\033[23;0t'",
            ],
            80,
            24,
        )
        .expect("create event loop");
        let mut titles = Vec::new();
        event_loop
            .run(|_, event| match event {
                TerminalEvent::TitleChanged(title) => {
                    titles.push(title);
                    true
                }
                TerminalEvent::ChildExited { .. } => false,
                _ => true,
            })
            .expect("run");
        assert_eq!(titles, ["shell", "vim", "shell"]);
        assert_eq!(event_loop.terminal().title(), "shell");
    }

    #[test]
    fn resize_terminal() {
        let mut event_loop = EventLoop::new(80, 24).expect("Failed to create event loop");

        assert!(event_loop.resize(120, 40).is_ok());
        assert_eq!(event_loop.terminal().dimensions().cols, 120);
        assert_eq!(event_loop.terminal().dimensions().rows, 40);
    }

    /// `ws_xpixel` and `ws_ypixel` as `TIOCGWINSZ` reports them on the master.
    fn reported_pixels(event_loop: &EventLoop) -> (u16, u16) {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: the fd is the live PTY master and `size` is a valid winsize.
        let status = unsafe { libc::ioctl(event_loop.child.pty_fd(), libc::TIOCGWINSZ, &mut size) };
        assert_eq!(status, 0, "TIOCGWINSZ failed");
        (size.ws_xpixel, size.ws_ypixel)
    }

    #[test]
    fn resize_reports_the_cell_pixel_size_through_tiocgwinsz() {
        let mut event_loop = EventLoop::with_command("/bin/cat", &["/bin/cat"], 80, 24)
            .expect("Failed to spawn cat");
        event_loop.terminal().set_cell_pixel_size(9, 18);

        event_loop.resize(100, 30).expect("resize");

        assert_eq!(reported_pixels(&event_loop), (900, 540));
    }

    #[test]
    fn test_resize_publishes_pixel_metrics_when_grid_is_unchanged() {
        let mut event_loop =
            EventLoop::with_command("/bin/sleep", &["/bin/sleep", "5"], 80, 24).unwrap();
        for (width, height) in [(8, 16), (11, 23)] {
            event_loop.terminal().set_cell_pixel_size(width, height);
            event_loop.resize(80, 24).unwrap();
            let mut size = libc::winsize {
                ws_row: 0,
                ws_col: 0,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // SAFETY: the child owns this live PTY fd and size is writable winsize storage.
            assert_eq!(
                unsafe { libc::ioctl(event_loop.child.pty_fd(), libc::TIOCGWINSZ, &mut size) },
                0
            );
            assert_eq!((size.ws_col, size.ws_row), (80, 24));
            assert_eq!((size.ws_xpixel, size.ws_ypixel), (80 * width, 24 * height));
        }
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
        let text = row_text(&event_loop.terminal(), 0);
        assert!(text.contains("hello"), "Expected 'hello', got: {}", text);
    }

    /// Poll `check` against the terminal until it passes or 5s elapse,
    /// without ever calling `process` (the I/O thread must do the work).
    fn eventually(event_loop: &EventLoop, check: impl Fn(&Terminal) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if check(&event_loop.terminal()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn row_text(terminal: &Terminal, row: u16) -> String {
        terminal.grid().row(row).unwrap().text_trimmed()
    }

    #[test]
    fn output_is_parsed_by_the_io_thread_without_process_calls() {
        let event_loop = EventLoop::with_command(
            "/bin/sh",
            &[
                "/bin/sh",
                "-c",
                "printf 'abcdefghijklmnopqrstuvwxyz0123456789'; sleep 5",
            ],
            80,
            24,
        )
        .expect("spawn printf");
        assert!(eventually(&event_loop, |t| row_text(t, 0)
            == "abcdefghijklmnopqrstuvwxyz0123456789"));
    }

    #[test]
    fn terminal_queries_are_answered_by_the_io_thread() {
        // The child asks for the cursor position and prints the reply's
        // row;col. Nothing on this side calls `process`.
        let event_loop = EventLoop::with_command(
            "/bin/bash",
            &[
                "/bin/bash",
                "-c",
                "stty -echo; printf '\\033[6n'; IFS= read -r -s -d R reply; \
                 printf '\\rgot:%s' \"${reply#*[}\"; sleep 5",
            ],
            80,
            24,
        )
        .expect("spawn bash");
        assert!(
            eventually(&event_loop, |t| row_text(t, 0).starts_with("got:1;1")),
            "row 0 was {:?}",
            row_text(&event_loop.terminal(), 0)
        );
    }

    #[test]
    fn wakes_coalesce_until_events_are_collected() {
        use std::sync::atomic::AtomicUsize;

        let mut event_loop = EventLoop::with_command(
            "/bin/sh",
            &["/bin/sh", "-c", "yes line | head -c 2000000; sleep 5"],
            80,
            24,
        )
        .expect("spawn yes");
        let wakes = Arc::new(AtomicUsize::new(0));
        event_loop.set_waker({
            let wakes = Arc::clone(&wakes);
            Arc::new(move || {
                wakes.fetch_add(1, Ordering::SeqCst);
            })
        });

        assert!(eventually(&event_loop, |_| wakes.load(Ordering::SeqCst) > 0));
        // Megabytes of output keep arriving, but until the UI collects
        // events the I/O thread must not wake it again.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        let events = event_loop.process().expect("process");
        assert!(matches!(events.first(), Some(TerminalEvent::Redraw)));
        assert!(event_loop.last_drain_bytes() > 0);
    }

    #[test]
    fn dropping_the_event_loop_stops_its_io_thread_promptly() {
        let event_loop =
            EventLoop::with_command("/bin/cat", &["/bin/cat"], 80, 24).expect("spawn cat");
        let started = Instant::now();
        drop(event_loop);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// A shell child that runs `script` with `$1` set to a temp file path.
    fn child_writing_to(script: &str) -> (EventLoop, tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stdin.bin");
        let path_str = path.to_str().unwrap().to_string();
        let event_loop = EventLoop::with_command(
            "/bin/sh",
            &["/bin/sh", "-c", script, "sh", &path_str],
            80,
            24,
        )
        .expect("spawn sh");
        (event_loop, dir, path)
    }

    fn file_eventually(path: &std::path::Path, check: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let data = std::fs::read(path).unwrap_or_default();
            if check(&data) || Instant::now() >= deadline {
                return data;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn take_in_band_eventually(event_loop: &EventLoop) -> Option<InBandInput> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(input) = event_loop.take_in_band() {
                return Some(input);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn query_replies_wait_for_the_in_band_answer() {
        // The child sends an in-band string ("hi") and then DA1, the probe
        // sequence, and records everything it is sent.
        let (event_loop, _dir, path) =
            child_writing_to(r#"stty raw -echo; printf '\033_UaGk\033\\\033[c'; exec cat > "$1""#);
        let input = take_in_band_eventually(&event_loop).expect("in-band input");
        assert_eq!(input.bytes, b"hi");
        assert!(!input.broken);

        // Until the UI answers, the DA1 reply must not reach the child.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!std::fs::read(&path).unwrap_or_default().contains(&b'['));

        event_loop.finish_in_band(b"ok").expect("finish");
        let answer = ultty_protocol::in_band::encode_to_vec(b"ok");
        let got = file_eventually(&path, |d| d.len() > answer.len() && d.ends_with(b"c"));
        assert!(got.starts_with(&answer), "answer first: {got:?}");
        assert!(
            got[answer.len()..].starts_with(b"\x1b[?"),
            "then DA1: {got:?}"
        );
    }

    #[test]
    fn invalid_in_band_text_is_reported_broken() {
        let (event_loop, _dir, _path) = child_writing_to(r#"printf '\033_U!!\033\\'; sleep 5"#);
        let input = take_in_band_eventually(&event_loop).expect("in-band input");
        assert!(input.broken);
        assert!(input.bytes.is_empty());
        event_loop.finish_in_band(&[]).expect("finish");
    }

    #[test]
    fn input_and_in_band_writes_reach_the_child_in_order() {
        let (mut event_loop, _dir, path) = child_writing_to(r#"stty raw -echo; exec cat > "$1""#);
        // Let the child switch its terminal to raw mode before writing.
        std::thread::sleep(Duration::from_millis(200));
        let frames = vec![0x55u8; 100_000];
        event_loop.send_input(b"1").unwrap();
        event_loop.send_in_band(&frames).unwrap();
        event_loop.send_input(b"2").unwrap();

        let mut expected = b"1".to_vec();
        expected.extend(ultty_protocol::in_band::encode_to_vec(&frames));
        expected.push(b'2');
        let got = file_eventually(&path, |d| d.len() >= expected.len());
        assert_eq!(got.len(), expected.len());
        assert!(got == expected, "bytes arrived out of order or altered");
    }
}
