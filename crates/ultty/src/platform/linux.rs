//! Linux-specific platform code.

use super::Platform;
use std::path::PathBuf;

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn socket_path() -> PathBuf {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        runtime_dir.join("ultimate-terminal.sock")
    }

    fn spawn_shell() -> std::io::Result<()> {
        unimplemented!("PTY spawning not yet implemented")
    }
}
