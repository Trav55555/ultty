//! The controlling terminal, as the PTY drivers see it.
//!
//! The drivers run *inside* the terminal under test and talk only to
//! `/dev/tty`, so they behave the same in Ultty, Ghostty, GNOME Terminal and
//! Alacritty. Raw mode turns off echo, line buffering and output processing,
//! so corpus bytes (already CRLF) reach the terminal unchanged and replies
//! such as a cursor position report can be read byte by byte.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg, Termios};

/// `/dev/tty` in raw mode; the original settings come back on drop.
pub struct RawTty {
    file: File,
    original: Termios,
}

impl RawTty {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let original = tcgetattr(file.as_fd()).map_err(io::Error::from)?;
        let mut raw = original.clone();
        cfmakeraw(&mut raw);
        tcsetattr(file.as_fd(), SetArg::TCSANOW, &raw).map_err(io::Error::from)?;
        Ok(RawTty { file, original })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// Wait up to `timeout` for one byte of input.
    pub fn read_byte(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
        read_byte(&mut self.file, timeout)
    }

    /// A second handle for reading, so one thread can wait for keys while
    /// another writes. The termios settings are per tty, so it is raw too.
    pub fn reader(&self) -> io::Result<TtyReader> {
        Ok(TtyReader(self.file.try_clone()?))
    }

    /// Discard input already queued (typeahead, stale replies).
    pub fn drain_input(&mut self) -> io::Result<()> {
        while self.read_byte(Duration::from_millis(20))?.is_some() {}
        Ok(())
    }

    /// Send `CSI 6n` and wait for the `CSI row ; col R` reply.
    ///
    /// A terminal answers in order, so the reply proves it has consumed
    /// every byte written before the query: the fence that makes
    /// throughput measurable from inside the terminal. Returns the
    /// reported (row, col).
    pub fn cursor_position_fence(&mut self, timeout: Duration) -> io::Result<(u32, u32)> {
        self.write_all(b"\x1b[6n")?;
        self.flush()?;
        let deadline = Instant::now() + timeout;
        let mut reply = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no cursor position report within {timeout:?}"),
                ));
            }
            let Some(b) = self.read_byte(left)? else {
                continue;
            };
            reply.push(b);
            if b == b'R' {
                if let Some(pos) = parse_cpr(&reply) {
                    return Ok(pos);
                }
                reply.clear();
            }
        }
    }
}

impl Drop for RawTty {
    fn drop(&mut self) {
        // Best effort: nothing useful can be done if the tty is gone.
        let _ = tcsetattr(self.file.as_fd(), SetArg::TCSANOW, &self.original);
    }
}

/// Read side of the tty; see [`RawTty::reader`].
pub struct TtyReader(File);

impl TtyReader {
    pub fn read_byte(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
        read_byte(&mut self.0, timeout)
    }
}

fn read_byte(file: &mut File, timeout: Duration) -> io::Result<Option<u8>> {
    let ms = timeout.as_millis().min(u128::from(u16::MAX)) as u16;
    let mut fds = [PollFd::new(file.as_fd(), PollFlags::POLLIN)];
    let ready = poll(&mut fds, PollTimeout::from(ms)).map_err(io::Error::from)?;
    if ready == 0 {
        return Ok(None);
    }
    let mut byte = [0u8; 1];
    match file.read(&mut byte)? {
        0 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tty closed")),
        _ => Ok(Some(byte[0])),
    }
}

/// Parse the last `ESC [ row ; col R` in `bytes`.
pub fn parse_cpr(bytes: &[u8]) -> Option<(u32, u32)> {
    let end = bytes.iter().rposition(|&b| b == b'R')?;
    let start = bytes[..end].windows(2).rposition(|w| w == b"\x1b[")? + 2;
    let body = std::str::from_utf8(&bytes[start..end]).ok()?;
    let (row, col) = body.split_once(';')?;
    Some((row.parse().ok()?, col.parse().ok()?))
}

/// `stty size` of the controlling terminal: (rows, cols).
pub fn stty_size() -> io::Result<(u16, u16)> {
    let tty = File::open("/dev/tty")?;
    let out = Command::new("stty")
        .arg("size")
        .stdin(Stdio::from(tty))
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split_whitespace().map(str::parse::<u16>);
    match (parts.next(), parts.next()) {
        (Some(Ok(rows)), Some(Ok(cols))) => Ok((rows, cols)),
        _ => Err(io::Error::other(format!(
            "unexpected stty size output {text:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cpr;

    #[test]
    fn parses_cursor_position_reports() {
        assert_eq!(parse_cpr(b"\x1b[12;40R"), Some((12, 40)));
        assert_eq!(parse_cpr(b"junk\x1b[1;1R"), Some((1, 1)));
        assert_eq!(parse_cpr(b"\x1b[;R"), None);
        assert_eq!(parse_cpr(b"12;40R"), None);
    }
}
