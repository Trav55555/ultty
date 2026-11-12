//! What the window shows assistive technology (ADR 0021 §2, §3, §6): the
//! tab bar, the active tab's panes, and the focused pane's screen or the
//! native app's scene drawn over it, with the inline surfaces on that
//! screen, gathered into a [`Snapshot`] for `ultty::a11y`.
//!
//! None of it runs unless a screen reader is listening; see
//! [`App::update_accessibility`].

use std::hash::{Hash, Hasher};
use std::time::Instant;

use accesskit::Rect;
use ultty::a11y::scene::{Placement, SceneSnapshot};
use ultty::a11y::tree::{GridSnapshot, PaneSnapshot, Snapshot, SurfaceSnapshot, TabSnapshot};
use ultty::protocol::session::LiveChange;
use ultty::protocol::{NativeSession, ProtocolServer};
use ultty::terminal_manager::ManagedTerminal;
use ultty_protocol::RegionId;

use super::App;

impl App {
    /// Tell the accessibility adapter what changed this pass, including
    /// text appended to live regions, and send it a tree if one is due.
    /// `screen_changed` is output in the focused pane or a new title from
    /// any pane.
    ///
    /// Inactive (no screen reader) this is two checks: nothing is hashed,
    /// locked, kept or copied.
    pub(super) fn update_accessibility(&mut self, screen_changed: bool, now: Instant) {
        let active = self.accessibility.as_ref().is_some_and(|a| a.is_active());
        if let Some(server) = self.protocol_server.as_mut() {
            server.set_live_announcements(active);
        }
        if !active {
            return;
        }
        let digest = self.shown_digest();
        let Some(adapter) = self.accessibility.as_mut() else {
            return;
        };
        // A scene is shown once its session is bound to a pane of the
        // visible tab (ADR 0025; see `accessibility_snapshot`); text for a
        // scene that is not shown is dropped rather than flushed into a
        // tree that leaves it out.
        if let Some(server) = self.protocol_server.as_mut() {
            let shown: std::collections::HashSet<u64> = server
                .active_tab_sessions()
                .filter(|session| session.pane().is_some())
                .map(|session| session.session_id())
                .collect();
            server.take_live_appends(|session, change| match change {
                LiveChange::Appended { region, live, text } if shown.contains(&session) => {
                    adapter.appended(session, region.0, live, &text, now);
                }
                LiveChange::Appended { .. } => {}
                LiveChange::Removed(region) => adapter.removed(session, region.0),
            });
        }
        if screen_changed {
            adapter.changed();
        }
        adapter.observe_shown(digest);
        let snapshot = || {
            accessibility_snapshot(
                &self.terminal_manager,
                &self.tab_bar.tabs,
                self.protocol_server.as_ref(),
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
    /// focused pane, the cell size, whether the focused pane is scrolled
    /// back, and the active tab's native sessions with their
    /// [`NativeSession::shown_generation`] (frames, focus, surfaces).
    /// Hashing it each pass is cheaper than finding every place these
    /// change. The window size is the renderer's, kept from the last
    /// resize, not asked of the window system.
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
        if let Some(terminal) = self.terminal_manager.focused_terminal() {
            scrolled_back(&terminal.event_loop.terminal()).hash(&mut hasher);
        }
        if let Some(server) = &self.protocol_server {
            let mut sessions: Vec<(u64, u64)> = server
                .active_tab_sessions()
                .map(|session| (session.session_id(), session.shown_generation()))
                .collect();
            sessions.sort_unstable();
            sessions.hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// The tabs from the tab bar (its titles are the ones drawn), and the
/// active tab's panes in window pixels with the focused one's screen, the
/// full scenes of the sessions bound to each pane (ADR 0025), and, when the
/// tab has one pane, the inline surfaces drawn over its screen: the
/// renderer draws those only then (spec §10.6).
///
/// Each pane is locked once, briefly: the focused one to copy its rows and
/// find its surfaces, the others to read their titles. Native scenes are
/// copied after the lock is released. While the focused pane is scrolled
/// back its surfaces are left out: the rows are the live screen
/// (scrollback is deferred, ADR 0021 §2), so their anchors would be
/// misplaced.
fn accessibility_snapshot(
    terminal_manager: &ultty::terminal_manager::TerminalManager,
    bar: &[ultty::tabs::TabState],
    server: Option<&ProtocolServer>,
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
    let rects = terminal_manager
        .active_pane_rects(window_width as f32, window_height as f32 - tab_bar_height);
    // The shown sessions, in a stable order; each pane takes the ones bound
    // to it. Surfaces are drawn only in a one-pane tab.
    let mut sessions: Vec<&NativeSession> = server
        .map(|server| server.active_tab_sessions().collect())
        .unwrap_or_default();
    sessions.sort_by_key(|session| session.session_id());
    let cell = (f64::from(cell_width), f64::from(cell_height));
    let panes = rects
        .into_iter()
        .filter_map(|(id, (x, y, width, height))| {
            let managed = terminal_manager.terminal(id)?;
            let sessions: Vec<&NativeSession> = sessions
                .iter()
                .copied()
                .filter(|session| session.pane() == Some(id))
                .collect();
            let full_scene = sessions.iter().any(|session| session.is_full_scene());
            let bounds = Rect {
                x0: f64::from(x),
                y0: f64::from(y + tab_bar_height),
                x1: f64::from(x + width),
                y1: f64::from(y + tab_bar_height + height),
            };
            let is_focused = Some(id) == focused;
            let at = |y0: f64| Placement {
                x: bounds.x0,
                y: y0,
                cell_width: cell.0,
                cell_height: cell.1,
            };
            let mut on_screen = Vec::new();
            let (title, grid) = {
                let terminal = managed.event_loop.terminal();
                let grid = (is_focused && !full_scene).then(|| {
                    if !scrolled_back(&terminal) {
                        on_screen = ultty::protocol::pairing::surfaces_on_screen(
                            &terminal,
                            sessions.iter().copied(),
                        );
                    }
                    GridSnapshot::capture(terminal.grid(), cell.0, cell.1)
                });
                (pane_title(managed, terminal.title()), grid)
            };
            // The terminal is unlocked; now copy the scenes: the renderer
            // draws every bound full-scene session's ROOT tree over the
            // pane, hiding its grid.
            let scenes = sessions
                .iter()
                .filter(|session| session.is_full_scene())
                .map(|session| capture(session, RegionId::ROOT, at(bounds.y0)))
                .collect();
            let surfaces = on_screen
                .into_iter()
                .filter_map(|surface| {
                    let session = sessions
                        .iter()
                        .find(|session| session.session_id() == surface.session_id)?;
                    let top = bounds.y0 + surface.row as f64 * cell.1;
                    Some(SurfaceSnapshot {
                        row: surface.row,
                        scene: capture(session, surface.id, at(top)),
                    })
                })
                .collect();
            Some(PaneSnapshot {
                id: id.raw(),
                title,
                focused: is_focused,
                bounds,
                grid,
                scenes,
                surfaces,
            })
        })
        .collect();
    Snapshot { tabs, panes }
}

/// Whether `terminal`'s view is scrolled back into its history.
fn scrolled_back(terminal: &ultty::grid::Terminal) -> bool {
    terminal.main_buffer().scroll_offset() != 0
}

/// The region tree under `root` of `session`, drawn at `at`.
fn capture(session: &NativeSession, root: RegionId, at: Placement) -> SceneSnapshot {
    SceneSnapshot::capture(
        session.scene(),
        root,
        session.session_id(),
        session.focused_region(),
        |id| session.scroll_offset(id),
        at,
    )
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
