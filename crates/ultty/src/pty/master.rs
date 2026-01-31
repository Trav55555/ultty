//! PTY master side (terminal emulator side).

use super::{PtyError, Result};
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::libc;
use nix::pty::{openpty, OpenptyResult, Winsize};
use nix::unistd::{read, write};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

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

    fn to_winsize(&self) -> Winsize {
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
            Ok(n) => Ok(n),
            Err(nix::Error::EAGAIN) => Ok(0),
            Err(nix::Error::EIO) => Err(PtyError::Closed),
            Err(e) => Err(PtyError::ReadFailed(io::Error::from_raw_os_error(e as i32))),
        }
    }

    /// Write data to the PTY.
    pub fn write(&self, data: &[u8]) -> Result<usize> {
        match write(&self.master, data) {
            Ok(n) => Ok(n),
            Err(nix::Error::EAGAIN) => Ok(0),
            Err(nix::Error::EIO) => Err(PtyError::Closed),
            Err(e) => Err(PtyError::WriteFailed(io::Error::from_raw_os_error(e as i32))),
        }
    }

    /// Write all data to the PTY (blocking until complete).
    pub fn write_all(&self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            match self.write(data) {
                Ok(0) => {
                    // Would block, try again
                    std::thread::yield_now();
                }
                Ok(n) => {
                    data = &data[n..];
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl Read for Pty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Pty::read(self, buf).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

impl Write for Pty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Pty::write(self, buf).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
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
fn get_slave_name(master_fd: RawFd) -> Result<String> {
    // Use ptsname_r for thread safety
    let mut name_buf = [0u8; 256];
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
    let len = name_buf.iter().position(|&c| c == 0).unwrap_or(name_buf.len());
    let name = String::from_utf8_lossy(&name_buf[..len]).to_string();
    Ok(name)
}

/// Set a file descriptor to non-blocking mode.
fn set_nonblocking(fd: RawFd) -> Result<()> {
    let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(PtyError::OpenFailed)?;
    let flags = OFlag::from_bits_truncate(flags);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(PtyError::OpenFailed)?;
    Ok(())
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
        assert!(pty.slave_name().starts_with("/dev/pts/"));
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
