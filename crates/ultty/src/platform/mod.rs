//! Platform-specific code.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

// Common platform trait
pub trait Platform {
    /// Get the socket path for native protocol.
    fn socket_path() -> std::path::PathBuf;

    /// Spawn a shell process with PTY.
    fn spawn_shell() -> std::io::Result<()>;
}
