//! In-band transport on the app's own terminal (ADR 0018).
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

use super::transport::ReadOutcome;

const STDIN: RawFd = 0;
const STDOUT: RawFd = 1;

/// The heartbeat interval. The terminal ends an in-band session after 10 s
/// without traffic, so an idle app says something well inside that.
pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// What the panic hook needs: the terminal's fds and settings to restore.
#[derive(Clone, Copy)]
struct Restore {
    fd_in: RawFd,
    fd_out: RawFd,
    saved: libc::termios,
}

/// Terminal state to restore if the process panics while in-band.
static PANIC_RESTORE: Mutex<Option<Restore>> = Mutex::new(None);

/// The app's terminal, in raw mode, carrying in-band strings.
pub(super) struct InBandTty {
    fd_in: RawFd,
    fd_out: RawFd,
    saved: libc::termios,
    splitter: Splitter,
    last_sent: Instant,
}

impl InBandTty {
    /// Put stdin's terminal in raw mode for in-band use.
    pub(super) fn open() -> io::Result<Self> {
        Self::open_on(STDIN, STDOUT)
    }

    /// Use the terminal on `fd_in` (read, and switched to raw mode) and
    /// `fd_out` (written). Both must stay open while this value lives.
    pub(super) fn open_on(fd_in: RawFd, fd_out: RawFd) -> io::Result<Self> {
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
        install_panic_hook();
        *PANIC_RESTORE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Restore {
            fd_in,
            fd_out,
            saved,
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
    pub(super) fn write_frames(&mut self, frames: &[u8]) -> io::Result<()> {
        self.write_raw(&in_band::encode_to_vec(frames))?;
        self.last_sent = Instant::now();
        Ok(())
    }

    /// Write bytes to the terminal unchanged.
    pub(super) fn write_raw(&mut self, mut bytes: &[u8]) -> io::Result<()> {
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
    pub(super) fn heartbeat_due(&self) -> bool {
        self.last_sent.elapsed() >= HEARTBEAT_INTERVAL
    }

    /// Wait up to `timeout` for input, then read everything available.
    /// Decoded frame bytes go to `on_frames`; everything else on stdin
    /// (replies such as DA1, stray keystrokes) goes to `on_other`, in order.
    pub(super) fn read(
        &mut self,
        timeout: Duration,
        mut on_frames: impl FnMut(&[u8]),
        mut on_other: impl FnMut(&[u8]),
    ) -> io::Result<ReadOutcome> {
        let mut wait = timeout;
        let mut buf = [0u8; 64 * 1024];
        let mut decoded = Vec::new();
        loop {
            if !poll_readable(self.fd_in, wait)? {
                return Ok(ReadOutcome::Open);
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
                return Ok(ReadOutcome::Closed);
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
        *PANIC_RESTORE.lock().unwrap_or_else(PoisonError::into_inner) = None;
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

/// On panic, tell the terminal the session is over and restore the tty.
///
/// Release builds use `panic = "abort"`, so `Drop` never runs on a panic.
/// Without this the terminal would keep showing the dead app's scene until
/// it noticed the silence.
fn install_panic_hook() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let restore = PANIC_RESTORE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(r) = restore {
                let wire = in_band::encode_to_vec(&AppMessage::Disconnect.encode_to_bytes());
                // SAFETY: writes from a valid buffer, then restores settings
                // that came from tcgetattr. Both only touch the recorded fds.
                unsafe {
                    libc::write(r.fd_out, wire.as_ptr().cast(), wire.len());
                    libc::tcsetattr(r.fd_in, libc::TCSANOW, &r.saved);
                }
            }
            previous(info);
        }));
    });
}

/// Finds a DA1 reply (`ESC [ ? <digits and ;> c`) in stdin bytes that may
/// arrive split across reads.
#[derive(Debug, Default)]
pub(super) struct Da1Scanner {
    tail: Vec<u8>,
}

impl Da1Scanner {
    /// Feed bytes; true once a complete DA1 reply has been seen.
    pub(super) fn feed(&mut self, bytes: &[u8]) -> bool {
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
    use super::Da1Scanner;

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
