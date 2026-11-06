//! Tab bar state and rendering helpers.
//!
//! This module provides the data structures for rendering a tab bar UI.
//!
//! The geometry constants below are logical pixels. [`TabBarState`] holds the
//! window's scale factor and turns them into physical pixels through its
//! accessors ([`TabBarState::title_bar_height`] and friends), so hit testing,
//! layout and drawing all agree on one scale.

use crate::terminal_manager::{Tab, TabId};

/// Height of the window control band in logical pixels.
pub const TITLE_BAR_HEIGHT: f32 = 44.0;

/// Height of the tab row in logical pixels.
pub const TAB_ROW_HEIGHT: f32 = 28.0;

/// Height of the complete chrome area in logical pixels.
pub const TAB_BAR_HEIGHT: f32 = TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT;

/// Gap between the bar and the first terminal row, in the terminal's
/// background, so a full-height cursor on the top row is not clipped by the
/// bar's edge.
pub const TERMINAL_TOP_PADDING: f32 = 4.0;

/// Padding inside each tab.
pub const TAB_PADDING: f32 = 12.0;

/// Distance between the tab close target and the tab's right edge.
pub const TAB_CLOSE_EDGE_PADDING: f32 = 8.0;

/// Width and height of a tab close target.
pub const TAB_CLOSE_BUTTON_SIZE: f32 = 18.0;

/// Width and height of the visible close-hover affordance.
pub const TAB_CLOSE_HOVER_SIZE: f32 = 22.0;

/// Minimum tab width.
pub const MIN_TAB_WIDTH: f32 = 80.0;

/// Extra gap between the split buttons and the window controls in the title
/// band; a thin separator is drawn in its middle.
pub const TITLE_GROUP_GAP: f32 = 12.0;

/// Side of the visible square drawn for each chrome button, centered in its
/// slot (the window controls' WINDOW_BUTTON_WIDTH, the "+" button's
/// NEW_TAB_BUTTON_WIDTH).
pub const CHROME_BUTTON_SIZE: f32 = 28.0;

/// Gap between the window edge and the slot of the nearest chrome button
/// (the "+" on the left, the window controls on the right). Half a slot's
/// spare width, so the visible gap from a button to the edge matches the gap
/// between two buttons, and the very corners stay free for resizing.
pub const CHROME_EDGE_INSET: f32 = (WINDOW_BUTTON_WIDTH - CHROME_BUTTON_SIZE) / 2.0;

/// Width of each title-band button slot. The visible button is
/// CHROME_BUTTON_SIZE, so neighbouring buttons sit 10 px apart.
pub const WINDOW_BUTTON_WIDTH: f32 = 38.0;

/// An entry in the main menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    NewTab,
    NewWindow,
    SplitRight,
    SplitDown,
    Settings,
    Shortcuts,
    About,
}

impl MenuItem {
    /// Menu order, top to bottom.
    pub const ALL: [MenuItem; 7] = [
        MenuItem::NewTab,
        MenuItem::NewWindow,
        MenuItem::SplitRight,
        MenuItem::SplitDown,
        MenuItem::Settings,
        MenuItem::Shortcuts,
        MenuItem::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuItem::NewTab => "New tab",
            MenuItem::NewWindow => "New window",
            MenuItem::SplitRight => "Split right",
            MenuItem::SplitDown => "Split down",
            MenuItem::Settings => "Settings",
            MenuItem::Shortcuts => "Keyboard shortcuts",
            MenuItem::About => "About Ultty",
        }
    }

    /// A thin rule is drawn above these to group the menu.
    pub fn starts_group(self) -> bool {
        matches!(self, MenuItem::SplitRight | MenuItem::Settings)
    }
}

/// Where the open main menu sits, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub item_height: f32,
    /// Space above the first and below the last item ([`MENU_PADDING`],
    /// scaled).
    pub padding: f32,
}

impl MenuLayout {
    pub fn height(&self) -> f32 {
        self.item_height * MenuItem::ALL.len() as f32 + 2.0 * self.padding
    }

    /// The item under (x, y), if any.
    pub fn item_at(&self, x: f32, y: f32) -> Option<MenuItem> {
        if x < self.x || x >= self.x + self.width {
            return None;
        }
        let offset = y - self.y - self.padding;
        if offset < 0.0 || self.item_height <= 0.0 {
            return None;
        }
        MenuItem::ALL
            .get((offset / self.item_height) as usize)
            .copied()
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height()
    }
}

/// Letters in the title's `ultty`.
pub const TITLE_WORD_LETTERS: usize = 5;
/// At launch the title shows `->>>_` this long before typing `ultty`.
const TITLE_TYPE_START: std::time::Duration = std::time::Duration::from_millis(700);
/// Time between typed letters of the launch animation.
const TITLE_TYPE_STEP: std::time::Duration = std::time::Duration::from_millis(90);

/// How many letters of `ultty` the title shows `elapsed` after launch.
pub fn title_letters_at(elapsed: std::time::Duration) -> usize {
    match elapsed.checked_sub(TITLE_TYPE_START) {
        None => 0,
        Some(typing) => {
            let steps = typing.as_millis() / TITLE_TYPE_STEP.as_millis() + 1;
            (steps as usize).min(TITLE_WORD_LETTERS)
        }
    }
}

/// When, measured from launch, the title next changes; None once typed.
pub fn title_next_change(elapsed: std::time::Duration) -> Option<std::time::Duration> {
    let shown = title_letters_at(elapsed);
    (shown < TITLE_WORD_LETTERS).then(|| TITLE_TYPE_START + TITLE_TYPE_STEP * shown as u32)
}

/// Space above the first and below the last menu item.
pub const MENU_PADDING: f32 = 6.0;

/// A button in the title band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleButton {
    /// The main menu, at the far left.
    Menu,
    /// New tab; in the title band only while the tab row is hidden.
    NewTab,
    /// Split the focused pane side by side.
    SplitRight,
    /// Split the focused pane top and bottom.
    SplitDown,
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
    pub hovered_title_button: Option<TitleButton>,
    /// Which tab's close button is hovered (if any).
    pub hovered_close_button: Option<usize>,
    /// The focused pane's working directory, home shown as `~`.
    pub title_path: String,
    /// The git branch at `title_path` (short commit when detached).
    pub title_branch: Option<String>,
    /// Letters of `ultty` the title shows: 0 shows `->>>_`; the launch
    /// animation types the rest.
    pub title_letters: usize,
    /// Whether the main menu is open.
    pub menu_open: bool,
    /// The menu item under the pointer.
    pub hovered_menu_item: Option<MenuItem>,
    /// The window's scale factor: physical pixels per logical pixel. All of
    /// the bar's geometry is the logical constants times this.
    pub scale: f32,
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
            hovered_title_button: None,
            hovered_close_button: None,
            title_path: String::new(),
            title_branch: None,
            title_letters: TITLE_WORD_LETTERS,
            menu_open: false,
            hovered_menu_item: None,
            scale: 1.0,
        }
    }

    /// `logical` pixels at the bar's scale.
    pub fn scaled(&self, logical: f32) -> f32 {
        logical * self.scale
    }

    /// Height of the title band. Bar heights are rounded to whole pixels so
    /// the terminal below starts on a pixel boundary at any scale.
    pub fn title_bar_height(&self) -> f32 {
        self.scaled(TITLE_BAR_HEIGHT).round()
    }

    /// Height of the tab row.
    pub fn tab_row_height(&self) -> f32 {
        self.scaled(TAB_ROW_HEIGHT).round()
    }

    /// Gap between the bar and the first terminal row.
    pub fn terminal_top_padding(&self) -> f32 {
        self.scaled(TERMINAL_TOP_PADDING).round()
    }

    /// Width of each title-band button slot.
    pub fn window_button_width(&self) -> f32 {
        self.scaled(WINDOW_BUTTON_WIDTH)
    }

    /// Side of the visible square drawn for each chrome button.
    pub fn chrome_button_size(&self) -> f32 {
        self.scaled(CHROME_BUTTON_SIZE)
    }

    /// Gap between the window edge and the nearest chrome button's slot.
    pub fn chrome_edge_inset(&self) -> f32 {
        self.scaled(CHROME_EDGE_INSET)
    }

    /// Width of the "+" target at the end of the tab row.
    pub fn new_tab_button_width(&self) -> f32 {
        self.tab_row_height()
    }

    /// Minimum tab width.
    pub fn min_tab_width(&self) -> f32 {
        self.scaled(MIN_TAB_WIDTH)
    }

    /// Update the tab bar state from terminal manager tabs.
    pub fn update(&mut self, tabs: &[Tab], active_tab: TabId, total_width: f32) {
        self.update_with_tab_width_override(tabs, active_tab, total_width, None);
    }

    /// Update the tab bar state with an optional fixed tab width.
    pub fn update_with_tab_width_override(
        &mut self,
        tabs: &[Tab],
        active_tab: TabId,
        total_width: f32,
        tab_width_override: Option<f32>,
    ) {
        self.update_with_titles_and_tab_width_override(
            tabs,
            &[],
            active_tab,
            total_width,
            tab_width_override,
        );
    }

    /// Update the tab bar state with computed display titles and optional fixed tab width.
    pub fn update_with_titles_and_tab_width_override(
        &mut self,
        tabs: &[Tab],
        titles: &[String],
        active_tab: TabId,
        total_width: f32,
        tab_width_override: Option<f32>,
    ) {
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

        // Layout: the title band holds [drag area][split right][split down]
        // [minimize][maximize][close] (see title_buttons). The tab row holds
        // the tabs, then "+" right after the last one.
        let tabs_start_x = 0.0;
        let usable_width = (total_width - self.new_tab_button_width()).max(0.0);
        let ideal_width = usable_width / tab_count as f32;
        let min_tab_width = self.min_tab_width();
        let tab_width = tab_width_override
            .filter(|width| width.is_finite() && *width > 0.0)
            .map(|width| width.max(min_tab_width))
            .unwrap_or_else(|| ideal_width.max(min_tab_width));

        let mut x = tabs_start_x;
        for (index, tab) in tabs.iter().enumerate() {
            let is_active = tab.id == active_tab;
            if is_active {
                self.active_index = index;
            }

            self.tabs.push(TabState {
                id: tab.id,
                title: titles
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| tab.title.clone()),
                x,
                width: tab_width,
                is_active,
                is_hovered: self.hovered_tab == Some(index),
                scale: self.scale,
            });

            x += tab_width;
        }
        // In the tab row, "+" follows the last tab (clamped inside the
        // window when tabs overflow it).
        self.new_tab_button_x = x.min(total_width - self.new_tab_button_width()).max(0.0);
    }

    /// Width of the "+" target at the end of the tab row, in logical pixels;
    /// see [`Self::new_tab_button_width`].
    pub const NEW_TAB_BUTTON_WIDTH: f32 = TAB_ROW_HEIGHT;

    /// Title-band buttons and the x of each one's slot
    /// ([`Self::window_button_width`] wide), left to right. The one source of
    /// this layout: hit testing, hover and drawing all read it.
    pub fn title_buttons(&self) -> Vec<(TitleButton, f32)> {
        let slot_width = self.window_button_width();
        let inset = self.chrome_edge_inset();
        let mut x = self.total_width - inset;
        let mut slots = Vec::with_capacity(6);
        for button in [
            TitleButton::Close,
            TitleButton::Maximize,
            TitleButton::Minimize,
        ] {
            x -= slot_width;
            slots.push((button, x));
        }
        x -= self.scaled(TITLE_GROUP_GAP);
        let mut left = vec![TitleButton::SplitDown, TitleButton::SplitRight];
        if !self.shows_tab_row() {
            left.push(TitleButton::NewTab);
        }
        for button in left {
            x -= slot_width;
            slots.push((button, x));
        }
        slots.push((TitleButton::Menu, inset));
        slots.reverse();
        slots
    }

    /// Smallest window that still shows the title's location in full plus
    /// ten columns of room, never narrower than the logo and buttons need,
    /// and three terminal rows tall. Sizes are in pixels.
    pub fn min_window_size(&self, cell_width: f32, cell_height: f32) -> (f32, f32) {
        let location_chars = self.title_path.chars().count()
            + self
                .title_branch
                .as_ref()
                .map_or(0, |b| " git:()".len() + b.chars().count());
        let for_location = (location_chars as f32 + 10.0) * cell_width;
        // Menu button and the `->>> ultty_` logo, plus the right-hand buttons.
        let logo_end =
            self.chrome_edge_inset() + 2.0 * self.window_button_width() + 13.0 * cell_width;
        let right_buttons = self.total_width - self.title_buttons_start();
        let for_chrome = logo_end + right_buttons;
        let height = self.content_top() + 3.0 * cell_height;
        (for_location.max(for_chrome).ceil(), height.ceil())
    }

    /// Where the main menu opens: under the menu button, sized to its
    /// longest label.
    pub fn menu_layout(&self, cell_width: f32, cell_height: f32) -> MenuLayout {
        let longest = MenuItem::ALL
            .iter()
            .map(|item| item.label().chars().count())
            .max()
            .unwrap_or(0) as f32;
        let button_left = self.chrome_edge_inset()
            + (self.window_button_width() - self.chrome_button_size()) / 2.0;
        MenuLayout {
            x: button_left,
            y: self.title_bar_height() - self.scaled(4.0),
            width: (longest + 4.0) * cell_width,
            item_height: (cell_height + self.scaled(12.0)).round(),
            padding: self.scaled(MENU_PADDING),
        }
    }

    /// x of the separator between the split buttons and the window controls.
    pub fn title_group_separator_x(&self) -> f32 {
        self.total_width
            - self.chrome_edge_inset()
            - 3.0 * self.window_button_width()
            - self.scaled(TITLE_GROUP_GAP) / 2.0
    }

    /// Left edge of the right-hand title-band buttons; title text must end
    /// before it.
    pub fn title_buttons_start(&self) -> f32 {
        self.title_buttons()
            .into_iter()
            .find(|(button, _)| *button != TitleButton::Menu)
            .map_or(self.total_width, |(_, x)| x)
    }

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

    /// Check if a click at (x, y) is on the new tab "+" button: at the end
    /// of the tab row, or in the title band while the row is hidden.
    pub fn is_new_tab_button_at(&self, x: f32, y: f32) -> bool {
        if !self.visible {
            return false;
        }
        if self.shows_tab_row() {
            self.in_tab_row(y)
                && x >= self.new_tab_button_x
                && x < self.new_tab_button_x + self.new_tab_button_width()
        } else {
            self.title_button_at(x, y) == Some(TitleButton::NewTab)
        }
    }

    /// Check if a click at (x, y) is in the title band area that toggles maximize on double click.
    pub fn is_title_bar_toggle_area(&self, x: f32, y: f32) -> bool {
        self.visible
            && (0.0..self.title_bar_height()).contains(&y)
            && !self.is_new_tab_button_at(x, y)
            && self.title_button_at(x, y).is_none()
    }

    /// The title-band button at (x, y), if any.
    pub fn title_button_at(&self, x: f32, y: f32) -> Option<TitleButton> {
        if !self.visible || !(0.0..self.title_bar_height()).contains(&y) {
            return None;
        }
        let slot_width = self.window_button_width();
        self.title_buttons()
            .into_iter()
            .find(|(_, slot_x)| (*slot_x..*slot_x + slot_width).contains(&x))
            .map(|(button, _)| button)
    }

    /// Check if position is in the draggable title bar area (not on a tab, button, or control).
    pub fn is_drag_area(&self, x: f32, y: f32) -> bool {
        if !self.visible || !(0.0..self.height()).contains(&y) {
            return false;
        }
        // Not on window controls
        if self.title_button_at(x, y).is_some() {
            return false;
        }
        // Not on "+" button
        if self.is_new_tab_button_at(x, y) {
            return false;
        }
        if y < self.title_bar_height() || !self.shows_tab_row() {
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
            self.hovered_title_button = None;
            self.hovered_close_button = None;
            return;
        }

        self.hovered_tab = if self.shows_tab_row() && y >= self.title_bar_height() {
            self.tab_at_x(x)
        } else {
            None
        };
        self.new_tab_button_hovered = self.is_new_tab_button_at(x, y);
        self.hovered_title_button = self.title_button_at(x, y);
        self.hovered_close_button = self.close_button_at(x, y);

        // Update tab states
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            tab.is_hovered = self.hovered_tab == Some(index);
        }
    }

    /// Check if a click at (x, y) is on the close button of a tab.
    pub fn close_button_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.visible || !self.shows_tab_row() || !self.in_tab_row(y) {
            return None;
        }

        let close_size = self.scaled(TAB_CLOSE_BUTTON_SIZE);
        for (index, tab) in self.tabs.iter().enumerate() {
            let close_x = tab.close_button_x();
            if x >= close_x && x < close_x + close_size {
                return Some(index);
            }
        }
        None
    }

    /// Whether `y` falls in the tab row's band (whether or not it shows).
    fn in_tab_row(&self, y: f32) -> bool {
        let top = self.title_bar_height();
        (top..top + self.tab_row_height()).contains(&y)
    }

    /// Where the terminal content starts: below the bar plus
    /// [`Self::terminal_top_padding`]. Chrome hit tests use [`Self::height`].
    pub fn content_top(&self) -> f32 {
        self.height() + self.terminal_top_padding()
    }

    /// Height of the bar: the title band, plus the tab row when it shows
    /// (0 when hidden).
    pub fn height(&self) -> f32 {
        if !self.visible {
            0.0
        } else if self.shows_tab_row() {
            self.title_bar_height() + self.tab_row_height()
        } else {
            self.title_bar_height()
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
        let tab_width = self.tabs.get(dragging_index)?.width;

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
    /// The bar's scale factor, copied from [`TabBarState::scale`].
    pub scale: f32,
}

impl TabState {
    /// `logical` pixels at the tab's scale.
    fn scaled(&self, logical: f32) -> f32 {
        logical * self.scale
    }

    /// Top of the tab row (the title band's height, rounded as
    /// [`TabBarState::title_bar_height`] rounds it).
    fn row_top(&self) -> f32 {
        self.scaled(TITLE_BAR_HEIGHT).round()
    }

    /// Height of the tab row.
    fn row_height(&self) -> f32 {
        self.scaled(TAB_ROW_HEIGHT).round()
    }

    /// Get the bounds of this tab.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        (self.x, self.row_top(), self.width, self.row_height())
    }

    /// Get the text rendering position.
    pub fn text_position(&self) -> (f32, f32) {
        (
            self.x + self.scaled(TAB_PADDING),
            self.row_top() + self.row_height() / 2.0 - self.scaled(6.0),
        )
    }

    /// Get the maximum text width.
    pub fn max_text_width(&self) -> f32 {
        // Leave space for close button
        self.width
            - self.scaled(TAB_PADDING)
            - self.scaled(TAB_CLOSE_EDGE_PADDING)
            - self.scaled(TAB_CLOSE_BUTTON_SIZE)
            - self.scaled(4.0)
    }

    /// Get the close button's left edge.
    pub fn close_button_x(&self) -> f32 {
        self.x + self.width
            - self.scaled(TAB_CLOSE_EDGE_PADDING)
            - self.scaled(TAB_CLOSE_BUTTON_SIZE)
    }

    /// Get the close hover affordance bounds.
    pub fn close_hover_bounds(&self) -> (f32, f32, f32, f32) {
        let center_x = self.close_button_x() + self.scaled(TAB_CLOSE_BUTTON_SIZE) / 2.0;
        let center_y = self.row_top() + self.row_height() / 2.0;
        let size = self.scaled(TAB_CLOSE_HOVER_SIZE);
        (center_x - size / 2.0, center_y - size / 2.0, size, size)
    }
}

/// `dir` with the home directory shown as `~`.
pub fn display_path(dir: &str, home: Option<&str>) -> String {
    match home.filter(|home| !home.is_empty() && *home != "/") {
        Some(home) if dir == home => "~".to_string(),
        Some(home) => match dir.strip_prefix(home) {
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => dir.to_string(),
        },
        None => dir.to_string(),
    }
}

/// The git branch checked out at `dir` or its nearest ancestor repository:
/// the branch name, or the first 7 characters of the commit when detached.
/// Reads `.git/HEAD` directly (following a `gitdir:` file for worktrees and
/// submodules), so it costs a few small file reads and no subprocess.
pub fn git_branch(dir: &std::path::Path) -> Option<String> {
    let git = dir
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|g| g.exists())?;
    let git_dir = if git.is_file() {
        let pointer = std::fs::read_to_string(&git).ok()?;
        let target = std::path::PathBuf::from(pointer.strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            git.parent()?.join(target)
        }
    } else {
        git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(reference) => {
            let reference = reference.trim();
            Some(
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_string(),
            )
        }
        None => head.get(..7).map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_manager::TerminalId;

    fn slot(state: &TabBarState, button: TitleButton) -> f32 {
        state
            .title_buttons()
            .into_iter()
            .find(|(b, _)| *b == button)
            .map(|(_, x)| x)
            .unwrap()
    }

    #[test]
    fn menu_button_sits_at_the_left_edge_and_opens_below() {
        let state = bar_with(2, 0, 800.0);
        assert_eq!(
            state.title_buttons()[0],
            (TitleButton::Menu, CHROME_EDGE_INSET)
        );
        assert_eq!(
            state.title_button_at(CHROME_EDGE_INSET + 5.0, 10.0),
            Some(TitleButton::Menu)
        );
        let layout = state.menu_layout(8.0, 16.0);
        assert!(layout.y < TITLE_BAR_HEIGHT && layout.y + layout.height() > TITLE_BAR_HEIGHT);
        assert!(layout.width >= ("Keyboard shortcuts".len() as f32) * 8.0);
    }

    #[test]
    fn menu_items_map_to_their_rows() {
        let state = bar_with(1, 0, 800.0);
        let layout = state.menu_layout(8.0, 16.0);
        let x = layout.x + 10.0;
        for (index, item) in MenuItem::ALL.iter().enumerate() {
            let y = layout.y + MENU_PADDING + (index as f32 + 0.5) * layout.item_height;
            assert_eq!(layout.item_at(x, y), Some(*item));
        }
        assert_eq!(layout.item_at(x, layout.y + 1.0), None);
        assert_eq!(layout.item_at(layout.x - 1.0, layout.y + 20.0), None);
        assert_eq!(
            layout.item_at(layout.x + layout.width, layout.y + 20.0),
            None
        );
        assert!(!layout.contains(x, layout.y + layout.height()));
    }

    #[test]
    fn min_window_size_fits_the_location_plus_ten_columns_and_three_rows() {
        let mut state = bar_with(1, 0, 800.0);
        state.title_path = "~/projects/a-rather-long-project-name/crates".to_string();
        state.title_branch = Some("main".to_string());
        let (cw, ch) = (8.0, 16.0);
        let chars = state.title_path.len() + " git:(main)".len();
        let (width, height) = state.min_window_size(cw, ch);
        assert_eq!(width, (chars as f32 + 10.0) * cw);
        assert_eq!(height, state.content_top() + 3.0 * ch);

        // A short location still leaves room for the logo and buttons.
        state.title_path = "~".to_string();
        state.title_branch = None;
        let (width, _) = state.min_window_size(cw, ch);
        let right_buttons = state.total_width - state.title_buttons_start();
        assert!(width >= right_buttons + 13.0 * cw);
    }

    #[test]
    fn title_types_ultty_after_a_pause() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        assert_eq!(title_letters_at(ms(0)), 0);
        assert_eq!(title_letters_at(ms(699)), 0);
        assert_eq!(title_letters_at(ms(700)), 1);
        assert_eq!(title_letters_at(ms(789)), 1);
        assert_eq!(title_letters_at(ms(790)), 2);
        assert_eq!(title_letters_at(ms(1060)), 5);
        assert_eq!(title_letters_at(ms(60_000)), 5);
        assert_eq!(title_next_change(ms(0)), Some(ms(700)));
        assert_eq!(title_next_change(ms(700)), Some(ms(790)));
        assert_eq!(title_next_change(ms(1060)), None);
    }

    #[test]
    fn display_path_shows_home_as_tilde() {
        let home = Some("/home/ana");
        assert_eq!(display_path("/home/ana", home), "~");
        assert_eq!(display_path("/home/ana/src/ultty", home), "~/src/ultty");
        assert_eq!(display_path("/home/anabel/x", home), "/home/anabel/x");
        assert_eq!(display_path("/etc", home), "/etc");
        assert_eq!(display_path("/etc", None), "/etc");
        assert_eq!(display_path("/etc", Some("/")), "/etc");
    }

    #[test]
    fn git_branch_reads_head_from_the_nearest_repository() {
        let repo = tempfile::tempdir().unwrap();
        let git = repo.path().join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/topbar\n").unwrap();
        let nested = repo.path().join("crates/ultty");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(git_branch(&nested).as_deref(), Some("feature/topbar"));

        std::fs::write(
            git.join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(git_branch(repo.path()).as_deref(), Some("0123456"));
    }

    #[test]
    fn git_branch_follows_a_gitdir_file() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("main.git/worktrees/wt");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();
        let worktree = root.path().join("wt");
        std::fs::create_dir(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: ../main.git/worktrees/wt\n").unwrap();
        assert_eq!(git_branch(&worktree).as_deref(), Some("wt-branch"));
    }

    #[test]
    fn git_branch_is_none_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        // tempdir sits under /tmp, which is not inside a repository here.
        if dir.path().ancestors().any(|d| d.join(".git").exists()) {
            return;
        }
        assert_eq!(git_branch(dir.path()), None);
    }

    #[test]
    fn title_buttons_run_right_to_left_from_the_edge() {
        let state = bar_with(2, 0, 800.0);
        let buttons: Vec<TitleButton> = state.title_buttons().iter().map(|(b, _)| *b).collect();
        assert_eq!(
            buttons,
            [
                TitleButton::Menu,
                TitleButton::SplitRight,
                TitleButton::SplitDown,
                TitleButton::Minimize,
                TitleButton::Maximize,
                TitleButton::Close,
            ]
        );
        let close_x = state.title_buttons().last().unwrap().1;
        assert_eq!(close_x + WINDOW_BUTTON_WIDTH, 800.0 - CHROME_EDGE_INSET);
        assert_eq!(
            state.title_button_at(close_x + 1.0, 10.0),
            Some(TitleButton::Close)
        );
        let split_x = slot(&state, TitleButton::SplitRight);
        assert_eq!(
            state.title_button_at(split_x + 1.0, 10.0),
            Some(TitleButton::SplitRight)
        );
        // The group gap belongs to no button.
        let gap_x = state.title_group_separator_x();
        assert_eq!(state.title_button_at(gap_x, 10.0), None);
    }

    #[test]
    fn new_tab_sits_in_the_title_band_only_while_the_tab_row_is_hidden() {
        let single = bar_with(1, 0, 800.0);
        let plus_x = slot(&single, TitleButton::NewTab);
        assert!(single.is_new_tab_button_at(plus_x + 5.0, 10.0));

        let multi = bar_with(3, 0, 800.0);
        assert!(!multi
            .title_buttons()
            .iter()
            .any(|(b, _)| *b == TitleButton::NewTab));
        let last_tab = multi.tabs.last().unwrap();
        assert_eq!(multi.new_tab_button_x, last_tab.x + last_tab.width);
        assert!(multi.is_new_tab_button_at(multi.new_tab_button_x + 5.0, TITLE_BAR_HEIGHT + 5.0));
        assert!(!multi.is_new_tab_button_at(multi.new_tab_button_x + 5.0, 10.0));
    }

    // ---------------------------------------------------------------
    // Scale factor
    // ---------------------------------------------------------------

    #[test]
    fn scale_one_matches_the_logical_constants() {
        let single = bar_with(1, 0, 800.0);
        assert_eq!(single.title_bar_height(), 44.0);
        assert_eq!(single.height(), 44.0);
        assert_eq!(single.content_top(), 48.0);
        let multi = bar_with(2, 0, 800.0);
        assert_eq!(multi.height(), 72.0);
        assert_eq!(multi.content_top(), 76.0);
        assert_eq!(multi.window_button_width(), 38.0);
        assert_eq!(multi.chrome_edge_inset(), 5.0);
        assert_eq!(multi.tabs[0].close_button_x(), multi.tabs[0].width - 26.0);
    }

    #[test]
    fn scale_two_doubles_the_bar() {
        let single = scaled_bar_with(1, 1600.0, 2.0);
        assert_eq!(single.height(), 88.0);
        assert_eq!(single.content_top(), 96.0);
        let multi = scaled_bar_with(2, 1600.0, 2.0);
        assert_eq!(multi.height(), 144.0);
        assert_eq!(multi.content_top(), 152.0);
        assert_eq!(
            multi.tabs[0].bounds(),
            (0.0, 88.0, multi.tabs[0].width, 56.0)
        );
        assert_eq!(
            multi.tabs[0].width,
            (1600.0 - multi.new_tab_button_width()) / 2.0
        );
        assert_eq!(multi.new_tab_button_width(), 56.0);
    }

    #[test]
    fn scale_two_doubles_every_title_button_slot() {
        let one = bar_with(2, 0, 800.0);
        let two = scaled_bar_with(2, 1600.0, 2.0);
        let doubled: Vec<(TitleButton, f32)> = one
            .title_buttons()
            .into_iter()
            .map(|(button, x)| (button, x * 2.0))
            .collect();
        assert_eq!(two.title_buttons(), doubled);
        assert_eq!(
            two.title_group_separator_x(),
            one.title_group_separator_x() * 2.0
        );
        assert_eq!(two.title_buttons_start(), one.title_buttons_start() * 2.0);
    }

    #[test]
    fn scale_two_hit_tests_follow_the_doubled_geometry() {
        let state = scaled_bar_with(2, 1600.0, 2.0);
        let close_x = slot(&state, TitleButton::Close);
        // Below the 1x band but inside the 2x one.
        assert_eq!(
            state.title_button_at(close_x + 70.0, 80.0),
            Some(TitleButton::Close)
        );
        assert_eq!(state.title_button_at(close_x + 1.0, 88.0), None);
        assert_eq!(state.tab_at_x(10.0), Some(0));
        assert!(state.is_drag_area(800.0, 80.0));
        assert!(!state.is_drag_area(10.0, 100.0));

        let tab = &state.tabs[1];
        let close = tab.close_button_x();
        assert_eq!(close, tab.x + tab.width - 52.0);
        assert_eq!(state.close_button_at(close + 30.0, 140.0), Some(1));
        assert_eq!(state.close_button_at(close + 36.0, 140.0), None);
        assert_eq!(state.close_button_at(close + 30.0, 144.0), None);

        let plus = state.new_tab_button_x;
        assert!(state.is_new_tab_button_at(plus + 50.0, 100.0));
        assert!(!state.is_new_tab_button_at(plus + 50.0, 80.0));

        let single = scaled_bar_with(1, 1600.0, 2.0);
        let plus = slot(&single, TitleButton::NewTab);
        assert!(single.is_new_tab_button_at(plus + 70.0, 80.0));
    }

    #[test]
    fn scale_two_scales_the_menu_and_min_window_size() {
        let one = bar_with(1, 0, 800.0);
        let two = scaled_bar_with(1, 1600.0, 2.0);
        // Cell metrics are physical already, so they double with the font.
        let layout_one = one.menu_layout(8.0, 16.0);
        let layout_two = two.menu_layout(16.0, 32.0);
        assert_eq!(layout_two.x, layout_one.x * 2.0);
        assert_eq!(layout_two.y, layout_one.y * 2.0);
        assert_eq!(layout_two.width, layout_one.width * 2.0);
        assert_eq!(layout_two.item_height, layout_one.item_height * 2.0);
        assert_eq!(layout_two.height(), layout_one.height() * 2.0);
        let last = MenuItem::ALL.len() as f32 - 0.5;
        let y = layout_two.y + layout_two.padding + last * layout_two.item_height;
        assert_eq!(
            layout_two.item_at(layout_two.x + 1.0, y),
            Some(MenuItem::About)
        );

        let (w1, h1) = one.min_window_size(8.0, 16.0);
        let (w2, h2) = two.min_window_size(16.0, 32.0);
        assert_eq!((w2, h2), (w1 * 2.0, h1 * 2.0));
    }

    #[test]
    fn fractional_scale_keeps_the_bar_on_whole_pixels() {
        let state = scaled_bar_with(2, 1000.0, 1.1);
        assert_eq!(state.title_bar_height(), 48.0);
        assert_eq!(state.height(), 48.0 + 31.0);
        assert_eq!(state.content_top(), 48.0 + 31.0 + 4.0);
    }

    // ---------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------

    /// A bar at `scale` updated with `count` tabs, the first one active.
    fn scaled_bar_with(count: usize, width: f32, scale: f32) -> TabBarState {
        let mut state = TabBarState::new();
        state.scale = scale;
        state.update(&make_tabs(count), TabId::new(0), width);
        state
    }

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
    fn tab_width_fills_available_rail_equally() {
        // The rail leaves room for "+" after the last tab.
        let state = bar_with(2, 0, 1000.0);
        let expected = (1000.0 - TabBarState::NEW_TAB_BUTTON_WIDTH) / 2.0;
        for tab in &state.tabs {
            assert_eq!(tab.width, expected);
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
        // 10 tabs in 900px => ideal 90 => above MIN_TAB_WIDTH.
        let state = bar_with(10, 0, 900.0);
        let usable = 900.0 - TabBarState::NEW_TAB_BUTTON_WIDTH;
        let expected = usable / 10.0;
        for tab in &state.tabs {
            assert!(
                (tab.width - expected).abs() < 0.1,
                "expected {} got {}",
                expected,
                tab.width
            );
        }
    }

    #[test]
    fn tab_width_override_keeps_width_after_tab_count_changes() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(8);

        state.update_with_tab_width_override(&tabs, TabId::new(0), 900.0, Some(90.0));

        for tab in &state.tabs {
            assert_eq!(tab.width, 90.0);
        }
    }

    #[test]
    fn tab_width_override_is_clamped_to_minimum_only() {
        let mut state = TabBarState::new();
        let tabs = make_tabs(8);

        state.update_with_tab_width_override(&tabs, TabId::new(0), 900.0, Some(10.0));
        for tab in &state.tabs {
            assert_eq!(tab.width, MIN_TAB_WIDTH);
        }

        state.update_with_tab_width_override(&tabs, TabId::new(0), 900.0, Some(500.0));
        for tab in &state.tabs {
            assert_eq!(tab.width, 500.0);
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
        state.set_hover(state.tabs[1].x + 5.0, TITLE_BAR_HEIGHT + 10.0);
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
        let plus = slot(&state, TitleButton::NewTab) + 5.0;
        assert!(state.is_new_tab_button_at(plus, 10.0));
        assert!(!state.is_drag_area(plus, 10.0));
        assert!(!state.is_new_tab_button_at(plus, -1.0));
        assert!(!state.is_new_tab_button_at(plus, TITLE_BAR_HEIGHT));
    }

    #[test]
    fn multi_tab_plus_is_clickable_in_the_tab_row() {
        let state = bar_with(2, 0, 800.0);
        let plus = state.new_tab_button_x + 5.0;
        assert!(state.is_new_tab_button_at(plus, TITLE_BAR_HEIGHT + 10.0));
        assert!(!state.is_new_tab_button_at(plus, 10.0));
        assert!(!state.is_drag_area(plus, TITLE_BAR_HEIGHT + 10.0));
        assert_eq!(state.tab_at_x(5.0), Some(0));
        assert!(!state.is_drag_area(5.0, TITLE_BAR_HEIGHT + 10.0));
    }

    #[test]
    fn title_band_between_controls_is_draggable() {
        let state = bar_with(2, 0, 800.0);
        assert!(state.is_drag_area(400.0, 10.0));
        assert!(!state.is_drag_area(400.0, -1.0));
        assert!(!state.is_drag_area(400.0, TAB_BAR_HEIGHT));
        assert!(!state.is_drag_area(800.0 - CHROME_EDGE_INSET - 10.0, 10.0));
    }

    #[test]
    fn title_band_toggle_area_excludes_interactive_chrome_controls() {
        let state = bar_with(2, 0, 800.0);
        assert!(state.is_title_bar_toggle_area(400.0, 10.0));
        let split = slot(&state, TitleButton::SplitRight) + 5.0;
        assert!(!state.is_title_bar_toggle_area(split, 10.0));
        assert!(!state.is_title_bar_toggle_area(800.0 - CHROME_EDGE_INSET - 10.0, 10.0));
        assert!(!state.is_title_bar_toggle_area(400.0, -1.0));
        assert!(!state.is_title_bar_toggle_area(400.0, TITLE_BAR_HEIGHT));
        assert!(!state.is_title_bar_toggle_area(400.0, TITLE_BAR_HEIGHT + 10.0));
    }

    #[test]
    fn chrome_buttons_sit_as_far_from_the_edge_as_from_each_other() {
        let between_buttons = WINDOW_BUTTON_WIDTH - CHROME_BUTTON_SIZE;
        let to_edge = CHROME_EDGE_INSET + (WINDOW_BUTTON_WIDTH - CHROME_BUTTON_SIZE) / 2.0;
        assert_eq!(to_edge, between_buttons);
    }

    #[test]
    fn chrome_buttons_leave_the_top_corners_free() {
        let state = bar_with(2, 0, 800.0);
        // The outermost CHROME_EDGE_INSET pixels belong to no button.
        for x in [0.0, CHROME_EDGE_INSET - 1.0] {
            assert_eq!(state.title_button_at(x, 4.0), None);
            assert!(!state.is_new_tab_button_at(x, 4.0));
        }
        for x in [800.0 - CHROME_EDGE_INSET, 799.0] {
            assert_eq!(state.title_button_at(x, 4.0), None);
        }
        // The buttons themselves are still there, just inset.
        assert_eq!(
            state.title_button_at(800.0 - CHROME_EDGE_INSET - 4.0, 10.0),
            Some(TitleButton::Close)
        );
    }

    #[test]
    fn window_controls_are_only_in_title_band() {
        let state = bar_with(2, 0, 800.0);
        let minimize_x = state
            .title_buttons()
            .iter()
            .find(|(b, _)| *b == TitleButton::Minimize)
            .map(|(_, x)| x + 10.0)
            .unwrap();
        assert_eq!(
            state.title_button_at(minimize_x, 10.0),
            Some(TitleButton::Minimize)
        );
        assert_eq!(state.title_button_at(minimize_x, -1.0), None);
        assert_eq!(state.title_button_at(minimize_x, TITLE_BAR_HEIGHT), None);
        assert_eq!(
            state.title_button_at(minimize_x, TITLE_BAR_HEIGHT + 1.0),
            None
        );
    }

    // ---------------------------------------------------------------
    // close_button_at
    // ---------------------------------------------------------------

    #[test]
    fn close_button_hit() {
        let state = bar_with(3, 0, 600.0);
        let tab = &state.tabs[1];
        let close_x = tab.close_button_x();

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
        let close_x = tab.close_button_x();
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
        let close_x = tab.close_button_x();
        // Just right of the close button
        assert_eq!(
            state.close_button_at(close_x + TAB_CLOSE_BUTTON_SIZE, TITLE_BAR_HEIGHT + 10.0),
            None
        );
    }

    #[test]
    fn close_button_below_bar_returns_none() {
        let state = bar_with(3, 0, 600.0);
        let tab = &state.tabs[0];
        let close_x = tab.close_button_x() + TAB_CLOSE_BUTTON_SIZE / 2.0;
        assert_eq!(state.close_button_at(close_x, TAB_BAR_HEIGHT + 1.0), None);
        assert_eq!(state.close_button_at(close_x, TAB_BAR_HEIGHT), None);
    }

    #[test]
    fn close_button_single_tab_hidden_with_tab_row() {
        let state = bar_with(1, 0, 800.0);
        assert!(state.visible);
        let tab = &state.tabs[0];
        let close_x = tab.close_button_x();
        assert_eq!(
            state.close_button_at(close_x + TAB_CLOSE_BUTTON_SIZE / 2.0, 10.0),
            None
        );
    }

    #[test]
    fn close_button_for_each_tab() {
        let state = bar_with(3, 0, 600.0);
        for (i, tab) in state.tabs.iter().enumerate() {
            let close_x = tab.close_button_x();
            assert_eq!(
                state.close_button_at(
                    close_x + TAB_CLOSE_BUTTON_SIZE / 2.0,
                    TITLE_BAR_HEIGHT + 10.0
                ),
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
        let expected =
            tab.width - TAB_PADDING - TAB_CLOSE_EDGE_PADDING - TAB_CLOSE_BUTTON_SIZE - 4.0;
        assert_eq!(tab.max_text_width(), expected);
        assert!(tab.max_text_width() > 0.0);
    }

    #[test]
    fn tab_state_close_geometry() {
        let state = bar_with(2, 0, 800.0);
        let tab = &state.tabs[0];
        assert_eq!(
            tab.close_button_x(),
            tab.x + tab.width - TAB_CLOSE_EDGE_PADDING - TAB_CLOSE_BUTTON_SIZE
        );
        let (x, y, w, h) = tab.close_hover_bounds();
        assert_eq!(w, TAB_CLOSE_HOVER_SIZE);
        assert_eq!(h, TAB_CLOSE_HOVER_SIZE);
        assert!(x < tab.close_button_x());
        assert!(y > TITLE_BAR_HEIGHT);
        assert!(y + h < TAB_BAR_HEIGHT);
    }

    #[test]
    fn min_width_tab_preserves_close_target() {
        let state = bar_with(20, 0, 800.0);
        let tab = &state.tabs[0];
        assert_eq!(tab.width, MIN_TAB_WIDTH);

        let close_x = tab.close_button_x();
        assert!(close_x > tab.x);
        assert!(close_x + TAB_CLOSE_BUTTON_SIZE < tab.x + tab.width);
        assert_eq!(
            state.close_button_at(
                close_x + TAB_CLOSE_BUTTON_SIZE / 2.0,
                TITLE_BAR_HEIGHT + 10.0
            ),
            Some(0)
        );
    }

    #[test]
    fn min_width_tab_hover_affordance_stays_inside_tab() {
        let state = bar_with(20, 0, 800.0);
        let tab = &state.tabs[0];
        let (x, y, w, h) = tab.close_hover_bounds();

        assert!(x >= tab.x);
        assert!(x + w <= tab.x + tab.width);
        assert!(y > TITLE_BAR_HEIGHT);
        assert!(y + h < TAB_BAR_HEIGHT);
    }

    #[test]
    fn min_width_tab_text_budget_stops_before_close_target() {
        let state = bar_with(20, 0, 800.0);
        let tab = &state.tabs[0];
        let text_start = tab.x + TAB_PADDING;
        let text_end = text_start + tab.max_text_width();

        assert!(tab.max_text_width() > 0.0);
        assert!(text_end < tab.close_button_x());
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
    fn min_tab_width_positive() {
        const {
            assert!(MIN_TAB_WIDTH > 0.0);
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

    #[test]
    fn tab_close_edge_padding_positive() {
        const {
            assert!(TAB_CLOSE_EDGE_PADDING > 0.0);
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
