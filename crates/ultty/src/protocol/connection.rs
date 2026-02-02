//! Connection management for TUI applications.
//!
//! This module is a placeholder for potential future connection abstraction.
//! Currently, PTY connections are managed by `EventLoop` and native protocol
//! connections are managed by `ProtocolServer`.

use std::path::PathBuf;

/// Connection to a TUI application (unused - placeholder for future abstraction).
pub enum Connection {
    Native { socket_path: PathBuf },
    Pty {},
}
