//! Rendering backends.

mod ansi;
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;
mod native;
pub(crate) mod native_diff;

use ultty_client::detect;

use crate::event::Event;
use crate::widgets::View;

/// Terminal restore helpers for the ANSI backend.
pub use ansi::{install_panic_hook, restore_terminal};
/// Native Ultty protocol backend.
pub use native::NativeBackend;

/// Backend for rendering.
pub enum Backend {
    /// Native protocol (Ultty).
    Native(NativeBackend),
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
    ///
    /// Order: the tab's socket when this process runs on the Ultty tab that
    /// owns it, then the in-band probe on any terminal, then ANSI.
    pub fn detect() -> Self {
        if detect::is_ultty() {
            Backend::Native(native::NativeBackend::new())
        } else if detect::in_band_candidate() {
            Backend::Native(native::NativeBackend::new_in_band())
        } else {
            Backend::Ansi(ansi::AnsiBackend::new())
        }
    }

    /// Initialize the backend.
    ///
    /// For native backends, if the connection fails, automatically falls back to ANSI.
    pub fn init(&mut self) -> crate::Result<()> {
        match self {
            Backend::Native(b) => match b.init() {
                Ok(()) => Ok(()),
                Err(e) => {
                    if b.prefers_in_band() {
                        // The normal outcome on any terminal that is not Ultty.
                        tracing::debug!("in-band probe: {e}; using ANSI");
                    } else {
                        tracing::warn!(
                            "Native backend initialization failed: {}. Falling back to ANSI.",
                            e
                        );
                    }
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
    pub fn shutdown(&mut self) -> crate::Result<()> {
        match self {
            Backend::Native(b) => b.shutdown(),
            Backend::Ansi(b) => b.shutdown(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.shutdown(),
        }
    }

    /// Render a view.
    pub fn render(&mut self, view: &impl View) -> crate::Result<()> {
        match self {
            Backend::Native(b) => b.render(view),
            Backend::Ansi(b) => b.render(view),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.render(view),
        }
    }

    /// Tell the terminal which widget has focus (native mode only).
    pub fn report_focus(&mut self, widget_id: Option<&str>) -> crate::Result<()> {
        match self {
            Backend::Native(b) => b.report_focus(widget_id),
            Backend::Ansi(_) => Ok(()),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(_) => Ok(()),
        }
    }

    /// Poll for events.
    pub fn poll_event(&mut self) -> crate::Result<Option<Event>> {
        match self {
            Backend::Native(b) => b.poll_event(),
            Backend::Ansi(b) => b.poll_event(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.poll_event(),
        }
    }
}
