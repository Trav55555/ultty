//! Tab bar state and rendering helpers.
//!
//! This module provides the data structures for rendering a tab bar UI.

use crate::terminal_manager::{Tab, TabId};

/// Height of the window control band in pixels (at default scale).
pub const TITLE_BAR_HEIGHT: f32 = 28.0;

/// Height of the tab row in pixels (at default scale).
pub const TAB_ROW_HEIGHT: f32 = 28.0;

/// Height of the complete chrome area in pixels (at default scale).
pub const TAB_BAR_HEIGHT: f32 = TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT;

/// Padding inside each tab.
pub const TAB_PADDING: f32 = 12.0;

/// Minimum tab width.
pub const MIN_TAB_WIDTH: f32 = 80.0;

/// Maximum tab width.
pub const MAX_TAB_WIDTH: f32 = 200.0;

/// Width reserved for window control buttons (close, maximize, minimize) on the right.
pub const WINDOW_CONTROLS_WIDTH: f32 = 100.0;

/// Width of each window control button.
pub const WINDOW_BUTTON_WIDTH: f32 = 32.0;

/// Which window control button was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControl {
    Minimize,
    Maximize,
    Close,
}

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
    /// X position of the "+" new tab button in the title/control band.
    pub new_tab_button_x: f32,
    /// Whether the "+" button is hovered.
    pub new_tab_button_hovered: bool,
    /// Which window control button is hovered (if any).
    pub hovered_window_control: Option<WindowControl>,
    /// Which tab's close button is hovered (if any).
    pub hovered_close_button: Option<usize>,
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
            new_tab_button_x: 0.0,
            new_tab_button_hovered: false,
            hovered_window_control: None,
            hovered_close_button: None,
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

        // Chrome is always visible (CSD title band plus tab row).
        // Individual tab entries only render when there are 2+ tabs.
        self.visible = true;

        // Layout: title/control band contains [+][drag area][minimize][maximize][close].
        // The lower tab row contains only tabs plus trailing drag area.
        let tabs_start_x = 0.0;
        let usable_width = total_width;
        let ideal_width = usable_width / tab_count as f32;
        let tab_width = ideal_width.clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH);

        // "+" button is at the left edge
        self.new_tab_button_x = 0.0;

        let mut x = tabs_start_x;
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

    /// Width of the new-tab "+" button.
    pub const NEW_TAB_BUTTON_WIDTH: f32 = 28.0;

    /// Whether the lower tab row should be visible.
    pub fn shows_tab_row(&self) -> bool {
        self.tabs.len() > 1
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

    /// Check if a click at (x, y) is on the new tab "+" button.
    pub fn is_new_tab_button_at(&self, x: f32, y: f32) -> bool {
        self.visible
            && (0.0..=TITLE_BAR_HEIGHT).contains(&y)
            && x >= self.new_tab_button_x
            && x < self.new_tab_button_x + Self::NEW_TAB_BUTTON_WIDTH
    }

    /// Check if a click at (x, y) is on a window control button.
    pub fn window_control_at(&self, x: f32, y: f32) -> Option<WindowControl> {
        if !self.visible || y > TITLE_BAR_HEIGHT {
            return None;
        }
        let controls_start = self.total_width - WINDOW_CONTROLS_WIDTH;
        if x < controls_start {
            return None;
        }
        let offset = x - controls_start;
        // Layout: [minimize][maximize][close] — each WINDOW_BUTTON_WIDTH wide, centered
        let padding = (WINDOW_CONTROLS_WIDTH - WINDOW_BUTTON_WIDTH * 3.0) / 2.0;
        let btn_offset = offset - padding;
        if btn_offset < 0.0 {
            return None;
        }
        let btn_idx = (btn_offset / WINDOW_BUTTON_WIDTH) as usize;
        match btn_idx {
            0 => Some(WindowControl::Minimize),
            1 => Some(WindowControl::Maximize),
            2 => Some(WindowControl::Close),
            _ => None,
        }
    }

    /// Check if position is in the draggable title bar area (not on a tab, button, or control).
    pub fn is_drag_area(&self, x: f32, y: f32) -> bool {
        if !self.visible || y > self.height() {
            return false;
        }
        // Not on window controls
        if self.window_control_at(x, y).is_some() {
            return false;
        }
        // Not on "+" button
        if self.is_new_tab_button_at(x, y) {
            return false;
        }
        if y < TITLE_BAR_HEIGHT || !self.shows_tab_row() {
            return true;
        }
        // Not on a tab
        if self.tab_at_x(x).is_some() {
            return false;
        }
        true
    }

    /// Set the hovered tab based on mouse position.
    pub fn set_hover(&mut self, x: f32, y: f32) {
        if !self.visible || y > self.height() {
            self.hovered_tab = None;
            self.new_tab_button_hovered = false;
            self.hovered_window_control = None;
            self.hovered_close_button = None;
            return;
        }

        self.hovered_tab = if self.shows_tab_row() && y >= TITLE_BAR_HEIGHT {
            self.tab_at_x(x)
        } else {
            None
        };
        self.new_tab_button_hovered = self.is_new_tab_button_at(x, y);
        self.hovered_window_control = self.window_control_at(x, y);
        self.hovered_close_button = self.close_button_at(x, y);

        // Update tab states
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            tab.is_hovered = self.hovered_tab == Some(index);
        }
    }

    /// Check if a click at (x, y) is on the close button of a tab.
    pub fn close_button_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.visible
            || !self.shows_tab_row()
            || !(TITLE_BAR_HEIGHT..=TAB_BAR_HEIGHT).contains(&y)
        {
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
        if !self.visible {
            0.0
        } else if self.shows_tab_row() {
            TAB_BAR_HEIGHT
        } else {
            TITLE_BAR_HEIGHT
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
        if self.drag_offset_x > tab_width / 2.0 && dragging_index + 1 < self.tabs.len() {
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
        (self.x, TITLE_BAR_HEIGHT, self.width, TAB_ROW_HEIGHT)
    }

    /// Get the text rendering position.
    pub fn text_position(&self) -> (f32, f32) {
        (
            self.x + TAB_PADDING,
            TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT / 2.0 - 6.0,
        )
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

    // ---------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------

    fn make_tabs(count: usize) -> Vec<Tab> {
        (0..count)
            .map(|i| Tab::new(TabId::new(i), TerminalId::new(i)))
            .collect()
    }

    fn make_tabs_with_titles(titles: &[&str]) -> Vec<Tab> {
        titles
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut tab = Tab::new(TabId::new(i), TerminalId::new(i));
                tab.title = title.to_string();
                tab
            })
            .collect()
    }

    /// Build a TabBarState that has already been updated with `count` tabs.
    fn bar_with(count: usize, active: usize, width: f32) -> TabBarState {
        let mut state = TabBarState::new();
        let tabs = make_tabs(count);
        state.update(&tabs, TabId::new(active), width);
        state
    }

    // ---------------------------------------------------------------
    // TabBarState::new / Default
    // ---------------------------------------------------------------

    #[test]
    fn new_returns_empty_state() {
        let s = TabBarState::new();
        assert!(s.tabs.is_empty());
        assert_eq!(s.active_index, 0);
        assert_eq!(s.total_width, 0.0);
        assert!(s.visible);
        assert!(s.hovered_tab.is_none());
        assert!(s.dragging_tab.is_none());
        assert_eq!(s.drag_start_x, 0.0);
        assert_eq!(s.drag_offset_x, 0.0);
    }

    #[test]
    fn default_equals_new() {
        let a = TabBarState::new();
        let b = TabBarState::default();
        assert_eq!(a.tabs.len(), b.tabs.len());
        assert_eq!(a.active_index, b.active_index);
        assert_eq!(a.total_width, b.total_width);
        assert_eq!(a.visible, b.visible);
    }

    // ---------------------------------------------------------------
    // Visibility
    // ---------------------------------------------------------------

    #[test]
    fn zero_tabs_not_visible() {
        let mut state = TabBarState::new();
        let tabs: Vec<Tab> = vec![];
        state.update(&tabs, TabId::new(0), 800.0);
        assert!(!state.visible);
        assert!(state.tabs.is_empty());
    }

    #[test]
    fn single_tab_always_visible() {
        let state = bar_with(1, 0, 800.0);
        assert!(state.visible); // CSD: title bar always visible
    }

    #[test]
    fn two_tabs_visible() {
        let state = bar_with(2, 0, 800.0);
        assert!(state.visible);
        assert_eq!(state.tabs.len(), 2);
    }

    #[test]
    fn multiple_tabs_visible() {
        let state = bar_with(5, 2, 1000.0);
        assert!(state.visible);
        assert_eq!(state.tabs.len(), 5);
        assert_eq!(state.active_index, 2);
    }

    // ---------------------------------------------------------------
    // Height
    // ---------------------------------------------------------------

    #[test]
    fn height_always_nonzero() {
        let state = bar_with(1, 0, 800.0);
        assert_eq!(state.height(), TITLE_BAR_HEIGHT); // CSD title band stays visible
    }

    #[test]
    fn height_nonzero_when_visible() {
        let state = bar_with(3, 0, 800.0);
        assert_eq!(state.height(), TAB_BAR_HEIGHT);
    }

    #[test]
    fn tab_row_only_shows_for_multiple_tabs() {
        assert!(!bar_with(1, 0, 800.0).shows_tab_row());
        assert!(bar_with(2, 0, 800.0).shows_tab_row());
    }

    // ---------------------------------------------------------------
    // Tab width clamping
    // ---------------------------------------------------------------

    #[test]
    fn tab_width_clamped_to_max() {
        // 2 tabs in 1000px => ideal 500px, clamped to MAX_TAB_WIDTH (200)
        let state = bar_with(2, 0, 1000.0);
        for tab in &state.tabs {
            assert_eq!(tab.width, MAX_TAB_WIDTH);
        }
    }

    #[test]
    fn tab_width_clamped_to_min() {
        // 20 tabs in 800px => ideal 40px, clamped to MIN_TAB_WIDTH (80)
        let state = bar_with(20, 0, 800.0);
        for tab in &state.tabs {
            assert_eq!(tab.width, MIN_TAB_WIDTH);
        }
    }

    #[test]
    fn tab_width_in_range_not_clamped() {
        // 5 tabs in 750px => ideal 150 => between 80 and 200
        let state = bar_with(5, 0, 750.0);
        let usable = 750.0;
        let expected = usable / 5.0;
        for tab in &state.tabs {
            assert!(
                (tab.width - expected).abs() < 0.1,
                "expected {} got {}",
                expected,
                tab.width
            );
        }
    }

    // ---------------------------------------------------------------
    // Tab x positions
    // ---------------------------------------------------------------

    #[test]
    fn tab_x_positions_are_sequential() {
        let state = bar_with(4, 0, 600.0);
        let w = state.tabs[0].width;
        let offset = 0.0;
        for (i, tab) in state.tabs.iter().enumerate() {
            let expected_x = offset + i as f32 * w;
            assert!(
                (tab.x - expected_x).abs() < f32::EPSILON,
                "tab {} x: expected {} got {}",
                i,
                expected_x,
                tab.x,
            );
        }
    }

    #[test]
    fn first_tab_starts_at_tab_rail_edge() {
        let state = bar_with(3, 0, 900.0);
        assert_eq!(state.tabs[0].x, 0.0);
    }

    // ---------------------------------------------------------------
    // Active tab tracking
    // ---------------------------------------------------------------

    #[test]
    fn active_index_tracks_active_tab_id() {
        for active in 0..5 {
            let state = bar_with(5, active, 1000.0);
            assert_eq!(state.active_index, active);
            assert!(state.tabs[active].is_active);
            for (i, tab) in state.tabs.iter().enumerate() {
                assert_eq!(tab.is_active, i == active);
            }
        }
    }

    #[test]
    fn tab_ids_match_source() {
        let state = bar_with(3, 0, 900.0);
        for (i, tab) in state.tabs.iter().enumerate() {
            assert_eq!(tab.id, TabId::new(i));
        }
    }

    #[test]
    fn tab_titles_populated() {
        let titles = ["alpha", "beta", "gamma"];
        let mut state = TabBarState::new();
        let tabs = make_tabs_with_titles(&titles);
        state.update(&tabs, TabId::new(0), 900.0);
        for (i, tab) in state.tabs.iter().enumerate() {
            assert_eq!(tab.title, titles[i]);
        }
    }

    // ---------------------------------------------------------------
    // update clears old state
    // ---------------------------------------------------------------

    #[test]
    fn update_replaces_previous_tabs() {
        let mut state = TabBarState::new();
        let tabs3 = make_tabs(3);
        state.update(&tabs3, TabId::new(0), 600.0);
        assert_eq!(state.tabs.len(), 3);

        let tabs2 = make_tabs(2);
        state.update(&tabs2, TabId::new(1), 400.0);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.total_width, 400.0);
        assert_eq!(state.active_index, 1);
    }

    // ---------------------------------------------------------------
    // tab_at_x
    // ---------------------------------------------------------------

    #[test]
    fn tab_at_x_exact_boundaries() {
        let state = bar_with(3, 0, 600.0);
        let w = state.tabs[0].width;
        let offset = 0.0;

        // Left edge of first tab
        assert_eq!(state.tab_at_x(offset), Some(0));
        // Right edge of first tab (exclusive)
        assert_eq!(state.tab_at_x(offset + w), Some(1));
        // Last pixel of first tab
        assert_eq!(state.tab_at_x(offset + w - 0.001), Some(0));
        // Left edge of third tab
        assert_eq!(state.tab_at_x(offset + 2.0 * w), Some(2));
        // Past all tabs
        assert_eq!(state.tab_at_x(offset + 3.0 * w), None);
        // Before tabs
        assert_eq!(state.tab_at_x(-0.001), None);
    }

    #[test]
    fn tab_at_x_negative() {
        let state = bar_with(3, 0, 600.0);
        assert_eq!(state.tab_at_x(-1.0), None);
    }

    #[test]
    fn tab_at_x_empty() {
        let state = TabBarState::new();
        assert_eq!(state.tab_at_x(100.0), None);
    }

    // ---------------------------------------------------------------
    // set_hover
    // ---------------------------------------------------------------

    #[test]
    fn hover_sets_correct_tab() {
        let mut state = bar_with(3, 0, 600.0);
        state.set_hover(250.0, TITLE_BAR_HEIGHT + 10.0);
        assert_eq!(state.hovered_tab, Some(1));
        assert!(state.tabs[1].is_hovered);
        assert!(!state.tabs[0].is_hovered);
        assert!(!state.tabs[2].is_hovered);
    }

    #[test]
    fn hover_title_band_does_not_hover_tabs() {
        let mut state = bar_with(3, 0, 600.0);
        state.set_hover(250.0, 10.0);
        assert_eq!(state.hovered_tab, None);
        assert!(state.tabs.iter().all(|tab| !tab.is_hovered));
    }

    #[test]
    fn hover_below_bar_clears() {
        let mut state = bar_with(3, 0, 600.0);
        state.set_hover(100.0, TITLE_BAR_HEIGHT + 10.0);
        assert!(state.hovered_tab.is_some());

        // y > TAB_BAR_HEIGHT => no hover
        state.set_hover(100.0, TAB_BAR_HEIGHT + 1.0);
        assert!(state.hovered_tab.is_none());
    }

    #[test]
    fn hover_single_tab_visible() {
        let mut state = bar_with(1, 0, 800.0);
        assert!(state.visible);
        // Single-tab chrome has no tab rail to hover.
        let tab_x = state.tabs[0].x + 5.0;
        state.set_hover(tab_x, 10.0);
        assert_eq!(state.hovered_tab, None);
    }

    #[test]
    fn hover_updates_all_tab_flags() {
        let mut state = bar_with(4, 0, 800.0);
        // Hover over tab 2
        state.set_hover(state.tabs[2].x + 5.0, TITLE_BAR_HEIGHT + 5.0);
        for (i, tab) in state.tabs.iter().enumerate() {
            assert_eq!(tab.is_hovered, i == 2, "tab {} is_hovered mismatch", i);
        }
    }

    #[test]
    fn hover_persists_through_update() {
        let mut state = bar_with(3, 0, 600.0);
        state.hovered_tab = Some(1);

        // Re-update: hovered_tab persists and is applied to new TabStates
        let tabs = make_tabs(3);
        state.update(&tabs, TabId::new(0), 600.0);
        // After update, tab at index 1 should reflect the persisted hovered_tab
        assert!(state.tabs[1].is_hovered);
        assert!(!state.tabs[0].is_hovered);
    }

    // ---------------------------------------------------------------
    // chrome hit testing
    // ---------------------------------------------------------------

    #[test]
    fn single_tab_plus_is_clickable_in_title_band() {
        let state = bar_with(1, 0, 800.0);
        assert!(state.is_new_tab_button_at(5.0, 10.0));
        assert!(!state.is_drag_area(5.0, 10.0));
    }

    #[test]
    fn multi_tab_plus_is_clickable_in_title_band() {
        let state = bar_with(2, 0, 800.0);
        assert!(state.is_new_tab_button_at(5.0, 10.0));
        assert!(!state.is_new_tab_button_at(5.0, TITLE_BAR_HEIGHT + 10.0));
        assert!(!state.is_drag_area(5.0, 10.0));
        assert_eq!(state.tab_at_x(5.0), Some(0));
        assert!(!state.is_drag_area(5.0, TITLE_BAR_HEIGHT + 10.0));
    }

    #[test]
    fn title_band_between_controls_is_draggable() {
        let state = bar_with(2, 0, 800.0);
        assert!(state.is_drag_area(400.0, 10.0));
        assert!(!state.is_drag_area(790.0, 10.0));
    }

    // ---------------------------------------------------------------
    // close_button_at
    // ---------------------------------------------------------------

    #[test]
    fn close_button_hit() {
        let state = bar_with(3, 0, 600.0);
        // Close button is at: tab.x + tab.width - TAB_PADDING - 12.0
        let tab = &state.tabs[1];
        let close_x = tab.x + tab.width - TAB_PADDING - 12.0;

        // Hit the close button region
        assert_eq!(
            state.close_button_at(close_x + 5.0, TITLE_BAR_HEIGHT + 10.0),
            Some(1)
        );
    }

    #[test]
    fn close_button_miss_left() {
        let state = bar_with(3, 0, 600.0);
        let tab = &state.tabs[0];
        let close_x = tab.x + tab.width - TAB_PADDING - 12.0;
        // Just left of the close button
        assert_eq!(
            state.close_button_at(close_x - 1.0, TITLE_BAR_HEIGHT + 10.0),
            None
        );
    }

    #[test]
    fn close_button_miss_right() {
        let state = bar_with(3, 0, 600.0);
        let tab = &state.tabs[0];
        let close_x = tab.x + tab.width - TAB_PADDING - 12.0;
        // Just right of the close button
        assert_eq!(
            state.close_button_at(close_x + 12.0, TITLE_BAR_HEIGHT + 10.0),
            None
        );
    }

    #[test]
    fn close_button_below_bar_returns_none() {
        let state = bar_with(3, 0, 600.0);
        let tab = &state.tabs[0];
        let close_x = tab.x + tab.width - TAB_PADDING - 6.0;
        assert_eq!(state.close_button_at(close_x, TAB_BAR_HEIGHT + 1.0), None);
    }

    #[test]
    fn close_button_single_tab_hidden_with_tab_row() {
        let state = bar_with(1, 0, 800.0);
        assert!(state.visible);
        let tab = &state.tabs[0];
        let close_x = tab.x + tab.width - TAB_PADDING - 12.0;
        assert_eq!(state.close_button_at(close_x + 6.0, 10.0), None);
    }

    #[test]
    fn close_button_for_each_tab() {
        let state = bar_with(3, 0, 600.0);
        for (i, tab) in state.tabs.iter().enumerate() {
            let close_x = tab.x + tab.width - TAB_PADDING - 12.0;
            assert_eq!(
                state.close_button_at(close_x + 6.0, TITLE_BAR_HEIGHT + 10.0),
                Some(i),
                "close button for tab {}",
                i,
            );
        }
    }

    // ---------------------------------------------------------------
    // Drag and drop
    // ---------------------------------------------------------------

    #[test]
    fn start_drag_valid_index() {
        let mut state = bar_with(3, 0, 600.0);
        state.start_drag(1, 300.0);
        assert_eq!(state.dragging_tab, Some(1));
        assert_eq!(state.drag_start_x, 300.0);
        assert_eq!(state.drag_offset_x, 0.0);
        assert!(state.is_dragging());
    }

    #[test]
    fn start_drag_out_of_bounds() {
        let mut state = bar_with(3, 0, 600.0);
        state.start_drag(10, 300.0);
        assert!(state.dragging_tab.is_none());
        assert!(!state.is_dragging());
    }

    #[test]
    fn end_drag_clears_state() {
        let mut state = bar_with(3, 0, 600.0);
        state.start_drag(1, 300.0);
        state.drag_offset_x = 50.0;
        state.end_drag();
        assert!(state.dragging_tab.is_none());
        assert_eq!(state.drag_offset_x, 0.0);
        assert!(!state.is_dragging());
    }

    #[test]
    fn update_drag_no_swap_small_movement() {
        let mut state = bar_with(3, 0, 600.0);
        let w = state.tabs[0].width;
        state.start_drag(1, 300.0);
        // Move less than half a tab width
        let result = state.update_drag(300.0 + w / 4.0);
        assert!(result.is_none());
        assert_eq!(state.dragging_tab, Some(1));
    }

    #[test]
    fn update_drag_swap_right() {
        let mut state = bar_with(3, 0, 600.0);
        let w = state.tabs[0].width;
        state.start_drag(0, 100.0);
        // Move more than half a tab width to the right
        let result = state.update_drag(100.0 + w / 2.0 + 1.0);
        assert_eq!(result, Some((0, 1)));
        // dragging_tab should now point to new index
        assert_eq!(state.dragging_tab, Some(1));
    }

    #[test]
    fn update_drag_swap_left() {
        let mut state = bar_with(3, 0, 600.0);
        let w = state.tabs[0].width;
        state.start_drag(2, 500.0);
        // Move more than half a tab width to the left
        let result = state.update_drag(500.0 - w / 2.0 - 1.0);
        assert_eq!(result, Some((2, 1)));
        assert_eq!(state.dragging_tab, Some(1));
    }

    #[test]
    fn update_drag_no_swap_at_left_edge() {
        let mut state = bar_with(3, 0, 600.0);
        let w = state.tabs[0].width;
        state.start_drag(0, 50.0);
        // Try to drag left when at index 0
        let result = state.update_drag(50.0 - w);
        assert!(result.is_none());
        assert_eq!(state.dragging_tab, Some(0));
    }

    #[test]
    fn update_drag_no_swap_at_right_edge() {
        let mut state = bar_with(3, 0, 600.0);
        let w = state.tabs[0].width;
        state.start_drag(2, 500.0);
        // Try to drag right when at last index
        let result = state.update_drag(500.0 + w);
        assert!(result.is_none());
        assert_eq!(state.dragging_tab, Some(2));
    }

    #[test]
    fn update_drag_resets_offset_on_swap() {
        let mut state = bar_with(4, 0, 800.0);
        let w = state.tabs[0].width;
        state.start_drag(1, 300.0);
        let x = 300.0 + w / 2.0 + 1.0;
        let result = state.update_drag(x);
        assert!(result.is_some());
        // After swap, offset resets
        assert_eq!(state.drag_offset_x, 0.0);
        assert_eq!(state.drag_start_x, x);
    }

    #[test]
    fn update_drag_when_not_dragging() {
        let mut state = bar_with(3, 0, 600.0);
        let result = state.update_drag(300.0);
        assert!(result.is_none());
    }

    #[test]
    fn is_dragging_reflects_state() {
        let mut state = bar_with(3, 0, 600.0);
        assert!(!state.is_dragging());
        state.start_drag(0, 10.0);
        assert!(state.is_dragging());
        state.end_drag();
        assert!(!state.is_dragging());
    }

    // ---------------------------------------------------------------
    // TabState geometry
    // ---------------------------------------------------------------

    #[test]
    fn tab_state_bounds() {
        let state = bar_with(3, 0, 600.0);
        let tab = &state.tabs[1];
        let (x, y, w, h) = tab.bounds();
        assert_eq!(x, tab.x);
        assert_eq!(y, TITLE_BAR_HEIGHT);
        assert_eq!(w, tab.width);
        assert_eq!(h, TAB_ROW_HEIGHT);
    }

    #[test]
    fn tab_state_text_position() {
        let state = bar_with(2, 0, 800.0);
        let tab = &state.tabs[0];
        let (tx, ty) = tab.text_position();
        assert_eq!(tx, tab.x + TAB_PADDING);
        assert_eq!(ty, TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT / 2.0 - 6.0);
    }

    #[test]
    fn tab_state_max_text_width() {
        let state = bar_with(4, 0, 800.0);
        let tab = &state.tabs[0];
        let expected = tab.width - TAB_PADDING * 2.0 - 16.0;
        assert_eq!(tab.max_text_width(), expected);
        assert!(tab.max_text_width() > 0.0);
    }

    #[test]
    fn tab_bounds_are_contiguous() {
        let state = bar_with(4, 0, 800.0);
        for i in 0..state.tabs.len().saturating_sub(1) {
            let (x1, _, w1, _) = state.tabs[i].bounds();
            let (x2, _, _, _) = state.tabs[i + 1].bounds();
            assert!(
                (x1 + w1 - x2).abs() < f32::EPSILON,
                "gap between tab {} and {}",
                i,
                i + 1,
            );
        }
    }

    // ---------------------------------------------------------------
    // Constants sanity checks
    // ---------------------------------------------------------------

    #[test]
    fn min_less_than_max_tab_width() {
        const {
            assert!(MIN_TAB_WIDTH < MAX_TAB_WIDTH);
        }
    }

    #[test]
    fn tab_bar_height_positive() {
        const {
            assert!(TAB_BAR_HEIGHT > 0.0);
        }
    }

    #[test]
    fn tab_padding_positive() {
        const {
            assert!(TAB_PADDING > 0.0);
        }
    }

    // ---------------------------------------------------------------
    // Edge case: very narrow window
    // ---------------------------------------------------------------

    #[test]
    fn very_narrow_window_clamps_to_min() {
        // Window narrower than a single MIN_TAB_WIDTH
        let state = bar_with(2, 0, 50.0);
        for tab in &state.tabs {
            assert_eq!(tab.width, MIN_TAB_WIDTH);
        }
    }

    // ---------------------------------------------------------------
    // Repeated updates are idempotent
    // ---------------------------------------------------------------

    #[test]
    fn repeated_update_is_stable() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(3);
        state.update(&tabs, TabId::new(1), 600.0);
        let len1 = state.tabs.len();
        let active1 = state.active_index;

        state.update(&tabs, TabId::new(1), 600.0);
        assert_eq!(state.tabs.len(), len1);
        assert_eq!(state.active_index, active1);
    }
}
