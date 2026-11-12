//! Native sessions in panes (ADR 0025): each session is bound to the pane
//! whose PTY runs its app and kept at that pane's size.

use super::App;
use ultty::pane_layout::Rect;
use ultty::platform::socket::process_group;
use ultty::terminal_manager::TerminalId;
use ultty_protocol::{InputType, MouseButton};

impl App {
    /// Bind the sessions that have no pane yet to the pane running their
    /// app (the pane of their tab whose PTY has the peer's process group in
    /// the foreground; a one-pane tab's pane regardless), and size them to
    /// it. A session in no pane's foreground is tried again next time.
    pub(super) fn bind_native_sessions(&mut self) {
        let Some(server) = self.protocol_server.as_mut() else {
            return;
        };
        let terminal_manager = &self.terminal_manager;
        let bound = server.bind_unbound_sessions(|tab_id, peer_pid| {
            terminal_manager.pane_for_process_group(tab_id, peer_pid.and_then(process_group))
        });
        if bound > 0 {
            self.sync_native_viewports();
        }
    }

    /// Size the sessions to their panes if panes were laid out since the
    /// last time: one compare when nothing changed.
    pub(super) fn sync_native_viewports_if_panes_changed(&mut self) {
        if self.terminal_manager.layout_generation() != self.native_layout_generation {
            self.sync_native_viewports();
        }
    }

    /// Size every session to where it is drawn: a bound session to its
    /// pane's grid, an unbound one to the window's. Sessions whose size
    /// changed get `Resize` and `LayoutComputed`. First the sessions of
    /// panes that no longer exist are disconnected: every pane close lays
    /// panes out again, so this runs right after one.
    pub(super) fn sync_native_viewports(&mut self) {
        self.native_layout_generation = self.terminal_manager.layout_generation();
        let Some(server) = self.protocol_server.as_mut() else {
            return;
        };
        let window = self.renderer.terminal_dimensions();
        let terminal_manager = &self.terminal_manager;
        server.retain_pane_sessions(|pane| terminal_manager.terminal(pane).is_some());
        server.resize_sessions(window, |pane| terminal_manager.pane_grid_size(pane));
    }
}

/// The pane whose native sessions took a mouse press, and the button,
/// while that button is held (ADR 0025 §4): moves and the release go to
/// it, wherever the pointer is.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(super) struct MouseCapture {
    held: Option<(TerminalId, MouseButton)>,
}

impl MouseCapture {
    /// Where `event` at pixel `(x, y)` (below the tab bar) goes, among the
    /// shown panes `rects`, and the position in that pane's cells; `None`
    /// when it goes nowhere. Keeps the capture: a press captures its pane;
    /// the release of that button ends the capture wherever it lands, even
    /// over no pane; a capture whose pane is no longer shown (closed, or
    /// another tab in front) is dropped first, so it never routes to a
    /// pane that is gone or outlives the drag.
    pub(super) fn route(
        &mut self,
        event: &InputType,
        rects: &[(TerminalId, Rect)],
        at: (f32, f32),
        cell: (f32, f32),
    ) -> Option<(TerminalId, f32, f32)> {
        if let Some((pane, _)) = self.held {
            if !rects.iter().any(|(id, _)| *id == pane) {
                self.held = None;
            }
        }
        let captured = match event {
            InputType::MouseMove => self.held.map(|(pane, _)| pane),
            InputType::MouseUp { button } => match self.held {
                Some((pane, held)) if held == *button => {
                    self.held = None;
                    Some(pane)
                }
                _ => None,
            },
            _ => None,
        };
        let target = native_mouse_target(rects, captured, at, cell);
        if let (InputType::MouseDown { button }, Some((pane, _, _))) = (event, target) {
            self.held = Some((pane, *button));
        }
        target
    }

    /// Forget the capture: the window lost focus, so the release will
    /// never arrive.
    pub(super) fn clear(&mut self) {
        self.held = None;
    }
}

/// Where a mouse event at pixel `(x, y)` (below the tab bar) goes: the
/// pane under it, or, while a button is held, `captured`, the pane that
/// took the press, with the position clamped into it so a drag that leaves
/// the pane (or the window) still ends there. Returns the pane and the
/// position in its cells; `None` outside every pane with nothing captured.
pub(super) fn native_mouse_target(
    rects: &[(TerminalId, Rect)],
    captured: Option<TerminalId>,
    (x, y): (f32, f32),
    (cell_width, cell_height): (f32, f32),
) -> Option<(TerminalId, f32, f32)> {
    let (pane, (left, top, width, height)) = match captured {
        Some(captured) => rects.iter().copied().find(|(id, _)| *id == captured)?,
        None => rects
            .iter()
            .copied()
            .find(|(_, (px, py, pw, ph))| x >= *px && x < px + pw && y >= *py && y < py + ph)?,
    };
    let x = x.clamp(left, (left + width - 1.0).max(left));
    let y = y.clamp(top, (top + height - 1.0).max(top));
    let cell_x = ((x - left) / cell_width).floor().max(0.0);
    let cell_y = ((y - top) / cell_height).floor().max(0.0);
    Some((pane, cell_x, cell_y))
}

#[cfg(test)]
mod tests {
    use super::{native_mouse_target, MouseCapture};
    use ultty::terminal_manager::TerminalId;
    use ultty_protocol::{InputType, MouseButton};

    const CELL: (f32, f32) = (10.0, 20.0);

    /// Two panes side by side, 500 px wide each, 600 px tall.
    fn split() -> Vec<(TerminalId, (f32, f32, f32, f32))> {
        vec![
            (TerminalId::new(1), (0.0, 0.0, 500.0, 600.0)),
            (TerminalId::new(2), (500.0, 0.0, 500.0, 600.0)),
        ]
    }

    #[test]
    fn a_point_goes_to_the_pane_under_it_in_its_cells() {
        assert_eq!(
            native_mouse_target(&split(), None, (35.0, 45.0), CELL),
            Some((TerminalId::new(1), 3.0, 2.0))
        );
        assert_eq!(
            native_mouse_target(&split(), None, (535.0, 45.0), CELL),
            Some((TerminalId::new(2), 3.0, 2.0))
        );
        assert_eq!(
            native_mouse_target(&split(), None, (35.0, 700.0), CELL),
            None
        );
    }

    #[test]
    fn a_held_button_keeps_the_pane_that_took_the_press() {
        let captured = Some(TerminalId::new(1));
        // Dragged into the right pane: still the left one, at its edge.
        assert_eq!(
            native_mouse_target(&split(), captured, (535.0, 45.0), CELL),
            Some((TerminalId::new(1), 49.0, 2.0))
        );
        // Released outside the window: still delivered, clamped.
        assert_eq!(
            native_mouse_target(&split(), captured, (-20.0, 700.0), CELL),
            Some((TerminalId::new(1), 0.0, 29.0))
        );
        // A captured pane that closed: nothing.
        assert_eq!(
            native_mouse_target(&split(), Some(TerminalId::new(9)), (35.0, 45.0), CELL),
            None
        );
    }

    fn down(button: MouseButton) -> InputType {
        InputType::MouseDown { button }
    }

    fn up(button: MouseButton) -> InputType {
        InputType::MouseUp { button }
    }

    #[test]
    fn a_press_captures_its_pane_until_that_button_is_released() {
        let mut capture = MouseCapture::default();
        let rects = split();
        assert_eq!(
            capture.route(&down(MouseButton::Left), &rects, (35.0, 45.0), CELL),
            Some((TerminalId::new(1), 3.0, 2.0))
        );
        // Dragged into the right pane: still the left one.
        assert_eq!(
            capture.route(&InputType::MouseMove, &rects, (535.0, 45.0), CELL),
            Some((TerminalId::new(1), 49.0, 2.0))
        );
        // Another button's release does not end it.
        assert_eq!(
            capture.route(&up(MouseButton::Right), &rects, (535.0, 45.0), CELL),
            Some((TerminalId::new(2), 3.0, 2.0))
        );
        assert_eq!(
            capture.route(&InputType::MouseMove, &rects, (535.0, 45.0), CELL),
            Some((TerminalId::new(1), 49.0, 2.0))
        );
        // The press button's release, outside the window, ends it there.
        assert_eq!(
            capture.route(&up(MouseButton::Left), &rects, (-20.0, 700.0), CELL),
            Some((TerminalId::new(1), 0.0, 29.0))
        );
        assert_eq!(
            capture.route(&InputType::MouseMove, &rects, (535.0, 45.0), CELL),
            Some((TerminalId::new(2), 3.0, 2.0)),
            "after the release, hover follows the pointer again"
        );
    }

    #[test]
    fn a_release_over_no_pane_after_the_captured_pane_closed_still_ends_the_capture() {
        let mut capture = MouseCapture::default();
        capture.route(&down(MouseButton::Left), &split(), (35.0, 45.0), CELL);
        // The left pane closed (or another tab is shown): only the right
        // one remains, and the release lands outside every pane.
        let right_only = vec![split()[1]];
        assert_eq!(
            capture.route(&up(MouseButton::Left), &right_only, (35.0, 700.0), CELL),
            None
        );
        assert_eq!(
            capture.route(&InputType::MouseMove, &right_only, (535.0, 45.0), CELL),
            Some((TerminalId::new(2), 3.0, 2.0)),
            "hover is not stuck on the pane that is gone"
        );
    }

    #[test]
    fn a_move_after_the_captured_pane_closed_follows_the_pointer() {
        let mut capture = MouseCapture::default();
        capture.route(&down(MouseButton::Left), &split(), (35.0, 45.0), CELL);
        let right_only = vec![split()[1]];
        assert_eq!(
            capture.route(&InputType::MouseMove, &right_only, (535.0, 45.0), CELL),
            Some((TerminalId::new(2), 3.0, 2.0))
        );
    }

    #[test]
    fn losing_focus_ends_the_capture() {
        let mut capture = MouseCapture::default();
        capture.route(&down(MouseButton::Left), &split(), (35.0, 45.0), CELL);
        capture.clear();
        assert_eq!(
            capture.route(&InputType::MouseMove, &split(), (535.0, 45.0), CELL),
            Some((TerminalId::new(2), 3.0, 2.0))
        );
        assert_eq!(
            capture.route(&InputType::MouseMove, &split(), (35.0, 700.0), CELL),
            None
        );
    }
}
