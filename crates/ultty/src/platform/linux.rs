//! Linux-specific platform code.

use super::socket::private_socket_dir;
use super::Platform;
use crate::pty::{Child, PtySize};
use std::path::{Path, PathBuf};

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn socket_dir() -> std::io::Result<PathBuf> {
        private_socket_dir(Path::new("/tmp"))
    }

    fn spawn_shell() -> std::io::Result<Child> {
        let size = PtySize::default();
        Child::spawn_shell(size, &[]).map_err(std::io::Error::other)
    }
}
