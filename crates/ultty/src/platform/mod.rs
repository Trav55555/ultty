//! Platform-specific code.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

use crate::pty::Child;

// Common platform trait
pub trait Platform {
    /// Get the socket path for native protocol.
    fn socket_path() -> std::path::PathBuf;

    /// Spawn a shell process with PTY using default size.
    fn spawn_shell() -> std::io::Result<Child>;
}
