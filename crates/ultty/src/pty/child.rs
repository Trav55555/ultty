//! Child process management for PTY.

use super::{
    current_dir, default_shell, shell_integration, Pty, PtyError, PtySize, Result, SlavePty,
};
use crate::grid::DEFAULT_TERMINAL_NAME;
use nix::libc;
use nix::sys::signal::{self, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, setsid, ForkResult, Pid};
use std::ffi::{CString, OsString};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
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
    /// The `TERM` the child was started with
    term: String,
}

impl Child {
    /// Spawn a new shell in a PTY.
    ///
    /// `env` is added to the inherited environment (see [`Child::spawn`]).
    /// A bash, zsh or fish shell is started so that it loads Ultty's shell
    /// integration; see [`shell_integration`].
    pub fn spawn_shell(size: PtySize, env: &[(OsString, OsString)]) -> Result<Self> {
        let shell = default_shell();
        let working_dir = current_dir();
        Self::spawn_interactive_shell(&shell, &working_dir, size, env)
    }

    /// Spawn a shell in a specific working directory.
    ///
    /// `env` is added to the inherited environment (see [`Child::spawn`]).
    pub fn spawn_shell_in_dir(
        size: PtySize,
        working_dir: &str,
        env: &[(OsString, OsString)],
    ) -> Result<Self> {
        let shell = default_shell();
        let wd = std::path::PathBuf::from(working_dir);
        let wd = if wd.exists() { wd } else { current_dir() };
        Self::spawn_interactive_shell(&shell, &wd, size, env)
    }

    /// Spawn `shell` with no arguments, plus whatever it needs to load the
    /// shell integration.
    fn spawn_interactive_shell(
        shell: &str,
        working_dir: &Path,
        size: PtySize,
        env: &[(OsString, OsString)],
    ) -> Result<Self> {
        let Some(injection) = shell_integration::for_spawn(shell, &[shell], env) else {
            return Self::spawn(shell, &[shell], working_dir, size, env);
        };
        let mut env: Vec<_> = env
            .iter()
            .filter(|(key, _)| !injection.env.iter().any(|(added, _)| added == key))
            .cloned()
            .collect();
        env.extend(injection.env);
        let args: Vec<&str> = injection.args.iter().map(String::as_str).collect();
        Self::spawn(shell, &args, working_dir, size, &env)
    }

    /// Spawn a command in a PTY.
    ///
    /// The child inherits this process's environment plus `env`, whose
    /// entries override inherited variables of the same name. Passing the
    /// extra variables per spawn keeps them out of `std::env`, which is
    /// unsound to mutate in a multithreaded process.
    ///
    /// `TERM` is never inherited: it is the `TERM` in `env`, or
    /// [`DEFAULT_TERMINAL_NAME`] when `env` has none.
    pub fn spawn(
        program: &str,
        args: &[&str],
        working_dir: &Path,
        size: PtySize,
        env: &[(OsString, OsString)],
    ) -> Result<Self> {
        Self::spawn_inner(program, args, working_dir, size, env, false)
    }

    /// Spawn a command, reporting failed working-directory setup or exec.
    /// Success means the child closed its setup channel without reporting an
    /// error, normally at exec. It does not establish application readiness.
    pub fn spawn_checked(
        program: &str,
        args: &[&str],
        working_dir: &Path,
        size: PtySize,
        env: &[(OsString, OsString)],
    ) -> Result<Self> {
        Self::spawn_inner(program, args, working_dir, size, env, true)
    }

    fn spawn_inner(
        program: &str,
        args: &[&str],
        working_dir: &Path,
        size: PtySize,
        env: &[(OsString, OsString)],
        strict_directory: bool,
    ) -> Result<Self> {
        let (pty, slave) = Pty::open_with_size(size)?;
        // A child offered a native protocol socket is also told which
        // terminal that socket belongs to, so a program running somewhere
        // else (a tmux pane, a `script` recording) that inherited the
        // variables can tell the socket is not for it.
        let mut env = env.to_vec();
        // Tell commands in the shell which Ultty they run in, so `ulttyctl`
        // controls this window rather than the newest one.
        env.push(("ULTTY_PID".into(), std::process::id().to_string().into()));
        if env.iter().any(|(key, _)| key == "ULTTY_SOCKET") {
            env.push(("ULTTY_TTY".into(), pty.slave_name().into()));
        }
        let term = match env.iter().find(|(key, _)| key == "TERM") {
            Some((_, value)) => value.to_string_lossy().into_owned(),
            None => {
                env.push(("TERM".into(), DEFAULT_TERMINAL_NAME.into()));
                DEFAULT_TERMINAL_NAME.to_string()
            }
        };
        let exec_plan = ChildExec::new(program, args, working_dir, &env)?;
        // Allocate the receipt descriptors before fork, above stdio so dup2
        // cannot overwrite them even when the caller has closed stdin/stdout.
        let (read_end, write_end) =
            UnixStream::pair().map_err(|source| PtyError::StartupFailed {
                stage: "receipt setup",
                source,
            })?;
        read_end
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .map_err(PtyError::ReadFailed)?;
        let read_end = duplicate_above_stdio(read_end.into())?;
        let write_end = duplicate_above_stdio(write_end.into())?;

        // SAFETY: the child path only performs session/fd setup and execve with
        // argv/env/cwd prepared before fork. It must not allocate or take locks.
        match unsafe { fork() }.map_err(PtyError::ForkFailed)? {
            ForkResult::Parent { child } => {
                // Parent process - we're the terminal emulator
                // Drop slave fd in parent
                drop(slave);
                drop(write_end);
                let mut child = Child {
                    pty,
                    pid: child,
                    exited: false,
                    exit_status: None,
                    term,
                };
                let mut receipt = UnixStream::from(read_end);
                let mut bytes = [0u8; 5];
                // Normally EOF means exec closed the last writer. A setup failure
                // writes a stage and errno before _exit; no shell text involved.
                let size = loop {
                    match receipt.read(&mut bytes[..1]) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        result => break result.map_err(PtyError::ReadFailed)?,
                    }
                };
                if size == 0 {
                    return Ok(child);
                }
                receipt
                    .read_exact(&mut bytes[1..])
                    .map_err(PtyError::ReadFailed)?;
                let errno = i32::from_ne_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let stage = match bytes[0] {
                    1 => "session setup",
                    2 => "controlling terminal",
                    3 => "stdio setup",
                    4 => "working directory",
                    _ => "exec",
                };
                // Reap the failed child; Drop is still the failure cleanup for
                // broken/malformed receipts and interrupted parent reads.
                let _ = child.wait();
                Err(PtyError::StartupFailed {
                    stage,
                    source: std::io::Error::from_raw_os_error(errno),
                })
            }
            ForkResult::Child => {
                // Child process - will become the shell
                // This function does not return on success
                setup_child(slave, &exec_plan, write_end.as_raw_fd(), strict_directory);
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

    /// The `TERM` the child was started with.
    pub fn term(&self) -> &str {
        &self.term
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

/// Variables a tmux or screen session sets for programs running inside it.
/// Ultty may be started from such a session, but its tabs are not inside
/// it, and programs that see these variables behave as if they were: ulttui
/// skips its in-band probe, for one.
const INHERITED_MULTIPLEXER_VARS: [&str; 4] = ["TMUX", "TMUX_PANE", "STY", "WINDOW"];

struct ChildExec {
    program: CString,
    _args: Vec<CString>,
    argv: Vec<*const c_char>,
    _env: Vec<CString>,
    envp: Vec<*const c_char>,
    working_dir: CString,
    /// One past the highest fd the child may have open, read before fork
    /// for the fallback in [`close_inherited_fds`].
    fd_limit: libc::c_int,
}

impl ChildExec {
    fn new(
        program: &str,
        args: &[&str],
        working_dir: &Path,
        extra_env: &[(OsString, OsString)],
    ) -> Result<Self> {
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
            if key == "TERM"
                || key == "COLORTERM"
                || INHERITED_MULTIPLEXER_VARS.iter().any(|var| key == *var)
                || extra_env.iter().any(|(extra_key, _)| *extra_key == key)
            {
                continue;
            }
            env.push(env_entry(&key, &value)?);
        }
        for (key, value) in extra_env {
            env.push(env_entry(key, value)?);
        }
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
            fd_limit: open_fd_limit(),
        })
    }
}

/// Highest fd number plus one that this process can hold, capped so the
/// fallback close loop stays short when the limit is "unlimited".
fn open_fd_limit() -> libc::c_int {
    const CAP: libc::rlim_t = 1 << 20;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes into the rlimit struct we pass.
    let current = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        limit.rlim_cur
    } else {
        CAP
    };
    libc::c_int::try_from(current.min(CAP)).unwrap_or(libc::c_int::MAX)
}

/// Close every fd above stderr in the child, so descriptors this process
/// opened without CLOEXEC (GPU drivers do, for buffers that may hold screen
/// contents) never reach the shell. Async-signal-safe: plain syscalls only.
fn close_inherited_fds(fd_limit: libc::c_int, receipt: libc::c_int) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: close only the ranges around the owned receipt fd. All
        // remaining child operations use stdio or that CLOEXEC descriptor.
        let low =
            receipt == 3 || unsafe { libc::syscall(libc::SYS_close_range, 3, receipt - 1, 0) } == 0;
        // SAFETY: this range excludes the receipt descriptor and standard IO.
        let high =
            unsafe { libc::syscall(libc::SYS_close_range, receipt + 1, libc::c_uint::MAX, 0) } == 0;
        if low && high {
            return;
        }
    }
    // Kernels before 5.9 and other Unixes: close them one by one.
    for fd in 3..fd_limit {
        if fd == receipt {
            continue;
        }
        // SAFETY: closing an fd that is not open just returns EBADF.
        unsafe {
            libc::close(fd);
        }
    }
}

fn duplicate_above_stdio(fd: OwnedFd) -> Result<OwnedFd> {
    let duplicate = nix::fcntl::fcntl(fd.as_raw_fd(), nix::fcntl::FcntlArg::F_DUPFD_CLOEXEC(3))
        .map_err(PtyError::ExecFailed)?;
    // SAFETY: fcntl just returned a new owned descriptor; fd owns only the original.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn cstring_from_bytes(bytes: &[u8]) -> Result<CString> {
    CString::new(bytes).map_err(|_| PtyError::ExecFailed(nix::Error::EINVAL))
}

fn env_entry(key: &std::ffi::OsStr, value: &std::ffi::OsStr) -> Result<CString> {
    let mut entry = Vec::with_capacity(key.len() + value.len() + 1);
    entry.extend_from_slice(key.as_bytes());
    entry.push(b'=');
    entry.extend_from_slice(value.as_bytes());
    cstring_from_bytes(&entry)
}

fn resolve_program_path(program: &str) -> std::borrow::Cow<'_, str> {
    match std::env::var_os("PATH") {
        Some(paths) => resolve_in_path(program, &paths),
        None => std::borrow::Cow::Borrowed(program),
    }
}

/// The first executable file named `program` in the directories of
/// `paths`, as a shell finds it. A file without execute permission is
/// skipped: `~/.local/bin/env`, which uv installs to be sourced, must not
/// hide `/usr/bin/env`.
fn resolve_in_path<'a>(program: &'a str, paths: &std::ffi::OsStr) -> std::borrow::Cow<'a, str> {
    use std::os::unix::fs::PermissionsExt;

    if program.contains('/') {
        return std::borrow::Cow::Borrowed(program);
    }
    for dir in std::env::split_paths(paths) {
        let candidate = dir.join(program);
        let executable = std::fs::metadata(&candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
        if executable {
            return std::borrow::Cow::Owned(candidate.to_string_lossy().into_owned());
        }
    }
    std::borrow::Cow::Borrowed(program)
}

/// Set up the child process (runs in child after fork).
///
/// This function does not return on success - it calls exec.
fn setup_child(
    slave: SlavePty,
    exec_plan: &ChildExec,
    receipt: libc::c_int,
    strict_directory: bool,
) -> ! {
    // Create a new session and set controlling terminal
    if setsid().is_err() {
        fail_child(receipt, 1);
    }

    let slave_fd = slave.into_raw_fd();

    // Set the slave as the controlling terminal
    // SAFETY: slave_fd is a valid PTY slave file descriptor. We're in a new session
    // (after setsid), so TIOCSCTTY will set this terminal as our controlling terminal.
    // The third argument (0) means don't steal the terminal from another session.
    unsafe {
        if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
            fail_child(receipt, 2);
        }
    }

    // Redirect stdin, stdout, stderr to the slave
    // SAFETY: slave_fd is an open descriptor we own; dup2 onto the standard
    // fds is async-signal-safe and valid between fork and exec.
    unsafe {
        if libc::dup2(slave_fd, libc::STDIN_FILENO) < 0 {
            fail_child(receipt, 3);
        }
        if libc::dup2(slave_fd, libc::STDOUT_FILENO) < 0 {
            fail_child(receipt, 3);
        }
        if libc::dup2(slave_fd, libc::STDERR_FILENO) < 0 {
            fail_child(receipt, 3);
        }
    }

    // Close the original slave fd if it's not one of the standard fds
    if slave_fd > libc::STDERR_FILENO {
        // SAFETY: slave_fd is still open and is not one of the standard fds
        // we just duplicated onto, so closing it cannot invalidate them.
        unsafe {
            libc::close(slave_fd);
        }
    }

    close_inherited_fds(exec_plan.fd_limit, receipt);

    // Change to working directory
    // SAFETY: working_dir is a NUL-terminated CString built before fork and
    // outlives this call; chdir is async-signal-safe. Legacy shell restoration
    // tolerates missing directories; explicit launches require the requested cwd.
    unsafe {
        if libc::chdir(exec_plan.working_dir.as_ptr()) < 0 && strict_directory {
            fail_child(receipt, 4);
        }
    }

    // The Rust runtime ignores SIGPIPE and this thread may have signals
    // blocked; neither must leak into the shell. Both calls are
    // async-signal-safe.
    // SAFETY: plain libc calls with valid arguments; `mask` is a fully
    // initialized sigset_t (sigemptyset) before it is passed to sigprocmask
    // and the old-mask pointer is allowed to be null.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
    }

    // Execute the program
    // SAFETY: program, argv and envp are NUL-terminated arrays of CStrings
    // built before fork; argv and envp end with a null pointer. execve only
    // returns on failure, after which we _exit without touching Rust state.
    unsafe {
        libc::execve(
            exec_plan.program.as_ptr(),
            exec_plan.argv.as_ptr(),
            exec_plan.envp.as_ptr(),
        );
    }
    fail_child(receipt, 5);
}

/// Only stack data and async-signal-safe syscalls are permitted after fork.
fn fail_child(receipt: libc::c_int, stage: u8) -> ! {
    let errno = nix::errno::Errno::last() as i32;
    let error = errno.to_ne_bytes();
    let bytes = [stage, error[0], error[1], error[2], error[3]];
    let mut offset = 0;
    while offset < bytes.len() {
        // SAFETY: receipt is the owned setup socket, bytes is valid
        // stack storage, and write is async-signal-safe.
        let written = unsafe {
            libc::write(
                receipt,
                bytes[offset..].as_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if written > 0 {
            offset += written as usize;
        } else if written == 0 || nix::errno::Errno::last() != nix::errno::Errno::EINTR {
            break;
        }
    }
    exit_child(127);
}

fn exit_child(code: i32) -> ! {
    // SAFETY: _exit is async-signal-safe and never returns; it skips Rust
    // destructors and atexit handlers, which is required in a forked child.
    unsafe {
        libc::_exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checked_spawn_reports_exec_and_directory_failure() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let result = Child::spawn_checked(
            missing.to_str().unwrap(),
            &[],
            dir.path(),
            PtySize::default(),
            &[],
        );
        assert!(matches!(
            result,
            Err(PtyError::StartupFailed { stage: "exec", .. })
        ));
        let result = Child::spawn_checked("/bin/true", &[], &missing, PtySize::default(), &[]);
        assert!(matches!(
            result,
            Err(PtyError::StartupFailed {
                stage: "working directory",
                ..
            })
        ));
        let mut child =
            Child::spawn_checked("/bin/true", &[], dir.path(), PtySize::default(), &[]).unwrap();
        assert_eq!(child.wait().unwrap(), 0);
    }

    #[test]
    fn test_checked_spawn_preserves_exact_argv_and_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("receipt");
        let args = [
            "sh",
            "-c",
            "printf '%s\\n%s\\n' \"$PWD\" \"$2\" > \"$1\"",
            "argv-test",
            output.to_str().unwrap(),
            "space ; $HOME 'quote'\nsecond line",
        ];
        let mut child =
            Child::spawn_checked("/bin/sh", &args, dir.path(), PtySize::default(), &[]).unwrap();
        assert_eq!(child.wait().unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(output).unwrap(),
            format!(
                "{}\nspace ; $HOME 'quote'\nsecond line\n",
                dir.path().display()
            )
        );
    }

    #[test]
    fn path_lookup_skips_files_that_are_not_executable() {
        use std::os::unix::fs::PermissionsExt;
        let sourced = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        // Like uv's ~/.local/bin/env: a script to source, not to run.
        std::fs::write(sourced.path().join("tool"), "export X=1\n").unwrap();
        let program = real.path().join("tool");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

        let paths = std::env::join_paths([sourced.path(), real.path()]).unwrap();
        assert_eq!(resolve_in_path("tool", &paths), program.to_string_lossy());
        assert_eq!(resolve_in_path("missing", &paths), "missing");
        assert_eq!(resolve_in_path("/bin/sh", &paths), "/bin/sh");
    }

    #[test]
    fn spawn_echo() {
        let mut child = Child::spawn(
            "/bin/echo",
            &["/bin/echo", "hello"],
            &current_dir(),
            PtySize::default(),
            &[],
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
            &[],
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

    /// Read PTY output until `needle` appears or the timeout elapses.
    fn read_until(child: &Child, needle: &str, timeout: std::time::Duration) -> String {
        let deadline = std::time::Instant::now() + timeout;
        let mut output = Vec::new();
        let mut buf = [0u8; 4096];
        while std::time::Instant::now() < deadline {
            match child.read(&mut buf) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(n) => {
                    output.extend_from_slice(&buf[..n]);
                    if String::from_utf8_lossy(&output).contains(needle) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&output).into_owned()
    }

    fn run_in_pty(script: &str) -> String {
        run_in_pty_with_env(script, &[])
    }

    fn run_in_pty_with_env(script: &str, env: &[(OsString, OsString)]) -> String {
        let mut child = Child::spawn(
            "/bin/sh",
            &["/bin/sh", "-c", script],
            &current_dir(),
            PtySize::default(),
            env,
        )
        .expect("Failed to spawn sh");
        let output = read_until(&child, "DONE", std::time::Duration::from_secs(5));
        let _ = child.wait();
        output
    }

    /// Extract the hexadecimal mask of a `Sig*:` line from `/proc/self/status`.
    #[cfg(target_os = "linux")]
    fn proc_status_mask(output: &str, field: &str) -> u64 {
        let line = output
            .lines()
            .find(|line| line.starts_with(field))
            .unwrap_or_else(|| panic!("no {field} line in {output:?}"));
        u64::from_str_radix(line.trim_start_matches(field).trim(), 16)
            .unwrap_or_else(|_| panic!("unparsable mask in {line:?}"))
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn child_gets_default_sigpipe_disposition() {
        // The Rust runtime sets SIGPIPE to SIG_IGN, and ignored dispositions
        // survive exec. The child must start with SIGPIPE back at SIG_DFL.
        let output = run_in_pty("grep SigIgn /proc/self/status; echo DONE");
        let ignored = proc_status_mask(&output, "SigIgn:");
        let sigpipe_bit = 1u64 << (libc::SIGPIPE - 1);
        assert_eq!(
            ignored & sigpipe_bit,
            0,
            "child inherited ignored SIGPIPE: {output:?}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn child_starts_with_empty_signal_mask() {
        use nix::sys::signal::{pthread_sigmask, SigSet, SigmaskHow, Signal};

        // Block a signal in the spawning thread; fork() copies that mask.
        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGUSR1);
        let mut previous = SigSet::empty();
        pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous)).unwrap();

        let output = run_in_pty("grep SigBlk /proc/self/status; echo DONE");

        pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None).unwrap();

        let blocked = proc_status_mask(&output, "SigBlk:");
        assert_eq!(
            blocked, 0,
            "child inherited a blocked-signal mask: {output:?}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn child_inherits_only_the_standard_fds() {
        // An fd opened without CLOEXEC (as some GPU drivers do) must not
        // reach the shell.
        // SAFETY: the path is a NUL-terminated literal; the fd is closed below.
        let leaked = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
        assert!(leaked > 2, "open failed: {leaked}");

        // `ls` opens the directory it lists, so 3 is its own fd.
        let output = run_in_pty("echo FDS $(ls /proc/self/fd); echo DONE");

        // SAFETY: `leaked` is the fd opened above and still owned here.
        unsafe { libc::close(leaked) };
        let fds = output
            .lines()
            .find_map(|line| line.trim().strip_prefix("FDS "))
            .unwrap_or_else(|| panic!("no fd list in {output:?}"));
        assert_eq!(
            fds.trim(),
            "0 1 2 3",
            "child inherited extra fds: {output:?}"
        );
    }

    #[test]
    fn a_child_offered_a_protocol_socket_learns_its_own_terminal() {
        let env = [(
            OsString::from("ULTTY_SOCKET"),
            OsString::from("/run/x.sock"),
        )];
        let output = run_in_pty_with_env(
            "if [ \"$ULTTY_TTY\" = \"$(tty)\" ]; then echo TTY-MATCH; fi; echo DONE",
            &env,
        );
        assert!(output.contains("TTY-MATCH"), "ULTTY_TTY != tty: {output:?}");

        // Without a socket nothing is added (an Ultty running these tests
        // passes its own ULTTY_TTY down, so only check outside one).
        if std::env::var_os("ULTTY_TTY").is_none() {
            let output = run_in_pty("echo \"tty=[$ULTTY_TTY]\"; echo DONE");
            assert!(
                output.contains("tty=[]"),
                "set without a socket: {output:?}"
            );
        }
    }

    #[test]
    fn multiplexer_variables_are_not_inherited() {
        // Only meaningful when the test itself runs inside tmux or screen.
        if std::env::var_os("TMUX").is_none() && std::env::var_os("STY").is_none() {
            return;
        }
        let output = run_in_pty("echo \"mux=[$TMUX$STY$TMUX_PANE]\"; echo DONE");
        assert!(output.contains("mux=[]"), "leaked: {output:?}");
    }

    #[test]
    fn per_spawn_env_reaches_only_that_child() {
        let env = [(
            OsString::from("ULTTY_CHILD_ENV_TEST"),
            OsString::from("per-spawn"),
        )];
        let output = run_in_pty_with_env("echo \"value=$ULTTY_CHILD_ENV_TEST\"; echo DONE", &env);
        assert!(
            output.contains("value=per-spawn"),
            "child did not receive per-spawn env: {output:?}"
        );

        // The variable is scoped to that spawn: a sibling spawned without it
        // must not see it (nothing leaked into a process-wide table or std::env).
        let output = run_in_pty("echo \"value=[$ULTTY_CHILD_ENV_TEST]\"; echo DONE");
        assert!(
            output.contains("value=[]"),
            "per-spawn env leaked into another child: {output:?}"
        );
    }

    #[test]
    fn term_comes_from_the_spawn_env_or_defaults_to_xterm_256color() {
        let env = [(OsString::from("TERM"), OsString::from("xterm-ultty"))];
        let output = run_in_pty_with_env("echo \"term=[$TERM]\"; echo DONE", &env);
        assert!(output.contains("term=[xterm-ultty]"), "got {output:?}");

        let output = run_in_pty("echo \"term=[$TERM]\"; echo DONE");
        assert!(output.contains("term=[xterm-256color]"), "got {output:?}");
    }

    #[test]
    fn child_remembers_the_term_it_was_started_with() {
        let env = [(OsString::from("TERM"), OsString::from("xterm-ultty"))];
        let mut child = Child::spawn("/bin/true", &[], &current_dir(), PtySize::default(), &env)
            .expect("Failed to spawn true");
        assert_eq!(child.term(), "xterm-ultty");
        let _ = child.wait();

        let mut child = Child::spawn("/bin/true", &[], &current_dir(), PtySize::default(), &[])
            .expect("Failed to spawn true");
        assert_eq!(child.term(), DEFAULT_TERMINAL_NAME);
        let _ = child.wait();
    }

    #[test]
    fn per_spawn_env_overrides_inherited_variable() {
        // PATH is always inherited; the per-spawn value must replace it
        // rather than appear alongside it.
        let env = [(OsString::from("PATH"), OsString::from("/ultty-override"))];
        let output = run_in_pty_with_env("echo \"path=$PATH\"; echo DONE", &env);
        assert!(
            output.contains("path=/ultty-override"),
            "override did not replace inherited PATH: {output:?}"
        );
    }

    #[test]
    fn resize_child() {
        let child = Child::spawn(
            "/bin/sh",
            &["/bin/sh"],
            &current_dir(),
            PtySize::new(80, 24),
            &[],
        )
        .expect("Failed to spawn shell");

        // Resize should succeed
        assert!(child.resize(PtySize::new(120, 40)).is_ok());
    }
}
