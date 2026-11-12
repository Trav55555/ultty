//! Deferred replies to the `resize` control request.
//!
//! winit's `request_inner_size` returns the new size when the platform
//! resizes the window at once (Wayland), and `None` when the new size comes
//! later in a `WindowEvent::Resized` (X11, where the window manager may also
//! clamp or refuse it). In that case the reply waits for the window to reach
//! the requested size, or for [`RESIZE_REPLY_TIMEOUT`] to pass, and then
//! names the size the window has.

use std::time::{Duration, Instant};

use ultty_protocol::control::ControlResponse;
use winit::dpi::PhysicalSize;

/// Error code for a resize the platform did not apply as requested.
pub(super) const RESIZE_NOT_APPLIED: u16 = 8;

/// How long a resize reply waits for the window to reach the size.
pub(super) const RESIZE_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// A `resize` request the platform applies later, waiting for the window to
/// reach the requested size.
pub(super) struct PendingResize {
    client_key: usize,
    requested: PhysicalSize<u32>,
    expires: Instant,
}

impl PendingResize {
    pub(super) fn new(client_key: usize, requested: PhysicalSize<u32>, now: Instant) -> Self {
        PendingResize {
            client_key,
            requested,
            expires: now + RESIZE_REPLY_TIMEOUT,
        }
    }

    pub(super) fn client_key(&self) -> usize {
        self.client_key
    }

    /// When the reply gives up waiting.
    pub(super) fn deadline(&self) -> Instant {
        self.expires
    }

    /// The reply once the window is `current`: `Ok` when it is the requested
    /// size, an error naming `current` once the deadline has passed, and
    /// `None` to keep waiting.
    pub(super) fn reply(
        &self,
        current: PhysicalSize<u32>,
        now: Instant,
    ) -> Option<ControlResponse> {
        if current == self.requested {
            return Some(ControlResponse::Ok);
        }
        if now < self.expires {
            return None;
        }
        Some(ControlResponse::Error {
            code: RESIZE_NOT_APPLIED,
            message: format!(
                "Window resize request not applied by platform/compositor within {} s: requested {}x{}, current {}x{}",
                RESIZE_REPLY_TIMEOUT.as_secs(),
                self.requested.width,
                self.requested.height,
                current.width,
                current.height
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(now: Instant) -> PendingResize {
        PendingResize::new(7, PhysicalSize::new(1000, 700), now)
    }

    #[test]
    fn waits_while_the_window_has_another_size() {
        let now = Instant::now();
        let resize = pending(now);

        assert!(resize
            .reply(
                PhysicalSize::new(800, 600),
                now + Duration::from_millis(500)
            )
            .is_none());
        assert_eq!(resize.client_key(), 7);
        assert_eq!(resize.deadline(), now + RESIZE_REPLY_TIMEOUT);
    }

    #[test]
    fn answers_ok_when_the_window_reaches_the_requested_size() {
        let now = Instant::now();
        let resize = pending(now);

        assert!(matches!(
            resize.reply(PhysicalSize::new(1000, 700), now),
            Some(ControlResponse::Ok)
        ));
        // Reaching the size just as the deadline passes still succeeds.
        assert!(matches!(
            resize.reply(PhysicalSize::new(1000, 700), now + RESIZE_REPLY_TIMEOUT),
            Some(ControlResponse::Ok)
        ));
    }

    #[test]
    fn reports_the_actual_size_when_the_deadline_passes() {
        let now = Instant::now();
        let resize = pending(now);

        let reply = resize.reply(PhysicalSize::new(968, 690), now + RESIZE_REPLY_TIMEOUT);

        assert!(matches!(
            reply,
            Some(ControlResponse::Error {
                code: RESIZE_NOT_APPLIED,
                ref message
            }) if message.contains("requested 1000x700")
                && message.contains("current 968x690")
        ));
    }
}
