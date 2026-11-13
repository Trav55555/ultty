//! Application control event publication.

use super::App;
use ultty::control::events::pane_changes;
pub(super) use ultty::control::events::{control_event, open_panes};

impl App {
    /// Tell subscribers about panes opened or closed since the last call,
    /// whether by a key binding, a control request or a shell exiting.
    pub(super) fn publish_pane_changes(&mut self) {
        let now = open_panes(&self.runtime.manager);
        if now.len() == self.published_panes.len()
            && now
                .iter()
                .zip(&self.published_panes)
                .all(|((pane, _), seen)| pane == seen)
        {
            return;
        }
        let changes = pane_changes(&self.published_panes, &now);
        self.published_panes = now.into_iter().map(|(pane, _)| pane).collect();
        if let Some(server) = self.control_server.as_mut() {
            for event in &changes {
                server.publish(event);
            }
        }
    }
}
