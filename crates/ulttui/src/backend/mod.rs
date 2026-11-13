//! Rendering backends.

mod ansi;
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;
mod native;
pub(crate) mod native_diff;

use std::os::fd::RawFd;
use std::time::Instant;

use ultty_client::detect;

use crate::event::Event;
use crate::widgets::View;

/// Terminal restore helpers for the ANSI backend.
pub use ansi::{install_panic_hook, restore_terminal};
/// Native Ultty protocol backend.
pub use native::{NativeBackend, OverridePush};

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
    /// Write optional status through the backend's own output, without reading input.
    pub fn report_status(
        &mut self,
        report: &ultty_client::program_status::Report,
    ) -> crate::Result<()> {
        let wire = report
            .encode()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        match self {
            Backend::Native(backend) => backend.write_status(&wire),
            Backend::Ansi(backend) => backend.write_status(&wire),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(backend) => backend.write_status(&wire),
        }
    }

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

    /// The ANSI backend, whatever the terminal: for a pane where Ultty does
    /// not draw native scenes (a split tab, spec §1.3).
    pub fn ansi() -> Self {
        Backend::Ansi(ansi::AnsiBackend::new())
    }

    /// Whether an app on this backend reads the user's override files
    /// (ADR 0023): every real backend does; a mock does not, so tests never
    /// depend on the files of whoever runs them.
    pub(crate) fn reads_user_overrides(&self) -> bool {
        match self {
            Backend::Native(_) | Backend::Ansi(_) => true,
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(_) => false,
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

    pub(crate) fn render_with_actions(
        &mut self,
        view: &impl View,
        supports: impl FnMut(&str, ultty_protocol::SemanticActionKind) -> bool,
        root_widget: Option<(&str, bool)>,
    ) -> crate::Result<()> {
        match self {
            Backend::Native(backend) => backend.render_with_actions(view, supports, root_widget),
            _ => self.render(view),
        }
    }

    /// Take a native request for explicit application dispatch.
    pub fn take_action_request(&mut self) -> Option<ultty_protocol::SemanticActionRequest> {
        match self {
            Backend::Native(backend) => backend.take_action_request(),
            Backend::Ansi(_) => None,
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(backend) => backend.action_requests.pop_front(),
        }
    }

    pub(crate) fn validate_action_request(
        &self,
        request: &ultty_protocol::SemanticActionRequest,
    ) -> Result<(), String> {
        match self {
            Backend::Native(backend) => backend.validate_action_request(request),
            Backend::Ansi(_) => Err("semantic actions require a native connection".into()),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(_) => Ok(()),
        }
    }

    /// Report whether an action was applied by its handler or refused.
    pub fn report_action_result(
        &mut self,
        request_id: u32,
        error: Option<String>,
    ) -> crate::Result<()> {
        match self {
            Backend::Native(backend) => backend.send_action_result(request_id, error),
            Backend::Ansi(_) => Ok(()),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(backend) => {
                backend.action_results.push((request_id, error));
                Ok(())
            }
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

    /// The descriptor that becomes readable when the terminal has input for
    /// [`Backend::poll_event`]; the event loop sleeps on it.
    pub fn wait_fd(&self) -> Option<RawFd> {
        match self {
            Backend::Native(b) => b.wait_fd(),
            Backend::Ansi(b) => b.wait_fd(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(_) => None,
        }
    }

    /// Whether [`Backend::poll_event`] has an event without the terminal
    /// sending more (it read ahead), so the event loop must not sleep.
    pub fn has_pending_events(&mut self) -> crate::Result<bool> {
        match self {
            Backend::Native(b) => Ok(b.has_pending_events()),
            Backend::Ansi(b) => b.has_pending_events(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => Ok(b.has_pending_events()),
        }
    }

    /// When the backend next needs [`Backend::poll_event`] called even
    /// without input (the in-band heartbeat).
    pub fn deadline(&self) -> Option<Instant> {
        match self {
            Backend::Native(b) => b.deadline(),
            Backend::Ansi(_) => None,
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(_) => None,
        }
    }

    /// Poll for events. Never waits for the terminal.
    pub fn poll_event(&mut self) -> crate::Result<Option<Event>> {
        match self {
            Backend::Native(b) => b.poll_event(),
            Backend::Ansi(b) => b.poll_event(),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.poll_event(),
        }
    }

    /// The next override the terminal pushed into this app (ADR 0023 §9),
    /// read ahead by [`Backend::poll_event`]. Only a native session can
    /// carry one; an ANSI app reads its files.
    pub fn take_override_push(&mut self) -> Option<OverridePush> {
        match self {
            Backend::Native(b) => b.take_override_push(),
            Backend::Ansi(_) => None,
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => b.take_override_push(),
        }
    }

    /// Tell the terminal whether a pushed override was applied; `error` is
    /// why not.
    pub fn report_override_result(
        &mut self,
        request_id: u32,
        error: Option<String>,
    ) -> crate::Result<()> {
        match self {
            Backend::Native(b) => b.send_override_result(request_id, error),
            Backend::Ansi(_) => Ok(()),
            #[cfg(any(test, feature = "test-utils"))]
            Backend::Mock(b) => {
                b.send_override_result(request_id, error);
                Ok(())
            }
        }
    }
}
