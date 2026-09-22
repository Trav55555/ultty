//! PTY master side (terminal emulator side).

use super::{PtyError, Result};
use nix::fcntl::{fcntl, FcntlArg, FdFlag, OFlag};
use nix::libc;
use nix::pty::{openpty, OpenptyResult, Winsize};
use nix::unistd::{read, write};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

/// How long a single `poll()` waits for the PTY to become writable.
const WRITE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Total time [`Pty::write_all`] waits for a stalled reader before giving up.
pub const WRITE_ALL_TIMEOUT: Duration = Duration::from_secs(2);

/// PTY size in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    /// Pixel width (optional, can be 0)
    pub pixel_width: u16,
    /// Pixel height (optional, can be 0)
    pub pixel_height: u16,
}

impl PtySize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    pub fn with_pixels(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Self {
        Self {
            cols,
            rows,
            pixel_width,
            pixel_height,
        }
    }

    fn to_winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: self.pixel_width,
            ws_ypixel: self.pixel_height,
        }
    }
}

impl Default for PtySize {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

/// The master side of a PTY pair.
///
/// This represents the terminal emulator's end of the PTY.
/// Data written here goes to the shell's stdin, and data from
/// the shell's stdout/stderr can be read from here.
pub struct Pty {
    master: OwnedFd,
    slave_name: String,
}

impl Pty {
    /// Open a new PTY pair.
    pub fn open() -> Result<(Self, SlavePty)> {
        Self::open_with_size(PtySize::default())
    }

    /// Open a new PTY pair with the specified size.
    pub fn open_with_size(size: PtySize) -> Result<(Self, SlavePty)> {
        let winsize = size.to_winsize();

        // Open the PTY pair
        let OpenptyResult { master, slave } =
            openpty(Some(&winsize), None).map_err(PtyError::OpenFailed)?;

        // Get slave name for child process
        let slave_name = get_slave_name(master.as_raw_fd())?;

        // Neither end may leak into spawned children: the child gets its
        // slave via dup2 (which clears CLOEXEC on the new fds) and nothing
        // else should ever inherit the master.
        set_cloexec(master.as_raw_fd())?;
        set_cloexec(slave.as_raw_fd())?;

        // Set master to non-blocking
        set_nonblocking(master.as_raw_fd())?;

        let pty = Pty { master, slave_name };
        let slave_pty = SlavePty { fd: slave };

        Ok((pty, slave_pty))
    }

    /// Get the file descriptor for polling.
    pub fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }

    /// Get the slave PTY name (e.g., "/dev/pts/0").
    pub fn slave_name(&self) -> &str {
        &self.slave_name
    }

    /// Resize the PTY.
    pub fn resize(&self, size: PtySize) -> Result<()> {
        let winsize = size.to_winsize();
        // SAFETY: self.master is a valid, open file descriptor for a PTY master.
        // TIOCSWINSZ is a standard ioctl that sets the window size, and winsize
        // is a valid pointer to a properly initialized Winsize struct.
        unsafe {
            if libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize) < 0 {
                return Err(PtyError::ResizeFailed(nix::Error::last()));
            }
        }
        Ok(())
    }

    /// Read available data from the PTY (non-blocking).
    ///
    /// Returns `Ok(0)` if no data is available, `Err(Closed)` if the PTY is closed.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        match read(self.master.as_raw_fd(), buf) {
            Ok(n) => {
                if n > 0 {
                    Self::debug_log_pty_data(&buf[..n]);
                }
                Ok(n)
            }
            Err(nix::Error::EAGAIN) => Ok(0),
            Err(nix::Error::EIO) => Err(PtyError::Closed),
            Err(e) => Err(PtyError::ReadFailed(io::Error::from_raw_os_error(e as i32))),
        }
    }

    fn debug_log_pty_data(data: &[u8]) {
        use std::sync::OnceLock;
        static LOG_FILE: OnceLock<Option<std::sync::Mutex<std::fs::File>>> = OnceLock::new();

        let file = LOG_FILE.get_or_init(|| {
            std::env::var("ULTTY_PTY_LOG")
                .ok()
                .and_then(|path| std::fs::File::create(&path).ok().map(std::sync::Mutex::new))
        });

        if let Some(mutex) = file {
            if let Ok(mut f) = mutex.lock() {
                use std::io::Write;
                let _ = f.write_all(data);
                let _ = f.flush();
            }
        }
    }

    /// Write data to the PTY.
    pub fn write(&self, data: &[u8]) -> Result<usize> {
        match write(&self.master, data) {
            Ok(n) => Ok(n),
            Err(nix::Error::EAGAIN) => Ok(0),
            Err(nix::Error::EIO) => Err(PtyError::Closed),
            Err(e) => Err(PtyError::WriteFailed(io::Error::from_raw_os_error(
                e as i32,
            ))),
        }
    }

    /// Write all data to the PTY, waiting up to [`WRITE_ALL_TIMEOUT`] for a
    /// stalled reader before failing with a timed-out `WriteFailed` error.
    pub fn write_all(&self, data: &[u8]) -> Result<()> {
        self.write_all_with_timeout(data, WRITE_ALL_TIMEOUT)
    }

    /// Write all data to the PTY, giving up after `timeout` of no progress.
    ///
    /// When the write would block, the fd is polled for writability in
    /// [`WRITE_POLL_INTERVAL`] slices instead of busy-spinning.
    pub fn write_all_with_timeout(&self, mut data: &[u8], timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !data.is_empty() {
            match self.write(data) {
                Ok(0) => {
                    if Instant::now() >= deadline {
                        return Err(PtyError::WriteFailed(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "PTY not writable: child is not draining its input",
                        )));
                    }
                    self.wait_writable(WRITE_POLL_INTERVAL)?;
                }
                Ok(n) => {
                    data = &data[n..];
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Block until the master is writable or `timeout` elapses.
    fn wait_writable(&self, timeout: Duration) -> Result<()> {
        let mut pollfd = libc::pollfd {
            fd: self.master.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let timeout_ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: `pollfd` is a valid, initialized array of exactly one entry
        // and `self.master` is an open fd owned by this Pty for the call's
        // duration. poll only writes `revents`.
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(PtyError::WriteFailed(err));
        }
        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(PtyError::Closed);
        }
        Ok(())
    }
}

impl Read for Pty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Pty::read(self, buf).map_err(io::Error::other)
    }
}

impl Write for Pty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Pty::write(self, buf).map_err(io::Error::other)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AsRawFd for Pty {
    fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }
}

/// The slave side of a PTY pair (for the child process).
pub struct SlavePty {
    fd: OwnedFd,
}

impl AsFd for SlavePty {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl SlavePty {
    /// Get the file descriptor.
    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Consume and return the raw fd for use in the child process.
    pub fn into_raw_fd(self) -> RawFd {
        let fd = self.fd.as_raw_fd();
        std::mem::forget(self.fd);
        fd
    }
}

/// Get the name of the slave PTY device.
#[cfg(target_os = "linux")]
fn get_slave_name(master_fd: RawFd) -> Result<String> {
    // Use ptsname_r for thread safety
    let mut name_buf = [0u8; 256];
    // SAFETY: master_fd is a valid PTY master file descriptor obtained from openpty.
    // name_buf is a valid, properly sized buffer (256 bytes, sufficient for /dev/pts/N).
    // ptsname_r is thread-safe unlike ptsname, and writes a null-terminated string.
    unsafe {
        if libc::ptsname_r(
            master_fd,
            name_buf.as_mut_ptr() as *mut libc::c_char,
            name_buf.len(),
        ) != 0
        {
            return Err(PtyError::OpenFailed(nix::Error::last()));
        }
    }

    // Find null terminator
    let len = name_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(name_buf.len());
    let name = String::from_utf8_lossy(&name_buf[..len]).to_string();
    Ok(name)
}

/// Get the name of the slave PTY device.
///
/// macOS libc has no `ptsname_r`; `ptsname` returns a pointer to static
/// storage, so the string is copied out immediately under the call.
#[cfg(not(target_os = "linux"))]
fn get_slave_name(master_fd: RawFd) -> Result<String> {
    // SAFETY: master_fd is a valid PTY master file descriptor obtained from
    // openpty. ptsname returns null on failure or a pointer to a
    // NUL-terminated string in static storage that stays valid until the
    // next ptsname call; we copy it before returning.
    let name = unsafe {
        let ptr = libc::ptsname(master_fd);
        if ptr.is_null() {
            return Err(PtyError::OpenFailed(nix::Error::last()));
        }
        std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    Ok(name)
}

/// Set a file descriptor to non-blocking mode.
fn set_nonblocking(fd: RawFd) -> Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(PtyError::OpenFailed)?;
    let flags = OFlag::from_bits_truncate(flags);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(PtyError::OpenFailed)?;
    Ok(())
}

/// Mark a file descriptor close-on-exec.
fn set_cloexec(fd: RawFd) -> Result<()> {
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(PtyError::OpenFailed)?;
    Ok(())
}

/// Whether a file descriptor is close-on-exec.
#[cfg(test)]
fn is_cloexec(fd: RawFd) -> bool {
    fcntl(fd, FcntlArg::F_GETFD)
        .map(|flags| FdFlag::from_bits_truncate(flags).contains(FdFlag::FD_CLOEXEC))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_pty() {
        let result = Pty::open();
        assert!(result.is_ok());

        let (pty, _slave) = result.unwrap();
        assert!(!pty.slave_name().is_empty());
        #[cfg(target_os = "linux")]
        assert!(pty.slave_name().starts_with("/dev/pts/"));
        #[cfg(target_os = "macos")]
        assert!(pty.slave_name().starts_with("/dev/ttys"));
    }

    #[test]
    fn pty_fds_are_close_on_exec() {
        let (pty, slave) = Pty::open().unwrap();
        assert!(is_cloexec(pty.as_raw_fd()), "master must be CLOEXEC");
        assert!(is_cloexec(slave.as_raw_fd()), "slave must be CLOEXEC");
    }

    #[test]
    fn write_all_times_out_instead_of_spinning_when_nobody_reads() {
        // Nothing reads the slave, so the input queue fills and the master
        // reports EAGAIN forever. write_all must give up within the budget.
        // Raw mode is needed: canonical mode silently discards overlong lines
        // instead of applying back-pressure.
        use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};
        let (pty, slave) = Pty::open().unwrap();
        let mut termios = tcgetattr(&slave).unwrap();
        cfmakeraw(&mut termios);
        tcsetattr(&slave, SetArg::TCSANOW, &termios).unwrap();
        let data = vec![b'x'; 1 << 20];
        let budget = Duration::from_millis(200);

        let start = Instant::now();
        let result = pty.write_all_with_timeout(&data, budget);
        let elapsed = start.elapsed();

        match result {
            Err(PtyError::WriteFailed(e)) => assert_eq!(e.kind(), io::ErrorKind::TimedOut),
            other => panic!("expected timed-out write, got {:?}", other.map(|_| ())),
        }
        assert!(elapsed >= budget, "gave up too early: {elapsed:?}");
        assert!(
            elapsed < budget + Duration::from_secs(1),
            "took far longer than the budget: {elapsed:?}"
        );
    }

    #[test]
    fn pty_size() {
        let size = PtySize::new(120, 40);
        assert_eq!(size.cols, 120);
        assert_eq!(size.rows, 40);

        let (pty, _slave) = Pty::open_with_size(size).unwrap();
        assert!(pty.resize(PtySize::new(80, 24)).is_ok());
    }
}
