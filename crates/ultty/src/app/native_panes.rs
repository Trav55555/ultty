//! Native sessions in panes (ADR 0025): each session is bound to the pane
//! whose PTY runs its app and kept at that pane's size.

use super::App;
use ultty::platform::socket::process_group;

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
    /// changed get `Resize` and `LayoutComputed`.
    pub(super) fn sync_native_viewports(&mut self) {
        self.native_layout_generation = self.terminal_manager.layout_generation();
        let Some(server) = self.protocol_server.as_mut() else {
            return;
        };
        let window = self.renderer.terminal_dimensions();
        let terminal_manager = &self.terminal_manager;
        server.resize_sessions(window, |pane| terminal_manager.pane_grid_size(pane));
    }
}
