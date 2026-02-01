//! Backend detection logic.

pub fn is_ultty() -> bool {
    std::env::var("ULTTY_SOCKET").is_ok()
}

pub fn socket_path() -> Option<std::path::PathBuf> {
    std::env::var("ULTTY_SOCKET")
        .ok()
        .map(std::path::PathBuf::from)
}
