//! Connection management for TUI applications.

use std::path::PathBuf;

/// Connection to a TUI application.
pub enum Connection {
    /// Native protocol over Unix socket.
    Native {
        socket_path: PathBuf,
        // TODO: async socket handle
    },
    /// Legacy PTY connection (for ANSI apps).
    Pty {
        // TODO: PTY master fd
    },
}
