//! macOS-specific platform code.

use super::Platform;
use crate::pty::{Child, PtySize};
use std::path::PathBuf;

pub struct MacosPlatform;

impl Platform for MacosPlatform {
    fn socket_path() -> PathBuf {
        let tmpdir = std::env::var("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        tmpdir.join("ultimate-terminal.sock")
    }

    fn spawn_shell() -> std::io::Result<Child> {
        let size = PtySize::default();
        Child::spawn_shell(size).map_err(std::io::Error::other)
    }
}
