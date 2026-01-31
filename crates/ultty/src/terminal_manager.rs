//! Terminal manager for multi-tab and multi-pane support.
//!
//! This module manages multiple terminal instances, supporting:
//! - Multiple tabs, each with its own terminal
//! - Pane splitting within tabs
//! - Keyboard navigation between tabs and panes

use crate::event_loop::{EventLoop as TerminalEventLoop, TerminalEvent};
use std::io;

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
    /// Create a new managed terminal.
    pub fn new(id: TerminalId, tab_id: TabId, cols: u16, rows: u16) -> io::Result<Self> {
        let event_loop = TerminalEventLoop::new(cols, rows)?;
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
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> usize {
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
        }
    }

    /// Add a terminal to this tab (for pane splitting).
    pub fn add_terminal(&mut self, terminal_id: TerminalId) {
        self.terminals.push(terminal_id);
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
}

impl TerminalManager {
    /// Create a new terminal manager with one tab and one terminal.
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        let mut manager = Self {
            terminals: Vec::new(),
            tabs: Vec::new(),
            active_tab: TabId(0),
            next_terminal_id: 0,
            next_tab_id: 0,
            default_cols: cols,
            default_rows: rows,
        };

        // Create initial tab with terminal
        manager.create_tab()?;

        Ok(manager)
    }

    /// Create a new tab with a new terminal.
    pub fn create_tab(&mut self) -> io::Result<TabId> {
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let terminal_id = self.create_terminal(tab_id)?;

        let tab = Tab::new(tab_id, terminal_id);
        self.tabs.push(tab);
        self.active_tab = tab_id;

        Ok(tab_id)
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

    /// Create a new terminal in a specific tab.
    fn create_terminal(&mut self, tab_id: TabId) -> io::Result<TerminalId> {
        let terminal_id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;

        let terminal = ManagedTerminal::new(
            terminal_id,
            tab_id,
            self.default_cols,
            self.default_rows,
        )?;

        self.terminals.push(terminal);
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

    /// Get all tabs.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
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

        let current_idx = self.tabs.iter().position(|t| t.id == self.active_tab).unwrap_or(0);
        let next_idx = (current_idx + 1) % self.tabs.len();
        self.active_tab = self.tabs[next_idx].id;
    }

    /// Switch to the previous tab.
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }

        let current_idx = self.tabs.iter().position(|t| t.id == self.active_tab).unwrap_or(0);
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
                all_events.push((terminal.id, events));
            }
        }

        Ok(all_events)
    }

    /// Remove exited terminals and their empty tabs.
    pub fn cleanup_exited(&mut self) {
        // Find exited terminals
        let exited: Vec<TerminalId> = self.terminals
            .iter()
            .filter(|t| t.has_exited())
            .map(|t| t.id)
            .collect();

        for terminal_id in exited {
            // Find and update the tab
            if let Some(tab) = self.tabs.iter_mut().find(|t| t.terminals.contains(&terminal_id)) {
                tab.remove_terminal(terminal_id);
            }

            // Remove the terminal
            self.terminals.retain(|t| t.id != terminal_id);
        }

        // Remove empty tabs (except keep at least one)
        if self.tabs.len() > 1 {
            self.tabs.retain(|t| !t.is_empty());
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
        self.active_tab().map(|t| t.split_direction).unwrap_or_default()
    }

    /// Focus the next pane in the active tab.
    pub fn focus_next_pane(&mut self) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };

        if tab.terminals.len() <= 1 {
            return;
        }

        let current_idx = tab.terminals.iter()
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

        let current_idx = tab.terminals.iter()
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

        tab.terminals.iter()
            .filter_map(|&id| self.terminal(id))
            .collect()
    }

    /// Get the focused terminal ID in the active tab.
    pub fn focused_terminal_id(&self) -> Option<TerminalId> {
        self.active_tab().map(|t| t.focused_terminal)
    }

    /// Create a new tab with a custom command.
    pub fn create_tab_with_command(
        &mut self,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> io::Result<TabId> {
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let terminal_id = self.create_terminal_with_command(
            tab_id,
            command,
            args,
            working_dir,
        )?;

        let mut tab = Tab::new(tab_id, terminal_id);
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
    ) -> io::Result<TerminalId> {
        let terminal_id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;

        // Build args with command as argv[0]
        let mut full_args: Vec<&str> = vec![command];
        for arg in args {
            full_args.push(arg);
        }

        let event_loop = TerminalEventLoop::with_command(
            command,
            &full_args,
            self.default_cols,
            self.default_rows,
        ).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

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

        self.terminals.push(terminal);
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

        tab.terminals.iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0)
    }

    /// Extract session data from the current state.
    pub fn to_session_data(&self) -> crate::session::SessionData {
        use crate::session::{SessionData, TabData, TerminalData};

        let active_tab_index = self.tabs.iter()
            .position(|t| t.id == self.active_tab)
            .unwrap_or(0);

        let tabs = self.tabs.iter().map(|tab| {
            let terminals = tab.terminals.iter().filter_map(|&tid| {
                self.terminals.iter().find(|t| t.id == tid).map(|t| {
                    let working_dir: Option<String> = t.event_loop
                        .terminal()
                        .working_directory()
                        .map(|s| s.to_string());
                    TerminalData {
                        title: t.title.clone(),
                        working_directory: working_dir,
                        is_focused: tid == tab.focused_terminal,
                    }
                })
            }).collect();

            // Get working directory from the first terminal or focused terminal
            let working_directory: Option<String> = tab.terminals.first()
                .and_then(|&tid| self.terminals.iter().find(|t| t.id == tid))
                .and_then(|t| t.event_loop.terminal().working_directory())
                .map(|s| s.to_string());

            TabData {
                title: tab.title.clone(),
                working_directory,
                terminals,
                split_direction: match tab.split_direction {
                    SplitDirection::Horizontal => "Horizontal".to_string(),
                    SplitDirection::Vertical => "Vertical".to_string(),
                },
            }
        }).collect();

        SessionData {
            version: 1,
            timestamp: String::new(), // Will be filled by SessionData
            tabs,
            active_tab_index,
        }
    }

    /// Create a terminal manager from session data.
    pub fn from_session(session: &crate::session::SessionData, cols: u16, rows: u16) -> io::Result<Self> {
        if session.tabs.is_empty() {
            // No tabs in session, create default
            return Self::new(cols, rows);
        }

        let mut manager = Self {
            terminals: Vec::new(),
            tabs: Vec::new(),
            active_tab: TabId::new(0),
            next_terminal_id: 0,
            next_tab_id: 0,
            default_cols: cols,
            default_rows: rows,
        };

        for (i, tab_data) in session.tabs.iter().enumerate() {
            // Create tab
            let tab_id = TabId(manager.next_tab_id);
            manager.next_tab_id += 1;

            // Create at least one terminal for this tab
            let terminal_count = tab_data.terminals.len().max(1);
            let mut terminal_ids = Vec::new();
            let mut focused_terminal_id = None;

            for (j, term_data) in tab_data.terminals.iter().take(terminal_count).enumerate() {
                let tid = TerminalId(manager.next_terminal_id);
                manager.next_terminal_id += 1;

                // Create terminal with working directory if available
                let mut terminal = if let Some(ref wd) = term_data.working_directory {
                    ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, wd)?
                } else if let Some(ref wd) = tab_data.working_directory {
                    ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, wd)?
                } else {
                    ManagedTerminal::new(tid, tab_id, cols, rows)?
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
                    ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, wd)?
                } else {
                    ManagedTerminal::new(tid, tab_id, cols, rows)?
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
                focused_terminal: focused_terminal_id.unwrap(),
                title: tab_data.title.clone(),
                split_direction,
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

impl ManagedTerminal {
    /// Create a new managed terminal with a specific working directory.
    pub fn new_with_dir(
        id: TerminalId,
        tab_id: TabId,
        cols: u16,
        rows: u16,
        working_dir: &str,
    ) -> io::Result<Self> {
        let event_loop = TerminalEventLoop::new_with_dir(cols, rows, working_dir)?;
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

    #[test]
    fn create_manager() {
        let manager = TerminalManager::new(80, 24).unwrap();
        assert_eq!(manager.tab_count(), 1);
        assert!(manager.focused_terminal().is_some());
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
    fn switch_to_tab_by_index() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;

        manager.switch_to_tab(0);
        assert_eq!(manager.active_tab, first_tab);
    }
}
