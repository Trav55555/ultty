//! When to ask the window system for a frame, and whether to draw one when
//! it says to.
//!
//! Every frame is drawn from `WindowEvent::RedrawRequested`. The update loop
//! decides that a frame is due (`update::should_render`: the 16 ms frame
//! interval, the input echo, synchronized updates) and asks for a redraw;
//! winit delivers it when the window system is ready. On Wayland that is
//! when the compositor's frame callback for the last frame has arrived, so
//! frames follow the compositor's pace and stop while it withholds callbacks
//! (a hidden or minimized window), and presenting never waits on a callback
//! that may not come. On X11, which has no frame callbacks, winit delivers
//! the request on the same loop iteration, so pacing is the frame interval
//! alone, as before.
//!
//! Because the callbacks pace frames on Wayland, the surface presents in
//! Mailbox there rather than FIFO (`renderer::present_mode`): a FIFO present
//! could otherwise be held a further frame by the driver's own throttling.
//!
//! An occluded window draws nothing and asks for nothing until it is shown
//! again; output is still parsed meanwhile.

/// Redraw requests in flight and the window's occlusion.
#[derive(Debug, Default)]
pub(super) struct FramePacing {
    /// A redraw has been requested and `RedrawRequested` has not arrived.
    requested: bool,
    /// The window system reported the window fully hidden.
    occluded: bool,
}

impl FramePacing {
    /// Whether to call `Window::request_redraw` now, given whether a frame
    /// is `due`. Asks once per frame: a request already in flight (on
    /// Wayland, held until the frame callback) covers any later output.
    pub(super) fn should_request(&mut self, due: bool) -> bool {
        if due && !self.requested && !self.occluded {
            self.requested = true;
            true
        } else {
            false
        }
    }

    /// `RedrawRequested` arrived: whether to draw now, given whether a frame
    /// is `due`. A redraw the window system asked for on its own, before the
    /// frame interval is up, draws nothing; the loop asks again when it is.
    pub(super) fn should_draw(&mut self, due: bool) -> bool {
        self.requested = false;
        due && !self.occluded
    }

    /// Record the window's occlusion. Returns true when the window has just
    /// been shown again and must draw the content it missed.
    pub(super) fn set_occluded(&mut self, occluded: bool) -> bool {
        let shown = self.occluded && !occluded;
        self.occluded = occluded;
        shown
    }

    /// Whether a pending frame is up to the loop's own frame timer. Not
    /// while a redraw request is in flight (its `RedrawRequested` wakes the
    /// loop) or the window is occluded: polling then would spin for as long
    /// as the compositor withholds frames.
    pub(super) fn frame_timer_applies(&self) -> bool {
        !self.requested && !self.occluded
    }
}

#[cfg(test)]
mod tests {
    use super::FramePacing;

    #[test]
    fn a_due_frame_is_requested_once_until_it_is_delivered() {
        let mut pacing = FramePacing::default();
        assert!(!pacing.should_request(false));
        assert!(pacing.should_request(true));
        // Output keeps arriving while the compositor holds the request.
        assert!(!pacing.should_request(true));
        assert!(pacing.should_draw(true));
        assert!(pacing.should_request(true));
    }

    #[test]
    fn a_pending_request_leaves_the_frame_to_the_window_system() {
        let mut pacing = FramePacing::default();
        assert!(pacing.frame_timer_applies());
        pacing.should_request(true);
        assert!(!pacing.frame_timer_applies());
        pacing.should_draw(true);
        assert!(pacing.frame_timer_applies());
    }

    #[test]
    fn an_unrequested_redraw_before_the_frame_is_due_draws_nothing() {
        let mut pacing = FramePacing::default();
        assert!(!pacing.should_draw(false));
        assert!(pacing.should_draw(true));
    }

    #[test]
    fn a_redraw_that_draws_nothing_lets_the_loop_ask_again() {
        let mut pacing = FramePacing::default();
        pacing.should_request(true);
        // A synchronized update began before the redraw was delivered.
        assert!(!pacing.should_draw(false));
        assert!(pacing.frame_timer_applies());
        assert!(pacing.should_request(true));
    }

    #[test]
    fn an_occluded_window_neither_requests_nor_draws_nor_polls() {
        let mut pacing = FramePacing::default();
        assert!(!pacing.set_occluded(true));
        assert!(!pacing.should_request(true));
        assert!(!pacing.should_draw(true));
        assert!(!pacing.frame_timer_applies());
    }

    #[test]
    fn showing_an_occluded_window_draws_once_more() {
        let mut pacing = FramePacing::default();
        pacing.set_occluded(true);
        assert!(pacing.set_occluded(false));
        assert!(!pacing.set_occluded(false));
        assert!(pacing.should_request(true));
        assert!(pacing.should_draw(true));
    }
}
