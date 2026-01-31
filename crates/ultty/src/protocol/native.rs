//! Native binary protocol handler.

use ultty_protocol::{AppMessage, TerminalMessage};

/// Native protocol connection handler.
pub struct NativeProtocol {
    // TODO: Connection state
}

impl NativeProtocol {
    /// Create a new native protocol handler.
    pub fn new() -> Self {
        Self {}
    }

    /// Process an incoming message.
    pub fn handle_message(&mut self, _msg: AppMessage) -> Option<TerminalMessage> {
        // TODO: Process message and update scene graph
        None
    }
}

impl Default for NativeProtocol {
    fn default() -> Self {
        Self::new()
    }
}
