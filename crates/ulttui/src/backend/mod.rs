//! Rendering backends.

mod ansi;
mod detect;
mod native;

use crate::event::Event;
use crate::widgets::View;

/// Backend for rendering.
pub enum Backend {
    /// Native protocol (Ultty).
    Native(native::NativeBackend),
    /// ANSI fallback (any terminal).
    Ansi(ansi::AnsiBackend),
}

impl Backend {
    /// Detect and create the appropriate backend.
    pub fn detect() -> Self {
        if detect::is_ultty() {
            Backend::Native(native::NativeBackend::new())
        } else {
            Backend::Ansi(ansi::AnsiBackend::new())
        }
    }

    /// Initialize the backend.
    pub fn init(&mut self) -> anyhow::Result<()> {
        match self {
            Backend::Native(b) => b.init(),
            Backend::Ansi(b) => b.init(),
        }
    }

    /// Shutdown the backend.
    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        match self {
            Backend::Native(b) => b.shutdown(),
            Backend::Ansi(b) => b.shutdown(),
        }
    }

    /// Render a view.
    pub fn render(&mut self, view: &impl View) -> anyhow::Result<()> {
        match self {
            Backend::Native(b) => b.render(view),
            Backend::Ansi(b) => b.render(view),
        }
    }

    /// Poll for events.
    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        match self {
            Backend::Native(b) => b.poll_event(),
            Backend::Ansi(b) => b.poll_event(),
        }
    }
}
