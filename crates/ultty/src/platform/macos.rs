//! macOS-specific platform code.

use super::socket::private_socket_dir;
use super::Platform;
use crate::pty::{Child, PtySize};
use std::path::PathBuf;

pub struct MacosPlatform;

impl Platform for MacosPlatform {
    fn socket_dir() -> std::io::Result<PathBuf> {
        // TMPDIR on macOS is already a per-user directory; the ultty-<uid>
        // subdirectory keeps the mode-0700 guarantee either way.
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        private_socket_dir(&base)
    }

    fn spawn_shell() -> std::io::Result<Child> {
        let size = PtySize::default();
        Child::spawn_shell(size, &[]).map_err(std::io::Error::other)
    }
}
