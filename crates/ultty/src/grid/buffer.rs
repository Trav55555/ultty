//! Terminal buffer with scrollback history.

use std::collections::VecDeque;

use super::{CellFlags, Dimensions, Grid, Row};

/// Default scrollback buffer size (lines).
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// Terminal buffer with scrollback history.
///
/// This wraps a [`Grid`] and maintains a history of scrolled-off lines
/// that can be viewed by scrolling back.
#[derive(Debug, Clone)]
pub struct Buffer {
    /// The visible grid
    grid: Grid,
    /// Scrollback history (oldest first)
    history: VecDeque<Row>,
    /// Maximum history size
    max_history: usize,
    /// Current scroll offset (0 = at bottom, >0 = scrolled up)
    scroll_offset: usize,
}

impl Buffer {
    /// Create a new buffer with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        Self::with_scrollback(dims, DEFAULT_SCROLLBACK)
    }

    /// Create a new buffer with custom scrollback size.
    pub fn with_scrollback(dims: Dimensions, max_history: usize) -> Self {
        Self {
            grid: Grid::new(dims),
            history: VecDeque::with_capacity(max_history.min(1000)), // Pre-allocate reasonably
            max_history,
            scroll_offset: 0,
        }
    }

    /// Get the underlying grid.
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Get mutable access to the grid.
    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// Get grid dimensions.
    pub fn dimensions(&self) -> Dimensions {
        self.grid.dimensions()
    }

    /// Get total lines (visible + history).
    pub fn total_lines(&self) -> usize {
        self.history.len() + self.grid.dimensions().rows as usize
    }

    /// Get current scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Check if scrolled back from bottom.
    pub fn is_scrolled(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Scroll up by n lines.
    pub fn scroll_up(&mut self, n: usize) {
        let max_offset = self.history.len();
        self.scroll_offset = (self.scroll_offset + n).min(max_offset);
    }

    /// Scroll down by n lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll to top of history.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = self.history.len();
    }

    /// Scroll to bottom (most recent).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Push lines from grid top into history (called when grid scrolls up).
    pub fn push_to_history(&mut self, rows: Vec<Row>) {
        if self.max_history == 0 {
            return; // No history storage configured (e.g., alternate screen)
        }
        for row in rows {
            if self.history.len() >= self.max_history {
                self.history.pop_front();
            }
            self.history.push_back(row);
        }
    }

    /// Get a row at the given visual index (accounting for scroll).
    ///
    /// Index 0 is the top of the visible area.
    pub fn visible_row(&self, visual_index: u16) -> Option<&Row> {
        let total_history = self.history.len();
        let visible_rows = self.grid.dimensions().rows as usize;

        // Calculate which row this maps to
        if self.scroll_offset > 0 {
            // We're scrolled back
            let history_start = total_history.saturating_sub(self.scroll_offset);
            let absolute_index = history_start + visual_index as usize;

            if absolute_index < total_history {
                // This row is in history
                self.history.get(absolute_index)
            } else {
                // This row is in the visible grid
                let grid_index = absolute_index - total_history;
                if grid_index < visible_rows {
                    self.grid.row(grid_index as u16)
                } else {
                    None
                }
            }
        } else {
            // Not scrolled, show grid directly
            self.grid.row(visual_index)
        }
    }

    /// Get a row by absolute index (for searching all content).
    ///
    /// Index 0 is the oldest history row.
    /// Indices beyond history access the current grid.
    pub fn row_at(&self, absolute_index: usize) -> Option<&Row> {
        let history_len = self.history.len();
        if absolute_index < history_len {
            self.history.get(absolute_index)
        } else {
            let grid_index = absolute_index - history_len;
            self.grid.row(grid_index as u16)
        }
    }

    /// Clear scrollback history.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.scroll_offset = 0;
    }

    /// Get history length.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Resize the buffer with proper reflow.
    pub fn resize(&mut self, new_dims: Dimensions) {
        let old_dims = self.grid.dimensions();

        if new_dims == old_dims {
            return;
        }

        // If width changed, we need to reflow both history and grid together
        if new_dims.cols != old_dims.cols {
            self.reflow_all(new_dims);
        } else {
            // Only height changed - handle specially
            self.resize_height(new_dims);
        }

        self.scroll_offset = self.scroll_offset.min(self.history.len());
    }

    /// Handle resize when only height changes.
    fn resize_height(&mut self, new_dims: Dimensions) {
        let old_rows = self.grid.dimensions().rows as usize;
        let new_rows = new_dims.rows as usize;

        if new_rows < old_rows {
            // Shrinking - move top rows to history, keep bottom rows in grid
            let rows_to_move = old_rows - new_rows;
            let grid_rows: Vec<Row> = self.grid.rows().take(rows_to_move).cloned().collect();
            self.push_to_history(grid_rows);
            // Remove those rows from the grid (shift everything up)
            self.grid.remove_top_rows(rows_to_move);
        }

        self.grid.resize(new_dims);

        if new_rows > old_rows {
            // Growing - try to pull rows back from history
            let rows_to_pull = (new_rows - old_rows).min(self.history.len());
            if rows_to_pull > 0 {
                // Pull from end of history (most recent)
                let start = self.history.len() - rows_to_pull;
                let pulled: Vec<Row> = self.history.drain(start..).collect();
                // Insert at beginning of grid
                self.grid.prepend_rows(pulled);
            }
        }
    }

    /// Reflow all content (history + visible grid) to new dimensions.
    ///
    /// Tracks cursor position through the reflow so it stays on the same
    /// logical content after re-wrapping.
    fn reflow_all(&mut self, new_dims: Dimensions) {
        use crate::grid::cell::Cell;

        let new_cols = new_dims.cols;
        let new_rows = new_dims.rows as usize;
        let old_cursor_row = self.grid.cursor().position.row as usize;
        let old_cursor_col = self.grid.cursor().position.col as usize;

        // Track cursor as (logical_line_index, offset_within_logical_line)
        let mut cursor_logical_line: usize = 0;
        let mut cursor_logical_col: usize = 0;
        let mut found_cursor = false;

        // Collect all logical lines from history and grid
        let mut logical_lines: Vec<Vec<Cell>> = Vec::new();
        let mut current_line: Vec<Cell> = Vec::new();
        let mut current_logical_idx: usize = 0;

        // Process history rows
        for row in self.history.drain(..) {
            for cell in row.cells() {
                current_line.push(*cell);
            }
            if !row.is_wrapped() {
                while current_line.last().is_some_and(|c| c.is_blank()) {
                    current_line.pop();
                }
                logical_lines.push(std::mem::take(&mut current_line));
                current_logical_idx += 1;
            }
        }

        // Process grid rows, but skip trailing blank rows
        let grid_rows: Vec<_> = self.grid.rows().cloned().collect();

        let last_content_row = grid_rows
            .iter()
            .rposition(|row| !row.is_blank())
            .unwrap_or(0);

        for (idx, row) in grid_rows.iter().enumerate() {
            if idx > last_content_row && row.is_blank() {
                // Track cursor on blank trailing rows before skipping
                if idx == old_cursor_row && !found_cursor {
                    cursor_logical_line = current_logical_idx;
                    cursor_logical_col = 0;
                    found_cursor = true;
                }
                continue;
            }

            let cells_before = current_line.len();
            for cell in row.cells() {
                current_line.push(*cell);
            }

            // Track cursor: it's on this grid row
            if idx == old_cursor_row && !found_cursor {
                cursor_logical_line = current_logical_idx;
                cursor_logical_col = cells_before + old_cursor_col;
                found_cursor = true;
            }

            if !row.is_wrapped() {
                while current_line.last().is_some_and(|c| c.is_blank()) {
                    current_line.pop();
                }
                // Clamp cursor col if it fell in trimmed blanks
                // (allow cursor at len() — one past last char is valid)
                if found_cursor && cursor_logical_line == current_logical_idx {
                    cursor_logical_col = cursor_logical_col.min(current_line.len());
                }
                logical_lines.push(std::mem::take(&mut current_line));
                current_logical_idx += 1;
            }
        }

        if !current_line.is_empty() {
            while current_line.last().is_some_and(|c| c.is_blank()) {
                current_line.pop();
            }
            if found_cursor && cursor_logical_line == current_logical_idx {
                cursor_logical_col = cursor_logical_col.min(current_line.len());
            }
            logical_lines.push(current_line);
        }

        while logical_lines.last().is_some_and(|line| line.is_empty()) {
            logical_lines.pop();
        }

        // Re-wrap all logical lines to new width, tracking cursor
        let mut all_rows: Vec<Row> = Vec::new();
        let mut new_cursor_abs_row: Option<usize> = None;
        let mut new_cursor_col: usize = 0;

        for (line_idx, line) in logical_lines.iter().enumerate() {
            if line.is_empty() {
                if found_cursor && line_idx == cursor_logical_line {
                    new_cursor_abs_row = Some(all_rows.len());
                    new_cursor_col = 0;
                }
                all_rows.push(Row::new(new_cols));
                continue;
            }

            let mut pos = 0;
            while pos < line.len() {
                let row_start = pos;
                let mut new_row = Row::new(new_cols);
                let mut col = 0u16;

                while col < new_cols && pos < line.len() {
                    let cell = &line[pos];

                    // Skip spacer cells from old layout — regenerated when placing wide chars
                    if cell.is_wide_spacer() {
                        pos += 1;
                        continue;
                    }

                    // Wide char needs 2 columns; if only 1 remains, wrap to next row
                    if cell.is_wide() && col + 1 >= new_cols {
                        break;
                    }

                    if let Some(dest) = new_row.get_mut(col) {
                        *dest = *cell;
                    }
                    col += 1;
                    pos += 1;

                    // After placing a wide char, insert its spacer cell
                    if cell.is_wide() {
                        if let Some(spacer) = new_row.get_mut(col) {
                            spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
                        }
                        col += 1;
                    }
                }

                // Check if cursor falls in this physical row
                if found_cursor
                    && line_idx == cursor_logical_line
                    && new_cursor_abs_row.is_none()
                    && cursor_logical_col >= row_start
                    && cursor_logical_col < pos
                {
                    new_cursor_abs_row = Some(all_rows.len());
                    new_cursor_col = cursor_logical_col - row_start;
                }

                if pos < line.len() {
                    new_row.set_wrapped(true);
                }

                all_rows.push(new_row);
            }

            // Cursor at exact end of logical line
            if found_cursor && line_idx == cursor_logical_line && new_cursor_abs_row.is_none() {
                new_cursor_abs_row = Some(all_rows.len().saturating_sub(1));
                new_cursor_col = cursor_logical_col
                    .saturating_sub(line.len().saturating_sub(line.len() % new_cols as usize));
            }
        }

        while all_rows.len() < new_rows {
            all_rows.push(Row::new(new_cols));
        }

        // Split into history and visible grid
        let split_point = if all_rows.len() > new_rows {
            let sp = all_rows.len() - new_rows;
            if self.max_history > 0 {
                self.history = all_rows.drain(..sp).collect();
                while self.history.len() > self.max_history {
                    self.history.pop_front();
                }
            } else {
                all_rows.drain(..sp);
                self.history.clear();
            }
            sp
        } else {
            self.history.clear();
            0
        };

        self.grid.set_rows(all_rows, new_dims);

        // Restore tracked cursor position
        if let Some(abs_row) = new_cursor_abs_row {
            let grid_row = abs_row.saturating_sub(split_point);
            self.grid.cursor_mut().position.row =
                (grid_row as u16).min(new_dims.rows.saturating_sub(1));
            self.grid.cursor_mut().position.col =
                (new_cursor_col as u16).min(new_dims.cols.saturating_sub(1));
        }
    }

    /// Write a character, saving to history if scrolling occurs.
    pub fn write_char(&mut self, c: char) {
        use unicode_width::UnicodeWidthChar;

        self.scroll_offset = 0;

        let cursor_col = self.grid.cursor().position.col;
        let cursor_row = self.grid.cursor().position.row;
        let wrap_pending = self.grid.cursor().wrap_pending;
        let auto_wrap = self.grid.auto_wrap();
        let scroll_bottom = self
            .grid
            .scroll_region()
            .map(|r| r.bottom)
            .unwrap_or(self.grid.dimensions().rows.saturating_sub(1));
        let max_col = self.grid.dimensions().cols;
        let char_width = c.width().unwrap_or(0) as u16;

        // Determine if this write will cause scrolling.
        // With pending wrap behavior:
        // - If wrap_pending is true, the next non-zero-width char triggers deferred wrap
        // - If wrap_pending is false and we're writing to the last column, that only sets
        //   wrap_pending but doesn't actually scroll (scroll happens on NEXT char)
        // - Wide chars that don't fit (width > 1 and col + width > cols) wrap before writing
        // - If auto_wrap is off, no wrapping ever happens
        let will_scroll = if char_width == 0 || !auto_wrap {
            false
        } else if wrap_pending || (char_width > 1 && cursor_col + char_width > max_col) {
            cursor_row >= scroll_bottom
        } else {
            false
        };

        if will_scroll {
            // Save the row that will be displaced by the scroll.
            // Only save to history when scrolling the full screen (no scroll region),
            // since scroll regions are used by apps on the alternate screen which
            // has no meaningful scrollback.
            if self.grid.scroll_region().is_none() {
                let top_row = self.grid.row(0).cloned();
                if let Some(row) = top_row {
                    self.push_to_history(vec![row]);
                }
            }
        }

        self.grid.write_char(c);
    }

    /// Write a contiguous printable ASCII span, saving scrolled rows to history.
    pub fn write_ascii_span(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        if self.grid.insert_mode() || !self.grid.auto_wrap() {
            for &byte in bytes {
                self.write_char(byte as char);
            }
            return;
        }

        self.scroll_offset = 0;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            if self.grid.cursor().wrap_pending && self.grid.scroll_region().is_none() {
                let scroll_bottom = self.grid.dimensions().rows.saturating_sub(1);
                if self.grid.cursor().position.row >= scroll_bottom {
                    if let Some(row) = self.grid.row(0).cloned() {
                        self.push_to_history(vec![row]);
                    }
                }
            }

            let cols = self.grid.dimensions().cols;
            if cols == 0 {
                return;
            }

            let col = self.grid.cursor().position.col;
            let chunk_len = if self.grid.cursor().wrap_pending {
                1
            } else {
                (cols.saturating_sub(col).max(1) as usize).min(remaining.len())
            };
            self.grid.write_ascii_span(&remaining[..chunk_len]);
            remaining = &remaining[chunk_len..];
        }
    }

    /// Scroll grid up, moving top line to history.
    pub fn scroll_grid_up(&mut self, n: u16) {
        let n_usize = n as usize;

        if self.grid.scroll_region().is_none() {
            // Full-screen scroll — save top rows to history
            let rows_to_save: Vec<Row> = self.grid.rows().take(n_usize).cloned().collect();
            self.push_to_history(rows_to_save);
        }
        // With a scroll region, rows displaced from region.top are discarded
        // (scroll regions are used by apps on the alternate screen which has
        // no meaningful scrollback).

        self.grid.scroll_up(n);
    }

    /// Newline - scroll to history if at bottom.
    pub fn newline(&mut self) {
        self.grid.cursor_mut().wrap_pending = false;
        let cursor_row = self.grid.cursor().position.row;
        let scroll_bottom = self
            .grid
            .scroll_region()
            .map(|r| r.bottom)
            .unwrap_or(self.grid.dimensions().rows.saturating_sub(1));

        if cursor_row >= scroll_bottom {
            if self.grid.scroll_region().is_none() {
                // Full-screen scroll — save top row to history
                self.scroll_grid_up(1);
            } else {
                // Scroll within region — no history capture
                self.grid.newline();
            }
        } else {
            self.grid.newline();
        }
    }

    // === Damage tracking ===

    /// Check if any visible content is dirty.
    pub fn has_dirty(&self) -> bool {
        self.grid.has_dirty()
    }

    /// Clear dirty flags after rendering.
    pub fn clear_dirty(&mut self) {
        self.grid.clear_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_empty_history() {
        let buf = Buffer::new(Dimensions::new(80, 24));
        assert_eq!(buf.history_len(), 0);
        assert_eq!(buf.scroll_offset(), 0);
        assert!(!buf.is_scrolled());
    }

    #[test]
    fn scroll_up_down() {
        let mut buf = Buffer::new(Dimensions::new(80, 24));

        // Add some history
        for i in 0..50 {
            let mut row = Row::new(80);
            row[0].set_char((b'A' + (i % 26)) as char);
            buf.push_to_history(vec![row]);
        }

        assert_eq!(buf.history_len(), 50);

        buf.scroll_up(10);
        assert_eq!(buf.scroll_offset(), 10);
        assert!(buf.is_scrolled());

        buf.scroll_down(5);
        assert_eq!(buf.scroll_offset(), 5);

        buf.scroll_to_bottom();
        assert_eq!(buf.scroll_offset(), 0);

        buf.scroll_to_top();
        assert_eq!(buf.scroll_offset(), 50);
    }

    #[test]
    fn history_limit() {
        let mut buf = Buffer::with_scrollback(Dimensions::new(10, 5), 100);

        // Add more than max history
        for _ in 0..150 {
            buf.push_to_history(vec![Row::new(10)]);
        }

        assert_eq!(buf.history_len(), 100);
    }

    #[test]
    fn write_ascii_span_saves_to_history_on_scroll() {
        let mut buf = Buffer::new(Dimensions::new(5, 2));
        buf.grid_mut().row_mut(0).unwrap()[0].set_char('A');
        buf.grid_mut().row_mut(1).unwrap()[0].set_char('B');
        buf.grid_mut().set_cursor_position(2, 1);

        buf.write_ascii_span(b"abcdef");

        assert_eq!(buf.history_len(), 1);
        assert_eq!(buf.history[0][0].char(), 'A');
        assert_eq!(buf.grid().row(0).unwrap().text(), "abcde");
        assert_eq!(buf.grid().row(1).unwrap().text_trimmed(), "f");
    }

    #[test]
    fn visible_row_with_scroll() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write to grid
        buf.grid_mut().write_char('X');

        // Add history
        for i in 0..10 {
            let mut row = Row::new(10);
            row[0].set_char((b'0' + i) as char);
            buf.push_to_history(vec![row]);
        }

        // Not scrolled - should see grid
        assert_eq!(buf.visible_row(0).unwrap()[0].char(), 'X');

        // Scroll up 5 lines
        buf.scroll_up(5);

        // Should now see history row at index (10-5) = 5
        assert_eq!(buf.visible_row(0).unwrap()[0].char(), '5');
    }

    #[test]
    fn scroll_grid_up_saves_history() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write on first row
        buf.grid_mut().write_char('A');

        // Scroll up
        buf.scroll_grid_up(1);

        // First row should be in history now
        assert_eq!(buf.history_len(), 1);
        assert_eq!(buf.history[0][0].char(), 'A');

        // Grid first row should be empty
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), ' ');
    }

    #[test]
    fn resize_reflows_content_narrower() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Write a long line that will wrap when narrowed
        let text = "Hello World!!!!!";
        for c in text.chars() {
            buf.grid_mut().write_char(c);
        }

        // Verify initial state - should fit on one row
        assert_eq!(buf.grid().row(0).unwrap().text_trimmed(), text);

        // Narrow to 10 columns
        buf.resize(Dimensions::new(10, 5));

        // Content should now span two rows
        assert_eq!(buf.grid().row(0).unwrap().text_trimmed(), "Hello Worl");
        assert_eq!(buf.grid().row(1).unwrap().text_trimmed(), "d!!!!!");
        assert!(buf.grid().row(0).unwrap().is_wrapped());
    }

    #[test]
    fn resize_reflows_content_wider() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write a line that wraps at 10 columns
        let text = "Hello World!!!!!";
        for c in text.chars() {
            buf.grid_mut().write_char(c);
        }

        // Should be wrapped across two rows
        assert!(buf.grid().row(0).unwrap().is_wrapped());

        // Widen to 20 columns
        buf.resize(Dimensions::new(20, 5));

        // Content should now fit on one row
        assert_eq!(buf.grid().row(0).unwrap().text_trimmed(), text);
        assert!(!buf.grid().row(0).unwrap().is_wrapped());
    }

    #[test]
    fn resize_height_shrink_moves_to_history() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Directly set cells in each row using a single borrow
        {
            let grid = buf.grid_mut();
            grid.row_mut(0).unwrap()[0].set_char('A');
            grid.row_mut(1).unwrap()[0].set_char('B');
            grid.row_mut(2).unwrap()[0].set_char('C');
            grid.row_mut(3).unwrap()[0].set_char('D');
            grid.row_mut(4).unwrap()[0].set_char('E');
        }

        // Verify initial state - cell() takes (col, row)
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'A');
        assert_eq!(buf.grid().cell(0, 4).unwrap().char(), 'E');

        // Shrink height from 5 to 3 (width stays same)
        buf.resize(Dimensions::new(20, 3));

        // Top 2 rows (A, B) should move to history
        assert_eq!(buf.history_len(), 2);
        assert_eq!(buf.history[0][0].char(), 'A');
        assert_eq!(buf.history[1][0].char(), 'B');

        // Grid should have C, D, E - cell() takes (col, row)
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'C');
        assert_eq!(buf.grid().cell(0, 1).unwrap().char(), 'D');
        assert_eq!(buf.grid().cell(0, 2).unwrap().char(), 'E');
    }

    #[test]
    fn resize_height_grow_pulls_from_history() {
        let mut buf = Buffer::new(Dimensions::new(20, 3));

        // Add history
        let mut row_a = Row::new(20);
        row_a[0].set_char('A');
        let mut row_b = Row::new(20);
        row_b[0].set_char('B');
        buf.push_to_history(vec![row_a, row_b]);

        // Directly set cells in grid
        {
            let grid = buf.grid_mut();
            grid.row_mut(0).unwrap()[0].set_char('C');
            grid.row_mut(1).unwrap()[0].set_char('D');
        }

        // Verify state: 2 in history (A, B), grid has C, D, empty - cell() takes (col, row)
        assert_eq!(buf.history_len(), 2);
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'C'); // col 0, row 0

        // Grow height from 3 to 5 (width stays same)
        buf.resize(Dimensions::new(20, 5));

        // History should be pulled into grid
        // History rows A, B should now be at top of grid - cell() takes (col, row)
        assert_eq!(buf.history_len(), 0);
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'A'); // col 0, row 0
        assert_eq!(buf.grid().cell(0, 1).unwrap().char(), 'B'); // col 0, row 1
        assert_eq!(buf.grid().cell(0, 2).unwrap().char(), 'C'); // col 0, row 2
        assert_eq!(buf.grid().cell(0, 3).unwrap().char(), 'D'); // col 0, row 3
    }

    #[test]
    fn write_char_saves_to_history_on_scroll() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        for i in 0..5u16 {
            buf.grid_mut().row_mut(i).unwrap()[0].set_char((b'0' + i as u8) as char);
        }

        assert_eq!(buf.grid().row(0).unwrap()[0].char(), '0');
        assert_eq!(buf.grid().row(4).unwrap()[0].char(), '4');
        assert_eq!(buf.history_len(), 0);

        // Fill last row completely, then write one more char to trigger scroll
        // set_cursor_position is 1-based, so row 5 = 0-based row 4
        buf.grid_mut().set_cursor_position(5, 1);
        for _ in 0..10 {
            buf.write_char('X');
        }
        // Now cursor is at row 4, col 10 (past end) - next char triggers scroll
        buf.write_char('Y');

        assert_eq!(buf.history_len(), 1);
        assert_eq!(buf.history[0][0].char(), '0');
        assert_eq!(buf.grid().row(0).unwrap()[0].char(), '1');
    }

    #[test]
    fn write_char_fills_screen_and_scrolls() {
        // Simulate filling a small screen and scrolling multiple times
        let mut buf = Buffer::new(Dimensions::new(5, 3)); // 5 cols, 3 rows

        // Write enough to fill screen and scroll several times
        // Each row holds 5 chars, we have 3 rows = 15 chars before scroll
        // Then each additional 5 chars scrolls once
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 26 chars

        for c in text.chars() {
            buf.write_char(c);
        }

        // 26 chars / 5 cols = 5.2 rows needed
        // 3 visible rows means 2-3 rows should be in history
        // Let's verify history has content
        assert!(
            buf.history_len() >= 2,
            "Expected at least 2 history rows, got {}",
            buf.history_len()
        );

        // First history row should start with 'A'
        assert_eq!(buf.history[0][0].char(), 'A');
    }

    #[test]
    fn reflow_preserves_cursor_on_narrow() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Write "Hello World" on row 0
        for c in "Hello World".chars() {
            buf.grid_mut().write_char(c);
        }
        // Cursor should be at row 0, col 11 (after 'd')
        assert_eq!(buf.grid().cursor().position.row, 0);
        assert_eq!(buf.grid().cursor().position.col, 11);

        // Narrow to 6 columns — "Hello " wraps, cursor should track to "World" part
        buf.resize(Dimensions::new(6, 5));

        // "Hello " on row 0 (wrapped), "World" on row 1
        // Cursor was at col 11 in the logical line, which maps to row 1 col 5
        assert_eq!(buf.grid().cursor().position.row, 1);
        assert_eq!(buf.grid().cursor().position.col, 5);
    }

    #[test]
    fn reflow_preserves_cursor_on_widen() {
        let mut buf = Buffer::new(Dimensions::new(6, 5));

        // Write text that wraps at 6 columns
        for c in "Hello World".chars() {
            buf.grid_mut().write_char(c);
        }
        // Cursor on row 1 (wrapped), col 5
        assert_eq!(buf.grid().cursor().position.row, 1);
        assert_eq!(buf.grid().cursor().position.col, 5);

        // Widen to 20 — everything fits on one row
        buf.resize(Dimensions::new(20, 5));

        // Cursor should be back on row 0, col 11
        assert_eq!(buf.grid().cursor().position.row, 0);
        assert_eq!(buf.grid().cursor().position.col, 11);
    }

    #[test]
    fn reflow_cursor_on_blank_trailing_row() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Write text on row 0 only, leave cursor at bottom (row 4)
        for c in "Hello".chars() {
            buf.grid_mut().write_char(c);
        }
        // Move cursor to row 4 (blank trailing row)
        buf.grid_mut().set_cursor_position(5, 1); // 1-based: row 5, col 1

        assert_eq!(buf.grid().cursor().position.row, 4);

        // Narrow — reflow should not lose cursor
        buf.resize(Dimensions::new(10, 5));

        // Cursor should end up at a valid position (not row 0 col 0 due to lost tracking)
        let cursor = buf.grid().cursor().position;
        assert!(
            cursor.row < 5,
            "cursor row {} should be in bounds",
            cursor.row
        );
    }
}
