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
}

impl ManagedTerminal {
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
    /// Terminal IDs in this tab (first is primary/focused in simple case)
    pub terminals: Vec<TerminalId>,
    /// Currently focused terminal in this tab
    pub focused_terminal: TerminalId,
    /// Tab title (often the title of the focused terminal)
    pub title: String,
    /// How panes are arranged in this tab
    pub split_direction: SplitDirection,
    /// Relative size of each pane (normalized to sum to 1.0).
    /// Length matches `terminals`. Empty means equal distribution.
    pub pane_ratios: Vec<f32>,
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
            split_direction: SplitDirection::default(),
            pane_ratios: Vec::new(),
            shell_env: Vec::new(),
        }
    }

    /// Add a terminal to this tab (for pane splitting).
    pub fn add_terminal(&mut self, terminal_id: TerminalId) {
        self.terminals.push(terminal_id);
        // Reset to equal distribution when a pane is added
        self.pane_ratios.clear();
    }

    /// Remove a terminal from this tab.
    pub fn remove_terminal(&mut self, terminal_id: TerminalId) -> bool {
        if let Some(pos) = self.terminals.iter().position(|&id| id == terminal_id) {
            self.terminals.remove(pos);

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

    fn push_terminal(&mut self, terminal: ManagedTerminal) {
        if let Some(waker) = &self.waker {
            terminal.event_loop.set_waker(Arc::clone(waker));
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

    /// Remove exited terminals and their empty tabs.
    pub fn cleanup_exited(&mut self) {
        // Find exited terminals
        let exited: Vec<TerminalId> = self
            .terminals
            .iter()
            .filter(|t| t.has_exited())
            .map(|t| t.id)
            .collect();

        for terminal_id in exited {
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
    }

    fn repair_active_tab(&mut self) {
        if self.tabs.iter().any(|tab| tab.id == self.active_tab) {
            return;
        }

        if let Some(tab) = self.tabs.first() {
            self.active_tab = tab.id;
        }
    }

    /// Check if all terminals have exited.
    pub fn all_exited(&self) -> bool {
        self.terminals.is_empty() || self.terminals.iter().all(|t| t.has_exited())
    }

    /// Split the current pane horizontally (side by side).
    pub fn split_horizontal(&mut self) -> io::Result<Option<TerminalId>> {
        let Some(tab) = self.active_tab_mut() else {
            return Ok(None);
        };
        let tab_id = tab.id;

        // Set split direction if this is the first split
        if tab.terminals.len() == 1 {
            tab.split_direction = SplitDirection::Horizontal;
        }

        let terminal_id = self.create_terminal(tab_id)?;

        if let Some(tab) = self.active_tab_mut() {
            tab.add_terminal(terminal_id);
        }

        Ok(Some(terminal_id))
    }

    /// Split the current pane vertically (stacked).
    pub fn split_vertical(&mut self) -> io::Result<Option<TerminalId>> {
        let Some(tab) = self.active_tab_mut() else {
            return Ok(None);
        };
        let tab_id = tab.id;

        // Set split direction if this is the first split
        if tab.terminals.len() == 1 {
            tab.split_direction = SplitDirection::Vertical;
        }

        let terminal_id = self.create_terminal(tab_id)?;

        if let Some(tab) = self.active_tab_mut() {
            tab.add_terminal(terminal_id);
        }

        Ok(Some(terminal_id))
    }

    /// Get the number of panes in the active tab.
    pub fn pane_count(&self) -> usize {
        self.active_tab().map(|t| t.terminals.len()).unwrap_or(0)
    }

    /// Get the split direction of the active tab.
    pub fn split_direction(&self) -> SplitDirection {
        self.active_tab()
            .map(|t| t.split_direction)
            .unwrap_or_default()
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

        let child = Child::spawn(
            command,
            &full_args,
            &crate::pty::current_dir(),
            PtySize::new(self.default_cols, self.default_rows),
            env,
        )
        .map_err(io::Error::other)?;
        let event_loop =
            TerminalEventLoop::from_child(child, self.default_cols, self.default_rows)?;

        // Change working directory if specified
        if let Some(dir) = working_dir {
            // Working dir is handled in spawn, but we can set it for display
            let _ = dir; // Already passed to spawn
        }

        let terminal = ManagedTerminal {
            id: terminal_id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", terminal_id.0),
        };

        self.push_terminal(terminal);
        Ok(terminal_id)
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

    /// Get the pane ratios for the active tab. If empty, returns equal distribution.
    pub fn pane_ratios(&self) -> Vec<f32> {
        let Some(tab) = self.active_tab() else {
            return vec![1.0];
        };
        if tab.pane_ratios.is_empty() {
            let n = tab.terminals.len().max(1);
            vec![1.0 / n as f32; n]
        } else {
            tab.pane_ratios.clone()
        }
    }

    /// Resize a pane divider. `divider_index` is the border between pane[i] and pane[i+1].
    /// `delta` is the amount to move in pixels; `total_size` is the available dimension.
    pub fn resize_pane(&mut self, divider_index: usize, delta: f32, total_size: f32) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        let count = tab.terminals.len();
        if divider_index >= count.saturating_sub(1) || total_size <= 0.0 {
            return;
        }

        // Initialize ratios if empty
        if tab.pane_ratios.is_empty() {
            tab.pane_ratios = vec![1.0 / count as f32; count];
        }

        let delta_ratio = delta / total_size;
        let min_ratio = 0.05; // 5% minimum pane size

        let new_left = (tab.pane_ratios[divider_index] + delta_ratio).max(min_ratio);
        let new_right = (tab.pane_ratios[divider_index + 1] - delta_ratio).max(min_ratio);

        // Renormalize only the two affected panes
        let old_sum = tab.pane_ratios[divider_index] + tab.pane_ratios[divider_index + 1];
        let new_sum = new_left + new_right;
        tab.pane_ratios[divider_index] = new_left * old_sum / new_sum;
        tab.pane_ratios[divider_index + 1] = new_right * old_sum / new_sum;
    }

    /// Given a click position in pixels relative to the content area (below tab bar),
    /// determine which pane index was clicked. Returns None if only 1 pane or out of bounds.
    pub fn pane_at_position(
        &self,
        x: f32,
        y: f32,
        available_width: f32,
        available_height: f32,
    ) -> Option<usize> {
        let tab = self.active_tab()?;
        let count = tab.terminals.len();
        if count <= 1 {
            return None;
        }

        let ratios = self.pane_ratios();
        let (pos, total) = match tab.split_direction {
            SplitDirection::Horizontal => (x, available_width),
            SplitDirection::Vertical => (y, available_height),
        };

        let mut cumulative = 0.0;
        for (i, ratio) in ratios.iter().enumerate() {
            cumulative += ratio * total;
            if pos < cumulative {
                return Some(i);
            }
        }
        Some(count - 1)
    }

    /// Given a position, find the divider index being hovered (within a tolerance).
    /// Returns the divider index (border between pane[i] and pane[i+1]).
    pub fn divider_at_position(&self, pos: f32, total: f32, tolerance: f32) -> Option<usize> {
        let ratios = self.pane_ratios();
        let mut cumulative = 0.0;
        for (i, ratio) in ratios.iter().enumerate() {
            cumulative += ratio * total;
            if i < ratios.len() - 1 && (pos - cumulative).abs() < tolerance {
                return Some(i);
            }
        }
        None
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
                    split_direction: match tab.split_direction {
                        SplitDirection::Horizontal => "Horizontal".to_string(),
                        SplitDirection::Vertical => "Vertical".to_string(),
                    },
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

            let split_direction = match tab_data.split_direction.as_str() {
                "Vertical" => SplitDirection::Vertical,
                _ => SplitDirection::Horizontal,
            };

            let tab = Tab {
                id: tab_id,
                terminals: terminal_ids,
                focused_terminal: focused_terminal_id.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "no focused terminal in restored tab",
                    )
                })?,
                title: tab_data.title.clone(),
                split_direction,
                pane_ratios: Vec::new(),
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
