//! Terminal manager for multi-tab and multi-pane support.
//!
//! This module manages multiple terminal instances, supporting:
//! - Multiple tabs, each with its own terminal
//! - Pane splitting within tabs
//! - Keyboard navigation between tabs and panes

use std::ffi::OsString;
use std::io;
use std::path::Path;

use std::sync::Arc;

use crate::event_loop::{EventLoop as TerminalEventLoop, TerminalEvent, Waker};
use crate::grid::ThemeColors;
use crate::pane_layout::{Divider, FocusDirection, PaneLayout, PaneNode, Rect};
use crate::pty::{Child, PtySize};

/// Extra environment variables for the shells of one tab.
pub type TabEnv = Vec<(OsString, OsString)>;

/// Supplies a new tab's shell environment.
///
/// It is called with the tab's id before the tab's first shell is spawned,
/// so the caller can prepare per-tab resources the shell must know about
/// (the tab's native protocol socket, for example) and hand back the
/// variables that describe them. Panes split off the tab later inherit the
/// same environment.
pub type TabEnvFn<'a> = &'a mut dyn FnMut(TabId) -> io::Result<TabEnv>;

/// A [`TabEnvFn`] for tabs whose shells need no extra environment.
pub fn no_tab_env(_tab_id: TabId) -> io::Result<TabEnv> {
    Ok(Vec::new())
}

/// Direction of pane splits within a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitDirection {
    /// Panes arranged side by side (left to right)
    #[default]
    Horizontal,
    /// Panes stacked vertically (top to bottom)
    Vertical,
}

/// Unique identifier for a terminal instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalId(usize);

impl TerminalId {
    /// Create a new terminal ID.
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> usize {
        self.0
    }
}

/// The pane status a progress report (OSC 9;4) stands for.
pub fn progress_status(
    progress: crate::protocol::ansi::Progress,
) -> Option<ultty_protocol::control::PaneStatus> {
    use crate::protocol::ansi::Progress;
    use ultty_protocol::control::{PaneState, PaneStatus};
    let (state, percent) = match progress {
        Progress::Clear => return None,
        Progress::Normal(percent) => (PaneState::Working, Some(percent)),
        Progress::Indeterminate => (PaneState::Working, None),
        Progress::Error(percent) => (PaneState::Error, percent),
        Progress::Paused(percent) => (PaneState::Waiting, percent),
    };
    Some(PaneStatus {
        state,
        label: String::new(),
        progress: percent.map(|p| f32::from(p) / 100.0),
        attention: false,
    })
}

/// A single terminal instance.
pub struct ManagedTerminal {
    /// Unique ID
    pub id: TerminalId,
    /// The terminal event loop
    pub event_loop: TerminalEventLoop,
    /// Tab ID this terminal belongs to
    pub tab_id: TabId,
    /// Display title
    pub title: String,
    /// What the pane's program said it is doing (`ulttyctl status`).
    pub status: Option<ultty_protocol::control::PaneStatus>,
    /// The status its last progress report (OSC 9;4) implies.
    pub progress: Option<ultty_protocol::control::PaneStatus>,
}

impl ManagedTerminal {
    /// The status to show for the pane: what was set through the control
    /// socket, else what the program's progress reports imply.
    pub fn shown_status(&self) -> Option<&ultty_protocol::control::PaneStatus> {
        self.status.as_ref().or(self.progress.as_ref())
    }

    /// Create a new managed terminal whose shell gets `env` on top of the
    /// inherited environment.
    pub fn new(
        id: TerminalId,
        tab_id: TabId,
        cols: u16,
        rows: u16,
        env: &[(OsString, OsString)],
    ) -> io::Result<Self> {
        let child = Child::spawn_shell(PtySize::new(cols, rows), env).map_err(io::Error::other)?;
        let event_loop = TerminalEventLoop::from_child(child, cols, rows)?;
        Ok(Self {
            id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", id.0),
            status: None,
            progress: None,
        })
    }

    /// Resize this terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.event_loop.resize(cols, rows)
    }

    /// Process events for this terminal.
    pub fn process(&mut self) -> io::Result<Vec<TerminalEvent>> {
        self.event_loop.process()
    }

    /// Check if this terminal's process has exited.
    pub fn has_exited(&self) -> bool {
        self.event_loop.has_exited()
    }
}

/// Unique identifier for a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(usize);

impl TabId {
    /// Create a new tab ID.
    pub const fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw ID value.
    pub const fn raw(&self) -> usize {
        self.0
    }
}

/// A tab containing one or more terminal panes.
pub struct Tab {
    /// Unique ID
    pub id: TabId,
    /// This tab's panes in layout order, read-only: kept in step with
    /// `layout` by `split` and `remove_terminal`, the only writers.
    pub terminals: Vec<TerminalId>,
    /// Currently focused terminal in this tab
    pub focused_terminal: TerminalId,
    /// Tab title (often the title of the focused terminal)
    pub title: String,
    /// How panes are arranged: a tree of side-by-side and stacked splits.
    pub layout: PaneLayout,
    /// Extra environment given to every shell spawned in this tab.
    pub shell_env: TabEnv,
}

impl Tab {
    /// Create a new tab with a single terminal.
    pub fn new(id: TabId, terminal_id: TerminalId) -> Self {
        Self {
            id,
            terminals: vec![terminal_id],
            focused_terminal: terminal_id,
            title: format!("Tab {}", id.0 + 1),
            layout: PaneLayout::new(terminal_id),
            shell_env: Vec::new(),
        }
    }

    /// Put `new` next to `target`, side by side or stacked.
    pub fn split(&mut self, target: TerminalId, direction: SplitDirection, new: TerminalId) {
        if self.layout.split(target, direction, new) {
            self.terminals = self.layout.panes();
        }
    }

    /// Remove a terminal from this tab.
    pub fn remove_terminal(&mut self, terminal_id: TerminalId) -> bool {
        if let Some(pos) = self.terminals.iter().position(|&id| id == terminal_id) {
            self.terminals.remove(pos);
            // The layout keeps its last pane; a tab whose last pane goes is
            // empty and gets removed by its owner.
            if self.layout.remove(terminal_id) {
                self.terminals = self.layout.panes();
            }

            // Update focused terminal if needed
            if self.focused_terminal == terminal_id && !self.terminals.is_empty() {
                self.focused_terminal = self.terminals[0];
            }
            true
        } else {
            false
        }
    }

    /// Check if this tab is empty.
    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }

    fn default_title(&self) -> String {
        format!("Tab {}", self.id.0 + 1)
    }

    fn has_default_title(&self) -> bool {
        self.title.is_empty() || self.title == self.default_title()
    }
}

/// Manager for multiple terminals organized in tabs.
/// The pixel area a tab's panes share, and the cell size that turns pixels
/// into grid rows and columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneArea {
    pub width: f32,
    pub height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl PaneArea {
    /// One cell's size in whole pixels, `(width, height)`, as terminals
    /// report it to applications (XTWINOPS 14 and 16).
    pub fn cell_pixel_size(&self) -> (u16, u16) {
        let px = |v: f32| v.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        (px(self.cell_width), px(self.cell_height))
    }
}

pub struct TerminalManager {
    /// All managed terminals
    terminals: Vec<ManagedTerminal>,
    /// All tabs
    tabs: Vec<Tab>,
    /// Currently active tab
    active_tab: TabId,
    /// Next terminal ID
    next_terminal_id: usize,
    /// Next tab ID
    next_tab_id: usize,
    /// Default terminal dimensions
    default_cols: u16,
    default_rows: u16,
    /// Handed to every terminal so its I/O thread can wake the UI.
    waker: Option<Waker>,
    /// The colours terminals are drawn with, which they report to
    /// applications. Handed to every terminal, like the waker.
    theme_colors: ThemeColors,
    /// Lines of scrollback each terminal keeps (`behavior.scrollback_lines`).
    scrollback_lines: usize,
    /// Whether a pane closes when its program exits
    /// (`behavior.close_on_exit`). When false, the pane stays, showing its
    /// last output, until it is closed by hand.
    close_on_exit: bool,
    /// Where panes are laid out, once the window has a size. Splits,
    /// closes and divider drags re-lay panes out within it.
    pane_area: Option<PaneArea>,
}

impl TerminalManager {
    /// Register the callback every terminal's I/O thread uses to wake the
    /// UI, for existing terminals and any created later.
    pub fn set_waker(&mut self, waker: Waker) {
        for terminal in &self.terminals {
            terminal.event_loop.set_waker(Arc::clone(&waker));
        }
        self.waker = Some(waker);
    }

    /// Set the colours terminals are drawn with, for existing terminals and
    /// any created later, so colour queries report the live theme. Programs
    /// that asked (mode 2031) hear of a switch between dark and light.
    pub fn set_theme_colors(&mut self, theme: ThemeColors) {
        for terminal in &mut self.terminals {
            let report = {
                let mut state = terminal.event_loop.terminal();
                state.set_theme_colors(theme.clone());
                // The I/O thread sends replies as it parses, so anything
                // queued now is the report.
                state.take_responses().concat()
            };
            if !report.is_empty() {
                if let Err(err) = terminal.event_loop.send_input(&report) {
                    tracing::debug!("theme change report not sent: {err}");
                }
            }
        }
        self.theme_colors = theme;
    }

    /// Set how many lines of scrollback terminals keep, for existing
    /// terminals and any created later. Lowering it drops the oldest lines.
    pub fn set_scrollback_lines(&mut self, lines: usize) {
        for terminal in &self.terminals {
            terminal.event_loop.terminal().set_scrollback_limit(lines);
        }
        self.scrollback_lines = lines;
    }

    /// Set whether a pane closes when its program exits. When false,
    /// [`TerminalManager::cleanup_exited`] keeps exited panes and the window
    /// stays open until every pane has been closed by hand.
    pub fn set_close_on_exit(&mut self, close: bool) {
        self.close_on_exit = close;
    }

    /// Whether panes close when their program exits.
    pub fn close_on_exit(&self) -> bool {
        self.close_on_exit
    }

    fn push_terminal(&mut self, terminal: ManagedTerminal) {
        if let Some(waker) = &self.waker {
            terminal.event_loop.set_waker(Arc::clone(waker));
        }
        {
            let mut state = terminal.event_loop.terminal();
            state.set_theme_colors(self.theme_colors.clone());
            state.set_scrollback_limit(self.scrollback_lines);
            if let Some(area) = self.pane_area {
                let (width, height) = area.cell_pixel_size();
                state.set_cell_pixel_size(width, height);
            }
        }
        self.terminals.push(terminal);
    }

    /// Create a new terminal manager with one tab and one terminal.
    ///
    /// The initial tab's shell gets no extra environment; use
    /// [`TerminalManager::with_tab_env`] when it needs some.
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        Self::with_tab_env(cols, rows, &mut no_tab_env)
    }

    /// Create a new terminal manager with one tab whose shell environment
    /// comes from `tab_env`.
    pub fn with_tab_env(cols: u16, rows: u16, tab_env: TabEnvFn) -> io::Result<Self> {
        let mut manager = Self {
            terminals: Vec::new(),
            tabs: Vec::new(),
            active_tab: TabId(0),
            next_terminal_id: 0,
            next_tab_id: 0,
            default_cols: cols,
            default_rows: rows,
            waker: None,
            theme_colors: ThemeColors::default(),
            scrollback_lines: crate::grid::DEFAULT_SCROLLBACK,
            close_on_exit: true,
            pane_area: None,
        };

        // Create initial tab with terminal
        manager.create_tab_with_env(tab_env)?;

        Ok(manager)
    }

    /// Create a new tab with a new terminal and no extra shell environment.
    pub fn create_tab(&mut self) -> io::Result<TabId> {
        self.create_tab_with_env(&mut no_tab_env)
    }

    /// Create a new tab whose shell environment comes from `tab_env`.
    pub fn create_tab_with_env(&mut self, tab_env: TabEnvFn) -> io::Result<TabId> {
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let env = tab_env(tab_id)?;
        let terminal_id = self.create_terminal_with_env(tab_id, &env)?;

        let mut tab = Tab::new(tab_id, terminal_id);
        tab.shell_env = env;
        self.tabs.push(tab);
        self.active_tab = tab_id;

        Ok(tab_id)
    }

    /// Set a tab's explicit display title.
    pub fn set_tab_title(&mut self, tab_id: TabId, title: &str) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };

        tab.title = title.to_string();
        true
    }

    /// Close a tab and all its terminals.
    pub fn close_tab(&mut self, tab_id: TabId) -> bool {
        let Some(tab_idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return false;
        };

        let tab = self.tabs.remove(tab_idx);

        // Remove all terminals in this tab
        for terminal_id in tab.terminals {
            self.terminals.retain(|t| t.id != terminal_id);
        }

        // Update active tab if needed
        if self.active_tab == tab_id && !self.tabs.is_empty() {
            let new_idx = tab_idx.min(self.tabs.len() - 1);
            self.active_tab = self.tabs[new_idx].id;
        }

        true
    }

    /// Reorder tabs by moving a tab from one index to another.
    pub fn reorder_tabs(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return false;
        }

        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        true
    }

    /// Create a new terminal (pane) in an existing tab, inheriting the
    /// tab's shell environment.
    fn create_terminal(&mut self, tab_id: TabId) -> io::Result<TerminalId> {
        let env = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.shell_env.clone())
            .unwrap_or_default();
        self.create_terminal_with_env(tab_id, &env)
    }

    /// Create a new terminal in `tab_id` whose shell gets `env`.
    fn create_terminal_with_env(
        &mut self,
        tab_id: TabId,
        env: &[(OsString, OsString)],
    ) -> io::Result<TerminalId> {
        let terminal_id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;

        let terminal = ManagedTerminal::new(
            terminal_id,
            tab_id,
            self.default_cols,
            self.default_rows,
            env,
        )?;

        self.push_terminal(terminal);
        Ok(terminal_id)
    }

    /// Get the active tab.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab)
    }

    /// Get the active tab mutably.
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let active = self.active_tab;
        self.tabs.iter_mut().find(|t| t.id == active)
    }

    /// Get the currently focused terminal.
    pub fn focused_terminal(&self) -> Option<&ManagedTerminal> {
        let tab = self.active_tab()?;
        self.terminals.iter().find(|t| t.id == tab.focused_terminal)
    }

    /// Get the currently focused terminal mutably.
    pub fn focused_terminal_mut(&mut self) -> Option<&mut ManagedTerminal> {
        let tab = self.active_tab()?;
        let focused_id = tab.focused_terminal;
        self.terminals.iter_mut().find(|t| t.id == focused_id)
    }

    /// Get a terminal by ID.
    pub fn terminal(&self, id: TerminalId) -> Option<&ManagedTerminal> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// Get a terminal by ID mutably.
    pub fn terminal_mut(&mut self, id: TerminalId) -> Option<&mut ManagedTerminal> {
        self.terminals.iter_mut().find(|t| t.id == id)
    }

    /// Clear dirty flags on all terminals after rendering.
    pub fn clear_dirty(&mut self) {
        for terminal in &mut self.terminals {
            terminal.event_loop.terminal_mut().clear_dirty();
        }
    }

    /// Get all tabs.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Get the display title for a tab.
    pub fn tab_display_title(&self, tab_id: TabId) -> String {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return String::new();
        };

        if !tab.has_default_title() {
            return tab.title.clone();
        }

        let Some(terminal) = self.terminal(tab.focused_terminal) else {
            return tab.default_title();
        };

        if tab.id == self.active_tab {
            if let Some(app) = terminal.event_loop.foreground_process_name() {
                return app;
            }
        }

        let terminal_state = terminal.event_loop.terminal();
        let title = terminal_state.title().trim();
        if !title.is_empty() && !is_shell_label(title) {
            return title.to_string();
        }

        let cwd = terminal_state
            .working_directory()
            .map(str::to_string)
            .or_else(|| terminal.event_loop.child_working_directory());

        if let Some(cwd) = cwd.and_then(|path| compact_working_directory(&path)) {
            return cwd;
        }

        if !title.is_empty() {
            return title.to_string();
        }

        tab.default_title()
    }

    /// Get display titles for every tab in order.
    pub fn tab_display_titles(&self) -> Vec<String> {
        self.tabs
            .iter()
            .map(|tab| self.tab_display_title(tab.id))
            .collect()
    }

    /// Get number of tabs.
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Switch to the next tab.
    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }

        let current_idx = self
            .tabs
            .iter()
            .position(|t| t.id == self.active_tab)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % self.tabs.len();
        self.active_tab = self.tabs[next_idx].id;
    }

    /// Switch to the previous tab.
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }

        let current_idx = self
            .tabs
            .iter()
            .position(|t| t.id == self.active_tab)
            .unwrap_or(0);
        let prev_idx = if current_idx == 0 {
            self.tabs.len() - 1
        } else {
            current_idx - 1
        };
        self.active_tab = self.tabs[prev_idx].id;
    }

    /// Switch to a specific tab by index.
    pub fn switch_to_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = self.tabs[index].id;
        }
    }

    /// Resize all terminals.
    /// Size every pane's terminal to its share of `area`, in every tab, and
    /// remember `area` so later splits, closes and divider drags keep panes
    /// sized. A pane whose grid size is unchanged is left alone.
    pub fn layout_panes(&mut self, area: PaneArea) -> io::Result<()> {
        self.pane_area = Some(area);
        let (cols, rows) = crate::renderer::grid_dimensions_for(
            area.width,
            area.height,
            area.cell_width,
            area.cell_height,
        );
        self.default_cols = cols;
        self.default_rows = rows;

        let mut sizes = Vec::new();
        for tab in &self.tabs {
            for (terminal_id, (_, _, width, height)) in
                tab.layout.rects((0.0, 0.0, area.width, area.height))
            {
                let grid = crate::renderer::grid_dimensions_for(
                    width,
                    height,
                    area.cell_width,
                    area.cell_height,
                );
                sizes.push((terminal_id, grid));
            }
        }
        let (cell_width, cell_height) = area.cell_pixel_size();
        for (terminal_id, (cols, rows)) in sizes {
            let Some(terminal) = self.terminals.iter_mut().find(|t| t.id == terminal_id) else {
                continue;
            };
            let current = {
                let mut state = terminal.event_loop.terminal();
                state.set_cell_pixel_size(cell_width, cell_height);
                state.dimensions()
            };
            if (current.cols, current.rows) != (cols, rows) {
                terminal.resize(cols, rows)?;
            }
        }
        Ok(())
    }

    /// Lay panes out again after their number or ratios changed.
    fn relayout(&mut self) {
        if let Some(area) = self.pane_area {
            if let Err(e) = self.layout_panes(area) {
                tracing::warn!("Failed to resize panes: {}", e);
            }
        }
    }

    pub fn resize_all(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.default_cols = cols;
        self.default_rows = rows;

        for terminal in &mut self.terminals {
            terminal.resize(cols, rows)?;
        }
        Ok(())
    }

    /// Process events for all terminals.
    ///
    /// Returns events keyed by terminal ID.
    pub fn process_all(&mut self) -> io::Result<Vec<(TerminalId, Vec<TerminalEvent>)>> {
        let mut all_events = Vec::new();

        for terminal in &mut self.terminals {
            let events = terminal.process()?;
            if !self.close_on_exit {
                for event in &events {
                    if let TerminalEvent::ChildExited { status } = event {
                        // The pane stays open; say why nothing answers.
                        let note =
                            format!("\r\n\x1b[0;7m[process exited with status {status}]\x1b[0m");
                        terminal.event_loop.write_local(note.as_bytes());
                    }
                }
            }
            if !events.is_empty() {
                if let Some(title) = events.iter().rev().find_map(|event| match event {
                    TerminalEvent::TitleChanged(title) => Some(title),
                    _ => None,
                }) {
                    terminal.title = title.clone();
                }

                all_events.push((terminal.id, events));
            }
        }

        Ok(all_events)
    }

    /// Remove exited terminals and their empty tabs, unless panes are set
    /// to stay open after their program exits.
    pub fn cleanup_exited(&mut self) {
        if !self.close_on_exit {
            return;
        }
        // Find exited terminals
        let exited: Vec<TerminalId> = self
            .terminals
            .iter()
            .filter(|t| t.has_exited())
            .map(|t| t.id)
            .collect();

        for &terminal_id in &exited {
            // Find and update the tab
            if let Some(tab) = self
                .tabs
                .iter_mut()
                .find(|t| t.terminals.contains(&terminal_id))
            {
                tab.remove_terminal(terminal_id);
            }

            // Remove the terminal
            self.terminals.retain(|t| t.id != terminal_id);
        }

        // Remove empty tabs (except keep at least one)
        if self.tabs.len() > 1 {
            self.tabs.retain(|t| !t.is_empty());
        }

        self.repair_active_tab();
        if !exited.is_empty() {
            self.relayout();
        }
    }

    fn repair_active_tab(&mut self) {
        if self.tabs.iter().any(|tab| tab.id == self.active_tab) {
            return;
        }

        if let Some(tab) = self.tabs.first() {
            self.active_tab = tab.id;
        }
    }

    /// Whether the window has nothing left to show: no terminals, or, when
    /// panes close on exit, only terminals whose program has exited.
    pub fn all_exited(&self) -> bool {
        self.terminals.is_empty()
            || (self.close_on_exit && self.terminals.iter().all(|t| t.has_exited()))
    }

    /// Split the focused pane side by side.
    pub fn split_horizontal(&mut self) -> io::Result<Option<TerminalId>> {
        self.split_focused(SplitDirection::Horizontal)
    }

    /// Split the focused pane top and bottom.
    pub fn split_vertical(&mut self) -> io::Result<Option<TerminalId>> {
        self.split_focused(SplitDirection::Vertical)
    }

    /// Put a new pane next to the focused one. Splitting along the focused
    /// pane's own split adds a sibling; splitting across it nests.
    fn split_focused(&mut self, direction: SplitDirection) -> io::Result<Option<TerminalId>> {
        let Some(tab) = self.active_tab() else {
            return Ok(None);
        };
        let (tab_id, target) = (tab.id, tab.focused_terminal);
        let terminal_id = self.create_terminal(tab_id)?;
        if let Some(tab) = self.active_tab_mut() {
            tab.split(target, direction, terminal_id);
        }
        self.relayout();
        Ok(Some(terminal_id))
    }

    pub fn pane_count(&self) -> usize {
        self.active_tab().map(|t| t.terminals.len()).unwrap_or(0)
    }

    /// The active tab's panes and their rectangles within a `width` x
    /// `height` area, in layout order.
    pub fn active_pane_rects(&self, width: f32, height: f32) -> Vec<(TerminalId, Rect)> {
        self.active_tab()
            .map(|tab| tab.layout.rects((0.0, 0.0, width, height)))
            .unwrap_or_default()
    }

    /// Move focus to the nearest pane on one side of the focused one.
    pub fn focus_direction(&mut self, direction: FocusDirection, width: f32, height: f32) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        if let Some(next) =
            tab.layout
                .neighbor((0.0, 0.0, width, height), tab.focused_terminal, direction)
        {
            tab.focused_terminal = next;
        }
    }

    /// Focus the next pane in the active tab.
    pub fn focus_next_pane(&mut self) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };

        if tab.terminals.len() <= 1 {
            return;
        }

        let current_idx = tab
            .terminals
            .iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % tab.terminals.len();
        tab.focused_terminal = tab.terminals[next_idx];
    }

    /// Focus the previous pane in the active tab.
    pub fn focus_prev_pane(&mut self) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };

        if tab.terminals.len() <= 1 {
            return;
        }

        let current_idx = tab
            .terminals
            .iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            tab.terminals.len() - 1
        } else {
            current_idx - 1
        };
        tab.focused_terminal = tab.terminals[prev_idx];
    }

    /// Close the currently focused pane.
    ///
    /// Returns true if a pane was closed, false if it was the last pane.
    pub fn close_focused_pane(&mut self) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };

        // Don't close if it's the only pane
        if tab.terminals.len() <= 1 {
            return false;
        }

        let focused_id = tab.focused_terminal;
        let tab_id = tab.id;

        // Remove terminal from tab
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.remove_terminal(focused_id);
        }

        // Remove the terminal itself
        self.terminals.retain(|t| t.id != focused_id);
        self.relayout();

        true
    }

    /// Get all terminals in the active tab for rendering.
    pub fn active_tab_terminals(&self) -> Vec<&ManagedTerminal> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };

        tab.terminals
            .iter()
            .filter_map(|&id| self.terminal(id))
            .collect()
    }

    /// Whether the pane is in the active tab, so its output shows.
    pub fn is_on_screen(&self, id: TerminalId) -> bool {
        self.active_tab()
            .is_some_and(|tab| tab.terminals.contains(&id))
    }

    /// Get the focused terminal ID in the active tab.
    pub fn focused_terminal_id(&self) -> Option<TerminalId> {
        self.active_tab().map(|t| t.focused_terminal)
    }

    /// Create a new tab running a custom command, with the shell
    /// environment supplied by `tab_env`.
    pub fn create_tab_with_command(
        &mut self,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_env: TabEnvFn,
    ) -> io::Result<TabId> {
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let env = tab_env(tab_id)?;
        let terminal_id =
            self.create_terminal_with_command(tab_id, command, args, working_dir, &env)?;

        let mut tab = Tab::new(tab_id, terminal_id);
        tab.shell_env = env;
        if let Some(title) = title {
            tab.title = title.to_string();
        }

        self.tabs.push(tab);
        self.active_tab = tab_id;

        // Update terminal title
        if let Some(terminal) = self.terminal_mut(terminal_id) {
            if let Some(title) = title {
                terminal.title = title.to_string();
            }
        }

        Ok(tab_id)
    }

    /// Create a terminal with a custom command.
    fn create_terminal_with_command(
        &mut self,
        tab_id: TabId,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        env: &[(OsString, OsString)],
    ) -> io::Result<TerminalId> {
        let terminal_id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;

        // Build args with command as argv[0]
        let mut full_args: Vec<&str> = vec![command];
        for arg in args {
            full_args.push(arg);
        }

        // A failed chdir in the child is tolerated: the command then starts
        // in the directory Ultty runs in.
        let dir = working_dir
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::pty::current_dir);
        let child = Child::spawn(
            command,
            &full_args,
            &dir,
            PtySize::new(self.default_cols, self.default_rows),
            env,
        )
        .map_err(io::Error::other)?;
        let event_loop =
            TerminalEventLoop::from_child(child, self.default_cols, self.default_rows)?;

        let terminal = ManagedTerminal {
            id: terminal_id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", terminal_id.0),
            status: None,
            progress: None,
        };

        self.push_terminal(terminal);
        Ok(terminal_id)
    }

    /// Focus a pane of the active tab by its terminal ID.
    pub fn focus_terminal(&mut self, terminal_id: TerminalId) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.terminals.contains(&terminal_id) {
                tab.focused_terminal = terminal_id;
            }
        }
    }

    /// Focus a specific pane by index.
    pub fn focus_pane(&mut self, index: usize) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };

        if index < tab.terminals.len() {
            tab.focused_terminal = tab.terminals[index];
        }
    }

    /// Get the index of the focused pane.
    pub fn focused_pane_index(&self) -> usize {
        let Some(tab) = self.active_tab() else {
            return 0;
        };

        tab.terminals
            .iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0)
    }

    /// The pane at (x, y) in a `width` x `height` content area.
    pub fn pane_at(&self, x: f32, y: f32, width: f32, height: f32) -> Option<TerminalId> {
        self.active_pane_rects(width, height)
            .into_iter()
            .find(|(_, (px, py, pw, ph))| x >= *px && x < px + pw && y >= *py && y < py + ph)
            .map(|(id, _)| id)
    }

    /// The divider within `tolerance` pixels of (x, y), if any.
    pub fn divider_at(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        tolerance: f32,
    ) -> Option<Divider> {
        self.active_tab()?
            .layout
            .divider_at((0.0, 0.0, width, height), x, y, tolerance)
    }

    /// Move a divider of the active tab by `delta` pixels and resize the
    /// panes on either side.
    pub fn drag_divider(&mut self, divider: &Divider, delta: f32) {
        if let Some(tab) = self.active_tab_mut() {
            tab.layout.drag(divider, delta);
        }
        self.relayout();
    }

    /// Extract session data from the current state.
    pub fn to_session_data(&self) -> crate::session::SessionData {
        use crate::session::{SessionData, TabData, TerminalData};

        let active_tab_index = self
            .tabs
            .iter()
            .position(|t| t.id == self.active_tab)
            .unwrap_or(0);

        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let terminals = tab
                    .terminals
                    .iter()
                    .filter_map(|&tid| {
                        self.terminals.iter().find(|t| t.id == tid).map(|t| {
                            let working_dir: Option<String> = t
                                .event_loop
                                .terminal()
                                .working_directory()
                                .map(|s| s.to_string());
                            TerminalData {
                                title: t.title.clone(),
                                working_directory: working_dir,
                                is_focused: tid == tab.focused_terminal,
                            }
                        })
                    })
                    .collect();

                // Get working directory from the first terminal or focused terminal
                let working_directory: Option<String> = tab
                    .terminals
                    .first()
                    .and_then(|&tid| self.terminals.iter().find(|t| t.id == tid))
                    .and_then(|t| {
                        t.event_loop
                            .terminal()
                            .working_directory()
                            .map(str::to_string)
                    });

                TabData {
                    title: tab.title.clone(),
                    working_directory,
                    terminals,
                    split_direction: direction_name(tab.layout.root_direction()).to_string(),
                    layout: Some(layout_data(tab.layout.root(), &tab.terminals)),
                }
            })
            .collect();

        SessionData {
            version: 1,
            timestamp: String::new(), // Will be filled by SessionData
            tabs,
            active_tab_index,
        }
    }

    /// Create a terminal manager from session data.
    ///
    /// `tab_env` supplies the shell environment of every restored tab.
    pub fn from_session(
        session: &crate::session::SessionData,
        cols: u16,
        rows: u16,
        tab_env: TabEnvFn,
    ) -> io::Result<Self> {
        if session.tabs.is_empty() {
            // No tabs in session, create default
            return Self::with_tab_env(cols, rows, tab_env);
        }

        let mut manager = Self {
            terminals: Vec::new(),
            tabs: Vec::new(),
            active_tab: TabId::new(0),
            next_terminal_id: 0,
            next_tab_id: 0,
            default_cols: cols,
            default_rows: rows,
            waker: None,
            theme_colors: ThemeColors::default(),
            scrollback_lines: crate::grid::DEFAULT_SCROLLBACK,
            close_on_exit: true,
            pane_area: None,
        };

        for (i, tab_data) in session.tabs.iter().enumerate() {
            // Create tab
            let tab_id = TabId(manager.next_tab_id);
            manager.next_tab_id += 1;
            let env = tab_env(tab_id)?;

            // Create at least one terminal for this tab
            let terminal_count = tab_data.terminals.len().max(1);
            let mut terminal_ids = Vec::new();
            let mut focused_terminal_id = None;

            for (j, term_data) in tab_data.terminals.iter().take(terminal_count).enumerate() {
                let tid = TerminalId(manager.next_terminal_id);
                manager.next_terminal_id += 1;

                // Create terminal with working directory if available
                let mut terminal = if let Some(ref wd) = term_data.working_directory {
                    ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, wd, &env)?
                } else if let Some(ref wd) = tab_data.working_directory {
                    ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, wd, &env)?
                } else {
                    ManagedTerminal::new(tid, tab_id, cols, rows, &env)?
                };

                terminal.title = term_data.title.clone();
                manager.terminals.push(terminal);
                terminal_ids.push(tid);

                if term_data.is_focused || j == 0 {
                    focused_terminal_id = Some(tid);
                }
            }

            // If no terminals were created (empty terminals list), create one
            if terminal_ids.is_empty() {
                let tid = TerminalId(manager.next_terminal_id);
                manager.next_terminal_id += 1;
                let mut terminal = if let Some(ref wd) = tab_data.working_directory {
                    ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, wd, &env)?
                } else {
                    ManagedTerminal::new(tid, tab_id, cols, rows, &env)?
                };
                terminal.title = tab_data.title.clone();
                manager.terminals.push(terminal);
                terminal_ids.push(tid);
                focused_terminal_id = Some(tid);
            }

            let split_direction = parse_direction(&tab_data.split_direction);
            // The saved tree when it covers exactly these panes; otherwise
            // (older sessions, or a mismatch) one row or column of them.
            let layout = tab_data
                .layout
                .as_ref()
                .and_then(|data| restore_layout(data, &terminal_ids))
                .unwrap_or_else(|| flat_layout(split_direction, &terminal_ids));

            let tab = Tab {
                id: tab_id,
                focused_terminal: focused_terminal_id.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "no focused terminal in restored tab",
                    )
                })?,
                title: tab_data.title.clone(),
                terminals: layout.panes(),
                layout,
                shell_env: env,
            };

            manager.tabs.push(tab);

            // Set active tab
            if i == session.active_tab_index {
                manager.active_tab = tab_id;
            }
        }

        Ok(manager)
    }
}

fn direction_name(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Horizontal => "Horizontal",
        SplitDirection::Vertical => "Vertical",
    }
}

fn parse_direction(name: &str) -> SplitDirection {
    match name {
        "Vertical" => SplitDirection::Vertical,
        _ => SplitDirection::Horizontal,
    }
}

/// A layout tree for saving, panes as positions in `panes`.
fn layout_data(node: &PaneNode, panes: &[TerminalId]) -> crate::session::LayoutData {
    use crate::session::LayoutData;
    match node {
        PaneNode::Leaf(id) => LayoutData::Pane {
            index: panes.iter().position(|p| p == id).unwrap_or(0),
        },
        PaneNode::Split {
            direction,
            children,
        } => LayoutData::Split {
            direction: direction_name(*direction).to_string(),
            children: children
                .iter()
                .map(|(child, ratio)| (layout_data(child, panes), *ratio))
                .collect(),
        },
    }
}

/// Rebuild a saved tree over `panes`; None unless it names each pane once.
fn restore_layout(data: &crate::session::LayoutData, panes: &[TerminalId]) -> Option<PaneLayout> {
    use crate::session::LayoutData;
    fn build(data: &LayoutData, panes: &[TerminalId], used: &mut Vec<bool>) -> Option<PaneNode> {
        match data {
            LayoutData::Pane { index } => {
                let seen = used.get_mut(*index)?;
                if *seen {
                    return None;
                }
                *seen = true;
                Some(PaneNode::Leaf(panes[*index]))
            }
            LayoutData::Split {
                direction,
                children,
            } => {
                if children.len() < 2 {
                    return None;
                }
                let total: f32 = children.iter().map(|(_, r)| r).sum();
                if !(total.is_finite() && total > 0.0) {
                    return None;
                }
                let children = children
                    .iter()
                    .map(|(child, ratio)| Some((build(child, panes, used)?, ratio / total)))
                    .collect::<Option<Vec<_>>>()?;
                Some(PaneNode::Split {
                    direction: parse_direction(direction),
                    children,
                })
            }
        }
    }
    let mut used = vec![false; panes.len()];
    let root = build(data, panes, &mut used)?;
    used.iter().all(|u| *u).then(|| PaneLayout::from_root(root))
}

/// `panes` in one row (Horizontal) or column (Vertical), equal shares.
fn flat_layout(direction: SplitDirection, panes: &[TerminalId]) -> PaneLayout {
    match panes {
        [] => unreachable!("a restored tab always has a pane"),
        [only] => PaneLayout::new(*only),
        many => PaneLayout::from_root(PaneNode::Split {
            direction,
            children: many
                .iter()
                .map(|id| (PaneNode::Leaf(*id), 1.0 / many.len() as f32))
                .collect(),
        }),
    }
}

fn compact_working_directory(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let home = std::env::var("HOME").ok();
    if let Some(home) = home.as_deref().filter(|home| !home.is_empty()) {
        if path == home {
            return Some("~".to_string());
        }

        if let Some(rest) = path
            .strip_prefix(home)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return Some(format!("~/{rest}"));
        }
    }

    Some(path.to_string())
}

fn is_shell_label(label: &str) -> bool {
    let command = Path::new(label)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label);

    matches!(
        command,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish"
    )
}

impl ManagedTerminal {
    /// Create a new managed terminal with a specific working directory and
    /// extra shell environment.
    pub fn new_with_dir(
        id: TerminalId,
        tab_id: TabId,
        cols: u16,
        rows: u16,
        working_dir: &str,
        env: &[(OsString, OsString)],
    ) -> io::Result<Self> {
        let child = Child::spawn_shell_in_dir(PtySize::new(cols, rows), working_dir, env)
            .map_err(io::Error::other)?;
        let event_loop = TerminalEventLoop::from_child(child, cols, rows)?;
        Ok(Self {
            id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", id.0),
            status: None,
            progress: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reports_map_to_pane_statuses() {
        use crate::protocol::ansi::Progress;
        use ultty_protocol::control::PaneState;
        let shown = |progress| progress_status(progress).map(|s| (s.state, s.progress));
        assert_eq!(shown(Progress::Clear), None);
        assert_eq!(
            shown(Progress::Normal(40)),
            Some((PaneState::Working, Some(0.4)))
        );
        assert_eq!(
            shown(Progress::Indeterminate),
            Some((PaneState::Working, None))
        );
        assert_eq!(
            shown(Progress::Error(Some(70))),
            Some((PaneState::Error, Some(0.7)))
        );
        assert_eq!(
            shown(Progress::Paused(None)),
            Some((PaneState::Waiting, None))
        );
    }

    #[test]
    fn terminals_report_the_cell_size_and_theme_they_are_drawn_with() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let theme = ThemeColors {
            background: [1, 2, 3],
            ..ThemeColors::default()
        };
        manager.set_theme_colors(theme.clone());
        manager.layout_panes(AREA).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let second = manager.split_horizontal().unwrap().unwrap();
        manager.create_tab().unwrap();
        let third = manager.focused_terminal_id().unwrap();
        for id in [first, second, third] {
            let state = manager.terminal(id).unwrap().event_loop.terminal();
            assert_eq!(state.cell_pixel_size(), Some((10, 20)), "{id:?}");
            assert_eq!(state.palette().theme(), &theme, "{id:?}");
        }
    }

    #[test]
    fn mixed_splits_size_each_pane_to_its_place_in_the_tree() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        // [first | right], then split `right` down: [first | (right / below)].
        let right = manager.split_horizontal().unwrap().unwrap();
        manager.focus_terminal(right);
        let below = manager.split_vertical().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (40, 24));
        assert_eq!(grid(&manager, right), (40, 12));
        assert_eq!(grid(&manager, below), (40, 12));

        // Focus follows the geometry: left of `below` is `first`.
        manager.focus_terminal(below);
        manager.focus_direction(FocusDirection::Left, AREA.width, AREA.height);
        assert_eq!(manager.focused_terminal_id(), Some(first));
    }

    #[test]
    fn sessions_keep_nested_splits() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let right = manager.split_horizontal().unwrap().unwrap();
        manager.focus_terminal(right);
        manager.split_vertical().unwrap().unwrap();
        let saved = manager.to_session_data();
        let layout = saved.tabs[0].layout.clone().unwrap();
        let restored =
            restore_layout(&layout, &[TerminalId(10), TerminalId(11), TerminalId(12)]).unwrap();
        let rects = restored.rects((0.0, 0.0, 800.0, 480.0));
        assert_eq!(rects[0], (TerminalId(10), (0.0, 0.0, 400.0, 480.0)));
        assert_eq!(rects[1], (TerminalId(11), (400.0, 0.0, 400.0, 240.0)));
        assert_eq!(rects[2], (TerminalId(12), (400.0, 240.0, 400.0, 240.0)));
        // A tree that does not cover the panes falls back to a flat layout.
        assert!(restore_layout(&layout, &[TerminalId(10), TerminalId(11)]).is_none());
    }

    fn grid(manager: &TerminalManager, id: TerminalId) -> (u16, u16) {
        let terminal = manager.terminals.iter().find(|t| t.id == id).unwrap();
        let dims = terminal.event_loop.terminal().dimensions();
        (dims.cols, dims.rows)
    }

    /// 80 x 24 cells of 10 x 20 px.
    const AREA: PaneArea = PaneArea {
        width: 800.0,
        height: 480.0,
        cell_width: 10.0,
        cell_height: 20.0,
    };

    #[test]
    fn split_panes_get_grids_the_size_of_their_share() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        assert_eq!(grid(&manager, first), (80, 24));

        // Side by side: each pane gets half the columns, all the rows.
        let second = manager.split_horizontal().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (40, 24));
        assert_eq!(grid(&manager, second), (40, 24));

        // A third pane takes half of the pane it splits (the focused,
        // first one); the other pane keeps its size.
        let third = manager.split_horizontal().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (20, 24));
        assert_eq!(grid(&manager, third), (20, 24));
        assert_eq!(grid(&manager, second), (40, 24));

        // Closing a pane gives its space back to the two that remain.
        assert!(manager.close_focused_pane());
        let remaining = manager.active_tab().unwrap().terminals.clone();
        assert_eq!(remaining.len(), 2);
        for id in remaining {
            assert_eq!(grid(&manager, id), (40, 24));
        }
    }

    #[test]
    fn stacked_panes_split_the_rows() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        let second = manager.split_vertical().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (80, 12));
        assert_eq!(grid(&manager, second), (80, 12));
    }

    /// A manager whose only tab runs a shell that prints nothing.
    ///
    /// The I/O thread parses child output as it arrives, so a login shell's
    /// prompt (which on many systems sets the window title) would race with
    /// tests that set terminal state directly. `sh` stays a shell, so the
    /// foreground-process name does not stand in for the title either.
    fn quiet_manager() -> TerminalManager {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.active_tab().unwrap().id;
        manager
            .create_tab_with_command(
                "/bin/sh",
                &["-c".to_string(), "read never".to_string()],
                None,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        manager.close_tab(first);
        manager
    }

    use std::time::{Duration, Instant};

    /// Process output until the focused pane's screen contains `text`.
    fn wait_for_screen(manager: &mut TerminalManager, text: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            manager.process_all().unwrap();
            let screen = {
                let state = manager.focused_terminal().unwrap().event_loop.terminal();
                let grid = state.grid();
                (0..grid.dimensions().rows)
                    .filter_map(|r| grid.row(r).map(|row| row.text_trimmed()))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if screen.contains(text) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_program_that_asked_hears_the_theme_turn_light() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.set_theme_colors(ThemeColors {
            background: [0x20, 0x20, 0x20],
            ..ThemeColors::default()
        });
        // Turn on mode 2031, then print the next 9 bytes of input.
        let script = "stty raw -echo; printf '\\033[?2031hready'; \
                      r=$(dd bs=1 count=9 2>/dev/null | tr '\\033' E); \
                      printf ' got:%s' \"$r\"; sleep 5";
        manager
            .create_tab_with_command(
                "/bin/sh",
                &["-c".to_string(), script.to_string()],
                None,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        assert!(wait_for_screen(&mut manager, "ready"));
        manager.set_theme_colors(ThemeColors {
            background: [0xf8, 0xf8, 0xf8],
            ..ThemeColors::default()
        });
        assert!(wait_for_screen(&mut manager, "got:E[?997;2n"));
    }

    #[test]
    fn only_the_active_tabs_panes_are_on_screen() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let split = manager.split_horizontal().unwrap().unwrap();
        assert!(manager.is_on_screen(first) && manager.is_on_screen(split));
        manager.create_tab().unwrap();
        let other = manager.focused_terminal_id().unwrap();
        assert!(manager.is_on_screen(other));
        assert!(!manager.is_on_screen(first) && !manager.is_on_screen(split));
        manager.switch_to_tab(0);
        assert!(manager.is_on_screen(first) && !manager.is_on_screen(other));
    }

    #[test]
    fn a_tab_command_starts_in_the_working_directory_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager
            .create_tab_with_command(
                "sleep",
                &["5".to_string()],
                Some(dir.to_str().unwrap()),
                None,
                &mut no_tab_env,
            )
            .unwrap();
        // The child changes directory between fork and exec, so poll
        // briefly rather than racing it.
        let deadline = Instant::now() + Duration::from_secs(5);
        let cwd = loop {
            let cwd = manager
                .focused_terminal()
                .unwrap()
                .event_loop
                .child_working_directory();
            if cwd.as_deref() == dir.to_str() || Instant::now() > deadline {
                break cwd;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(cwd.as_deref(), dir.to_str());
    }

    #[test]
    fn create_manager() {
        let manager = TerminalManager::new(80, 24).unwrap();
        assert_eq!(manager.tab_count(), 1);
        assert!(manager.focused_terminal().is_some());
    }

    #[test]
    fn tab_display_title_uses_explicit_title() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let tab_id = manager.active_tab().unwrap().id;
        manager.set_tab_title(tab_id, "editor");
        manager.terminals[0]
            .event_loop
            .terminal_mut()
            .set_title("nvim".to_string());

        assert_eq!(manager.tab_display_title(tab_id), "editor");
    }

    #[test]
    fn tab_display_title_uses_osc_title_for_default_tabs() {
        let mut manager = quiet_manager();
        let tab_id = manager.active_tab().unwrap().id;
        manager.terminals[0]
            .event_loop
            .terminal_mut()
            .set_title("nvim".to_string());

        assert_eq!(manager.tab_display_title(tab_id), "nvim");
    }

    #[test]
    fn tab_display_title_falls_back_to_working_directory_for_shell_titles() {
        let mut manager = quiet_manager();
        let tab_id = manager.active_tab().unwrap().id;
        {
            let mut terminal = manager.terminals[0].event_loop.terminal_mut();
            terminal.set_title("zsh".to_string());
            terminal.set_working_directory("/tmp/ultty-project".to_string());
        }

        assert_eq!(manager.tab_display_title(tab_id), "/tmp/ultty-project");
    }

    #[test]
    fn compact_working_directory_ignores_empty_paths() {
        assert_eq!(compact_working_directory(""), None);
        assert_eq!(compact_working_directory("   "), None);
    }

    #[test]
    fn shell_label_filter_handles_paths() {
        assert!(is_shell_label("/bin/zsh"));
        assert!(!is_shell_label("nvim"));
    }

    #[test]
    fn create_multiple_tabs() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        assert_eq!(manager.tab_count(), 1);

        manager.create_tab().unwrap();
        assert_eq!(manager.tab_count(), 2);

        manager.create_tab().unwrap();
        assert_eq!(manager.tab_count(), 3);
    }

    #[test]
    fn tab_navigation() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;
        let second_tab = manager.tabs[1].id;
        let third_tab = manager.tabs[2].id;

        // Should be on third tab (most recently created)
        assert_eq!(manager.active_tab, third_tab);

        manager.prev_tab();
        assert_eq!(manager.active_tab, second_tab);

        manager.prev_tab();
        assert_eq!(manager.active_tab, first_tab);

        manager.prev_tab();
        assert_eq!(manager.active_tab, third_tab); // Wrap around

        manager.next_tab();
        assert_eq!(manager.active_tab, first_tab);
    }

    #[test]
    fn close_tab() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;
        let second_tab = manager.tabs[1].id;

        manager.active_tab = first_tab;
        manager.close_tab(first_tab);

        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.active_tab, second_tab);
    }

    /// Open a tab running `/bin/true` and return its terminal.
    fn tab_that_exits(manager: &mut TerminalManager) -> TerminalId {
        manager
            .create_tab_with_command("/bin/true", &[], None, None, &mut no_tab_env)
            .unwrap();
        manager.focused_terminal_id().unwrap()
    }

    /// Process and clean up until `done`, or panic after a generous deadline.
    fn run_until(manager: &mut TerminalManager, done: impl Fn(&TerminalManager) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !done(manager) {
            assert!(std::time::Instant::now() < deadline, "timed out");
            manager.process_all().unwrap();
            manager.cleanup_exited();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_pane_closes_when_its_program_exits_by_default() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let exiting = tab_that_exits(&mut manager);
        assert_eq!(manager.tab_count(), 2);

        run_until(&mut manager, |m| m.terminal(exiting).is_none());

        assert_eq!(manager.tab_count(), 1);
    }

    #[test]
    fn without_close_on_exit_an_exited_pane_stays_with_a_note() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.set_close_on_exit(false);
        let exiting = tab_that_exits(&mut manager);

        let shows_note = |m: &TerminalManager| {
            let terminal = m.terminal(exiting).unwrap().event_loop.terminal();
            let shown = terminal
                .grid()
                .rows()
                .any(|row| row.text().contains("[process exited with status 0]"));
            shown
        };
        run_until(&mut manager, shows_note);

        assert_eq!(manager.tab_count(), 2);
        assert!(manager.terminal(exiting).unwrap().has_exited());
        assert!(!manager.all_exited(), "the window would close");

        // Closing the tab by hand still works.
        let tab = manager.active_tab().unwrap().id;
        assert!(manager.close_tab(tab));
        assert!(manager.terminal(exiting).is_none());
    }

    #[test]
    fn scrollback_lines_apply_to_open_and_new_terminals() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let max_history = |m: &TerminalManager, id| {
            m.terminal(id)
                .unwrap()
                .event_loop
                .terminal()
                .buffer()
                .max_history()
        };
        assert_eq!(
            max_history(&manager, first),
            crate::grid::DEFAULT_SCROLLBACK
        );

        manager.set_scrollback_lines(500);
        manager.create_tab().unwrap();
        let second = manager.focused_terminal_id().unwrap();

        assert_eq!(max_history(&manager, first), 500);
        assert_eq!(max_history(&manager, second), 500);
    }

    #[test]
    fn cleanup_exited_repairs_active_tab_when_active_tab_was_removed() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;
        let first_terminal = manager.tabs[0].focused_terminal;
        let second_tab = manager.tabs[1].id;
        let second_terminal = manager.tabs[1].focused_terminal;

        manager.active_tab = second_tab;
        manager.tabs[1].remove_terminal(second_terminal);
        manager
            .terminals
            .retain(|terminal| terminal.id != second_terminal);

        manager.cleanup_exited();

        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.active_tab, first_tab);
        assert_eq!(
            manager.focused_terminal().map(|terminal| terminal.id),
            Some(first_terminal)
        );
    }

    #[test]
    fn switch_to_tab_by_index() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;

        manager.switch_to_tab(0);
        assert_eq!(manager.active_tab, first_tab);
    }

    /// Value of `key` in the child's environment once it has exec'd, or
    /// `None` if it never shows up within the timeout.
    #[cfg(target_os = "linux")]
    fn wait_for_child_env(pid: u32, key: &str) -> Option<String> {
        let prefix = format!("{key}=");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) {
                for entry in environ.split(|byte| *byte == 0) {
                    let entry = String::from_utf8_lossy(entry);
                    if let Some(value) = entry.strip_prefix(&prefix) {
                        return Some(value.to_string());
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[cfg(target_os = "linux")]
    fn env_for_tab(tab_id: TabId) -> io::Result<TabEnv> {
        Ok(vec![(
            OsString::from("ULTTY_TAB_ENV_TEST"),
            OsString::from(format!("tab-{}", tab_id.raw())),
        )])
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn tab_env_reaches_the_tab_shell_and_its_split_panes() {
        let mut seen = Vec::new();
        let mut manager = TerminalManager::with_tab_env(80, 24, &mut |tab_id| {
            seen.push(tab_id);
            env_for_tab(tab_id)
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![TabId::new(0)],
            "env fn is called once with the new tab id"
        );

        let first_tab = manager.active_tab().unwrap();
        assert_eq!(first_tab.shell_env, env_for_tab(TabId::new(0)).unwrap());
        let first_pid = manager.focused_terminal().unwrap().event_loop.child_pid();
        assert_eq!(
            wait_for_child_env(first_pid, "ULTTY_TAB_ENV_TEST").as_deref(),
            Some("tab-0"),
            "the tab's shell sees its env"
        );

        // A pane split off the tab inherits the tab's env without the env
        // fn being consulted again.
        let pane = manager.split_horizontal().unwrap().unwrap();
        let pane_pid = manager.terminal(pane).unwrap().event_loop.child_pid();
        assert_eq!(
            wait_for_child_env(pane_pid, "ULTTY_TAB_ENV_TEST").as_deref(),
            Some("tab-0")
        );

        // A second tab gets its own env, and the first tab's is unchanged.
        let second = manager.create_tab_with_env(&mut env_for_tab).unwrap();
        assert_eq!(second, TabId::new(1));
        let second_pid = manager.focused_terminal().unwrap().event_loop.child_pid();
        assert_eq!(
            wait_for_child_env(second_pid, "ULTTY_TAB_ENV_TEST").as_deref(),
            Some("tab-1")
        );
        assert_eq!(
            manager.active_tab().unwrap().shell_env,
            env_for_tab(TabId::new(1)).unwrap()
        );
    }

    #[test]
    fn tab_env_error_aborts_tab_creation() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let err = manager
            .create_tab_with_env(&mut |_| Err(io::Error::other("no socket")))
            .unwrap_err();
        assert_eq!(err.to_string(), "no socket");
        assert_eq!(manager.tab_count(), 1, "failed tab is not added");
    }
}
