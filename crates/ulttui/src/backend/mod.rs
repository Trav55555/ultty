//! Rendering backends.

mod ansi;
mod detect;
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;
mod native;

use crate::event::Event;
use crate::widgets::View;

/// Backend for rendering.
pub enum Backend {
    /// Native protocol (Ultty).
    Native(native::NativeBackend),
    /// ANSI fallback (any terminal).
    Ansi(ansi::AnsiBackend),
    /// Mock backend (tests only).
    #[cfg(any(test, feature = "test-utils"))]
    Mock(mock::MockBackend),
}

impl Backend {
    /// Detect and create the appropriate backend.
    ///
    /// This creates a backend based on environment detection, but the actual
    /// connection (for native) is deferred to `init()` so we can fall back to
    /// ANSI if the native connection fails.
    pub fn detect() -> Self {
        if detect::is_ultty() {
            Backend::Native(native::NativeBackend::new())
        } else {
            Backend::Ansi(ansi::AnsiBackend::new())
        }
    }

    /// Initialize the backend.
    ///
    /// For native backends, if the connection fails, automatically falls back to ANSI.
    pub fn init(&mut self) -> anyhow::Result<()> {
        match self {
            Backend::Native(b) => match b.init() {
                Ok(()) => Ok(()),
                Err(e) => {
                    tracing::warn!(
                        "Native backend initialization failed: {}. Falling back to ANSI.",
                        e
                    );
                    let mut ansi = ansi::AnsiBackend::new();
                    ansi.init()?;
                    *self = Backend::Ansi(ansi);
                    Ok(())
                }
            },
            Backend::Ansi(b) => b.init(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.init(),
        }
    }

    /// Shutdown the backend.
    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        match self {
            Backend::Native(b) => b.shutdown(),
            Backend::Ansi(b) => b.shutdown(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.shutdown(),
        }
    }

    /// Render a view.
    pub fn render(&mut self, view: &impl View) -> anyhow::Result<()> {
        match self {
            Backend::Native(b) => b.render(view),
            Backend::Ansi(b) => b.render(view),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.render(view),
        }
    }

    /// Poll for events.
    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        match self {
            Backend::Native(b) => b.poll_event(),
            Backend::Ansi(b) => b.poll_event(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.poll_event(),
        }
    }
}
