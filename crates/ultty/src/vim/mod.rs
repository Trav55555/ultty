//! Terminal-level vim mode for scrollback navigation and copy.
//!
//! This module handles vim-style navigation when the terminal is in "copy mode",
//! similar to tmux's copy-mode-vi. It operates independently of any TUI app
//! running in the terminal.
//!
//! Activation: Ctrl+Shift+V (or configurable keybind)
//!
//! Supported operations:
//! - Navigation: h/j/k/l, w/b/e, 0/$, gg/G, Ctrl-d/u/f/b, H/M/L
//! - Search: /, ?, n, N
//! - Selection: v (char), V (line), Ctrl-v (block)
//! - Yank: y (copies selection to clipboard)
//! - Exit: Esc, q, Enter (with selection copies and exits)

use ultty_protocol::{Key, Modifiers};

/// What copy mode is doing: moving the cursor, selecting, or reading a
/// search query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Moving the cursor.
    #[default]
    Normal,
    /// Character-wise selection (`v`).
    Visual,
    /// Line-wise selection (`V`).
    VisualLine,
    /// Block selection (Ctrl-v).
    VisualBlock,
    /// Typing a search query (`/` or `?`).
    Search,
}

impl VimMode {
    /// Returns true if this is a selection mode.
    pub fn is_visual(&self) -> bool {
        matches!(
            self,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        )
    }
}

/// Terminal copy mode state.
#[derive(Debug, Clone)]
pub struct CopyMode {
    active: bool,
    mode: VimMode,
    /// Direction of the last search: true for `/`, false for `?`.
    search_forward: bool,
    cursor_row: usize,
    cursor_col: usize,
    selection_anchor: Option<(usize, usize)>,
    scroll_offset: usize,
    viewport_rows: usize,
    viewport_cols: usize,
    total_rows: usize,
    search_query: String,
    search_matches: Vec<(usize, usize)>,
    current_match: usize,
    /// A `g` was pressed and we are waiting for the second key of `gg`.
    pending_g: bool,
    /// Count prefix typed before a motion (`5j`), if any.
    count: Option<usize>,
}

/// Largest count prefix honoured; larger digits strings saturate here.
const MAX_COUNT: usize = 9999;

impl Default for CopyMode {
    fn default() -> Self {
        Self {
            active: false,
            mode: VimMode::Normal,
            search_forward: false,
            cursor_row: 0,
            cursor_col: 0,
            selection_anchor: None,
            scroll_offset: 0,
            viewport_rows: 24,
            viewport_cols: 80,
            total_rows: 0,
            search_query: String::new(),
            search_matches: Vec::new(),
            current_match: 0,
            pending_g: false,
            count: None,
        }
    }
}

impl CopyMode {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn enter(&mut self, total_rows: usize, viewport_rows: usize, viewport_cols: usize) {
        self.active = true;
        self.mode = VimMode::Normal;
        self.search_forward = false;
        self.total_rows = total_rows;
        self.viewport_rows = viewport_rows;
        self.viewport_cols = viewport_cols;
        self.scroll_offset = total_rows.saturating_sub(viewport_rows);
        self.cursor_row = self.scroll_offset;
        self.cursor_col = 0;
        self.selection_anchor = None;
        self.pending_g = false;
        self.count = None;
    }

    pub fn enter_search(&mut self, total_rows: usize, viewport_rows: usize, viewport_cols: usize) {
        self.enter(total_rows, viewport_rows, viewport_cols);
        self.mode = VimMode::Search;
        self.search_query.clear();
    }

    /// Update viewport dimensions and clamp cursor/scroll to remain in bounds.
    pub fn resize(&mut self, total_rows: usize, viewport_rows: usize, viewport_cols: usize) {
        self.total_rows = total_rows;
        self.viewport_rows = viewport_rows;
        self.viewport_cols = viewport_cols;
        // Clamp cursor to valid range
        self.cursor_row = self.cursor_row.min(total_rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(viewport_cols.saturating_sub(1));
        // Clamp scroll offset
        self.scroll_offset = self
            .scroll_offset
            .min(total_rows.saturating_sub(viewport_rows));
        // Ensure cursor is visible within viewport
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        } else if self.cursor_row >= self.scroll_offset + viewport_rows {
            self.scroll_offset = self
                .cursor_row
                .saturating_sub(viewport_rows.saturating_sub(1));
        }
        // Clamp selection anchor if set
        if let Some((ref mut row, ref mut col)) = self.selection_anchor {
            *row = (*row).min(total_rows.saturating_sub(1));
            *col = (*col).min(viewport_cols.saturating_sub(1));
        }
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.selection_anchor = None;
        self.pending_g = false;
        self.count = None;
        self.mode = VimMode::Normal;
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection_anchor.map(|(anchor_row, anchor_col)| {
            let (start_row, start_col, end_row, end_col) = if anchor_row < self.cursor_row
                || (anchor_row == self.cursor_row && anchor_col <= self.cursor_col)
            {
                (anchor_row, anchor_col, self.cursor_row, self.cursor_col)
            } else {
                (self.cursor_row, self.cursor_col, anchor_row, anchor_col)
            };

            match self.mode {
                VimMode::Visual => Selection::Char {
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                },
                VimMode::VisualLine => Selection::Line { start_row, end_row },
                VimMode::VisualBlock => Selection::Block {
                    start_row,
                    start_col: start_col.min(self.cursor_col),
                    end_row,
                    end_col: start_col.max(self.cursor_col),
                },
                _ => Selection::Char {
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                },
            }
        })
    }

    pub fn vim_mode(&self) -> VimMode {
        self.mode
    }

    /// Handle a key with no knowledge of the cursor row's text.
    ///
    /// Word motions (`w`/`b`/`e`) need the row text; callers that have it
    /// should use [`CopyMode::handle_key_in_row`].
    pub fn handle_key(&mut self, key: &Key, modifiers: Modifiers) -> CopyModeResult {
        self.handle_key_in_row(key, modifiers, "")
    }

    /// Handle a key, given the text of the row the cursor is on.
    pub fn handle_key_in_row(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        row_text: &str,
    ) -> CopyModeResult {
        if !self.active {
            return CopyModeResult::NotHandled;
        }

        if self.mode == VimMode::Search {
            return self.handle_search_key(key, modifiers);
        }

        // `gg`: the second key completes the motion, anything else cancels it.
        if self.pending_g {
            self.pending_g = false;
            if matches!(key, Key::Char('g')) && !modifiers.ctrl() {
                let target = self.count.take().map(|n| n.saturating_sub(1));
                self.cursor_row = target.unwrap_or(0).min(self.total_rows.saturating_sub(1));
                self.ensure_cursor_visible();
                return CopyModeResult::Handled;
            }
        }

        // Count prefix: digits accumulate, except a leading `0` which is the
        // line-start motion.
        if let Key::Char(c) = key {
            if let Some(digit) = c.to_digit(10) {
                if !modifiers.ctrl() && (digit != 0 || self.count.is_some()) {
                    let current = self.count.unwrap_or(0);
                    self.count = Some(
                        current
                            .saturating_mul(10)
                            .saturating_add(digit as usize)
                            .min(MAX_COUNT),
                    );
                    return CopyModeResult::Handled;
                }
            }
        }
        let count = self.count.take();
        let repeat = count.unwrap_or(1).max(1);

        match key {
            Key::Escape | Key::Char('q') => {
                if count.is_some() {
                    // Cancel a pending count only
                    return CopyModeResult::Handled;
                }
                if self.mode.is_visual() {
                    self.selection_anchor = None;
                    self.mode = VimMode::Normal;
                    CopyModeResult::Handled
                } else {
                    self.exit();
                    CopyModeResult::Exit
                }
            }

            Key::Enter => {
                if self.selection_anchor.is_some() {
                    let selection = self.selection();
                    self.exit();
                    CopyModeResult::CopyAndExit(selection)
                } else {
                    self.exit();
                    CopyModeResult::Exit
                }
            }

            Key::Char('h') | Key::Left => {
                self.move_cursor(0, -(repeat as i32));
                CopyModeResult::Handled
            }
            Key::Char('l') | Key::Right => {
                self.move_cursor(0, repeat as i32);
                CopyModeResult::Handled
            }
            Key::Char('j') | Key::Down => {
                self.move_cursor(repeat as i32, 0);
                CopyModeResult::Handled
            }
            Key::Char('k') | Key::Up => {
                self.move_cursor(-(repeat as i32), 0);
                CopyModeResult::Handled
            }

            // Ctrl+key must come before plain key matches
            Key::Char('d') if modifiers.ctrl() => {
                self.scroll_half_page_down();
                CopyModeResult::Handled
            }
            Key::Char('u') if modifiers.ctrl() => {
                self.scroll_half_page_up();
                CopyModeResult::Handled
            }
            Key::Char('f') if modifiers.ctrl() => {
                self.scroll_page_down();
                CopyModeResult::Handled
            }
            Key::Char('b') if modifiers.ctrl() => {
                self.scroll_page_up();
                CopyModeResult::Handled
            }
            Key::PageDown => {
                self.scroll_page_down();
                CopyModeResult::Handled
            }
            Key::PageUp => {
                self.scroll_page_up();
                CopyModeResult::Handled
            }

            Key::Char('w') => {
                for _ in 0..repeat {
                    self.move_word_forward(row_text);
                }
                CopyModeResult::Handled
            }
            Key::Char('b') => {
                for _ in 0..repeat {
                    self.move_word_backward(row_text);
                }
                CopyModeResult::Handled
            }
            Key::Char('e') => {
                for _ in 0..repeat {
                    self.move_word_end(row_text);
                }
                CopyModeResult::Handled
            }

            Key::Char('0') | Key::Home => {
                self.cursor_col = 0;
                CopyModeResult::Handled
            }
            Key::Char('$') | Key::End => {
                self.cursor_col = self.viewport_cols.saturating_sub(1);
                CopyModeResult::Handled
            }
            Key::Char('^') => {
                self.cursor_col = 0;
                CopyModeResult::Handled
            }

            Key::Char('g') => {
                self.pending_g = true;
                // Keep the count for `5gg`
                self.count = count;
                CopyModeResult::Handled
            }
            Key::Char('G') => {
                let last = self.total_rows.saturating_sub(1);
                self.cursor_row = match count {
                    Some(n) => n.saturating_sub(1).min(last),
                    None => last,
                };
                self.ensure_cursor_visible();
                CopyModeResult::Handled
            }

            Key::Char('H') => {
                self.cursor_row = self.scroll_offset;
                CopyModeResult::Handled
            }
            Key::Char('M') => {
                self.cursor_row = self.scroll_offset + self.viewport_rows / 2;
                CopyModeResult::Handled
            }
            Key::Char('L') => {
                self.cursor_row = self.scroll_offset + self.viewport_rows - 1;
                CopyModeResult::Handled
            }

            Key::Char('v') if !modifiers.ctrl() => {
                if self.mode == VimMode::Visual {
                    self.selection_anchor = None;
                    self.mode = VimMode::Normal;
                } else {
                    self.selection_anchor = Some((self.cursor_row, self.cursor_col));
                    self.mode = VimMode::Visual;
                }
                CopyModeResult::Handled
            }
            Key::Char('V') => {
                if self.mode == VimMode::VisualLine {
                    self.selection_anchor = None;
                    self.mode = VimMode::Normal;
                } else {
                    self.selection_anchor = Some((self.cursor_row, 0));
                    self.mode = VimMode::VisualLine;
                }
                CopyModeResult::Handled
            }
            Key::Char('v') if modifiers.ctrl() => {
                if self.mode == VimMode::VisualBlock {
                    self.selection_anchor = None;
                    self.mode = VimMode::Normal;
                } else {
                    self.selection_anchor = Some((self.cursor_row, self.cursor_col));
                    self.mode = VimMode::VisualBlock;
                }
                CopyModeResult::Handled
            }

            Key::Char('y') => {
                if self.selection_anchor.is_some() {
                    let selection = self.selection();
                    self.selection_anchor = None;
                    self.mode = VimMode::Normal;
                    CopyModeResult::Copy(selection)
                } else {
                    CopyModeResult::Handled
                }
            }

            Key::Char('/') => {
                self.mode = VimMode::Search;
                self.search_forward = true;
                self.search_query.clear();
                CopyModeResult::Handled
            }
            Key::Char('?') => {
                self.mode = VimMode::Search;
                self.search_forward = false;
                self.search_query.clear();
                CopyModeResult::Handled
            }
            Key::Char('n') => {
                self.goto_next_match();
                CopyModeResult::Handled
            }
            Key::Char('N') => {
                self.goto_prev_match();
                CopyModeResult::Handled
            }

            _ => CopyModeResult::Handled,
        }
    }

    fn handle_search_key(&mut self, key: &Key, _modifiers: Modifiers) -> CopyModeResult {
        match key {
            Key::Escape => {
                self.mode = VimMode::Normal;
                self.search_query.clear();
                CopyModeResult::Handled
            }
            Key::Enter => {
                self.mode = VimMode::Normal;
                if !self.search_query.is_empty() {
                    CopyModeResult::Search(self.search_query.clone())
                } else {
                    CopyModeResult::Handled
                }
            }
            Key::Backspace => {
                self.search_query.pop();
                CopyModeResult::Handled
            }
            Key::Char(c) => {
                self.search_query.push(*c);
                CopyModeResult::Handled
            }
            _ => CopyModeResult::Handled,
        }
    }

    fn move_cursor(&mut self, row_delta: i32, col_delta: i32) {
        if row_delta != 0 {
            let new_row = if row_delta > 0 {
                self.cursor_row.saturating_add(row_delta as usize)
            } else {
                self.cursor_row.saturating_sub((-row_delta) as usize)
            };
            self.cursor_row = new_row.min(self.total_rows.saturating_sub(1));
        }

        if col_delta != 0 {
            let new_col = if col_delta > 0 {
                self.cursor_col.saturating_add(col_delta as usize)
            } else {
                self.cursor_col.saturating_sub((-col_delta) as usize)
            };
            self.cursor_col = new_col.min(self.viewport_cols.saturating_sub(1));
        }

        self.ensure_cursor_visible();
    }

    fn move_word_forward(&mut self, row_text: &str) {
        let chars: Vec<char> = row_text.chars().collect();
        self.cursor_col = word_forward(&chars, self.cursor_col).min(self.last_col());
    }

    fn move_word_backward(&mut self, row_text: &str) {
        let chars: Vec<char> = row_text.chars().collect();
        self.cursor_col = word_backward(&chars, self.cursor_col).min(self.last_col());
    }

    fn move_word_end(&mut self, row_text: &str) {
        let chars: Vec<char> = row_text.chars().collect();
        self.cursor_col = word_end(&chars, self.cursor_col).min(self.last_col());
    }

    fn last_col(&self) -> usize {
        self.viewport_cols.saturating_sub(1)
    }

    fn scroll_half_page_down(&mut self) {
        let half_page = self.viewport_rows / 2;
        self.cursor_row = (self.cursor_row + half_page).min(self.total_rows.saturating_sub(1));
        self.ensure_cursor_visible();
    }

    fn scroll_half_page_up(&mut self) {
        let half_page = self.viewport_rows / 2;
        self.cursor_row = self.cursor_row.saturating_sub(half_page);
        self.ensure_cursor_visible();
    }

    fn scroll_page_down(&mut self) {
        self.cursor_row =
            (self.cursor_row + self.viewport_rows).min(self.total_rows.saturating_sub(1));
        self.ensure_cursor_visible();
    }

    fn scroll_page_up(&mut self) {
        self.cursor_row = self.cursor_row.saturating_sub(self.viewport_rows);
        self.ensure_cursor_visible();
    }

    fn ensure_cursor_visible(&mut self) {
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        } else if self.cursor_row >= self.scroll_offset + self.viewport_rows {
            self.scroll_offset = self.cursor_row - self.viewport_rows + 1;
        }
    }

    pub fn set_search_matches(&mut self, matches: Vec<(usize, usize)>) {
        self.search_matches = matches;
        self.current_match = 0;
    }

    fn goto_next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        let forward = self.search_forward;
        if forward {
            self.current_match = (self.current_match + 1) % self.search_matches.len();
        } else if self.current_match == 0 {
            self.current_match = self.search_matches.len() - 1;
        } else {
            self.current_match -= 1;
        }

        let (row, col) = self.search_matches[self.current_match];
        self.cursor_row = row;
        self.cursor_col = col;
        self.ensure_cursor_visible();
    }

    fn goto_prev_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        let forward = self.search_forward;
        if !forward {
            self.current_match = (self.current_match + 1) % self.search_matches.len();
        } else if self.current_match == 0 {
            self.current_match = self.search_matches.len() - 1;
        } else {
            self.current_match -= 1;
        }

        let (row, col) = self.search_matches[self.current_match];
        self.cursor_row = row;
        self.cursor_col = col;
        self.ensure_cursor_visible();
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// Get the search matches and current match index.
    pub fn search_matches(&self) -> (&[(usize, usize)], usize) {
        (&self.search_matches, self.current_match)
    }
}

/// Selection region in the scrollback buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Char {
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    },
    Line {
        start_row: usize,
        end_row: usize,
    },
    Block {
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    },
}

/// Result of handling a key in copy mode.
#[derive(Debug)]
pub enum CopyModeResult {
    NotHandled,
    Handled,
    Exit,
    Copy(Option<Selection>),
    CopyAndExit(Option<Selection>),
    Search(String),
}

/// Vim word character classes: whitespace, keyword (`\w`), or other punctuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Blank,
    Keyword,
    Punctuation,
}

fn word_class(c: char) -> WordClass {
    if c.is_whitespace() {
        WordClass::Blank
    } else if c.is_alphanumeric() || c == '_' {
        WordClass::Keyword
    } else {
        WordClass::Punctuation
    }
}

/// Class of the cell at `col`; cells past the end of the text are blank.
fn class_at(chars: &[char], col: usize) -> WordClass {
    chars.get(col).copied().map_or(WordClass::Blank, word_class)
}

/// Vim `w`: the start of the next word on this row, or the last character of
/// the row when there is none (never moves left).
fn word_forward(chars: &[char], col: usize) -> usize {
    let len = chars.len();
    let mut i = col;
    let class = class_at(chars, i);
    if class != WordClass::Blank {
        while i < len && class_at(chars, i) == class {
            i += 1;
        }
    }
    while i < len && class_at(chars, i) == WordClass::Blank {
        i += 1;
    }
    if i >= len {
        col.max(len.saturating_sub(1))
    } else {
        i
    }
}

/// Vim `b`: the start of the current word if the cursor is inside it,
/// otherwise the start of the previous word; column 0 when there is none.
fn word_backward(chars: &[char], col: usize) -> usize {
    if col == 0 {
        return 0;
    }
    let mut i = col.min(chars.len()).saturating_sub(1);
    if col > chars.len() {
        i = chars.len().saturating_sub(1);
    }
    while i > 0 && class_at(chars, i) == WordClass::Blank {
        i -= 1;
    }
    let class = class_at(chars, i);
    if class == WordClass::Blank {
        return 0;
    }
    while i > 0 && class_at(chars, i - 1) == class {
        i -= 1;
    }
    i
}

/// Vim `e`: the end of the word the cursor moves into (at least one column
/// right), or the last character of the row when there is none.
fn word_end(chars: &[char], col: usize) -> usize {
    let len = chars.len();
    let mut i = col + 1;
    while i < len && class_at(chars, i) == WordClass::Blank {
        i += 1;
    }
    if i >= len {
        return col.max(len.saturating_sub(1));
    }
    let class = class_at(chars, i);
    while i + 1 < len && class_at(chars, i + 1) == class {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // Helper to create a CopyMode with standard dimensions and enter it.
    // total_rows=100, viewport_rows=24, viewport_cols=80
    // cursor starts at row 76 (= 100 - 24), col 0.
    // ---------------------------------------------------------------
    fn active_mode() -> CopyMode {
        let mut m = CopyMode::new();
        m.enter(100, 24, 80);
        m
    }

    // ---------------------------------------------------------------
    // 1. Lifecycle: new / enter / exit / is_active
    // ---------------------------------------------------------------

    #[test]
    fn new_is_inactive() {
        let m = CopyMode::new();
        assert!(!m.is_active());
        assert_eq!(m.vim_mode(), VimMode::Normal);
    }

    #[test]
    fn default_is_inactive() {
        let m = CopyMode::default();
        assert!(!m.is_active());
    }

    #[test]
    fn enter_activates_and_sets_cursor() {
        let mut m = CopyMode::new();
        m.enter(100, 24, 80);
        assert!(m.is_active());
        // cursor_row = scroll_offset = total_rows - viewport_rows = 76
        assert_eq!(m.cursor_position(), (76, 0));
        assert_eq!(m.scroll_offset(), 76);
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.selection().is_none());
    }

    #[test]
    fn enter_small_buffer() {
        // total_rows < viewport_rows
        let mut m = CopyMode::new();
        m.enter(10, 24, 80);
        assert_eq!(m.cursor_position(), (0, 0));
        assert_eq!(m.scroll_offset(), 0);
    }

    #[test]
    fn enter_zero_rows() {
        let mut m = CopyMode::new();
        m.enter(0, 24, 80);
        assert!(m.is_active());
        assert_eq!(m.cursor_position(), (0, 0));
    }

    #[test]
    fn exit_deactivates() {
        let mut m = active_mode();
        m.exit();
        assert!(!m.is_active());
        assert!(m.selection().is_none());
        assert_eq!(m.vim_mode(), VimMode::Normal);
    }

    #[test]
    fn enter_search_mode() {
        let mut m = CopyMode::new();
        m.enter_search(100, 24, 80);
        assert!(m.is_active());
        assert_eq!(m.vim_mode(), VimMode::Search);
        assert_eq!(m.search_query(), "");
    }

    // ---------------------------------------------------------------
    // 2. Inactive mode ignores keys
    // ---------------------------------------------------------------

    #[test]
    fn inactive_returns_not_handled() {
        let mut m = CopyMode::new();
        let r = m.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::NotHandled));
    }

    // ---------------------------------------------------------------
    // 3. Basic cursor movement: h / j / k / l and arrow keys
    // ---------------------------------------------------------------

    #[test]
    fn move_j_k() {
        let mut m = active_mode();
        let start = m.cursor_row;
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert_eq!(m.cursor_row, start + 1);
        m.handle_key(&Key::Char('k'), Modifiers::NONE);
        assert_eq!(m.cursor_row, start);
    }

    #[test]
    fn move_h_l() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 1);
        m.handle_key(&Key::Char('h'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    #[test]
    fn arrow_keys_match_hjkl() {
        let mut m = active_mode();
        let start_row = m.cursor_row;

        m.handle_key(&Key::Down, Modifiers::NONE);
        assert_eq!(m.cursor_row, start_row + 1);
        m.handle_key(&Key::Up, Modifiers::NONE);
        assert_eq!(m.cursor_row, start_row);
        m.handle_key(&Key::Right, Modifiers::NONE);
        assert_eq!(m.cursor_col, 1);
        m.handle_key(&Key::Left, Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    // ---------------------------------------------------------------
    // 4. Cursor boundary clamping
    // ---------------------------------------------------------------

    #[test]
    fn h_at_col_zero_stays() {
        let mut m = active_mode();
        assert_eq!(m.cursor_col, 0);
        m.handle_key(&Key::Char('h'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    #[test]
    fn k_at_row_zero_stays() {
        let mut m = CopyMode::new();
        m.enter(10, 24, 80); // cursor_row starts at 0
        m.handle_key(&Key::Char('k'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 0);
    }

    #[test]
    fn j_at_last_row_stays() {
        let mut m = active_mode();
        // Move to last row
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 99);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 99);
    }

    #[test]
    fn l_at_last_col_stays() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('$'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 79);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 79);
    }

    // ---------------------------------------------------------------
    // 5. Line-start / line-end: 0, ^, $, Home, End
    // ---------------------------------------------------------------

    #[test]
    fn zero_moves_to_col_zero() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('0'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    #[test]
    fn caret_moves_to_col_zero() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('^'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    #[test]
    fn dollar_moves_to_last_col() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('$'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 79);
    }

    #[test]
    fn home_end_keys() {
        let mut m = active_mode();
        m.handle_key(&Key::End, Modifiers::NONE);
        assert_eq!(m.cursor_col, 79);
        m.handle_key(&Key::Home, Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    // ---------------------------------------------------------------
    // 6. Word motions: w, b, e
    // ---------------------------------------------------------------

    const ROW: &str = "hello world, foo_bar";

    #[test]
    fn word_forward_moves_to_next_word_start() {
        let mut m = active_mode();
        assert_eq!(m.cursor_col, 0);
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 6); // "world"
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 11); // ","
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 13); // "foo_bar" (underscore is a word char)
    }

    #[test]
    fn word_forward_on_last_word_stops_at_row_end() {
        let mut m = active_mode();
        m.cursor_col = 13;
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 19);
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 19);
    }

    #[test]
    fn word_backward_moves_to_previous_word_start() {
        let mut m = active_mode();
        m.cursor_col = 15; // inside foo_bar
        m.handle_key_in_row(&Key::Char('b'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 13);
        m.handle_key_in_row(&Key::Char('b'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 11);
        m.handle_key_in_row(&Key::Char('b'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 6);
        m.handle_key_in_row(&Key::Char('b'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 0);
        m.handle_key_in_row(&Key::Char('b'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 0);
    }

    #[test]
    fn word_end_moves_to_end_of_word() {
        let mut m = active_mode();
        m.handle_key_in_row(&Key::Char('e'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 4); // end of "hello"
        m.handle_key_in_row(&Key::Char('e'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 10); // end of "world"
        m.handle_key_in_row(&Key::Char('e'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 11); // ","
        m.handle_key_in_row(&Key::Char('e'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 19); // end of foo_bar
    }

    #[test]
    fn word_motions_on_empty_row_stay_put() {
        let mut m = active_mode();
        m.cursor_col = 3;
        m.handle_key(&Key::Char('w'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 3);
        m.handle_key(&Key::Char('e'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 3);
        m.handle_key(&Key::Char('b'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    #[test]
    fn word_motions_never_leave_the_viewport() {
        let mut m = CopyMode::new();
        m.enter(100, 24, 10);
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert!(m.cursor_col <= 9);
    }

    #[test]
    fn count_prefix_repeats_motions() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('k'), Modifiers::NONE); // row 75
        m.handle_key(&Key::Char('k'), Modifiers::NONE); // row 74
        m.handle_key(&Key::Char('1'), Modifiers::NONE);
        m.handle_key(&Key::Char('0'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 84);
        m.handle_key(&Key::Char('5'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 5);
        m.handle_key(&Key::Char('2'), Modifiers::NONE);
        m.handle_key_in_row(&Key::Char('w'), Modifiers::NONE, ROW);
        assert_eq!(m.cursor_col, 11); // 5 -> "world" -> ","
    }

    #[test]
    fn count_with_upper_g_goes_to_that_line() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('3'), Modifiers::NONE);
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 2);
        m.handle_key(&Key::Char('9'), Modifiers::NONE);
        m.handle_key(&Key::Char('9'), Modifiers::NONE);
        m.handle_key(&Key::Char('9'), Modifiers::NONE);
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 99);
    }

    #[test]
    fn leading_zero_is_still_line_start() {
        let mut m = active_mode();
        m.cursor_col = 7;
        m.handle_key(&Key::Char('0'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    // ---------------------------------------------------------------
    // 7. Document motions: G, g (noop), H, M, L
    // ---------------------------------------------------------------

    #[test]
    fn g_upper_goes_to_last_row() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 99);
    }

    #[test]
    fn single_g_waits_for_second_g() {
        let mut m = active_mode();
        let before = m.cursor_position();
        m.handle_key(&Key::Char('g'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), before);
    }

    #[test]
    fn gg_goes_to_top() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('g'), Modifiers::NONE);
        m.handle_key(&Key::Char('g'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 0);
        assert_eq!(m.scroll_offset, 0);
    }

    #[test]
    fn g_followed_by_other_key_cancels_and_runs_that_key() {
        let mut m = active_mode();
        let (row, _) = m.cursor_position();
        m.handle_key(&Key::Char('g'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert_eq!(m.cursor_row, row + 1);
    }

    #[test]
    fn count_gg_goes_to_that_line() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('7'), Modifiers::NONE);
        m.handle_key(&Key::Char('g'), Modifiers::NONE);
        m.handle_key(&Key::Char('g'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 6);
    }

    #[test]
    fn h_upper_screen_top() {
        let mut m = active_mode();
        // scroll_offset = 76, so H -> row 76
        m.handle_key(&Key::Char('j'), Modifiers::NONE); // row 77
        m.handle_key(&Key::Char('H'), Modifiers::NONE);
        assert_eq!(m.cursor_row, m.scroll_offset());
    }

    #[test]
    fn m_upper_screen_middle() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('M'), Modifiers::NONE);
        // scroll_offset=76, viewport=24, mid=76+12=88
        assert_eq!(m.cursor_row, 88);
    }

    #[test]
    fn l_upper_screen_bottom() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('L'), Modifiers::NONE);
        // scroll_offset=76, viewport=24 -> 76+23=99
        assert_eq!(m.cursor_row, 99);
    }

    // ---------------------------------------------------------------
    // 8. Scrolling: Ctrl-d/u (half page), Ctrl-f/b (full page), PageUp/Down
    // ---------------------------------------------------------------

    #[test]
    fn ctrl_d_half_page_down() {
        let mut m = active_mode();
        let start = m.cursor_row; // 76
        m.handle_key(&Key::Char('d'), Modifiers::CTRL);
        assert_eq!(m.cursor_row, start + 12);
    }

    #[test]
    fn ctrl_u_half_page_up() {
        let mut m = active_mode();
        let start = m.cursor_row; // 76
        m.handle_key(&Key::Char('d'), Modifiers::CTRL);
        m.handle_key(&Key::Char('u'), Modifiers::CTRL);
        assert_eq!(m.cursor_row, start);
    }

    #[test]
    fn ctrl_f_full_page_down() {
        let mut m = active_mode();
        // cursor_row starts at 76; 76 + 24 = 100 -> clamped to 99
        m.handle_key(&Key::Char('f'), Modifiers::CTRL);
        assert_eq!(m.cursor_row, 99);
    }

    #[test]
    fn ctrl_b_full_page_up() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('G'), Modifiers::NONE); // row 99
        m.handle_key(&Key::Char('b'), Modifiers::CTRL);
        // 99 - 24 = 75
        assert_eq!(m.cursor_row, 75);
    }

    #[test]
    fn page_down_key() {
        let mut m = active_mode();
        let start = m.cursor_row;
        m.handle_key(&Key::PageDown, Modifiers::NONE);
        assert_eq!(m.cursor_row, (start + 24).min(99));
    }

    #[test]
    fn page_up_key() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('G'), Modifiers::NONE); // row 99
        m.handle_key(&Key::PageUp, Modifiers::NONE);
        assert_eq!(m.cursor_row, 75);
    }

    #[test]
    fn ctrl_d_clamps_at_bottom() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('G'), Modifiers::NONE); // row 99
        m.handle_key(&Key::Char('d'), Modifiers::CTRL);
        assert_eq!(m.cursor_row, 99);
    }

    #[test]
    fn ctrl_u_clamps_at_top() {
        let mut m = CopyMode::new();
        m.enter(10, 24, 80); // cursor starts at row 0
        m.handle_key(&Key::Char('u'), Modifiers::CTRL);
        assert_eq!(m.cursor_row, 0);
    }

    // ---------------------------------------------------------------
    // 9. Scroll offset tracking (ensure_cursor_visible)
    // ---------------------------------------------------------------

    #[test]
    fn scroll_offset_follows_cursor_down() {
        let mut m = active_mode();
        // cursor starts at row 76, scroll_offset 76, viewport 24
        // Move cursor to row 100 (beyond viewport bottom at 76+24=100)
        for _ in 0..24 {
            m.handle_key(&Key::Char('j'), Modifiers::NONE);
        }
        // cursor at 99 (clamped), scroll_offset adjusted
        assert!(m.cursor_row < m.scroll_offset() + m.viewport_rows);
        assert!(m.cursor_row >= m.scroll_offset());
    }

    #[test]
    fn scroll_offset_follows_cursor_up() {
        let mut m = active_mode();
        // Jump to bottom, then page up
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        m.handle_key(&Key::Char('b'), Modifiers::CTRL); // full page up
        assert!(m.cursor_row >= m.scroll_offset());
        assert!(m.cursor_row < m.scroll_offset() + m.viewport_rows);
    }

    // ---------------------------------------------------------------
    // 10. Visual mode (v) - character selection
    // ---------------------------------------------------------------

    #[test]
    fn v_enters_visual_mode() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Visual);
        assert!(m.selection().is_some());
    }

    #[test]
    fn v_toggles_visual_off() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Visual);
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.selection().is_none());
    }

    #[test]
    fn visual_char_selection_range() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        // anchor at (76, 0)
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        // cursor now at (77, 2)
        let sel = m.selection().unwrap();
        assert_eq!(
            sel,
            Selection::Char {
                start_row: 76,
                start_col: 0,
                end_row: 77,
                end_col: 2,
            }
        );
    }

    #[test]
    fn visual_char_selection_backwards() {
        let mut m = active_mode();
        // Move cursor to (78, 5) first
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.cursor_col = 5;
        // Enter visual
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        // anchor at (78, 5)
        // Move backwards
        m.handle_key(&Key::Char('k'), Modifiers::NONE);
        m.handle_key(&Key::Char('h'), Modifiers::NONE);
        // cursor at (77, 4)
        let sel = m.selection().unwrap();
        assert_eq!(
            sel,
            Selection::Char {
                start_row: 77,
                start_col: 4,
                end_row: 78,
                end_col: 5,
            }
        );
    }

    // ---------------------------------------------------------------
    // 11. Visual Line mode (V)
    // ---------------------------------------------------------------

    #[test]
    fn v_upper_enters_visual_line() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::VisualLine);
    }

    #[test]
    fn v_upper_toggles_off() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.selection().is_none());
    }

    #[test]
    fn visual_line_selection_range() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        let sel = m.selection().unwrap();
        assert_eq!(
            sel,
            Selection::Line {
                start_row: 76,
                end_row: 78
            }
        );
    }

    // ---------------------------------------------------------------
    // 12. Visual Block mode (Ctrl-v)
    // ---------------------------------------------------------------

    #[test]
    fn ctrl_v_enters_visual_block() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        assert_eq!(m.vim_mode(), VimMode::VisualBlock);
    }

    #[test]
    fn ctrl_v_toggles_off() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.selection().is_none());
    }

    #[test]
    fn visual_block_selection_range() {
        let mut m = active_mode();
        m.cursor_col = 3;
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        // anchor at (76, 3)
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        // cursor at (78, 6)
        let sel = m.selection().unwrap();
        assert_eq!(
            sel,
            Selection::Block {
                start_row: 76,
                start_col: 3,
                end_row: 78,
                end_col: 6,
            }
        );
    }

    #[test]
    fn visual_block_cursor_left_of_anchor() {
        let mut m = active_mode();
        m.cursor_col = 10;
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        // anchor at (76, 10)
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        // Move cursor left of anchor
        m.cursor_col = 3;
        // cursor at (77, 3), anchor col=10
        let sel = m.selection().unwrap();
        // Block selection should use min/max of cursor_col and start_col
        assert_eq!(
            sel,
            Selection::Block {
                start_row: 76,
                start_col: 3,
                end_row: 77,
                end_col: 10,
            }
        );
    }

    // ---------------------------------------------------------------
    // 13. Escape / q from visual returns to normal; from normal exits
    // ---------------------------------------------------------------

    #[test]
    fn escape_in_visual_returns_to_normal() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        let r = m.handle_key(&Key::Escape, Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.is_active()); // still in copy mode
    }

    #[test]
    fn escape_in_normal_exits() {
        let mut m = active_mode();
        let r = m.handle_key(&Key::Escape, Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Exit));
        assert!(!m.is_active());
    }

    #[test]
    fn q_in_normal_exits() {
        let mut m = active_mode();
        let r = m.handle_key(&Key::Char('q'), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Exit));
        assert!(!m.is_active());
    }

    #[test]
    fn q_in_visual_returns_to_normal() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        let r = m.handle_key(&Key::Char('q'), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.is_active());
    }

    // ---------------------------------------------------------------
    // 14. Enter key: copy+exit with selection, plain exit without
    // ---------------------------------------------------------------

    #[test]
    fn enter_without_selection_exits() {
        let mut m = active_mode();
        let r = m.handle_key(&Key::Enter, Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Exit));
        assert!(!m.is_active());
    }

    #[test]
    fn enter_with_selection_copies_and_exits() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        let r = m.handle_key(&Key::Enter, Modifiers::NONE);
        assert!(matches!(
            r,
            CopyModeResult::CopyAndExit(Some(Selection::Char { .. }))
        ));
        assert!(!m.is_active());
    }

    // ---------------------------------------------------------------
    // 15. Yank (y)
    // ---------------------------------------------------------------

    #[test]
    fn y_without_selection_is_noop() {
        let mut m = active_mode();
        let r = m.handle_key(&Key::Char('y'), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
        // Still active, still normal
        assert!(m.is_active());
    }

    #[test]
    fn y_with_visual_selection_copies_and_returns_to_normal() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        let r = m.handle_key(&Key::Char('y'), Modifiers::NONE);
        assert!(matches!(
            r,
            CopyModeResult::Copy(Some(Selection::Char { .. }))
        ));
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.selection().is_none());
        assert!(m.is_active()); // does NOT exit copy mode
    }

    #[test]
    fn y_with_visual_line_selection() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        let r = m.handle_key(&Key::Char('y'), Modifiers::NONE);
        assert!(matches!(
            r,
            CopyModeResult::Copy(Some(Selection::Line { .. }))
        ));
    }

    #[test]
    fn y_with_visual_block_selection() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        let r = m.handle_key(&Key::Char('y'), Modifiers::NONE);
        assert!(matches!(
            r,
            CopyModeResult::Copy(Some(Selection::Block { .. }))
        ));
    }

    // ---------------------------------------------------------------
    // 16. Search: /, ?, typing, Enter, Escape, n, N
    // ---------------------------------------------------------------

    #[test]
    fn slash_enters_search_forward() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Search);
        assert_eq!(m.search_query(), "");
    }

    #[test]
    fn question_enters_search_backward() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('?'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Search);
    }

    #[test]
    fn search_typing_builds_query() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('h'), Modifiers::NONE);
        m.handle_key(&Key::Char('e'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        m.handle_key(&Key::Char('o'), Modifiers::NONE);
        assert_eq!(m.search_query(), "hello");
    }

    #[test]
    fn search_backspace_removes_char() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('a'), Modifiers::NONE);
        m.handle_key(&Key::Char('b'), Modifiers::NONE);
        m.handle_key(&Key::Backspace, Modifiers::NONE);
        assert_eq!(m.search_query(), "a");
    }

    #[test]
    fn search_backspace_on_empty_is_harmless() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Backspace, Modifiers::NONE);
        assert_eq!(m.search_query(), "");
    }

    #[test]
    fn search_enter_returns_search_result() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('f'), Modifiers::NONE);
        m.handle_key(&Key::Char('o'), Modifiers::NONE);
        m.handle_key(&Key::Char('o'), Modifiers::NONE);
        let r = m.handle_key(&Key::Enter, Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Search(ref s) if s == "foo"));
        assert_eq!(m.vim_mode(), VimMode::Normal);
    }

    #[test]
    fn search_enter_empty_query_is_handled() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        let r = m.handle_key(&Key::Enter, Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
        assert_eq!(m.vim_mode(), VimMode::Normal);
    }

    #[test]
    fn search_escape_cancels() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('x'), Modifiers::NONE);
        m.handle_key(&Key::Escape, Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert_eq!(m.search_query(), ""); // cleared on escape
    }

    // ---------------------------------------------------------------
    // 17. Search match navigation: n, N with set_search_matches
    // ---------------------------------------------------------------

    #[test]
    fn n_without_matches_is_noop() {
        let mut m = active_mode();
        let pos_before = m.cursor_position();
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), pos_before);
    }

    #[test]
    fn n_forward_cycles_through_matches() {
        let mut m = active_mode();
        // Set up search direction as forward
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('x'), Modifiers::NONE);
        m.handle_key(&Key::Enter, Modifiers::NONE);
        // Provide matches
        m.set_search_matches(vec![(10, 5), (20, 3), (30, 7)]);
        // current_match starts at 0, n goes forward -> index 1
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (20, 3));
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (30, 7));
        // Wrap around
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (10, 5));
    }

    #[test]
    fn n_upper_goes_backward_in_forward_search() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('x'), Modifiers::NONE);
        m.handle_key(&Key::Enter, Modifiers::NONE);
        m.set_search_matches(vec![(10, 5), (20, 3), (30, 7)]);
        // current_match=0, N goes backward -> wraps to 2
        m.handle_key(&Key::Char('N'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (30, 7));
        m.handle_key(&Key::Char('N'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (20, 3));
    }

    #[test]
    fn search_backward_reverses_n_and_n_upper() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('?'), Modifiers::NONE); // backward search
        m.handle_key(&Key::Char('x'), Modifiers::NONE);
        m.handle_key(&Key::Enter, Modifiers::NONE);
        m.set_search_matches(vec![(10, 5), (20, 3), (30, 7)]);
        // In backward search, n goes backward (decrements), N goes forward (increments)
        // current_match=0, n in backward search -> wraps to 2
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (30, 7));
        // N in backward search -> increments -> index 0
        m.handle_key(&Key::Char('N'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (10, 5));
    }

    #[test]
    fn set_search_matches_resets_current() {
        let mut m = active_mode();
        m.set_search_matches(vec![(1, 1), (2, 2)]);
        let (matches, idx) = m.search_matches();
        assert_eq!(matches.len(), 2);
        assert_eq!(idx, 0);
    }

    #[test]
    fn n_with_single_match_stays() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('x'), Modifiers::NONE);
        m.handle_key(&Key::Enter, Modifiers::NONE);
        m.set_search_matches(vec![(42, 10)]);
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        // wraps: (0+1)%1 = 0
        assert_eq!(m.cursor_position(), (42, 10));
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (42, 10));
    }

    // ---------------------------------------------------------------
    // 18. Mode transitions: switching between visual modes
    // ---------------------------------------------------------------

    #[test]
    fn visual_to_visual_line_switches() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::Visual);
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::VisualLine);
        assert!(m.selection().is_some());
    }

    #[test]
    fn visual_line_to_visual_block_switches() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        assert_eq!(m.vim_mode(), VimMode::VisualLine);
        m.handle_key(&Key::Char('v'), Modifiers::CTRL);
        assert_eq!(m.vim_mode(), VimMode::VisualBlock);
    }

    // ---------------------------------------------------------------
    // 19. Edge case: empty / minimal buffers
    // ---------------------------------------------------------------

    #[test]
    fn empty_buffer_movements() {
        let mut m = CopyMode::new();
        m.enter(0, 24, 80);
        // All movements should be safe with 0 total_rows
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.handle_key(&Key::Char('k'), Modifiers::NONE);
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        m.handle_key(&Key::Char('d'), Modifiers::CTRL);
        m.handle_key(&Key::Char('u'), Modifiers::CTRL);
        // Should not panic, cursor stays at 0
        assert_eq!(m.cursor_row, 0);
    }

    #[test]
    fn single_row_buffer() {
        let mut m = CopyMode::new();
        m.enter(1, 24, 80);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 0);
        m.handle_key(&Key::Char('G'), Modifiers::NONE);
        assert_eq!(m.cursor_row, 0);
    }

    #[test]
    fn one_col_wide_viewport() {
        let mut m = CopyMode::new();
        m.enter(10, 5, 1);
        m.handle_key(&Key::Char('l'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
        m.handle_key(&Key::Char('$'), Modifiers::NONE);
        assert_eq!(m.cursor_col, 0);
    }

    // ---------------------------------------------------------------
    // 20. Selection with no anchor returns None
    // ---------------------------------------------------------------

    #[test]
    fn no_selection_in_normal_mode() {
        let m = active_mode();
        assert!(m.selection().is_none());
    }

    // ---------------------------------------------------------------
    // 21. Unrecognised key returns Handled (swallowed)
    // ---------------------------------------------------------------

    #[test]
    fn unknown_key_is_handled() {
        let mut m = active_mode();
        let r = m.handle_key(&Key::Char('z'), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
    }

    #[test]
    fn function_key_is_handled() {
        let mut m = active_mode();
        let r = m.handle_key(&Key::Function(5), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
    }

    // ---------------------------------------------------------------
    // 22. Search key in search mode for unhandled keys
    // ---------------------------------------------------------------

    #[test]
    fn search_mode_ignores_function_keys() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        let r = m.handle_key(&Key::Function(1), Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Handled));
        assert_eq!(m.vim_mode(), VimMode::Search);
    }

    // ---------------------------------------------------------------
    // 23. Re-entering copy mode resets state
    // ---------------------------------------------------------------

    #[test]
    fn re_enter_resets_state() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        // Now re-enter
        m.enter(200, 30, 100);
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.selection().is_none());
        assert_eq!(m.cursor_col, 0);
        assert_eq!(m.cursor_row, 170); // 200-30
        assert_eq!(m.scroll_offset(), 170);
    }

    // ---------------------------------------------------------------
    // 24. Combined sequences
    // ---------------------------------------------------------------

    #[test]
    fn visual_select_move_yank_sequence() {
        let mut m = active_mode();
        // v -> move right 3 -> move down 1 -> y
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        for _ in 0..3 {
            m.handle_key(&Key::Char('l'), Modifiers::NONE);
        }
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        let r = m.handle_key(&Key::Char('y'), Modifiers::NONE);
        match r {
            CopyModeResult::Copy(Some(Selection::Char {
                start_row,
                start_col,
                end_row,
                end_col,
            })) => {
                assert_eq!(start_row, 76);
                assert_eq!(start_col, 0);
                assert_eq!(end_row, 77);
                assert_eq!(end_col, 3);
            }
            _ => panic!("Expected Copy with Char selection"),
        }
        // Back to normal, still active
        assert_eq!(m.vim_mode(), VimMode::Normal);
        assert!(m.is_active());
    }

    #[test]
    fn visual_line_select_enter_copies_and_exits() {
        let mut m = active_mode();
        m.handle_key(&Key::Char('V'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        m.handle_key(&Key::Char('j'), Modifiers::NONE);
        let r = m.handle_key(&Key::Enter, Modifiers::NONE);
        match r {
            CopyModeResult::CopyAndExit(Some(Selection::Line { start_row, end_row })) => {
                assert_eq!(start_row, 76);
                assert_eq!(end_row, 78);
            }
            _ => panic!("Expected CopyAndExit with Line selection"),
        }
        assert!(!m.is_active());
    }

    #[test]
    fn search_then_navigate_matches() {
        let mut m = active_mode();
        // Start search
        m.handle_key(&Key::Char('/'), Modifiers::NONE);
        m.handle_key(&Key::Char('t'), Modifiers::NONE);
        m.handle_key(&Key::Char('e'), Modifiers::NONE);
        m.handle_key(&Key::Char('s'), Modifiers::NONE);
        m.handle_key(&Key::Char('t'), Modifiers::NONE);
        let r = m.handle_key(&Key::Enter, Modifiers::NONE);
        assert!(matches!(r, CopyModeResult::Search(ref s) if s == "test"));
        // Simulate external match setting
        m.set_search_matches(vec![(5, 0), (15, 10), (25, 20)]);
        // Navigate forward
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (15, 10));
        // Select from here
        m.handle_key(&Key::Char('v'), Modifiers::NONE);
        m.handle_key(&Key::Char('n'), Modifiers::NONE);
        assert_eq!(m.cursor_position(), (25, 20));
        // Yank the selection
        let r = m.handle_key(&Key::Char('y'), Modifiers::NONE);
        match r {
            CopyModeResult::Copy(Some(Selection::Char {
                start_row,
                start_col,
                end_row,
                end_col,
            })) => {
                assert_eq!(start_row, 15);
                assert_eq!(start_col, 10);
                assert_eq!(end_row, 25);
                assert_eq!(end_col, 20);
            }
            _ => panic!("Expected Copy with Char selection"),
        }
    }

    // ---------------------------------------------------------------
    // Resize clamps cursor and scroll
    // ---------------------------------------------------------------

    #[test]
    fn resize_clamps_cursor() {
        let mut m = CopyMode::new();
        m.enter(100, 24, 80);
        // Move cursor to a high position
        m.cursor_row = 90;
        m.cursor_col = 70;

        // Shrink viewport
        m.resize(50, 10, 40);

        // cursor_row should be clamped to 49 (total_rows - 1)
        assert_eq!(m.cursor_row, 49);
        // cursor_col should be clamped to 39 (viewport_cols - 1)
        assert_eq!(m.cursor_col, 39);
        // scroll_offset should keep cursor visible
        assert!(m.cursor_row >= m.scroll_offset);
        assert!(m.cursor_row < m.scroll_offset + 10);
    }

    #[test]
    fn resize_clamps_selection_anchor() {
        let mut m = CopyMode::new();
        m.enter(100, 24, 80);
        m.selection_anchor = Some((95, 75));

        m.resize(50, 24, 40);

        let (row, col) = m.selection_anchor.unwrap();
        assert_eq!(row, 49);
        assert_eq!(col, 39);
    }
}
