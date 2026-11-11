//! Native sessions in panes (ADR 0025): each session is bound to the pane
//! whose PTY runs its app and kept at that pane's size.

use super::App;
use ultty::pane_layout::Rect;
use ultty::platform::socket::process_group;
use ultty::terminal_manager::TerminalId;

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
    use super::native_mouse_target;
    use ultty::terminal_manager::TerminalId;

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
}
