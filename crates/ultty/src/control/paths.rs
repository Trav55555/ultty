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
        assert_eq!(pid_from_socket_name("ultty-ctl-42.sock"), Some(42));
        assert_eq!(pid_from_socket_name("ultty-ctl.sock"), None);
        assert_eq!(pid_from_socket_name("ultty-ctl-42.token"), None);
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
        assert_eq!(find_running_instance(dir.path()), Some(older));
    }

    #[test]
    fn ignores_files_that_are_not_sockets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ultty-ctl-7.sock"), b"").unwrap();
        assert_eq!(find_running_instance(dir.path()), None);
    }
}
