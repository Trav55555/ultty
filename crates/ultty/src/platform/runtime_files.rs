//! Sweeping runtime files left behind by Ultty instances that are gone.
//!
//! Every Ultty process keeps three kinds of file in its runtime directory,
//! each named after its pid: per-tab protocol sockets
//! (`ultty-<pid>-tab<N>.sock`), a control socket (`ultty-ctl-<pid>.sock`) and
//! that socket's auth token (`ultty-auth-<pid>.token`). The servers remove
//! them on a clean exit, but a process that is killed with SIGKILL or that
//! crashes cannot, so a new instance clears them out at start.
//!
//! A file is removed only when every check says it is stale: its name is one
//! of the three patterns exactly, it is a socket (or, for a token, a regular
//! file) owned by this user, the pid in its name is not a running Ultty, and
//! no one is listening on the socket (for a token, on the control socket of
//! the same pid).

use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::control::paths::ControlEndpoint;
use crate::platform::socket::current_uid;

/// What a runtime file is, going by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeFile {
    /// `ultty-<pid>-tab<N>.sock`
    TabSocket { pid: libc::pid_t },
    /// `ultty-ctl-<pid>.sock`
    ControlSocket { pid: libc::pid_t },
    /// `ultty-auth-<pid>.token`
    AuthToken { pid: libc::pid_t },
}

impl RuntimeFile {
    fn parse(name: &str) -> Option<Self> {
        if let Some(pid) = name
            .strip_prefix("ultty-ctl-")
            .and_then(|rest| rest.strip_suffix(".sock"))
        {
            return Some(Self::ControlSocket {
                pid: parse_pid(pid)?,
            });
        }
        if let Some(pid) = name
            .strip_prefix("ultty-auth-")
            .and_then(|rest| rest.strip_suffix(".token"))
        {
            return Some(Self::AuthToken {
                pid: parse_pid(pid)?,
            });
        }
        let (pid, tab) = name
            .strip_prefix("ultty-")?
            .strip_suffix(".sock")?
            .split_once("-tab")?;
        is_decimal(tab).then_some(())?;
        Some(Self::TabSocket {
            pid: parse_pid(pid)?,
        })
    }

    fn pid(self) -> libc::pid_t {
        match self {
            Self::TabSocket { pid } | Self::ControlSocket { pid } | Self::AuthToken { pid } => pid,
        }
    }
}

fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// A positive pid written in plain decimal digits. Zero and values past
/// `pid_t` are refused: `kill` would read them as process groups.
fn parse_pid(text: &str) -> Option<libc::pid_t> {
    if !is_decimal(text) {
        return None;
    }
    text.parse().ok().filter(|pid| *pid > 0)
}

/// Remove the runtime files in `dir` left behind by Ultty instances that
/// are gone, and return how many were removed.
pub fn sweep_stale_runtime_files(dir: &Path) -> usize {
    let stale = match stale_runtime_files(dir, current_uid(), is_running_ultty) {
        Ok(stale) => stale,
        Err(e) => {
            tracing::warn!("Could not look for stale runtime files in {:?}: {}", dir, e);
            return 0;
        }
    };
    let removed = stale
        .iter()
        .filter(|path| match fs::remove_file(path) {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!("Could not remove stale runtime file {:?}: {}", path, e);
                false
            }
        })
        .count();
    if removed > 0 {
        tracing::info!(
            "Removed {} runtime files left by Ultty instances that are gone from {:?}",
            removed,
            dir
        );
    }
    removed
}

/// The runtime files in `dir` that belong to `uid` and to no running Ultty.
fn stale_runtime_files(
    dir: &Path,
    uid: libc::uid_t,
    is_running_ultty: impl Fn(libc::pid_t) -> bool,
) -> io::Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(kind) = entry.file_name().to_str().and_then(RuntimeFile::parse) else {
            continue;
        };
        // Not followed through symlinks: the entry itself must qualify.
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let right_type = match kind {
            RuntimeFile::TabSocket { .. } | RuntimeFile::ControlSocket { .. } => {
                meta.file_type().is_socket()
            }
            RuntimeFile::AuthToken { .. } => meta.file_type().is_file(),
        };
        if !right_type || meta.uid() != uid || is_running_ultty(kind.pid()) {
            continue;
        }
        // Whatever the pid says, never take a socket from under a listener.
        let socket = match kind {
            RuntimeFile::AuthToken { pid } => ControlEndpoint::for_pid(dir, pid as u32).socket,
            RuntimeFile::TabSocket { .. } | RuntimeFile::ControlSocket { .. } => entry.path(),
        };
        if UnixStream::connect(&socket).is_ok() {
            continue;
        }
        stale.push(entry.path());
    }
    Ok(stale)
}

/// Whether `pid` is this process or another live process that is Ultty.
fn is_running_ultty(pid: libc::pid_t) -> bool {
    if u32::try_from(pid).is_ok_and(|pid| pid == std::process::id()) {
        return true;
    }
    if !is_alive(pid) {
        return false;
    }
    is_ultty(pid)
}

/// Whether a process `pid` exists, even one owned by another user.
fn is_alive(pid: libc::pid_t) -> bool {
    use nix::errno::Errno;
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(Errno::ESRCH) => false,
        // EPERM: it exists, it just isn't ours to signal.
        Err(_) => true,
    }
}

/// Whether the live process `pid` runs the Ultty binary.
#[cfg(target_os = "linux")]
fn is_ultty(pid: libc::pid_t) -> bool {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .is_ok_and(|comm| comm.trim_end_matches('\n') == "ultty")
}

/// Whether the live process `pid` runs the Ultty binary. Without a cheap
/// way to tell here, any live pid counts, which keeps its files: a reused
/// pid leaves stale files in place rather than risk a running instance's.
#[cfg(not(target_os = "linux"))]
fn is_ultty(_pid: libc::pid_t) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::socket::tab_socket_file_name;
    use crate::terminal_manager::TabId;
    use std::os::unix::net::UnixListener;

    /// Plant a socket file nobody listens on, as a killed instance leaves.
    fn dead_socket(path: &Path) {
        drop(UnixListener::bind(path).unwrap());
    }

    fn names(paths: Vec<PathBuf>) -> Vec<String> {
        let mut names: Vec<String> = paths
            .into_iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    fn nobody_running(_pid: libc::pid_t) -> bool {
        false
    }

    #[test]
    fn names_match_the_three_patterns_exactly() {
        assert_eq!(
            RuntimeFile::parse("ultty-42-tab3.sock"),
            Some(RuntimeFile::TabSocket { pid: 42 })
        );
        assert_eq!(
            RuntimeFile::parse("ultty-ctl-42.sock"),
            Some(RuntimeFile::ControlSocket { pid: 42 })
        );
        assert_eq!(
            RuntimeFile::parse("ultty-auth-42.token"),
            Some(RuntimeFile::AuthToken { pid: 42 })
        );
        for odd in [
            "ultty-auth.token",
            "ultty-ctl.sock",
            "ultty-ctl-42.token",
            "ultty-auth-42.sock",
            "ultty-ctl-+42.sock",
            "ultty-ctl-42.sock.bak",
            "ultty-ctl-0.sock",
            "ultty-ctl-99999999999.sock",
            "ultty-42-tab.sock",
            "ultty-42-tab3x.sock",
            "ultty-42-tab-3.sock",
            "ultty-x-tab3.sock",
            "ultty-42-tab3.token",
            "xultty-ctl-42.sock",
            "ultty-1000",
            "bus",
        ] {
            assert_eq!(RuntimeFile::parse(odd), None, "{odd}");
        }
    }

    #[test]
    fn names_the_servers_write_are_recognised() {
        let endpoint = ControlEndpoint::for_pid(Path::new("/run"), 77);
        let name = |p: &Path| p.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(
            RuntimeFile::parse(&name(&endpoint.socket)),
            Some(RuntimeFile::ControlSocket { pid: 77 })
        );
        assert_eq!(
            RuntimeFile::parse(&name(&endpoint.token)),
            Some(RuntimeFile::AuthToken { pid: 77 })
        );
        let own = std::process::id() as libc::pid_t;
        assert_eq!(
            RuntimeFile::parse(&tab_socket_file_name(TabId::new(5))),
            Some(RuntimeFile::TabSocket { pid: own })
        );
    }

    #[test]
    fn files_of_a_dead_instance_are_stale() {
        let dir = tempfile::tempdir().unwrap();
        dead_socket(&dir.path().join("ultty-4242-tab0.sock"));
        dead_socket(&dir.path().join("ultty-4242-tab12.sock"));
        dead_socket(&dir.path().join("ultty-ctl-4242.sock"));
        fs::write(dir.path().join("ultty-auth-4242.token"), b"abc").unwrap();

        let stale = stale_runtime_files(dir.path(), current_uid(), nobody_running).unwrap();
        assert_eq!(
            names(stale),
            [
                "ultty-4242-tab0.sock",
                "ultty-4242-tab12.sock",
                "ultty-auth-4242.token",
                "ultty-ctl-4242.sock",
            ]
        );
    }

    #[test]
    fn files_of_a_running_ultty_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        dead_socket(&dir.path().join("ultty-4242-tab0.sock"));
        dead_socket(&dir.path().join("ultty-ctl-4242.sock"));
        fs::write(dir.path().join("ultty-auth-4242.token"), b"abc").unwrap();

        let stale = stale_runtime_files(dir.path(), current_uid(), |pid| pid == 4242).unwrap();
        assert_eq!(names(stale), Vec::<String>::new());
    }

    #[test]
    fn a_socket_that_accepts_is_kept_whatever_its_pid() {
        let dir = tempfile::tempdir().unwrap();
        let _tab = UnixListener::bind(dir.path().join("ultty-4242-tab0.sock")).unwrap();
        let _ctl = UnixListener::bind(dir.path().join("ultty-ctl-4242.sock")).unwrap();
        // The token goes with its listening control socket.
        fs::write(dir.path().join("ultty-auth-4242.token"), b"abc").unwrap();

        let stale = stale_runtime_files(dir.path(), current_uid(), nobody_running).unwrap();
        assert_eq!(names(stale), Vec::<String>::new());
    }

    #[test]
    fn foreign_names_and_the_shared_token_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ultty-auth.token"), b"abc").unwrap();
        dead_socket(&dir.path().join("ultty-ctl.sock"));
        dead_socket(&dir.path().join("ultty-ctl-42.sock.old"));
        dead_socket(&dir.path().join("ultty-42-tabx.sock"));
        dead_socket(&dir.path().join("other-42.sock"));
        fs::create_dir(dir.path().join("ultty-1000")).unwrap();

        let stale = stale_runtime_files(dir.path(), current_uid(), nobody_running).unwrap();
        assert_eq!(names(stale), Vec::<String>::new());
    }

    #[test]
    fn files_of_the_wrong_type_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        // A regular file with a socket's name, a socket with a token's name,
        // a directory, and symlinks to qualifying files.
        fs::write(dir.path().join("ultty-ctl-4242.sock"), b"").unwrap();
        dead_socket(&dir.path().join("ultty-auth-4242.token"));
        fs::create_dir(dir.path().join("ultty-4242-tab0.sock")).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        dead_socket(&elsewhere.path().join("s"));
        fs::write(elsewhere.path().join("t"), b"abc").unwrap();
        std::os::unix::fs::symlink(
            elsewhere.path().join("s"),
            dir.path().join("ultty-4242-tab1.sock"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            elsewhere.path().join("t"),
            dir.path().join("ultty-auth-4343.token"),
        )
        .unwrap();

        let stale = stale_runtime_files(dir.path(), current_uid(), nobody_running).unwrap();
        assert_eq!(names(stale), Vec::<String>::new());
    }

    /// Files are made as this user; asking on behalf of another uid shows
    /// they would be another user's, and so kept.
    #[test]
    fn files_owned_by_another_user_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        dead_socket(&dir.path().join("ultty-ctl-4242.sock"));
        fs::write(dir.path().join("ultty-auth-4242.token"), b"abc").unwrap();

        let other_uid = current_uid().wrapping_add(1);
        let stale = stale_runtime_files(dir.path(), other_uid, nobody_running).unwrap();
        assert_eq!(names(stale), Vec::<String>::new());
    }

    #[test]
    fn sweep_removes_only_the_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        let dead = reaped_child_pid();
        dead_socket(&dir.path().join(format!("ultty-ctl-{dead}.sock")));
        fs::write(dir.path().join(format!("ultty-auth-{dead}.token")), b"abc").unwrap();
        // This process counts as running Ultty, so its files stay.
        let own = std::process::id();
        dead_socket(&dir.path().join(format!("ultty-ctl-{own}.sock")));
        fs::write(dir.path().join("ultty-auth.token"), b"abc").unwrap();

        assert_eq!(sweep_stale_runtime_files(dir.path()), 2);
        let left: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(
            names(left),
            [
                "ultty-auth.token".to_string(),
                format!("ultty-ctl-{own}.sock")
            ]
        );
    }

    /// The pid of a process that has exited and been reaped.
    fn reaped_child_pid() -> libc::pid_t {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        child.id() as libc::pid_t
    }

    #[test]
    fn a_reaped_child_is_not_running() {
        assert!(!is_running_ultty(reaped_child_pid()));
    }

    #[test]
    fn a_live_process_that_is_not_ultty_is_not_running_ultty() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        assert!(is_alive(pid));
        #[cfg(target_os = "linux")]
        assert!(!is_running_ultty(pid));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
