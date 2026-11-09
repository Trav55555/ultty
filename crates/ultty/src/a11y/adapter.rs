//! The platform accessibility adapter for Ultty's window.

use accesskit_winit::{Event, WindowEvent as AccessKitEvent};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use super::tree;

/// Owns the AccessKit adapter for one window.
///
/// Until a screen reader asks for the tree it costs a match per window
/// event and, on Linux, the D-Bus client's threads: the adapter watches the
/// session bus from its own thread and joins the AT-SPI bus only while
/// assistive technology is enabled. Without a session bus it stays inert.
pub struct Adapter {
    inner: accesskit_winit::Adapter,
    /// The window's title as last set. winit cannot read it back on X11 or
    /// Wayland (`Window::title` is always empty there).
    window_title: String,
}

impl Adapter {
    /// Create the adapter for `window`, titled `window_title`, which must
    /// not have been shown yet.
    ///
    /// AccessKit's requests arrive on the event loop through `proxy` and go
    /// to [`Adapter::handle`].
    ///
    /// # Panics
    ///
    /// If `window` is already visible: AccessKit must see the window before
    /// it is first shown.
    pub fn new<T: From<Event> + Send + 'static>(
        event_loop: &ActiveEventLoop,
        window: &Window,
        window_title: &str,
        proxy: EventLoopProxy<T>,
    ) -> Self {
        Self {
            inner: accesskit_winit::Adapter::with_event_loop_proxy(event_loop, window, proxy),
            window_title: window_title.to_owned(),
        }
    }

    /// Let the adapter see a window event (bounds and focus) before the
    /// app handles it.
    pub fn process_event(&mut self, window: &Window, event: &WindowEvent) {
        self.inner.process_event(window, event);
    }

    /// Answer a request AccessKit sent through the event loop.
    pub fn handle(&mut self, window: &Window, event: AccessKitEvent) {
        match event {
            AccessKitEvent::InitialTreeRequested => {
                tracing::debug!("Accessibility activated");
                // The adapter learns the window's bounds only from Moved and
                // Resized events; one that was never moved or resized would
                // be a 0x0 frame to screen readers and magnifiers.
                self.inner
                    .process_event(window, &WindowEvent::Resized(window.inner_size()));
                self.send_tree();
            }
            AccessKitEvent::AccessibilityDeactivated => {
                // The adapter is inactive again; nothing is sent until the
                // next `InitialTreeRequested`.
                tracing::debug!("Accessibility deactivated");
            }
            AccessKitEvent::ActionRequested(request) => {
                tracing::debug!(action = ?request.action, "Accessibility action not handled yet");
            }
        }
    }

    /// The window was retitled; a screen reader in use hears the new name.
    pub fn set_window_title(&mut self, title: &str) {
        if self.window_title != title {
            self.window_title = title.to_owned();
            self.send_tree();
        }
    }

    /// Send the tree, if assistive technology has asked for it.
    fn send_tree(&mut self) {
        let title = &self.window_title;
        self.inner.update_if_active(|| tree::window_only(title));
    }
}
