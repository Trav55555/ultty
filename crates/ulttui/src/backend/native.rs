//! Native protocol backend.

use crate::event::Event;
use crate::widgets::View;

/// Native protocol backend for Ultty.
pub struct NativeBackend {
    // TODO: Unix socket connection
}

impl NativeBackend {
    /// Create a new native backend.
    pub fn new() -> Self {
        Self {}
    }

    /// Initialize the backend.
    pub fn init(&mut self) -> anyhow::Result<()> {
        // TODO: Connect to socket
        Ok(())
    }

    /// Shutdown the backend.
    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        // TODO: Disconnect
        Ok(())
    }

    /// Render a view.
    pub fn render(&mut self, _view: &impl View) -> anyhow::Result<()> {
        // TODO: Convert view to protocol messages
        Ok(())
    }

    /// Poll for events.
    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        // TODO: Read from socket
        Ok(None)
    }
}

impl Default for NativeBackend {
    fn default() -> Self {
        Self::new()
    }
}
