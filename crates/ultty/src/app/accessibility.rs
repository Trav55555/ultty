//! What the window shows assistive technology (ADR 0021 §2, §3): the tab
//! bar, the active tab's panes and the focused pane's screen, gathered into
//! a [`Snapshot`] for `ultty::a11y`.
//!
//! None of it runs unless a screen reader is listening; see
//! [`App::update_accessibility`].

use std::hash::{Hash, Hasher};
use std::time::Instant;

use accesskit::Rect;
use ultty::a11y::tree::{GridSnapshot, PaneSnapshot, Snapshot, TabSnapshot};
use ultty::terminal_manager::ManagedTerminal;

use super::App;

impl App {
    /// Tell the accessibility adapter what changed this pass and send it a
    /// tree if one is due. `screen_changed` is output in the focused pane or
    /// a new title from any pane.
    ///
    /// Inactive (no screen reader) this is one check: nothing is hashed,
    /// locked or copied.
    pub(super) fn update_accessibility(&mut self, screen_changed: bool, now: Instant) {
        if !self.accessibility.as_ref().is_some_and(|a| a.is_active()) {
            return;
        }
        let digest = self.shown_digest();
        let Some(adapter) = self.accessibility.as_mut() else {
            return;
        };
        if screen_changed {
            adapter.changed();
        }
        adapter.observe_shown(digest);
        let snapshot = || {
            accessibility_snapshot(
                &self.terminal_manager,
                &self.tab_bar.tabs,
                self.renderer.surface_size(),
                self.renderer.tab_bar_height(),
                self.renderer.cell_dimensions(),
            )
        };
        adapter.update(now, snapshot);
    }

    /// A digest of what the tree shows besides screen text and titles,
    /// which arrive as events: the tabs, their bar titles, the active tab,
    /// its panes and their rectangles (so dragging a divider counts), the
    /// focused pane and the cell size. Hashing it each pass is cheaper than
    /// finding every place these change. The window size is the renderer's,
    /// kept from the last resize, not asked of the window system.
    fn shown_digest(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for tab in &self.tab_bar.tabs {
            (tab.id.raw(), &tab.title, tab.is_active).hash(&mut hasher);
        }
        if let Some(tab) = self.terminal_manager.active_tab() {
            tab.id.raw().hash(&mut hasher);
            for terminal in &tab.terminals {
                terminal.raw().hash(&mut hasher);
            }
            tab.focused_terminal.raw().hash(&mut hasher);
        }
        let (width, height) = self.renderer.surface_size();
        let top = self.renderer.tab_bar_height();
        for (id, (x, y, w, h)) in self
            .terminal_manager
            .active_pane_rects(width as f32, height as f32 - top)
        {
            id.raw().hash(&mut hasher);
            for value in [x, y, w, h] {
                value.to_bits().hash(&mut hasher);
            }
        }
        let (cell_width, cell_height) = self.renderer.cell_dimensions();
        for value in [cell_width, cell_height, top] {
            value.to_bits().hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// The tabs from the tab bar (its titles are the ones drawn), and the
/// active tab's panes in window pixels with the focused one's screen.
///
/// Each pane is locked once, briefly: the focused one to copy its rows,
/// the others to read their titles.
fn accessibility_snapshot(
    terminal_manager: &ultty::terminal_manager::TerminalManager,
    bar: &[ultty::tabs::TabState],
    (window_width, window_height): (u32, u32),
    tab_bar_height: f32,
    (cell_width, cell_height): (f32, f32),
) -> Snapshot {
    let tabs = bar
        .iter()
        .map(|tab| TabSnapshot {
            id: tab.id.raw(),
            title: tab.title.clone(),
            selected: tab.is_active,
        })
        .collect();
    let focused = terminal_manager.focused_terminal_id();
    let panes = terminal_manager
        .active_pane_rects(window_width as f32, window_height as f32 - tab_bar_height)
        .into_iter()
        .filter_map(|(id, (x, y, width, height))| {
            let managed = terminal_manager.terminal(id)?;
            let bounds = Rect {
                x0: f64::from(x),
                y0: f64::from(y + tab_bar_height),
                x1: f64::from(x + width),
                y1: f64::from(y + tab_bar_height + height),
            };
            let (title, grid) = {
                let terminal = managed.event_loop.terminal();
                let grid = (Some(id) == focused).then(|| {
                    GridSnapshot::capture(
                        terminal.grid(),
                        f64::from(cell_width),
                        f64::from(cell_height),
                    )
                });
                (pane_title(managed, terminal.title()), grid)
            };
            Some(PaneSnapshot {
                id: id.raw(),
                title,
                bounds,
                grid,
            })
        })
        .collect();
    Snapshot { tabs, panes }
}

/// The title the pane's program set, else its default ("Terminal 3").
fn pane_title(managed: &ManagedTerminal, set: &str) -> String {
    let set = set.trim();
    if set.is_empty() {
        managed.title.clone()
    } else {
        set.to_owned()
    }
}
