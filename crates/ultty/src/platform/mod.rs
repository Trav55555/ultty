//! Platform-specific code.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

pub mod socket;

use crate::pty::Child;

// Common platform trait
pub trait Platform {
    /// Directory that holds this process's per-tab native protocol sockets.
    ///
    /// The directory is private to the current user; see
    /// [`socket::private_socket_dir`]. Socket files inside it are named by
    /// [`socket::tab_socket_file_name`].
    fn socket_dir() -> std::io::Result<std::path::PathBuf>;

    /// Spawn a shell process with PTY using default size.
    fn spawn_shell() -> std::io::Result<Child>;
}

/// The native protocol socket directory for the current platform.
pub fn socket_dir() -> std::io::Result<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        <linux::LinuxPlatform as Platform>::socket_dir()
    }
    #[cfg(target_os = "macos")]
    {
        <macos::MacosPlatform as Platform>::socket_dir()
    }
}
