//! Backend detection logic.

/// Check if running in Ultty.
pub fn is_ultty() -> bool {
    std::env::var("ULTIMATE_TERMINAL").is_ok()
}

/// Get the socket path for native protocol.
pub fn socket_path() -> Option<std::path::PathBuf> {
    std::env::var("ULTIMATE_TERMINAL_SOCKET")
        .ok()
        .map(std::path::PathBuf::from)
}
