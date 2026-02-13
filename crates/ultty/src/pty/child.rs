//! Child process management for PTY.

use super::{current_dir, default_shell, Pty, PtyError, PtySize, Result, SlavePty};
use nix::libc;
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{close, dup2, execvp, fork, setsid, ForkResult, Pid};
use std::ffi::CString;
use std::path::Path;

/// A child process running in a PTY.
pub struct Child {
    /// The PTY master (our end)
    pty: Pty,
    /// Child process ID
    pid: Pid,
    /// Whether the child has exited
    exited: bool,
    /// Exit status (if exited)
    exit_status: Option<i32>,
}

impl Child {
    /// Spawn a new shell in a PTY.
    pub fn spawn_shell(size: PtySize) -> Result<Self> {
        let shell = default_shell();
        let working_dir = current_dir();
        Self::spawn(&shell, &[&shell], &working_dir, size)
    }

    /// Spawn a shell in a specific working directory.
    pub fn spawn_shell_in_dir(size: PtySize, working_dir: &str) -> Result<Self> {
        let shell = default_shell();
        let wd = std::path::PathBuf::from(working_dir);
        let wd = if wd.exists() { wd } else { current_dir() };
        Self::spawn(&shell, &[&shell], &wd, size)
    }

    /// Spawn a command in a PTY.
    pub fn spawn(program: &str, args: &[&str], working_dir: &Path, size: PtySize) -> Result<Self> {
        let (pty, slave) = Pty::open_with_size(size)?;

        // SAFETY: fork() is safe to call here because:
        // - We're single-threaded at this point (no other threads holding locks)
        // - The PTY file descriptors are set up correctly before fork
        // - The child process will exec immediately, avoiding post-fork issues
        match unsafe { fork() }.map_err(PtyError::ForkFailed)? {
            ForkResult::Parent { child } => {
                // Parent process - we're the terminal emulator
                // Drop slave fd in parent
                drop(slave);

                Ok(Child {
                    pty,
                    pid: child,
                    exited: false,
                    exit_status: None,
                })
            }
            ForkResult::Child => {
                // Child process - will become the shell
                // This function does not return on success
                setup_child(slave, program, args, working_dir);
            }
        }
    }

    /// Get the PTY for reading/writing.
    pub fn pty(&self) -> &Pty {
        &self.pty
    }

    /// Get mutable access to the PTY.
    pub fn pty_mut(&mut self) -> &mut Pty {
        &mut self.pty
    }

    /// Get the child process ID.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Check if the child has exited.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Get the exit status (if exited).
    pub fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Resize the PTY and notify the child.
    pub fn resize(&self, size: PtySize) -> Result<()> {
        self.pty.resize(size)?;
        // Send SIGWINCH to notify child of size change
        signal::kill(self.pid, Signal::SIGWINCH).ok();
        Ok(())
    }

    /// Read data from the child's output.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        self.pty.read(buf)
    }

    /// Write data to the child's input.
    pub fn write(&self, data: &[u8]) -> Result<usize> {
        self.pty.write(data)
    }

    /// Write all data to the child's input.
    pub fn write_all(&self, data: &[u8]) -> Result<()> {
        self.pty.write_all(data)
    }

    /// Check if the child has exited (non-blocking).
    ///
    /// Returns `true` if the child has exited, `false` if still running.
    pub fn try_wait(&mut self) -> Result<bool> {
        if self.exited {
            return Ok(true);
        }

        match waitpid(self.pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, status)) => {
                self.exited = true;
                self.exit_status = Some(status);
                Ok(true)
            }
            Ok(WaitStatus::Signaled(_, signal, _)) => {
                self.exited = true;
                self.exit_status = Some(128 + signal as i32);
                Ok(true)
            }
            Ok(WaitStatus::StillAlive) => Ok(false),
            Ok(_) => Ok(false), // Other statuses (stopped, continued) - still running
            Err(nix::Error::ECHILD) => {
                // No child process - it must have exited
                self.exited = true;
                Ok(true)
            }
            Err(e) => {
                tracing::warn!("waitpid failed: {}", e);
                Ok(false)
            }
        }
    }

    /// Wait for the child to exit (blocking).
    pub fn wait(&mut self) -> Result<i32> {
        if let Some(status) = self.exit_status {
            return Ok(status);
        }

        loop {
            match waitpid(self.pid, None) {
                Ok(WaitStatus::Exited(_, status)) => {
                    self.exited = true;
                    self.exit_status = Some(status);
                    return Ok(status);
                }
                Ok(WaitStatus::Signaled(_, signal, _)) => {
                    self.exited = true;
                    let status = 128 + signal as i32;
                    self.exit_status = Some(status);
                    return Ok(status);
                }
                Ok(_) => continue, // Keep waiting
                Err(nix::Error::ECHILD) => {
                    self.exited = true;
                    return Ok(0);
                }
                Err(e) => {
                    return Err(PtyError::ForkFailed(e));
                }
            }
        }
    }

    /// Send a signal to the child process.
    pub fn signal(&self, signal: Signal) -> Result<()> {
        signal::kill(self.pid, signal).map_err(PtyError::ForkFailed)
    }

    /// Kill the child process (SIGKILL).
    pub fn kill(&self) -> Result<()> {
        self.signal(Signal::SIGKILL)
    }

    /// Terminate the child process (SIGTERM).
    pub fn terminate(&self) -> Result<()> {
        self.signal(Signal::SIGTERM)
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.exited {
            // Try to terminate gracefully
            self.terminate().ok();

            // Give it a moment
            std::thread::sleep(std::time::Duration::from_millis(10));

            // Check if exited
            if self.try_wait().unwrap_or(false) {
                return;
            }

            // Force kill
            self.kill().ok();

            // Reap zombie in a background thread to avoid blocking the UI
            let pid = self.pid;
            std::thread::spawn(move || {
                let _ = waitpid(pid, None);
            });
        }
    }
}

/// Set up the child process (runs in child after fork).
///
/// This function does not return on success - it calls exec.
fn setup_child(slave: SlavePty, program: &str, args: &[&str], working_dir: &Path) -> ! {
    // Create a new session and set controlling terminal
    if setsid().is_err() {
        std::process::exit(1);
    }

    let slave_fd = slave.into_raw_fd();

    // Set the slave as the controlling terminal
    // SAFETY: slave_fd is a valid PTY slave file descriptor. We're in a new session
    // (after setsid), so TIOCSCTTY will set this terminal as our controlling terminal.
    // The third argument (0) means don't steal the terminal from another session.
    unsafe {
        if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) < 0 {
            std::process::exit(1);
        }
    }

    // Redirect stdin, stdout, stderr to the slave
    if dup2(slave_fd, libc::STDIN_FILENO).is_err() {
        std::process::exit(1);
    }
    if dup2(slave_fd, libc::STDOUT_FILENO).is_err() {
        std::process::exit(1);
    }
    if dup2(slave_fd, libc::STDERR_FILENO).is_err() {
        std::process::exit(1);
    }

    // Close the original slave fd if it's not one of the standard fds
    if slave_fd > libc::STDERR_FILENO {
        close(slave_fd).ok();
    }

    // Change to working directory
    std::env::set_current_dir(working_dir).ok();

    // Set up environment
    std::env::set_var("TERM", "xterm-256color");
    std::env::set_var("COLORTERM", "truecolor");

    // Convert args to CStrings
    let program_cstr = CString::new(program).unwrap_or_else(|_| CString::new("/bin/sh").unwrap());
    let args_cstr: Vec<CString> = args
        .iter()
        .map(|s| CString::new(*s).unwrap_or_else(|_| CString::new("").unwrap()))
        .collect();

    // Execute the program
    match execvp(&program_cstr, &args_cstr) {
        Ok(_) => unreachable!(), // execvp never returns on success
        Err(e) => {
            eprintln!("Failed to exec {}: {}", program, e);
            std::process::exit(127);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_echo() {
        let mut child = Child::spawn(
            "/bin/echo",
            &["/bin/echo", "hello"],
            &current_dir(),
            PtySize::default(),
        )
        .expect("Failed to spawn echo");

        // Wait a bit for output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Read output
        let mut buf = [0u8; 1024];
        let n = child.read(&mut buf).unwrap_or(0);

        if n > 0 {
            let output = String::from_utf8_lossy(&buf[..n]);
            assert!(
                output.contains("hello"),
                "Expected 'hello', got: {}",
                output
            );
        }

        // Wait for child to exit
        let status = child.wait().unwrap();
        assert_eq!(status, 0);
    }

    #[test]
    fn spawn_and_write() {
        let child = Child::spawn(
            "/bin/cat",
            &["/bin/cat"],
            &current_dir(),
            PtySize::default(),
        )
        .expect("Failed to spawn cat");

        // Write some data
        child.write_all(b"test input\n").unwrap();

        // Wait a bit for echo
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Read output
        let mut buf = [0u8; 1024];
        let n = child.read(&mut buf).unwrap_or(0);

        if n > 0 {
            let output = String::from_utf8_lossy(&buf[..n]);
            assert!(
                output.contains("test input"),
                "Expected 'test input', got: {}",
                output
            );
        }

        // Terminate
        child.terminate().unwrap();
    }

    #[test]
    fn resize_child() {
        let child = Child::spawn(
            "/bin/sh",
            &["/bin/sh"],
            &current_dir(),
            PtySize::new(80, 24),
        )
        .expect("Failed to spawn shell");

        // Resize should succeed
        assert!(child.resize(PtySize::new(120, 40)).is_ok());
    }
}
