//! Atomic viewer model replacement. The model owns no process or live native
//! session; closing or replacing it cannot terminate the owner's applications.

use ultty_protocol::control::{OwnerRef, PaneStatus};

use super::model::{SessionSnapshot, TabSnapshot};
use super::native::NativeReplica;
use crate::grid::Terminal;

/// A hydrated pane suitable for the existing GPU renderer and text selection.
pub struct PaneView {
    /// Stable pane identity.
    pub id: u64,
    /// Owning tab.
    pub tab: u64,
    /// Child identity retained across viewer reconnection.
    pub pid: u32,
    /// Current title.
    pub title: String,
    /// Projected program status.
    pub status: Option<PaneStatus>,
    /// Canonical cell rectangle, absent for zoom-hidden panes.
    pub rect: Option<[f32; 4]>,
    /// Viewer-local grid and selection model.
    pub terminal: Terminal,
    /// Committed, read-only native replicas.
    pub native: Vec<NativeReplica>,
}

/// The last complete snapshot accepted from one pinned owner.
#[derive(Default)]
pub struct SessionView {
    /// Lifetime identity, pinned on first valid replacement.
    pub owner: Option<OwnerRef>,
    /// Last accepted publication sequence.
    pub sequence: u64,
    /// Canonical viewport in cells. Observers never resize this.
    pub viewport: [u16; 2],
    /// Pixel size used by the owner for PTY queries and image reservation.
    pub cell_pixels: [u16; 2],
    /// Active tab in the owner.
    pub active_tab: Option<u64>,
    /// Tabs in display order.
    pub tabs: Vec<TabSnapshot>,
    /// All hydrated panes; hidden panes retain their state.
    pub panes: Vec<PaneView>,
}

impl SessionView {
    /// Validate and hydrate the entire replacement before changing this model.
    /// A malformed later pane, stale sequence or restarted owner leaves the last
    /// good view intact. Reconnection must explicitly retain this same owner.
    pub fn replace(&mut self, snapshot: SessionSnapshot) -> Result<(), String> {
        snapshot.validate()?;
        if self
            .owner
            .as_ref()
            .is_some_and(|owner| *owner != snapshot.owner)
        {
            return Err(
                "session owner changed; explicit attachment to the new owner is required".into(),
            );
        }
        if self.owner.is_some() && snapshot.sequence <= self.sequence {
            return Err("stale attachment publication sequence".into());
        }
        let mut panes = Vec::with_capacity(snapshot.panes.len());
        for pane in snapshot.panes {
            let mut terminal = pane.terminal.to_view()?;
            // Local scroll position belongs to the viewer. Keep the same
            // retained-history position while the owner appends new rows.
            if let Some(old) = self.panes.iter().find(|old| old.id == pane.id) {
                let previous = old.terminal.main_buffer();
                if previous.scroll_offset() > 0 {
                    let added = terminal
                        .main_buffer()
                        .history_len()
                        .saturating_sub(previous.history_len());
                    terminal.main_buffer_mut().scroll_to_bottom();
                    terminal
                        .main_buffer_mut()
                        .scroll_up(previous.scroll_offset().saturating_add(added));
                }
                if let (Some(selection), Some(text)) =
                    (old.terminal.selection(), old.terminal.selected_text())
                {
                    terminal.start_selection(selection.start, selection.mode);
                    terminal.update_selection(selection.end);
                    if terminal.selected_text().as_deref() != Some(text.as_str()) {
                        terminal.clear_selection();
                    }
                }
            }
            let native = pane
                .native
                .iter()
                .map(|native| native.to_view())
                .collect::<Result<_, _>>()?;
            panes.push(PaneView {
                id: pane.id,
                tab: pane.tab,
                pid: pane.pid,
                title: pane.title,
                status: pane.status,
                rect: pane.rect,
                terminal,
                native,
            });
        }
        self.owner = Some(snapshot.owner);
        self.sequence = snapshot.sequence;
        self.viewport = snapshot.viewport;
        self.cell_pixels = snapshot.cell_pixels;
        self.active_tab = snapshot.active_tab;
        self.tabs = snapshot.tabs;
        self.panes = panes;
        Ok(())
    }

    /// Pane receiving input in the selected tab.
    pub fn focused(&self) -> Option<&PaneView> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.active_tab)?;
        self.panes.iter().find(|pane| pane.id == tab.focused)
    }

    /// Mutable viewer-local model for scrolling and selection only.
    pub fn focused_mut(&mut self) -> Option<&mut PaneView> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.active_tab)?;
        self.panes.iter_mut().find(|pane| pane.id == tab.focused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attach::model::{PaneSnapshot, SNAPSHOT_VERSION};
    use crate::grid::{snapshot::TerminalSnapshot, Dimensions};

    fn snapshot(sequence: u64) -> SessionSnapshot {
        SessionSnapshot {
            version: SNAPSHOT_VERSION,
            owner: OwnerRef {
                pid: 1,
                epoch: "a".repeat(64),
            },
            sequence,
            viewport: [10, 2],
            cell_pixels: [8, 16],
            active_tab: Some(1),
            tabs: vec![TabSnapshot {
                id: 1,
                title: "test".into(),
                focused: 2,
                panes: vec![2],
            }],
            panes: vec![PaneSnapshot {
                id: 2,
                tab: 1,
                pid: 1234,
                revision: 0,
                title: "test".into(),
                status: None,
                rect: Some([0.0, 0.0, 10.0, 2.0]),
                terminal: TerminalSnapshot::capture(&Terminal::new(Dimensions::new(10, 2)))
                    .unwrap(),
                native: vec![],
            }],
        }
    }

    #[test]
    fn test_view_replacement_is_atomic_and_pins_owner() {
        let mut view = SessionView::default();
        view.replace(snapshot(1)).unwrap();
        assert_eq!(view.focused().unwrap().pid, 1234);
        let mut invalid = snapshot(2);
        invalid.panes[0].terminal.main.cols = 0;
        assert!(view.replace(invalid).is_err());
        assert_eq!(view.sequence, 1);
        let mut restarted = snapshot(2);
        restarted.owner.epoch = "b".repeat(64);
        assert!(view.replace(restarted).is_err());
        assert!(view.replace(snapshot(1)).is_err());
        view.replace(snapshot(2)).unwrap();
        assert_eq!(view.focused().unwrap().pid, 1234);
    }

    #[test]
    fn test_view_preserves_selection_only_while_selected_text_matches() {
        use crate::grid::{SelectionMode, SelectionPoint};
        let mut view = SessionView::default();
        let mut first = snapshot(1);
        let mut terminal = Terminal::new(Dimensions::new(10, 2));
        terminal.grid_mut().write_char('a');
        terminal.grid_mut().write_char('b');
        first.panes[0].terminal = TerminalSnapshot::capture(&terminal).unwrap();
        view.replace(first.clone()).unwrap();
        let pane = view.focused_mut().unwrap();
        pane.terminal
            .start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        pane.terminal.update_selection(SelectionPoint::new(1, 0));
        let selected = pane.terminal.selected_text().unwrap();
        first.sequence = 2;
        view.replace(first).unwrap();
        assert_eq!(
            view.focused().unwrap().terminal.selected_text(),
            Some(selected)
        );
        view.replace(snapshot(3)).unwrap();
        assert!(view.focused().unwrap().terminal.selection().is_none());
    }

    #[test]
    fn test_snapshot_rejects_aggregate_blank_cell_expansion_before_hydration() {
        let mut state = snapshot(1);
        let row = state.panes[0].terminal.main.rows[0].clone();
        state.panes[0].terminal.main.cols = 1000;
        state.panes[0].terminal.main.rows = vec![row; 1500];
        let mut second = state.panes[0].clone();
        second.id = 3;
        state.panes.push(second);
        state.tabs[0].panes.push(3);
        let mut view = SessionView::default();
        assert!(view
            .replace(state)
            .unwrap_err()
            .contains("aggregate cell limit"));
        assert!(view.owner.is_none());
    }
}
