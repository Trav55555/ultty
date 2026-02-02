//! Tab bar state and rendering helpers.
//!
//! This module provides the data structures for rendering a tab bar UI.

use crate::terminal_manager::{Tab, TabId};

/// Height of the tab bar in pixels (at default scale).
pub const TAB_BAR_HEIGHT: f32 = 28.0;

/// Padding inside each tab.
pub const TAB_PADDING: f32 = 12.0;

/// Minimum tab width.
pub const MIN_TAB_WIDTH: f32 = 80.0;

/// Maximum tab width.
pub const MAX_TAB_WIDTH: f32 = 200.0;

/// Tab bar visual state for rendering.
#[derive(Debug, Clone)]
pub struct TabBarState {
    /// Individual tab states
    pub tabs: Vec<TabState>,
    /// Currently active tab index
    pub active_index: usize,
    /// Total width available for the tab bar
    pub total_width: f32,
    /// Whether the tab bar is visible
    pub visible: bool,
    /// Hovered tab index (if any)
    pub hovered_tab: Option<usize>,
    /// Tab being dragged (index)
    pub dragging_tab: Option<usize>,
    /// Drag start X position
    pub drag_start_x: f32,
    /// Current drag X offset
    pub drag_offset_x: f32,
}

impl TabBarState {
    /// Create a new tab bar state.
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_index: 0,
            total_width: 0.0,
            visible: true,
            hovered_tab: None,
            dragging_tab: None,
            drag_start_x: 0.0,
            drag_offset_x: 0.0,
        }
    }

    /// Update the tab bar state from terminal manager tabs.
    pub fn update(&mut self, tabs: &[Tab], active_tab: TabId, total_width: f32) {
        self.total_width = total_width;
        self.tabs.clear();

        // Calculate tab widths
        let tab_count = tabs.len();
        if tab_count == 0 {
            self.visible = false;
            return;
        }

        // Only show tab bar with multiple tabs
        self.visible = tab_count > 1;

        let ideal_width = total_width / tab_count as f32;
        let tab_width = ideal_width.clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH);

        let mut x = 0.0;
        for (index, tab) in tabs.iter().enumerate() {
            let is_active = tab.id == active_tab;
            if is_active {
                self.active_index = index;
            }

            self.tabs.push(TabState {
                id: tab.id,
                title: tab.title.clone(),
                x,
                width: tab_width,
                is_active,
                is_hovered: self.hovered_tab == Some(index),
            });

            x += tab_width;
        }
    }

    /// Get the tab at a given x position.
    pub fn tab_at_x(&self, x: f32) -> Option<usize> {
        for (index, tab) in self.tabs.iter().enumerate() {
            if x >= tab.x && x < tab.x + tab.width {
                return Some(index);
            }
        }
        None
    }

    /// Set the hovered tab based on mouse position.
    pub fn set_hover(&mut self, x: f32, y: f32) {
        if !self.visible || y > TAB_BAR_HEIGHT {
            self.hovered_tab = None;
            return;
        }

        self.hovered_tab = self.tab_at_x(x);

        // Update tab states
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            tab.is_hovered = self.hovered_tab == Some(index);
        }
    }

    /// Check if a click at (x, y) is on the close button of a tab.
    pub fn close_button_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.visible || y > TAB_BAR_HEIGHT {
            return None;
        }

        for (index, tab) in self.tabs.iter().enumerate() {
            // Close button is on the right side of the tab
            let close_x = tab.x + tab.width - TAB_PADDING - 12.0;
            if x >= close_x && x < close_x + 12.0 {
                return Some(index);
            }
        }
        None
    }

    /// Get the height of the tab bar (0 if not visible).
    pub fn height(&self) -> f32 {
        if self.visible {
            TAB_BAR_HEIGHT
        } else {
            0.0
        }
    }

    /// Start dragging a tab.
    pub fn start_drag(&mut self, index: usize, x: f32) {
        if index < self.tabs.len() {
            self.dragging_tab = Some(index);
            self.drag_start_x = x;
            self.drag_offset_x = 0.0;
        }
    }

    /// Update drag position. Returns Some((from, to)) if tab should be moved.
    pub fn update_drag(&mut self, x: f32) -> Option<(usize, usize)> {
        let dragging_index = self.dragging_tab?;

        self.drag_offset_x = x - self.drag_start_x;

        // Check if we should swap with adjacent tabs
        let tab_width = if let Some(tab) = self.tabs.get(dragging_index) {
            tab.width
        } else {
            return None;
        };

        // If dragged more than half a tab width, swap
        if self.drag_offset_x > tab_width / 2.0 && dragging_index < self.tabs.len() - 1 {
            // Move right
            let new_index = dragging_index + 1;
            self.dragging_tab = Some(new_index);
            self.drag_start_x = x;
            self.drag_offset_x = 0.0;
            return Some((dragging_index, new_index));
        } else if self.drag_offset_x < -tab_width / 2.0 && dragging_index > 0 {
            // Move left
            let new_index = dragging_index - 1;
            self.dragging_tab = Some(new_index);
            self.drag_start_x = x;
            self.drag_offset_x = 0.0;
            return Some((dragging_index, new_index));
        }

        None
    }

    /// End dragging.
    pub fn end_drag(&mut self) {
        self.dragging_tab = None;
        self.drag_offset_x = 0.0;
    }

    /// Check if currently dragging.
    pub fn is_dragging(&self) -> bool {
        self.dragging_tab.is_some()
    }
}

impl Default for TabBarState {
    fn default() -> Self {
        Self::new()
    }
}

/// Visual state for a single tab.
#[derive(Debug, Clone)]
pub struct TabState {
    /// Tab ID
    pub id: TabId,
    /// Display title
    pub title: String,
    /// X position in pixels
    pub x: f32,
    /// Width in pixels
    pub width: f32,
    /// Whether this is the active tab
    pub is_active: bool,
    /// Whether this tab is hovered
    pub is_hovered: bool,
}

impl TabState {
    /// Get the bounds of this tab.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        (self.x, 0.0, self.width, TAB_BAR_HEIGHT)
    }

    /// Get the text rendering position.
    pub fn text_position(&self) -> (f32, f32) {
        (self.x + TAB_PADDING, TAB_BAR_HEIGHT / 2.0 - 6.0)
    }

    /// Get the maximum text width.
    pub fn max_text_width(&self) -> f32 {
        // Leave space for close button
        self.width - TAB_PADDING * 2.0 - 16.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_manager::TerminalId;

    fn make_tabs(count: usize) -> Vec<Tab> {
        (0..count)
            .map(|i| Tab::new(TabId::new(i), TerminalId::new(i)))
            .collect()
    }

    #[test]
    fn single_tab_not_visible() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(1);
        state.update(&tabs, TabId::new(0), 800.0);

        assert!(!state.visible);
    }

    #[test]
    fn multiple_tabs_visible() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(3);
        state.update(&tabs, TabId::new(1), 800.0);

        assert!(state.visible);
        assert_eq!(state.tabs.len(), 3);
        assert_eq!(state.active_index, 1);
    }

    #[test]
    fn tab_at_position() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(3);
        state.update(&tabs, TabId::new(0), 600.0);

        // With 600px width and 3 tabs, each tab is 200px
        assert_eq!(state.tab_at_x(50.0), Some(0));
        assert_eq!(state.tab_at_x(250.0), Some(1));
        assert_eq!(state.tab_at_x(450.0), Some(2));
        assert_eq!(state.tab_at_x(650.0), None);
    }

    #[test]
    fn hover_state() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(3);
        state.update(&tabs, TabId::new(0), 600.0);

        state.set_hover(250.0, 10.0);
        assert_eq!(state.hovered_tab, Some(1));
        assert!(state.tabs[1].is_hovered);
        assert!(!state.tabs[0].is_hovered);
    }

    #[test]
    fn height_calculation() {
        let mut state = TabBarState::new();

        // Single tab - no bar
        let tabs = make_tabs(1);
        state.update(&tabs, TabId::new(0), 800.0);
        assert_eq!(state.height(), 0.0);

        // Multiple tabs - show bar
        let tabs = make_tabs(2);
        state.update(&tabs, TabId::new(0), 800.0);
        assert_eq!(state.height(), TAB_BAR_HEIGHT);
    }
}
