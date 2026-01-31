//! Terminal state management with alternate screen support.

use super::{Buffer, CursorPosition, Dimensions, Grid};

/// Terminal modes that affect behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    /// Application cursor keys (DECCKM)
    pub application_cursor_keys: bool,
    /// Bracketed paste mode
    pub bracketed_paste: bool,
    /// Focus reporting
    pub focus_reporting: bool,
    /// Auto-wrap at end of line
    pub auto_wrap: bool,
    /// Origin mode (cursor relative to scroll region)
    pub origin_mode: bool,
    /// Insert mode (insert vs replace)
    pub insert_mode: bool,
}

/// Terminal state with main and alternate screen buffers.
///
/// This is the top-level terminal emulation state, managing:
/// - Main screen buffer with scrollback
/// - Alternate screen buffer (for full-screen apps)
/// - Terminal modes
/// - Selection state
#[derive(Debug)]
pub struct Terminal {
    /// Main screen buffer (with scrollback)
    main: Buffer,
    /// Alternate screen buffer (no scrollback)
    alternate: Buffer,
    /// Currently using alternate screen
    using_alternate: bool,
    /// Terminal modes
    modes: TerminalModes,
    /// Current selection (if any)
    selection: Option<Selection>,
    /// Window title
    title: String,
}

impl Terminal {
    /// Create a new terminal with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        Self {
            main: Buffer::new(dims),
            alternate: Buffer::with_scrollback(dims, 0), // No scrollback for alternate
            using_alternate: false,
            modes: TerminalModes::default(),
            selection: None,
            title: String::new(),
        }
    }

    /// Get the active buffer.
    pub fn buffer(&self) -> &Buffer {
        if self.using_alternate {
            &self.alternate
        } else {
            &self.main
        }
    }

    /// Get mutable access to the active buffer.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        // Clear selection when buffer is modified
        self.selection = None;

        if self.using_alternate {
            &mut self.alternate
        } else {
            &mut self.main
        }
    }

    /// Get the active grid.
    pub fn grid(&self) -> &Grid {
        self.buffer().grid()
    }

    /// Get mutable access to the active grid.
    pub fn grid_mut(&mut self) -> &mut Grid {
        self.buffer_mut().grid_mut()
    }

    /// Get terminal dimensions.
    pub fn dimensions(&self) -> Dimensions {
        self.main.dimensions()
    }

    /// Check if using alternate screen.
    pub fn is_alternate_screen(&self) -> bool {
        self.using_alternate
    }

    /// Enter alternate screen buffer.
    pub fn enter_alternate_screen(&mut self) {
        if !self.using_alternate {
            self.using_alternate = true;
            self.alternate.grid_mut().clear();
            self.alternate.grid_mut().cursor_mut().position = CursorPosition::origin();
            self.selection = None;
        }
    }

    /// Leave alternate screen buffer.
    pub fn leave_alternate_screen(&mut self) {
        if self.using_alternate {
            self.using_alternate = false;
            self.selection = None;
        }
    }

    /// Get terminal modes.
    pub fn modes(&self) -> &TerminalModes {
        &self.modes
    }

    /// Get mutable terminal modes.
    pub fn modes_mut(&mut self) -> &mut TerminalModes {
        &mut self.modes
    }

    /// Get current selection.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Start a new selection.
    pub fn start_selection(&mut self, pos: SelectionPoint, mode: SelectionMode) {
        self.selection = Some(Selection {
            start: pos,
            end: pos,
            mode,
        });
    }

    /// Update selection end point.
    pub fn update_selection(&mut self, end: SelectionPoint) {
        if let Some(sel) = &mut self.selection {
            sel.end = end;
        }
    }

    /// Clear selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Get selected text.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.ordered();

        let mut text = String::new();
        let buffer = self.buffer();

        match sel.mode {
            SelectionMode::Normal => {
                // Character-wise selection
                for row_idx in start.row..=end.row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        let start_col = if row_idx == start.row {
                            start.col
                        } else {
                            0
                        };
                        let end_col = if row_idx == end.row {
                            end.col
                        } else {
                            buffer.dimensions().cols as i32 - 1
                        };

                        for col in start_col..=end_col {
                            if let Some(cell) = row.get(col as u16) {
                                text.push(cell.char());
                            }
                        }

                        // Add newline between rows (except last)
                        if row_idx < end.row {
                            text.push('\n');
                        }
                    }
                }
            }
            SelectionMode::Line => {
                // Line-wise selection
                for row_idx in start.row..=end.row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        text.push_str(&row.text_trimmed());
                        if row_idx < end.row {
                            text.push('\n');
                        }
                    }
                }
            }
            SelectionMode::Block => {
                // Block/rectangular selection
                let (left, right) = if start.col <= end.col {
                    (start.col, end.col)
                } else {
                    (end.col, start.col)
                };

                for row_idx in start.row..=end.row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        for col in left..=right {
                            if let Some(cell) = row.get(col as u16) {
                                text.push(cell.char());
                            }
                        }
                        if row_idx < end.row {
                            text.push('\n');
                        }
                    }
                }
            }
        }

        // Trim trailing whitespace from each line
        let trimmed: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
        Some(trimmed.join("\n"))
    }

    /// Check if a cell is within the selection.
    pub fn is_selected(&self, col: u16, row: u16) -> bool {
        let Some(sel) = &self.selection else {
            return false;
        };

        let (start, end) = sel.ordered();
        let row = row as i32;
        let col = col as i32;

        match sel.mode {
            SelectionMode::Normal => {
                if row < start.row || row > end.row {
                    return false;
                }
                if row == start.row && row == end.row {
                    col >= start.col && col <= end.col
                } else if row == start.row {
                    col >= start.col
                } else if row == end.row {
                    col <= end.col
                } else {
                    true
                }
            }
            SelectionMode::Line => row >= start.row && row <= end.row,
            SelectionMode::Block => {
                let (left, right) = if start.col <= end.col {
                    (start.col, end.col)
                } else {
                    (end.col, start.col)
                };
                row >= start.row && row <= end.row && col >= left && col <= right
            }
        }
    }

    /// Get window title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set window title.
    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    /// Resize the terminal.
    pub fn resize(&mut self, new_dims: Dimensions) {
        self.main.resize(new_dims);
        self.alternate.resize(new_dims);
        self.selection = None;
    }
}

/// Selection point (can be negative for scroll offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    pub col: i32,
    pub row: i32,
}

impl SelectionPoint {
    pub fn new(col: i32, row: i32) -> Self {
        Self { col, row }
    }
}

/// Selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Normal character-wise selection
    Normal,
    /// Line-wise selection
    Line,
    /// Block/rectangular selection
    Block,
}

/// Selection state.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Start point of selection
    pub start: SelectionPoint,
    /// End point of selection (current position)
    pub end: SelectionPoint,
    /// Selection mode
    pub mode: SelectionMode,
}

impl Selection {
    /// Get ordered start and end (top-left to bottom-right).
    pub fn ordered(&self) -> (SelectionPoint, SelectionPoint) {
        if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    /// Check if selection is empty (start == end).
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_terminal() {
        let term = Terminal::new(Dimensions::new(80, 24));
        assert_eq!(term.dimensions().cols, 80);
        assert_eq!(term.dimensions().rows, 24);
        assert!(!term.is_alternate_screen());
    }

    #[test]
    fn alternate_screen() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Write something to main screen
        term.grid_mut().write_char('M');

        // Enter alternate screen
        term.enter_alternate_screen();
        assert!(term.is_alternate_screen());

        // Alternate should be clear
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), ' ');

        // Write to alternate
        term.grid_mut().write_char('A');
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'A');

        // Return to main
        term.leave_alternate_screen();
        assert!(!term.is_alternate_screen());

        // Main should still have original content
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'M');
    }

    #[test]
    fn selection_normal() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        // Write some text
        for c in "Hello".chars() {
            term.grid_mut().write_char(c);
        }

        // Select "ell"
        term.start_selection(SelectionPoint::new(1, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(3, 0));

        assert!(term.is_selected(1, 0));
        assert!(term.is_selected(2, 0));
        assert!(term.is_selected(3, 0));
        assert!(!term.is_selected(0, 0));
        assert!(!term.is_selected(4, 0));

        assert_eq!(term.selected_text(), Some("ell".to_string()));
    }

    #[test]
    fn selection_multiline() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        // Write text on two lines
        for c in "Line1".chars() {
            term.grid_mut().write_char(c);
        }
        term.grid_mut().carriage_return();
        term.grid_mut().newline();
        for c in "Line2".chars() {
            term.grid_mut().write_char(c);
        }

        // Select from "ne1" to "Lin" of second line
        term.start_selection(SelectionPoint::new(2, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(2, 1));

        let text = term.selected_text().unwrap();
        assert!(text.contains("ne1"));
        assert!(text.contains("Lin"));
    }

    #[test]
    fn selection_line_mode() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        for c in "Hello".chars() {
            term.grid_mut().write_char(c);
        }

        // Line selection
        term.start_selection(SelectionPoint::new(2, 0), SelectionMode::Line);
        term.update_selection(SelectionPoint::new(2, 0));

        // Entire line should be selected
        assert!(term.is_selected(0, 0));
        assert!(term.is_selected(5, 0));
        assert!(!term.is_selected(0, 1));

        assert_eq!(term.selected_text(), Some("Hello".to_string()));
    }

    #[test]
    fn selection_block_mode() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        // Write a grid
        for row in 0..3 {
            term.grid_mut().set_cursor_position(row + 1, 1);
            for c in "ABCDE".chars() {
                term.grid_mut().write_char(c);
            }
        }

        // Block select middle column across all rows
        term.start_selection(SelectionPoint::new(1, 0), SelectionMode::Block);
        term.update_selection(SelectionPoint::new(3, 2));

        // Should select columns 1-3 on rows 0-2
        assert!(term.is_selected(1, 0));
        assert!(term.is_selected(2, 1));
        assert!(term.is_selected(3, 2));
        assert!(!term.is_selected(0, 0));
        assert!(!term.is_selected(4, 0));

        let text = term.selected_text().unwrap();
        assert_eq!(text, "BCD\nBCD\nBCD");
    }

    #[test]
    fn resize_clears_selection() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        assert!(term.selection().is_some());

        term.resize(Dimensions::new(100, 30));
        assert!(term.selection().is_none());
    }
}
