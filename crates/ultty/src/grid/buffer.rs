//! Terminal buffer with scrollback history.

use super::{Dimensions, Grid, Row};

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
    history: Vec<Row>,
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
            history: Vec::with_capacity(max_history.min(1000)), // Pre-allocate reasonably
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
        // Reset scroll when modifying grid
        self.scroll_offset = 0;
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
        for row in rows {
            if self.history.len() >= self.max_history {
                self.history.remove(0);
            }
            self.history.push(row);
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

    /// Clear scrollback history.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.scroll_offset = 0;
    }

    /// Get history length.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Resize the buffer.
    pub fn resize(&mut self, new_dims: Dimensions) {
        self.grid.resize(new_dims);
        self.scroll_offset = self.scroll_offset.min(self.history.len());
    }

    /// Write a character (delegates to grid, handles history on scroll).
    pub fn write_char(&mut self, c: char) {
        self.scroll_offset = 0;
        self.grid.write_char(c);
    }

    /// Scroll grid up, moving top line to history.
    pub fn scroll_grid_up(&mut self, n: u16) {
        // Capture rows being scrolled off
        let n = n as usize;
        let rows_to_save: Vec<Row> = self.grid.rows().take(n).cloned().collect();
        self.push_to_history(rows_to_save);
        self.grid.scroll_up(n as u16);
    }

    /// Newline - scroll to history if at bottom.
    pub fn newline(&mut self) {
        let cursor_row = self.grid.cursor().position.row;
        let max_row = self.grid.dimensions().rows - 1;

        if cursor_row >= max_row {
            self.scroll_grid_up(1);
        } else {
            self.grid.newline();
        }
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
            row[0].set_char(('A' as u8 + (i % 26)) as char);
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
    fn visible_row_with_scroll() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write to grid
        buf.grid_mut().write_char('X');

        // Add history
        for i in 0..10 {
            let mut row = Row::new(10);
            row[0].set_char(('0' as u8 + i) as char);
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
}
