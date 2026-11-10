//! In-band transport on the app's own terminal (ADR 0018, spec §1.3).
//!
//! Frames go to stdout and come back on stdin as `ESC _ U <base64url> ESC \`
//! strings, so native mode works wherever the terminal's byte stream goes:
//! over ssh, under sudo, in containers, and in `script` recordings. The tty
//! is put in raw mode for the whole session. Without that, the line
//! discipline would hold the terminal's strings until a newline that never
//! comes, and echo them back as output.

use std::io;
use std::os::fd::RawFd;
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use ultty_protocol::in_band::{self, Piece, Splitter};
use ultty_protocol::{AppMessage, Encode};

/// Whether the terminal side is still there after [`InBandTty::read`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    /// Still open (input may or may not have arrived).
    Open,
    /// End of file on the terminal: no more input will arrive.
    Closed,
}

const STDIN: RawFd = 0;
const STDOUT: RawFd = 1;

/// The heartbeat interval. The terminal ends an in-band session after 10 s
/// without traffic, so an idle app says something well inside that.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// What the panic hook needs: the terminal's fds and settings to restore.
#[derive(Clone, Copy)]
struct Restore {
    fd_in: RawFd,
    fd_out: RawFd,
    saved: libc::termios,
    /// The thread that opened the session, which runs the app's event loop.
    owner: std::thread::ThreadId,
}

/// Terminal state to restore if the process panics or exits while in-band.
static EXIT_RESTORE: Mutex<Option<Restore>> = Mutex::new(None);

/// The app's terminal, in raw mode, carrying in-band strings.
pub struct InBandTty {
    fd_in: RawFd,
    fd_out: RawFd,
    saved: libc::termios,
    splitter: Splitter,
    last_sent: Instant,
}

impl InBandTty {
    /// Put stdin's terminal in raw mode for in-band use.
    pub fn open() -> io::Result<Self> {
        Self::open_on(STDIN, STDOUT)
    }

    /// Use the terminal on `fd_in` (read, and switched to raw mode) and
    /// `fd_out` (written). Both must stay open while this value lives.
    pub fn open_on(fd_in: RawFd, fd_out: RawFd) -> io::Result<Self> {
        // SAFETY: `termios` is plain data; tcgetattr fills it or fails.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: the caller keeps `fd_in` open; `saved` is a valid
        // out-pointer.
        if unsafe { libc::tcgetattr(fd_in, &mut saved) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = saved;
        // SAFETY: `raw` is a valid termios copied from the current settings.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: `fd_in` is open and `raw` is a fully initialized termios.
        if unsafe { libc::tcsetattr(fd_in, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        install_exit_hooks();
        *EXIT_RESTORE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Restore {
            fd_in,
            fd_out,
            saved,
            owner: std::thread::current().id(),
        });
        Ok(Self {
            fd_in,
            fd_out,
            saved,
            splitter: Splitter::default(),
            last_sent: Instant::now(),
        })
    }

    /// Write protocol frames as in-band strings.
    pub fn write_frames(&mut self, frames: &[u8]) -> io::Result<()> {
        self.write_raw(&in_band::encode_to_vec(frames))?;
        self.last_sent = Instant::now();
        Ok(())
    }

    /// Write bytes to the terminal unchanged.
    pub fn write_raw(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            // SAFETY: `bytes` is valid for its length; `fd_out` is open.
            let n = unsafe { libc::write(self.fd_out, bytes.as_ptr().cast(), bytes.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            bytes = &bytes[n as usize..];
        }
        Ok(())
    }

    /// Whether the app has been quiet long enough to owe a heartbeat.
    pub fn heartbeat_due(&self) -> bool {
        self.last_sent.elapsed() >= HEARTBEAT_INTERVAL
    }

    /// When the app will owe a heartbeat if it sends nothing before then.
    pub fn heartbeat_at(&self) -> Instant {
        self.last_sent + HEARTBEAT_INTERVAL
    }

    /// The descriptor input arrives on; readable when [`InBandTty::read`]
    /// has something to read.
    pub fn input_fd(&self) -> RawFd {
        self.fd_in
    }

    /// Wait up to `timeout` for input, then read everything available.
    /// Decoded frame bytes go to `on_frames`; everything else on stdin
    /// (replies such as DA1, stray keystrokes) goes to `on_other`, in order.
    pub fn read(
        &mut self,
        timeout: Duration,
        mut on_frames: impl FnMut(&[u8]),
        mut on_other: impl FnMut(&[u8]),
    ) -> io::Result<ReadStatus> {
        let mut wait = timeout;
        let mut buf = [0u8; 64 * 1024];
        let mut decoded = Vec::new();
        loop {
            if !poll_readable(self.fd_in, wait)? {
                return Ok(ReadStatus::Open);
            }
            // SAFETY: `buf` is valid for `buf.len()` bytes; `fd_in` is open.
            let n = unsafe { libc::read(self.fd_in, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            if n == 0 {
                return Ok(ReadStatus::Closed);
            }
            self.splitter.feed(&buf[..n as usize], |piece| match piece {
                Piece::Payload(text) => {
                    decoded.clear();
                    match in_band::decode_payload(text, &mut decoded) {
                        Ok(()) => on_frames(&decoded),
                        Err(err) => tracing::warn!("dropping in-band string: {err}"),
                    }
                }
                Piece::Other(bytes) => on_other(bytes),
            });
            // Drain whatever else is already queued without waiting.
            wait = Duration::ZERO;
        }
    }
}

impl Drop for InBandTty {
    fn drop(&mut self) {
        *EXIT_RESTORE.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // SAFETY: `fd_in` is still open and `saved` came from tcgetattr.
        unsafe { libc::tcsetattr(self.fd_in, libc::TCSANOW, &self.saved) };
    }
}

fn poll_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    loop {
        // SAFETY: one valid pollfd entry.
        let rc = unsafe { libc::poll(&mut pollfd, 1, ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(rc > 0);
    }
}

/// End the session and restore the tty if the process leaves without
/// shutting the backend down: a panic, or `std::process::exit`.
///
/// Release builds use `panic = "abort"`, and `process::exit` skips `Drop`,
/// so neither reaches [`InBandTty`]'s destructor. Without these hooks the
/// terminal would keep showing the dead app's scene until it noticed the
/// silence.
fn install_exit_hooks() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // With unwinding, a panic on another thread (a background task,
            // whose panic its runtime catches) leaves the app running; only
            // the event loop's own thread ends the session. With abort, any
            // panic ends the process.
            if cfg!(panic = "abort") || owns_session() {
                disconnect_and_restore();
            }
            previous(info);
        }));
        // SAFETY: registers a plain `extern "C"` function with no state.
        unsafe { libc::atexit(disconnect_at_exit) };
    });
}

fn owns_session() -> bool {
    EXIT_RESTORE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some_and(|r| r.owner == std::thread::current().id())
}

extern "C" fn disconnect_at_exit() {
    disconnect_and_restore();
}

/// Send `Disconnect` and restore the tty, once, if a session is still open.
fn disconnect_and_restore() {
    let restore = EXIT_RESTORE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(r) = restore {
        let wire = in_band::encode_to_vec(&AppMessage::Disconnect.encode_to_bytes());
        // SAFETY: writes from a valid buffer, then restores settings that
        // came from tcgetattr. Both only touch the recorded fds.
        unsafe {
            libc::write(r.fd_out, wire.as_ptr().cast(), wire.len());
            libc::tcsetattr(r.fd_in, libc::TCSANOW, &r.saved);
        }
    }
}

/// How long [`probe`] waits when the terminal answers neither the probe nor
/// DA1. Terminals answer DA1 at once, so this only matters for ones that do
/// not; set `ULTTY_IN_BAND=0` to skip the probe entirely.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Whether `buf` starts with a complete protocol frame.
fn holds_complete_frame(buf: &[u8]) -> bool {
    buf.len() >= 8 && buf.len() >= 8 + u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize
}

/// Send the in-band probe on `tty` and report whether the terminal speaks
/// the protocol.
///
/// First it asks the terminal's name (XTVERSION, then DA1) and stops unless
/// the answer is Ultty's. The APC string carrying the hello is not harmless
/// everywhere: tmux takes it as the pane's title, and a tmux pane reached
/// over ssh cannot be recognised from the environment. The reply comes from
/// whatever draws the screen, which is what the APC string would reach.
///
/// Then it sends `ClientHello` and DA1. A protocol terminal's first frame
/// (normally `ServerHello`) arrives before its DA1 reply; other terminals
/// answer only DA1. Frame bytes received are appended to `read_buf`, ready
/// for the rest of the handshake.
///
/// Errors when the terminal does not answer DA1 within `timeout`, or closes.
pub fn probe(
    tty: &mut InBandTty,
    read_buf: &mut Vec<u8>,
    timeout: Duration,
) -> Result<bool, crate::Error> {
    let deadline = Instant::now() + timeout;
    if !named_ultty(tty, deadline)? {
        return Ok(false);
    }

    let hello = AppMessage::ClientHello {
        version: ultty_protocol::PROTOCOL_VERSION,
        requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
    };
    tty.write_frames(&hello.encode_to_bytes())?;
    tty.write_raw(b"\x1b[c")?;

    let mut da1 = Da1Scanner::default();
    // Both callbacks see both flags; whichever arrives first decides.
    let answered = std::cell::Cell::new(false);
    let da1_first = std::cell::Cell::new(false);
    while !answered.get() && !da1_first.get() {
        let now = Instant::now();
        if now >= deadline {
            return Err(crate::Error::Timeout("the in-band probe or DA1 reply"));
        }
        let status = tty.read(
            (deadline - now).min(Duration::from_millis(50)),
            |frames| {
                read_buf.extend_from_slice(frames);
                if !da1_first.get() && holds_complete_frame(read_buf) {
                    answered.set(true);
                }
            },
            |other| {
                if !answered.get() && da1.feed(other) {
                    da1_first.set(true);
                }
            },
        )?;
        if status == ReadStatus::Closed {
            return Err(crate::Error::ConnectionClosed);
        }
    }
    Ok(answered.get())
}

/// What Ultty's XTVERSION reply starts with: `DCS > | ultty <version> ST`.
const ULTTY_VERSION_REPLY: &[u8] = b"\x1bP>|ultty ";

/// Ask the terminal's name (XTVERSION) and report whether it is Ultty, once
/// the DA1 sent after it is answered. Terminals that ignore XTVERSION answer
/// only DA1.
fn named_ultty(tty: &mut InBandTty, deadline: Instant) -> Result<bool, crate::Error> {
    tty.write_raw(b"\x1b[>q\x1b[c")?;
    // The name reply comes first; keystrokes typed meanwhile can follow it,
    // so a bounded prefix is enough.
    const KEEP: usize = 4096;
    let mut replies = Vec::new();
    let mut da1 = Da1Scanner::default();
    let mut answered = false;
    while !answered {
        let now = Instant::now();
        if now >= deadline {
            return Err(crate::Error::Timeout("the DA1 reply"));
        }
        let status = tty.read(
            (deadline - now).min(Duration::from_millis(50)),
            // No frames are expected before the hello; drop any.
            |_| {},
            |other| {
                let room = KEEP.saturating_sub(replies.len());
                replies.extend_from_slice(&other[..other.len().min(room)]);
                if !answered && da1.feed(other) {
                    answered = true;
                }
            },
        )?;
        if status == ReadStatus::Closed {
            return Err(crate::Error::ConnectionClosed);
        }
    }
    Ok(find(&replies, ULTTY_VERSION_REPLY).is_some())
}

/// Finds a DA1 reply (`ESC [ ? <digits and ;> c`) in stdin bytes that may
/// arrive split across reads.
#[derive(Debug, Default)]
pub struct Da1Scanner {
    tail: Vec<u8>,
}

impl Da1Scanner {
    /// Feed bytes; true once a complete DA1 reply has been seen.
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        self.tail.extend_from_slice(bytes);
        let mut i = 0;
        while let Some(p) = find(&self.tail[i..], b"\x1b[?") {
            let start = i + p + 3;
            let body_end = self.tail[start..]
                .iter()
                .position(|b| !(b.is_ascii_digit() || *b == b';'))
                .map(|q| start + q);
            match body_end {
                Some(end) if self.tail[end] == b'c' => return true,
                Some(_) => i = start,
                None => break, // incomplete; keep it for the next feed
            }
        }
        // Keep only what could still be the start of a reply.
        let keep_from = self
            .tail
            .iter()
            .rposition(|&b| b == 0x1b)
            .unwrap_or(self.tail.len());
        self.tail.drain(..keep_from);
        if self.tail.len() > 256 {
            self.tail.clear();
        }
        false
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::{probe, Da1Scanner, InBandTty};
    use std::io::Read;
    use std::io::Write;
    use std::time::Duration;

    use ultty_protocol::{Encode, TerminalMessage};

    /// Run the probe against a fake terminal on a PTY. It answers the name
    /// query with `name_reply` and, if the probe goes on to send its hello,
    /// answers that with `hello_reply`. Returns the result and every byte
    /// the probe wrote.
    fn probe_against(
        name_reply: &[u8],
        hello_reply: Vec<u8>,
    ) -> (Result<bool, crate::Error>, Vec<u8>) {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let (mut master, slave) = {
            let (mut m, mut s) = (0, 0);
            // SAFETY: openpty writes two fds; the name/termios/winsize
            // arguments may be null. `null_mut` fits both platforms: libc
            // declares termios/winsize `*const` on Linux, `*mut` on macOS.
            let rc = unsafe {
                libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(rc, 0, "openpty");
            // SAFETY: both fds were just opened and are owned here.
            unsafe {
                (
                    std::fs::File::from(OwnedFd::from_raw_fd(m)),
                    OwnedFd::from_raw_fd(s),
                )
            }
        };
        let name_reply = name_reply.to_vec();
        let goes_on = name_reply.starts_with(super::ULTTY_VERSION_REPLY);
        let terminal = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            let da1_requests = |seen: &[u8]| seen.windows(3).filter(|w| w == b"\x1b[c").count();
            for (round, reply) in [name_reply, hello_reply].into_iter().enumerate() {
                if round == 1 && !goes_on {
                    break;
                }
                while da1_requests(&seen) <= round {
                    let n = master.read(&mut buf).expect("read probe");
                    seen.extend_from_slice(&buf[..n]);
                }
                master.write_all(&reply).expect("reply");
            }
            // Closing the master would hang up the slave and discard the
            // reply before the probe reads it.
            (seen, master)
        });
        let mut tty = InBandTty::open_on(slave.as_raw_fd(), slave.as_raw_fd()).expect("open");
        let mut read_buf = Vec::new();
        let result = probe(&mut tty, &mut read_buf, Duration::from_secs(2));
        let (seen, _master) = terminal.join().unwrap();
        drop(tty);
        (result, seen)
    }

    const ULTTY_NAME: &[u8] = b"\x1bP>|ultty 0.1.0\x1b\\\x1b[?62;22c";
    const TMUX_NAME: &[u8] = b"\x1bP>|tmux 3.6\x1b\\\x1b[?1;2c";

    fn sent_hello(seen: &[u8]) -> bool {
        seen.windows(3).any(|w| w == b"\x1b_U")
    }

    fn server_hello_wire() -> Vec<u8> {
        let hello = TerminalMessage::ServerHello {
            version: ultty_protocol::PROTOCOL_VERSION,
            capabilities: 0,
        };
        ultty_protocol::in_band::encode_to_vec(&hello.encode_to_bytes())
    }

    #[test]
    fn probe_succeeds_when_the_hello_reply_precedes_da1() {
        let mut reply = server_hello_wire();
        reply.extend_from_slice(b"\x1b[?62;22c");
        let (result, seen) = probe_against(ULTTY_NAME, reply);
        assert!(result.unwrap());
        assert!(seen.starts_with(b"\x1b[>q\x1b[c"), "asks the name first");
        assert!(sent_hello(&seen));
    }

    #[test]
    fn probe_fails_on_ultty_that_only_answers_da1() {
        let (result, _) = probe_against(ULTTY_NAME, b"\x1b[?62;22c".to_vec());
        assert!(!result.unwrap());
    }

    #[test]
    fn probe_fails_when_da1_comes_first() {
        let mut reply = b"\x1b[?1;2c".to_vec();
        reply.extend(server_hello_wire());
        let (result, _) = probe_against(ULTTY_NAME, reply);
        assert!(!result.unwrap());
    }

    #[test]
    fn probe_sends_no_hello_to_another_terminal() {
        // tmux would take the hello's APC string as the pane title.
        let (result, seen) = probe_against(TMUX_NAME, Vec::new());
        assert!(!result.unwrap());
        assert!(!sent_hello(&seen), "sent {seen:?}");
    }

    #[test]
    fn probe_sends_no_hello_to_a_terminal_without_a_name() {
        let (result, seen) = probe_against(b"\x1b[?62;22c", Vec::new());
        assert!(!result.unwrap());
        assert!(!sent_hello(&seen), "sent {seen:?}");
    }

    #[test]
    fn da1_reply_is_found_across_any_split() {
        let wire = b"x\x1b[A\x1b[?62;22c";
        for split in 1..wire.len() {
            let mut scanner = Da1Scanner::default();
            let found = wire.chunks(split).any(|chunk| scanner.feed(chunk));
            assert!(found, "split {split}");
        }
    }

    #[test]
    fn other_sequences_are_not_da1() {
        let mut scanner = Da1Scanner::default();
        assert!(!scanner.feed(b"\x1b[?1;2R\x1b[?25h\x1b[c"));
    }
}
