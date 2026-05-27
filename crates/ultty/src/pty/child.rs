//! Child process management for PTY.

use super::{current_dir, default_shell, Pty, PtyError, PtySize, Result, SlavePty};
use nix::libc;
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, setsid, ForkResult, Pid};
use std::ffi::CString;
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
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
        let exec_plan = ChildExec::new(program, args, working_dir)?;

        // SAFETY: the child path only performs session/fd setup and execve with
        // argv/env/cwd prepared before fork. It must not allocate or take locks.
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
                setup_child(slave, &exec_plan);
            }
        }
    }

    /// Get the PTY for reading/writing.
    pub fn pty(&self) -> &Pty {
        &self.pty
    }

    /// Get the PTY master file descriptor.
    pub fn pty_fd(&self) -> std::os::fd::RawFd {
        self.pty.as_raw_fd()
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

struct ChildExec {
    program: CString,
    _args: Vec<CString>,
    argv: Vec<*const c_char>,
    _env: Vec<CString>,
    envp: Vec<*const c_char>,
    working_dir: CString,
}

impl ChildExec {
    fn new(program: &str, args: &[&str], working_dir: &Path) -> Result<Self> {
        let program = cstring_from_bytes(resolve_program_path(program).as_bytes())?;
        let working_dir = cstring_from_bytes(working_dir.as_os_str().as_bytes())?;

        let mut arg_strings = if args.is_empty() {
            vec![cstring_from_bytes(program.as_bytes())?]
        } else {
            args.iter()
                .map(|arg| cstring_from_bytes(arg.as_bytes()))
                .collect::<Result<Vec<_>>>()?
        };
        if arg_strings.is_empty() {
            arg_strings.push(cstring_from_bytes(program.as_bytes())?);
        }

        let mut env = Vec::new();
        for (key, value) in std::env::vars_os() {
            if key == "TERM" || key == "COLORTERM" {
                continue;
            }
            let mut entry = Vec::new();
            entry.extend_from_slice(key.as_os_str().as_bytes());
            entry.push(b'=');
            entry.extend_from_slice(value.as_os_str().as_bytes());
            env.push(cstring_from_bytes(&entry)?);
        }
        env.push(cstring_from_bytes(b"TERM=xterm-256color")?);
        env.push(cstring_from_bytes(b"COLORTERM=truecolor")?);

        let mut argv = arg_strings
            .iter()
            .map(|arg| arg.as_ptr())
            .collect::<Vec<_>>();
        argv.push(std::ptr::null());

        let mut envp = env.iter().map(|entry| entry.as_ptr()).collect::<Vec<_>>();
        envp.push(std::ptr::null());

        Ok(Self {
            program,
            _args: arg_strings,
            argv,
            _env: env,
            envp,
            working_dir,
        })
    }
}

fn cstring_from_bytes(bytes: &[u8]) -> Result<CString> {
    CString::new(bytes).map_err(|_| PtyError::ExecFailed(nix::Error::EINVAL))
}

fn resolve_program_path(program: &str) -> std::borrow::Cow<'_, str> {
    if program.contains('/') {
        return std::borrow::Cow::Borrowed(program);
    }

    let Some(paths) = std::env::var_os("PATH") else {
        return std::borrow::Cow::Borrowed(program);
    };

    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return std::borrow::Cow::Owned(candidate.to_string_lossy().into_owned());
        }
    }

    std::borrow::Cow::Borrowed(program)
}

/// Set up the child process (runs in child after fork).
///
/// This function does not return on success - it calls exec.
fn setup_child(slave: SlavePty, exec_plan: &ChildExec) -> ! {
    // Create a new session and set controlling terminal
    if setsid().is_err() {
        exit_child(1);
    }

    let slave_fd = slave.into_raw_fd();

    // Set the slave as the controlling terminal
    // SAFETY: slave_fd is a valid PTY slave file descriptor. We're in a new session
    // (after setsid), so TIOCSCTTY will set this terminal as our controlling terminal.
    // The third argument (0) means don't steal the terminal from another session.
    unsafe {
        if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) < 0 {
            exit_child(1);
        }
    }

    // Redirect stdin, stdout, stderr to the slave
    unsafe {
        if libc::dup2(slave_fd, libc::STDIN_FILENO) < 0 {
            exit_child(1);
        }
        if libc::dup2(slave_fd, libc::STDOUT_FILENO) < 0 {
            exit_child(1);
        }
        if libc::dup2(slave_fd, libc::STDERR_FILENO) < 0 {
            exit_child(1);
        }
    }

    // Close the original slave fd if it's not one of the standard fds
    if slave_fd > libc::STDERR_FILENO {
        unsafe {
            libc::close(slave_fd);
        }
    }

    // Change to working directory
    unsafe {
        libc::chdir(exec_plan.working_dir.as_ptr());
    }

    // Execute the program
    unsafe {
        libc::execve(
            exec_plan.program.as_ptr(),
            exec_plan.argv.as_ptr(),
            exec_plan.envp.as_ptr(),
        );
    }
    exit_child(127);
}

fn exit_child(code: i32) -> ! {
    unsafe {
        libc::_exit(code);
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
