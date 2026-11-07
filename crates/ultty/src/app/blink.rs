//! The cursor's blink phase, and when it next changes.

use std::time::{Duration, Instant};

/// How long the cursor stays on, and then off, while it blinks.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Blink phase of the focused pane's cursor.
///
/// The phase only runs while the cursor actually blinks. A steady cursor
/// has no next toggle, so it never wakes the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CursorBlink {
    /// Whether the cursor is lit in the current phase.
    visible: bool,
    /// When the current phase started; `None` while the cursor is steady.
    phase_start: Option<Instant>,
}

impl CursorBlink {
    pub(super) fn new() -> Self {
        Self {
            visible: true,
            phase_start: None,
        }
    }

    /// Whether the cursor is lit now.
    pub(super) fn visible(&self) -> bool {
        self.visible
    }

    /// Light the cursor and start a full on phase (on a key press).
    pub(super) fn restart(&mut self, now: Instant) {
        self.visible = true;
        if self.phase_start.is_some() {
            self.phase_start = Some(now);
        }
    }

    /// Advance the phase to `now`. `blinks` is whether the cursor should be
    /// blinking at all. Returns true when what is drawn changed.
    pub(super) fn update(&mut self, blinks: bool, now: Instant) -> bool {
        match (blinks, self.phase_start) {
            (false, _) => {
                self.phase_start = None;
                let was_hidden = !self.visible;
                self.visible = true;
                was_hidden
            }
            (true, None) => {
                self.phase_start = Some(now);
                false
            }
            (true, Some(start))
                if now.saturating_duration_since(start) >= CURSOR_BLINK_INTERVAL =>
            {
                self.visible = !self.visible;
                self.phase_start = Some(now);
                true
            }
            (true, Some(_)) => false,
        }
    }

    /// When the phase next flips, or `None` for a steady cursor.
    pub(super) fn next_toggle(&self) -> Option<Instant> {
        self.phase_start.map(|start| start + CURSOR_BLINK_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_cursor_never_schedules_a_toggle() {
        let now = Instant::now();
        let mut blink = CursorBlink::new();
        assert!(!blink.update(false, now));
        assert!(!blink.update(false, now + CURSOR_BLINK_INTERVAL * 3));
        assert!(blink.visible());
        assert_eq!(blink.next_toggle(), None);
    }

    #[test]
    fn blinking_cursor_toggles_every_interval() {
        let now = Instant::now();
        let mut blink = CursorBlink::new();
        assert!(!blink.update(true, now));
        assert_eq!(blink.next_toggle(), Some(now + CURSOR_BLINK_INTERVAL));

        let later = now + CURSOR_BLINK_INTERVAL;
        assert!(blink.update(true, later));
        assert!(!blink.visible());
        assert_eq!(blink.next_toggle(), Some(later + CURSOR_BLINK_INTERVAL));
    }

    #[test]
    fn stopping_the_blink_lights_the_cursor_and_drops_the_timer() {
        let now = Instant::now();
        let mut blink = CursorBlink::new();
        blink.update(true, now);
        blink.update(true, now + CURSOR_BLINK_INTERVAL);
        assert!(!blink.visible());

        assert!(blink.update(false, now + CURSOR_BLINK_INTERVAL * 2));
        assert!(blink.visible());
        assert_eq!(blink.next_toggle(), None);
    }

    #[test]
    fn restart_lights_the_cursor_for_a_full_phase() {
        let now = Instant::now();
        let mut blink = CursorBlink::new();
        blink.update(true, now);
        blink.update(true, now + CURSOR_BLINK_INTERVAL);

        let key = now + CURSOR_BLINK_INTERVAL + Duration::from_millis(100);
        blink.restart(key);
        assert!(blink.visible());
        assert_eq!(blink.next_toggle(), Some(key + CURSOR_BLINK_INTERVAL));
    }

    #[test]
    fn restart_of_a_steady_cursor_starts_no_timer() {
        let mut blink = CursorBlink::new();
        blink.restart(Instant::now());
        assert_eq!(blink.next_toggle(), None);
    }
}
