//! Where control sockets and their auth tokens live, and how `ulttyctl`
//! finds a running instance.
//!
//! Every Ultty process listens on its own socket, `ultty-ctl-<pid>.sock`,
//! next to its own token, `ultty-auth-<pid>.token`, so a second window (a
//! second process) gets a control socket too and one exiting never takes
//! another's away. Both live in `$XDG_RUNTIME_DIR`, or when that is unset in
//! a private `ultty-<uid>` directory (mode 0700, owned by this user) under
//! the temp directory; never directly in a shared `/tmp`.

use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::platform::runtime_files::is_running_ultty;
use crate::platform::socket::{current_uid, private_socket_dir};

const SOCKET_PREFIX: &str = "ultty-ctl-";
const SOCKET_SUFFIX: &str = ".sock";

/// The directory holding control sockets and tokens, created if needed.
pub fn control_dir() -> io::Result<PathBuf> {
    private_socket_dir(&std::env::temp_dir())
}

/// A control socket and the token file that authenticates to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlEndpoint {
    /// The Unix socket the instance listens on.
    pub socket: PathBuf,
    /// The file holding that instance's auth token.
    pub token: PathBuf,
}

impl ControlEndpoint {
    /// The endpoint of the Ultty process `pid` in `dir`.
    pub fn for_pid(dir: &Path, pid: u32) -> Self {
        Self {
            socket: dir.join(format!("{SOCKET_PREFIX}{pid}{SOCKET_SUFFIX}")),
            token: dir.join(format!("ultty-auth-{pid}.token")),
        }
    }

    /// The pid of the Ultty this endpoint belongs to, from its socket name.
    pub fn pid(&self) -> Option<u32> {
        pid_from_socket_name(self.socket.file_name()?.to_str()?)
    }
}

/// The most recently started instance in `dir` that is still listening.
///
/// Sockets left behind by a crashed instance, and anything not owned by
/// this user, are skipped.
pub fn find_running_instance(dir: &Path) -> Option<ControlEndpoint> {
    let uid = current_uid();
    let mut candidates: Vec<(i64, i64, u32)> = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let pid = pid_from_socket_name(entry.file_name().to_str()?)?;
            let meta = entry.metadata().ok()?;
            (meta.file_type().is_socket() && meta.uid() == uid)
                .then(|| (meta.mtime(), meta.mtime_nsec(), pid))
        })
        .collect();
    // Newest first.
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates
        .into_iter()
        .map(|(_, _, pid)| ControlEndpoint::for_pid(dir, pid))
        .find(|endpoint| UnixStream::connect(&endpoint.socket).is_ok())
}

/// Which instance a command asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceRequest {
    /// No preference: the newest running instance.
    Newest,
    /// The Ultty this command runs inside, from `ULTTY_PID`. While that
    /// process runs, only its socket will do; once it is gone (a tmux
    /// session can outlive the window that started it), the newest.
    Own(u32),
    /// Exactly this pid (`ulttyctl --pid`), never another.
    Exact(u32),
}

/// The instance [`find_instance`] picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundInstance {
    /// Where to connect.
    pub endpoint: ControlEndpoint,
    /// The `ULTTY_PID` that named an Ultty no longer running, when the
    /// newest instance was taken in its place. Callers must say so.
    pub replaces_gone: Option<u32>,
}

/// Why no instance was picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceError {
    /// The requested Ultty runs but is not listening (still starting, or
    /// stuck). Another instance is never used in its place.
    NotListening(u32),
    /// `--pid` named a process that is not a running Ultty.
    NotRunning(u32),
    /// No instance is listening at all.
    NoneRunning,
}

/// The instance a command should talk to; see [`InstanceRequest`].
///
/// Ultty puts its pid in `ULTTY_PID` for every shell it starts, so
/// `ulttyctl` run inside a window controls that window. A script that
/// starts its own Ultty and exports `ULTTY_PID` must never reach some
/// other window while its own is still starting up.
pub fn find_instance(dir: &Path, request: InstanceRequest) -> Result<FoundInstance, InstanceError> {
    // A pid reused by some other program counts as gone.
    let is_running_ultty = |pid| libc::pid_t::try_from(pid).is_ok_and(is_running_ultty);
    let listening = |pid| {
        let endpoint = ControlEndpoint::for_pid(dir, pid);
        UnixStream::connect(&endpoint.socket)
            .is_ok()
            .then_some(endpoint)
    };
    let found = |endpoint| FoundInstance {
        endpoint,
        replaces_gone: None,
    };
    match request {
        InstanceRequest::Exact(pid) => match listening(pid) {
            Some(endpoint) => Ok(found(endpoint)),
            None if is_running_ultty(pid) => Err(InstanceError::NotListening(pid)),
            None => Err(InstanceError::NotRunning(pid)),
        },
        InstanceRequest::Own(pid) => {
            if let Some(endpoint) = listening(pid) {
                return Ok(found(endpoint));
            }
            if is_running_ultty(pid) {
                return Err(InstanceError::NotListening(pid));
            }
            let endpoint = find_running_instance(dir).ok_or(InstanceError::NoneRunning)?;
            Ok(FoundInstance {
                endpoint,
                replaces_gone: Some(pid),
            })
        }
        InstanceRequest::Newest => find_running_instance(dir)
            .map(found)
            .ok_or(InstanceError::NoneRunning),
    }
}

fn pid_from_socket_name(name: &str) -> Option<u32> {
    name.strip_prefix(SOCKET_PREFIX)?
        .strip_suffix(SOCKET_SUFFIX)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn endpoint_names_carry_the_pid() {
        let endpoint = ControlEndpoint::for_pid(Path::new("/run/user/1000"), 42);
        assert_eq!(
            endpoint.socket,
            Path::new("/run/user/1000/ultty-ctl-42.sock")
        );
        assert_eq!(
            endpoint.token,
            Path::new("/run/user/1000/ultty-auth-42.token")
        );
        assert_eq!(endpoint.pid(), Some(42));
        assert_eq!(pid_from_socket_name("ultty-ctl-42.sock"), Some(42));
        assert_eq!(pid_from_socket_name("ultty-ctl.sock"), None);
        assert_eq!(pid_from_socket_name("ultty-ctl-42.token"), None);
    }

    /// Wait until `probe` returns `expected`. A dropped listener can keep
    /// accepting for a moment: a test running in parallel may fork a child,
    /// and the child holds copies of every open fd until it execs.
    fn eventually<T: PartialEq + std::fmt::Debug>(expected: T, mut probe: impl FnMut() -> T) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let got = probe();
            if got == expected || std::time::Instant::now() > deadline {
                assert_eq!(got, expected);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The pid of a process that has exited and been reaped.
    fn reaped_child_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    }

    /// Listen as the instance with pid `pid` in `dir`.
    fn listen(dir: &Path, pid: u32) -> (ControlEndpoint, UnixListener) {
        let endpoint = ControlEndpoint::for_pid(dir, pid);
        let listener = UnixListener::bind(&endpoint.socket).unwrap();
        (endpoint, listener)
    }

    fn found(endpoint: ControlEndpoint) -> Result<FoundInstance, InstanceError> {
        Ok(FoundInstance {
            endpoint,
            replaces_gone: None,
        })
    }

    // The tests below use their own pid as the live Ultty, since
    // `is_running_ultty` counts the calling process as one.

    #[test]
    fn a_live_own_instance_is_used_while_it_listens() {
        let dir = tempfile::tempdir().unwrap();
        let own_pid = std::process::id();
        let (own, _own_listener) = listen(dir.path(), own_pid);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (_newer, _newer_listener) = listen(dir.path(), 200);

        assert_eq!(
            find_instance(dir.path(), InstanceRequest::Own(own_pid)),
            found(own)
        );
    }

    #[test]
    fn a_live_own_instance_without_a_socket_is_an_error_not_another_instance() {
        let dir = tempfile::tempdir().unwrap();
        let (_other, _other_listener) = listen(dir.path(), 200);

        let request = InstanceRequest::Own(std::process::id());
        assert_eq!(
            find_instance(dir.path(), request),
            Err(InstanceError::NotListening(std::process::id()))
        );
    }

    #[test]
    fn a_live_own_instance_that_stops_listening_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (_other, _other_listener) = listen(dir.path(), 200);
        let (own, own_listener) = listen(dir.path(), std::process::id());
        let request = InstanceRequest::Own(std::process::id());
        assert_eq!(find_instance(dir.path(), request), found(own));

        drop(own_listener);
        eventually(Err(InstanceError::NotListening(std::process::id())), || {
            find_instance(dir.path(), request)
        });
    }

    #[test]
    fn a_gone_own_instance_falls_back_to_the_newest_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let (newest, _listener) = listen(dir.path(), 200);
        let gone = reaped_child_pid();

        assert_eq!(
            find_instance(dir.path(), InstanceRequest::Own(gone)),
            Ok(FoundInstance {
                endpoint: newest,
                replaces_gone: Some(gone),
            })
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            find_instance(empty.path(), InstanceRequest::Own(gone)),
            Err(InstanceError::NoneRunning)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_own_pid_reused_by_another_program_counts_as_gone() {
        let dir = tempfile::tempdir().unwrap();
        let (newest, _listener) = listen(dir.path(), 200);
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();

        let result = find_instance(dir.path(), InstanceRequest::Own(child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            result,
            Ok(FoundInstance {
                endpoint: newest,
                replaces_gone: Some(child.id()),
            })
        );
    }

    #[test]
    fn without_a_preference_the_newest_instance_is_used() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            find_instance(dir.path(), InstanceRequest::Newest),
            Err(InstanceError::NoneRunning)
        );
        let (_older, _older_listener) = listen(dir.path(), 100);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (newer, _newer_listener) = listen(dir.path(), 200);

        assert_eq!(
            find_instance(dir.path(), InstanceRequest::Newest),
            found(newer)
        );
    }

    #[test]
    fn an_exact_pid_never_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let (_other, _other_listener) = listen(dir.path(), 200);

        let live = InstanceRequest::Exact(std::process::id());
        assert_eq!(
            find_instance(dir.path(), live),
            Err(InstanceError::NotListening(std::process::id()))
        );
        let gone = reaped_child_pid();
        assert_eq!(
            find_instance(dir.path(), InstanceRequest::Exact(gone)),
            Err(InstanceError::NotRunning(gone))
        );

        let (own, _own_listener) = listen(dir.path(), std::process::id());
        assert_eq!(find_instance(dir.path(), live), found(own));
    }

    #[test]
    fn finds_the_newest_listening_instance_and_skips_stale_sockets() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_running_instance(dir.path()), None);

        let older = ControlEndpoint::for_pid(dir.path(), 100);
        let _older_listener = UnixListener::bind(&older.socket).unwrap();
        assert_eq!(find_running_instance(dir.path()), Some(older.clone()));

        // A newer instance wins while it runs.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = ControlEndpoint::for_pid(dir.path(), 200);
        let newer_listener = UnixListener::bind(&newer.socket).unwrap();
        assert_eq!(find_running_instance(dir.path()), Some(newer));

        // A crashed instance leaves its socket file behind; the older one
        // is still found.
        drop(newer_listener);
        eventually(Some(older), || find_running_instance(dir.path()));
    }

    #[test]
    fn ignores_files_that_are_not_sockets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ultty-ctl-7.sock"), b"").unwrap();
        assert_eq!(find_running_instance(dir.path()), None);
    }
}
