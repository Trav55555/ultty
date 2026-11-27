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
use ultty::a11y::tree::{
    GridSnapshot, PaletteItemSnapshot, PaletteSnapshot, PaneSnapshot, Snapshot, SurfaceSnapshot,
    TabSnapshot,
};
use ultty::command_palette::CommandPalette;
use ultty::protocol::session::LiveChange;
use ultty::protocol::{NativeSession, ProtocolServer};
use ultty::terminal_manager::{ManagedTerminal, WindowKey};
use ultty_protocol::RegionId;

use super::{App, WindowCtx};

impl App {
    /// Update every open window's accessibility adapter when assistive
    /// technology is active.
    pub(super) fn update_all_accessibility(
        &mut self,
        changed_windows: &std::collections::HashSet<WindowKey>,
        now: Instant,
    ) {
        let any_active = self
            .windows
            .values()
            .any(|win| win.accessibility.as_ref().is_some_and(|a| a.is_active()));
        if let Some(server) = self.shared.runtime.protocol.as_mut() {
            server.set_live_announcements(any_active);
        }
        if !any_active {
            return;
        }
        let mut appends_by_window: std::collections::HashMap<WindowKey, Vec<(u64, LiveChange)>> =
            std::collections::HashMap::new();
        if let Some(server) = self.shared.runtime.protocol.as_mut() {
            let shown: std::collections::HashSet<(ultty::terminal_manager::TabId, u64)> = server
                .visible_tab_sessions()
                .filter(|session| session.pane().is_some())
                .map(|session| (session.tab_id(), session.session_id()))
                .collect();
            server.take_visible_live_appends(|tab, session, change| {
                if let Some(window) = self.shared.runtime.manager.window_for_tab(tab) {
                    match change {
                        LiveChange::Appended { .. } if shown.contains(&(tab, session)) => {
                            appends_by_window
                                .entry(window)
                                .or_default()
                                .push((session, change));
                        }
                        LiveChange::Appended { .. } => {}
                        LiveChange::Removed(_) => {
                            appends_by_window
                                .entry(window)
                                .or_default()
                                .push((session, change));
                        }
                    }
                }
            });
        }
        for (&key, win) in &mut self.windows {
            let mut ctx = WindowCtx {
                app: &mut self.shared,
                win,
            };
            let changes = appends_by_window.remove(&key).unwrap_or_default();
            ctx.update_accessibility(changed_windows.contains(&key), changes, now);
        }
    }
}

impl WindowCtx<'_> {
    /// Route a request against the last published accessibility tree through
    /// the same application acknowledgement path as named control actions.
    pub(super) fn handle_accessibility_event(&mut self, event: accesskit_winit::WindowEvent) {
        let action = self.win.window.as_winit().and_then(|winit_window| {
            self.win
                .accessibility
                .as_mut()
                .and_then(|adapter| adapter.handle(winit_window, event))
        });
        let Some((target, action)) = action else {
            return;
        };
        let active_tab = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id);
        let pane = active_tab.and_then(|tab| {
            self.app.runtime.protocol.as_ref().and_then(|server| {
                server
                    .tab_sessions(tab)
                    .find(|session| session.session_id() == target.session)
                    .and_then(|session| session.pane())
            })
        });
        if let Some(pane) = pane {
            self.app.dispatch_accessibility_action(
                pane.raw() as u64,
                target.session,
                target.generation,
                target.region,
                target.widget_id,
                action,
            );
        }
    }

    /// Tell this window's accessibility adapter what changed this pass,
    /// including text appended to live regions, and send a tree if one is due.
    pub(super) fn update_accessibility(
        &mut self,
        screen_changed: bool,
        live_changes: Vec<(u64, LiveChange)>,
        now: Instant,
    ) {
        if !self
            .win
            .accessibility
            .as_ref()
            .is_some_and(|a| a.is_active())
        {
            return;
        }
        let digest = self.shown_digest();
        let Some(adapter) = self.win.accessibility.as_mut() else {
            return;
        };
        for (session, change) in live_changes {
            match change {
                LiveChange::Appended { region, live, text } => {
                    adapter.appended(session, region.0, live, &text, now);
                }
                LiveChange::Removed(region) => adapter.removed(session, region.0),
            }
        }
        if screen_changed {
            adapter.changed();
        }
        adapter.observe_shown(digest);
        let key = self.win.key;
        let snapshot = || {
            accessibility_snapshot(
                &self.app.runtime.manager,
                key,
                &self.win.tab_bar.tabs,
                self.app.runtime.protocol.as_ref(),
                self.win.renderer.surface_size(),
                self.win.renderer.tab_bar_height(),
                self.win.renderer.cell_dimensions(),
                &self.win.command_palette,
            )
        };
        adapter.update(now, snapshot);
    }

    fn shown_digest(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for tab in &self.win.tab_bar.tabs {
            (tab.id.raw(), &tab.title, tab.is_active).hash(&mut hasher);
        }
        let active_tab = self.app.runtime.manager.window_active_tab(self.win.key);
        if let Some(tab) = active_tab {
            tab.id.raw().hash(&mut hasher);
            for terminal in &tab.terminals {
                terminal.raw().hash(&mut hasher);
            }
            tab.focused_terminal.raw().hash(&mut hasher);
        }
        let (width, height) = self.win.renderer.surface_size();
        let top = self.win.renderer.tab_bar_height();
        for (id, (x, y, w, h)) in self.app.runtime.manager.window_active_pane_rects(
            self.win.key,
            width as f32,
            height as f32 - top,
        ) {
            id.raw().hash(&mut hasher);
            for value in [x, y, w, h] {
                value.to_bits().hash(&mut hasher);
            }
        }
        let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
        for value in [cell_width, cell_height, top] {
            value.to_bits().hash(&mut hasher);
        }
        if let Some(terminal) = self
            .app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)
        {
            scrolled_back(&terminal.event_loop.terminal()).hash(&mut hasher);
        }
        if self.win.command_palette.is_open() {
            self.win.command_palette.query().hash(&mut hasher);
            self.win.command_palette.selected().hash(&mut hasher);
            self.win.command_palette.rows().len().hash(&mut hasher);
        }
        if let (Some(server), Some(tab)) = (&self.app.runtime.protocol, active_tab) {
            let mut sessions: Vec<(u64, u64)> = server
                .tab_sessions(tab.id)
                .map(|session| (session.session_id(), session.shown_generation()))
                .collect();
            sessions.sort_unstable();
            sessions.hash(&mut hasher);
        }
        hasher.finish()
    }
}

#[allow(clippy::too_many_arguments)]
fn accessibility_snapshot(
    terminal_manager: &ultty::terminal_manager::TerminalManager,
    window: WindowKey,
    bar: &[ultty::tabs::TabState],
    server: Option<&ProtocolServer>,
    (window_width, window_height): (u32, u32),
    tab_bar_height: f32,
    (cell_width, cell_height): (f32, f32),
    palette: &CommandPalette,
) -> Snapshot {
    let tabs = bar
        .iter()
        .map(|tab| TabSnapshot {
            id: tab.id.raw(),
            title: tab.title.clone(),
            selected: tab.is_active,
        })
        .collect();
    let focused = terminal_manager.window_focused_terminal_id(window);
    let active_tab = terminal_manager.window_active_tab(window).map(|t| t.id);
    let rects = terminal_manager.window_active_pane_rects(
        window,
        window_width as f32,
        window_height as f32 - tab_bar_height,
    );
    let mut sessions: Vec<&NativeSession> = match (server, active_tab) {
        (Some(server), Some(tab)) => server.tab_sessions(tab).collect(),
        _ => Vec::new(),
    };
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
    Snapshot {
        tabs,
        panes,
        palette: palette_snapshot(palette),
    }
}

/// The palette's query and listed actions, while it is open.
fn palette_snapshot(palette: &CommandPalette) -> Option<PaletteSnapshot> {
    if !palette.is_open() {
        return None;
    }
    let items: Vec<PaletteItemSnapshot> = palette
        .rows()
        .map(|row| PaletteItemSnapshot {
            label: row.entry.label.to_string(),
            key: row.entry.key.clone(),
        })
        .collect();
    let selected = (!items.is_empty()).then(|| palette.selected());
    Some(PaletteSnapshot {
        query: palette.query().to_owned(),
        items,
        selected,
    })
}

/// Whether `terminal`'s view is scrolled back into its history.
fn scrolled_back(terminal: &ultty::grid::Terminal) -> bool {
    terminal.main_buffer().scroll_offset() != 0
}

/// The region tree under `root` of `session`, drawn at `at`.
fn capture(session: &NativeSession, root: RegionId, at: Placement) -> SceneSnapshot {
    let snapshot = SceneSnapshot::capture(
        session.scene(),
        root,
        session.session_id(),
        session.focused_region(),
        |id| session.scroll_offset(id),
        at,
    );
    if session.capabilities() & ultty_protocol::CAP_SEMANTIC_ACTIONS != 0 {
        snapshot.with_actions(session.scene(), session.shown_generation())
    } else {
        snapshot
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit::{NodeId, Role, TreeId};
    use ultty::a11y::tree::{build, pane_node_id, tab_node_id};
    use ultty::tabs::TabBarState;
    use ultty::terminal_manager::{no_tab_env, TerminalManager};

    struct RecordedChanges {
        added: Vec<NodeId>,
        removed: Vec<NodeId>,
    }

    impl accesskit_consumer::TreeChangeHandler for RecordedChanges {
        fn node_added(&mut self, node: &accesskit_consumer::NodeRef) {
            if let Some((local_id, _)) = node.tree_state.locate_node(node.id()) {
                self.added.push(local_id);
            }
        }

        fn node_updated(
            &mut self,
            _old: &accesskit_consumer::NodeRef,
            _new: &accesskit_consumer::NodeRef,
        ) {
        }

        fn focus_moved(
            &mut self,
            _old: Option<&accesskit_consumer::NodeRef>,
            _new: Option<&accesskit_consumer::NodeRef>,
        ) {
        }

        fn node_removed(&mut self, node: &accesskit_consumer::NodeRef) {
            if let Some((local_id, _)) = node.tree_state.locate_node(node.id()) {
                self.removed.push(local_id);
            }
        }
    }

    fn window_snapshot(manager: &TerminalManager, window: WindowKey) -> Snapshot {
        let mut bar = TabBarState::new();
        let tabs = manager.window_tabs(window);
        if let Some(active) = manager.window_active_tab(window) {
            let titles: Vec<String> = tabs
                .iter()
                .map(|tab| manager.tab_display_title(tab.id))
                .collect();
            bar.update_with_tab_refs_and_width_override(&tabs, &titles, active.id, 800.0, None);
        }
        accessibility_snapshot(
            manager,
            window,
            &bar.tabs,
            None,
            (800, 600),
            bar.content_top(),
            (10.0, 20.0),
            &CommandPalette::default(),
        )
    }

    #[test]
    fn moving_a_tab_updates_both_windows_accesskit_trees() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let tab_a = manager.tabs()[0].id;
        let tab_b = manager
            .create_tab_in_window(WindowKey::PRIMARY, &mut no_tab_env)
            .unwrap();
        let (win1, tab_c) = manager.create_window(80, 24, &mut no_tab_env).unwrap();
        let moved_pane = manager
            .tabs()
            .iter()
            .find(|t| t.id == tab_b)
            .unwrap()
            .focused_terminal;

        let mut tree_w0 = accesskit_consumer::Tree::new(
            build("Ultty", &window_snapshot(&manager, WindowKey::PRIMARY)),
            true,
        );
        let mut tree_w1 =
            accesskit_consumer::Tree::new(build("Ultty", &window_snapshot(&manager, win1)), true);

        assert!(tree_w0
            .state()
            .node_by_tree_local_id(tab_node_id(tab_a.raw()), TreeId::ROOT)
            .is_some());
        assert!(tree_w0
            .state()
            .node_by_tree_local_id(tab_node_id(tab_b.raw()), TreeId::ROOT)
            .is_some());
        assert!(tree_w1
            .state()
            .node_by_tree_local_id(tab_node_id(tab_c.raw()), TreeId::ROOT)
            .is_some());
        assert!(tree_w1
            .state()
            .node_by_tree_local_id(tab_node_id(tab_b.raw()), TreeId::ROOT)
            .is_none());

        manager.move_tab_to_window(tab_b, win1).unwrap();

        let mut changes_w0 = RecordedChanges {
            added: Vec::new(),
            removed: Vec::new(),
        };
        tree_w0.update_and_process_changes(
            build("Ultty", &window_snapshot(&manager, WindowKey::PRIMARY)),
            &mut changes_w0,
        );
        let mut changes_w1 = RecordedChanges {
            added: Vec::new(),
            removed: Vec::new(),
        };
        tree_w1.update_and_process_changes(
            build("Ultty", &window_snapshot(&manager, win1)),
            &mut changes_w1,
        );

        assert!(changes_w0.removed.contains(&tab_node_id(tab_b.raw())));
        assert!(tree_w0
            .state()
            .node_by_tree_local_id(tab_node_id(tab_b.raw()), TreeId::ROOT)
            .is_none());
        assert!(tree_w0
            .state()
            .node_by_tree_local_id(tab_node_id(tab_a.raw()), TreeId::ROOT)
            .is_some());

        assert!(changes_w1.added.contains(&tab_node_id(tab_b.raw())));
        let moved_tab_node = tree_w1
            .state()
            .node_by_tree_local_id(tab_node_id(tab_b.raw()), TreeId::ROOT)
            .expect("moved tab in destination tree");
        assert_eq!(moved_tab_node.role(), Role::Tab);
        assert_eq!(moved_tab_node.is_selected(), Some(true));
        let focus_w1 = tree_w1.state().focus().expect("destination tree focus");
        assert_eq!(
            focus_w1.tree_state.locate_node(focus_w1.id()),
            Some((pane_node_id(moved_pane.raw()), TreeId::ROOT))
        );
        assert_eq!(focus_w1.role(), Role::Terminal);
    }
}
