//! Unix-socket helpers shared by the platform backends: private socket
//! directories and peer-credential checks for the native protocol server.

use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::terminal_manager::TabId;

/// Directory permissions for the per-user socket directory.
const PRIVATE_DIR_MODE: u32 = 0o700;

/// Real user id of this process.
pub fn current_uid() -> libc::uid_t {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Native protocol socket file name for one tab of this process.
///
/// Every tab listens on its own socket so that a native app only ever talks
/// to the tab whose shell launched it.
pub fn tab_socket_file_name(tab_id: TabId) -> String {
    format!("ultty-{}-tab{}.sock", std::process::id(), tab_id.raw())
}

/// Resolve the directory that holds this process's native protocol sockets.
///
/// Uses `XDG_RUNTIME_DIR` when it is set (it is already private to the
/// user). Otherwise falls back to `<base>/ultty-<uid>/`, created with mode
/// 0700; if that directory already exists it must be a directory owned by
/// this user with mode 0700, or the call refuses with an error rather than
/// listen inside a directory someone else controls.
pub fn private_socket_dir(fallback_base: &Path) -> io::Result<PathBuf> {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime_dir = PathBuf::from(runtime_dir);
        if !runtime_dir.as_os_str().is_empty() {
            return Ok(runtime_dir);
        }
    }

    let dir = fallback_base.join(format!("ultty-{}", current_uid()));
    ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Create `dir` with mode 0700, or verify an existing one is safe to use.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => verify_private_dir(dir, &meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(PRIVATE_DIR_MODE))?;
            // Re-check: another process may have raced us on the same path.
            let meta = fs::symlink_metadata(dir)?;
            verify_private_dir(dir, &meta)
        }
        Err(e) => Err(e),
    }
}

fn verify_private_dir(dir: &Path, meta: &fs::Metadata) -> io::Result<()> {
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists and is not a directory", dir.display()),
        ));
    }
    if meta.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is owned by uid {}, not the current user",
                dir.display(),
                meta.uid()
            ),
        ));
    }
    let mode = meta.mode() & 0o777;
    if mode != PRIVATE_DIR_MODE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} has mode {:o}, expected {:o}",
                dir.display(),
                mode,
                PRIVATE_DIR_MODE
            ),
        ));
    }
    Ok(())
}

/// Effective user id of the process on the other end of a Unix socket.
#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<libc::uid_t> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `stream` owns a valid open socket fd for the duration of this
    // call; `cred` is a properly sized, writable ucred and `len` holds its
    // size, which is what SO_PEERCRED requires.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SO_PEERCRED returned an unexpected size",
        ));
    }
    Ok(cred.uid)
}

/// Effective user id of the process on the other end of a Unix socket.
#[cfg(target_os = "macos")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<libc::uid_t> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `stream` owns a valid open socket fd for the duration of this
    // call, and `uid`/`gid` are valid writable locations for getpeereid.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// Returns true when the peer of `stream` runs as the same user as this process.
pub fn peer_is_current_user(stream: &UnixStream) -> io::Result<bool> {
    Ok(peer_uid(stream)? == current_uid())
}

/// Mark a file descriptor close-on-exec so children spawned by the terminal
/// never inherit it.
pub fn set_cloexec(fd: &impl AsRawFd) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    // SAFETY: `fd` is a valid open descriptor owned by the caller; F_GETFD
    // and F_SETFD only read and write descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above; FD_CLOEXEC is a valid descriptor flag.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Returns true if `fd` has the close-on-exec flag set.
pub fn is_cloexec(fd: &impl AsRawFd) -> io::Result<bool> {
    // SAFETY: `fd` is a valid open descriptor owned by the caller; F_GETFD
    // only reads descriptor flags.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags & libc::FD_CLOEXEC != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir_path(tag: &str) -> PathBuf {
        let unique = format!(
            "ultty-socket-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn peer_uid_of_socket_pair_is_current_user() {
        let (a, b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), current_uid());
        assert!(peer_is_current_user(&b).unwrap());
    }

    #[test]
    fn set_cloexec_marks_descriptor() {
        let (a, _b) = UnixStream::pair().unwrap();
        set_cloexec(&a).unwrap();
        assert!(is_cloexec(&a).unwrap());
    }

    #[test]
    fn ensure_private_dir_creates_with_mode_0700() {
        let dir = temp_dir_path("create");
        ensure_private_dir(&dir).unwrap();
        let meta = fs::metadata(&dir).unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.mode() & 0o777, 0o700);
        // Idempotent on a good directory.
        ensure_private_dir(&dir).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn ensure_private_dir_refuses_wrong_mode() {
        let dir = temp_dir_path("mode");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let err = ensure_private_dir(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn ensure_private_dir_refuses_non_directory() {
        let path = temp_dir_path("file");
        fs::write(&path, b"not a dir").unwrap();
        let err = ensure_private_dir(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn private_socket_dir_falls_back_under_uid_dir() {
        let base = temp_dir_path("base");
        fs::create_dir(&base).unwrap();
        // Force the fallback regardless of the test environment.
        let saved = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::remove_var("XDG_RUNTIME_DIR");
        let dir = private_socket_dir(&base);
        if let Some(saved) = saved {
            std::env::set_var("XDG_RUNTIME_DIR", saved);
        }
        let dir = dir.unwrap();
        let expected_dir = base.join(format!("ultty-{}", current_uid()));
        assert_eq!(dir, expected_dir);
        assert_eq!(fs::metadata(&expected_dir).unwrap().mode() & 0o777, 0o700);
        fs::remove_dir(&expected_dir).unwrap();
        fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn tab_socket_file_name_is_unique_per_process_and_tab() {
        let pid = std::process::id();
        assert_eq!(
            tab_socket_file_name(TabId::new(0)),
            format!("ultty-{pid}-tab0.sock")
        );
        assert_eq!(
            tab_socket_file_name(TabId::new(7)),
            format!("ultty-{pid}-tab7.sock")
        );
    }
}
