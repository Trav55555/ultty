//! Application control event publication.

use super::ProcessState;
use ultty::control::events::pane_changes;
pub(super) use ultty::control::events::{control_event, open_panes};

impl ProcessState {
    /// Tell subscribers about panes opened or closed since the last call,
    /// whether by a key binding, a control request or a shell exiting.
    pub(super) fn publish_pane_changes(&mut self) {
        let now: Vec<_> = open_panes(&self.runtime.manager)
            .into_iter()
            .map(|(pane, tab_index, window)| (pane, tab_index, Some(window.raw() as u64)))
            .collect();
        if now.len() == self.published_panes.len()
            && now.iter().zip(&self.published_panes).all(
                |((pane, _, window), (seen_pane, seen_window))| {
                    pane == seen_pane && window == seen_window
                },
            )
        {
            return;
        }
        let changes = pane_changes(&self.published_panes, &now);
        self.published_panes = now
            .into_iter()
            .map(|(pane, _, window)| (pane, window))
            .collect();
        if let Some(server) = self.control_server.as_mut() {
            for event in &changes {
                server.publish(event);
            }
        }
    }
}
