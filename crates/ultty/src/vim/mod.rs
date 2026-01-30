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

use ultty_protocol::vim::{VimMode, VimState};
use ultty_protocol::{Key, Modifiers};

/// Terminal copy mode state.
#[derive(Debug)]
pub struct CopyMode {
    active: bool,
    vim_state: VimState,
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
}

impl Default for CopyMode {
    fn default() -> Self {
        Self {
            active: false,
            vim_state: VimState::new(),
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
        self.vim_state = VimState::new();
        self.total_rows = total_rows;
        self.viewport_rows = viewport_rows;
        self.viewport_cols = viewport_cols;
        self.scroll_offset = total_rows.saturating_sub(viewport_rows);
        self.cursor_row = self.scroll_offset;
        self.cursor_col = 0;
        self.selection_anchor = None;
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.selection_anchor = None;
        self.vim_state.reset_to_normal();
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

            match self.vim_state.mode {
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
        self.vim_state.mode
    }

    pub fn handle_key(&mut self, key: &Key, modifiers: Modifiers) -> CopyModeResult {
        if !self.active {
            return CopyModeResult::NotHandled;
        }

        if self.vim_state.mode == VimMode::Search {
            return self.handle_search_key(key, modifiers);
        }

        match key {
            Key::Escape | Key::Char('q') => {
                if self.vim_state.mode.is_visual() {
                    self.selection_anchor = None;
                    self.vim_state.mode = VimMode::Normal;
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
                self.move_cursor(0, -1);
                CopyModeResult::Handled
            }
            Key::Char('l') | Key::Right => {
                self.move_cursor(0, 1);
                CopyModeResult::Handled
            }
            Key::Char('j') | Key::Down => {
                self.move_cursor(1, 0);
                CopyModeResult::Handled
            }
            Key::Char('k') | Key::Up => {
                self.move_cursor(-1, 0);
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
                self.move_word_forward();
                CopyModeResult::Handled
            }
            Key::Char('b') => {
                self.move_word_backward();
                CopyModeResult::Handled
            }
            Key::Char('e') => {
                self.move_word_end();
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

            Key::Char('g') => CopyModeResult::Handled,
            Key::Char('G') => {
                self.cursor_row = self.total_rows.saturating_sub(1);
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
                if self.vim_state.mode == VimMode::Visual {
                    self.selection_anchor = None;
                    self.vim_state.mode = VimMode::Normal;
                } else {
                    self.selection_anchor = Some((self.cursor_row, self.cursor_col));
                    self.vim_state.mode = VimMode::Visual;
                }
                CopyModeResult::Handled
            }
            Key::Char('V') => {
                if self.vim_state.mode == VimMode::VisualLine {
                    self.selection_anchor = None;
                    self.vim_state.mode = VimMode::Normal;
                } else {
                    self.selection_anchor = Some((self.cursor_row, 0));
                    self.vim_state.mode = VimMode::VisualLine;
                }
                CopyModeResult::Handled
            }
            Key::Char('v') if modifiers.ctrl() => {
                if self.vim_state.mode == VimMode::VisualBlock {
                    self.selection_anchor = None;
                    self.vim_state.mode = VimMode::Normal;
                } else {
                    self.selection_anchor = Some((self.cursor_row, self.cursor_col));
                    self.vim_state.mode = VimMode::VisualBlock;
                }
                CopyModeResult::Handled
            }

            Key::Char('y') => {
                if self.selection_anchor.is_some() {
                    let selection = self.selection();
                    self.selection_anchor = None;
                    self.vim_state.mode = VimMode::Normal;
                    CopyModeResult::Copy(selection)
                } else {
                    CopyModeResult::Handled
                }
            }

            Key::Char('/') => {
                self.vim_state.mode = VimMode::Search;
                self.vim_state.search_forward = true;
                self.search_query.clear();
                CopyModeResult::Handled
            }
            Key::Char('?') => {
                self.vim_state.mode = VimMode::Search;
                self.vim_state.search_forward = false;
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
                self.vim_state.mode = VimMode::Normal;
                self.search_query.clear();
                CopyModeResult::Handled
            }
            Key::Enter => {
                self.vim_state.mode = VimMode::Normal;
                if !self.search_query.is_empty() {
                    self.vim_state.last_search = Some(self.search_query.clone());
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

    fn move_word_forward(&mut self) {
        self.cursor_col = (self.cursor_col + 5).min(self.viewport_cols.saturating_sub(1));
    }

    fn move_word_backward(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(5);
    }

    fn move_word_end(&mut self) {
        self.cursor_col = (self.cursor_col + 4).min(self.viewport_cols.saturating_sub(1));
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

        let forward = self.vim_state.search_forward;
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

        let forward = self.vim_state.search_forward;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copy_mode_enter_exit() {
        let mut mode = CopyMode::new();
        assert!(!mode.is_active());

        mode.enter(100, 24, 80);
        assert!(mode.is_active());
        assert_eq!(mode.cursor_row, 76);

        mode.exit();
        assert!(!mode.is_active());
    }

    #[test]
    fn test_cursor_movement() {
        let mut mode = CopyMode::new();
        mode.enter(100, 24, 80);

        let initial_row = mode.cursor_row;
        mode.handle_key(&Key::Char('j'), Modifiers::NONE);
        assert_eq!(mode.cursor_row, initial_row + 1);

        mode.handle_key(&Key::Char('k'), Modifiers::NONE);
        assert_eq!(mode.cursor_row, initial_row);

        mode.handle_key(&Key::Char('l'), Modifiers::NONE);
        assert_eq!(mode.cursor_col, 1);

        mode.handle_key(&Key::Char('h'), Modifiers::NONE);
        assert_eq!(mode.cursor_col, 0);
    }

    #[test]
    fn test_visual_selection() {
        let mut mode = CopyMode::new();
        mode.enter(100, 24, 80);

        mode.handle_key(&Key::Char('v'), Modifiers::NONE);
        assert_eq!(mode.vim_mode(), VimMode::Visual);
        assert!(mode.selection_anchor.is_some());

        mode.handle_key(&Key::Char('l'), Modifiers::NONE);
        mode.handle_key(&Key::Char('l'), Modifiers::NONE);

        let selection = mode.selection();
        assert!(matches!(selection, Some(Selection::Char { .. })));

        mode.handle_key(&Key::Escape, Modifiers::NONE);
        assert_eq!(mode.vim_mode(), VimMode::Normal);
        assert!(mode.selection_anchor.is_none());
    }

    #[test]
    fn test_page_navigation() {
        let mut mode = CopyMode::new();
        mode.enter(100, 24, 80);

        let initial_row = mode.cursor_row;
        mode.handle_key(&Key::Char('d'), Modifiers::CTRL);
        assert_eq!(mode.cursor_row, initial_row + 12);

        mode.handle_key(&Key::Char('u'), Modifiers::CTRL);
        assert_eq!(mode.cursor_row, initial_row);
    }
}
