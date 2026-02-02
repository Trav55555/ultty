//! Native binary protocol handler.
//!
//! This module is a placeholder for potential future refactoring.
//! Currently, native protocol handling is implemented in `session.rs` and `server.rs`.

use ultty_protocol::{AppMessage, TerminalMessage};

/// Native protocol connection handler (unused - see session.rs).
pub struct NativeProtocol {}

impl NativeProtocol {
    pub fn new() -> Self {
        Self {}
    }

    pub fn handle_message(&mut self, _msg: AppMessage) -> Option<TerminalMessage> {
        None
    }
}

impl Default for NativeProtocol {
    fn default() -> Self {
        Self::new()
    }
}
