//! Terminal grid and buffer management.
//!
//! This module provides the core data structures for terminal emulation:
//! - [`Cell`] - A single character cell with styling
//! - [`Row`] - A row of cells
//! - [`Grid`] - The visible terminal grid
//! - [`Buffer`] - Grid with scrollback history
//! - [`Terminal`] - Full terminal state with alternate screen and selection
//! - [`Handler`] - Applies ANSI parser actions to the terminal

mod buffer;
mod cell;
mod handler;
mod row;
mod term_handler;
mod terminal;

pub use buffer::Buffer;
pub use cell::{Cell, CellFlags, Color};
pub use handler::Handler;
pub use row::Row;
pub use term_handler::TerminalHandler;
pub use terminal::{Selection, SelectionMode, SelectionPoint, Terminal, TerminalModes};

use std::ops::{Index, IndexMut};

/// Terminal grid dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub cols: u16,
    pub rows: u16,
}

impl Dimensions {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }

    pub fn area(&self) -> usize {
        self.cols as usize * self.rows as usize
    }
}

/// Cursor position in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorPosition {
    pub col: u16,
    pub row: u16,
}

impl CursorPosition {
    pub fn new(col: u16, row: u16) -> Self {
        Self { col, row }
    }

    pub fn origin() -> Self {
        Self { col: 0, row: 0 }
    }
}

/// Terminal cursor state.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub position: CursorPosition,
    pub visible: bool,
    pub shape: CursorShape,
    /// Saved cursor for ESC 7 / ESC 8
    saved: Option<CursorPosition>,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            position: CursorPosition::origin(),
            visible: true,
            shape: CursorShape::Block,
            saved: None,
        }
    }
}

impl Cursor {
    pub fn save(&mut self) {
        self.saved = Some(self.position);
    }

    pub fn restore(&mut self) {
        if let Some(pos) = self.saved {
            self.position = pos;
        }
    }
}

/// Cursor shape for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// The visible terminal grid.
///
/// This stores the current screen content as a 2D array of cells.
/// Use [`Buffer`] for a grid with scrollback history.
#[derive(Debug, Clone)]
pub struct Grid {
    rows: Vec<Row>,
    dims: Dimensions,
    cursor: Cursor,
    /// Current text attributes for new characters
    attrs: Cell,
    /// Dirty rows that need redrawing (bitset would be more efficient)
    dirty: Vec<bool>,
}

impl Grid {
    /// Create a new grid with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        let rows = (0..dims.rows).map(|_| Row::new(dims.cols)).collect();
        let dirty = vec![true; dims.rows as usize];

        Self {
            rows,
            dims,
            cursor: Cursor::default(),
            attrs: Cell::default(),
            dirty,
        }
    }

    /// Get grid dimensions.
    pub fn dimensions(&self) -> Dimensions {
        self.dims
    }

    /// Get cursor state.
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    /// Get mutable cursor state.
    pub fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// Get current text attributes.
    pub fn attrs(&self) -> &Cell {
        &self.attrs
    }

    /// Set current text attributes.
    pub fn set_attrs(&mut self, attrs: Cell) {
        self.attrs = attrs;
    }

    /// Reset attributes to default.
    pub fn reset_attrs(&mut self) {
        self.attrs = Cell::default();
    }

    /// Get a row by index.
    pub fn row(&self, index: u16) -> Option<&Row> {
        self.rows.get(index as usize)
    }

    /// Get a mutable row by index.
    pub fn row_mut(&mut self, index: u16) -> Option<&mut Row> {
        self.mark_dirty(index);
        self.rows.get_mut(index as usize)
    }

    /// Get a cell at the given position.
    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        self.rows.get(row as usize)?.get(col)
    }

    /// Get a mutable cell at the given position.
    pub fn cell_mut(&mut self, col: u16, row: u16) -> Option<&mut Cell> {
        self.mark_dirty(row);
        self.rows.get_mut(row as usize)?.get_mut(col)
    }

    /// Write a character at the cursor position and advance.
    pub fn write_char(&mut self, c: char) {
        let col = self.cursor.position.col;
        let row = self.cursor.position.row;

        if col < self.dims.cols && row < self.dims.rows {
            // Copy attrs first to avoid borrow conflict
            let attrs = self.attrs;
            if let Some(cell) = self.cell_mut(col, row) {
                cell.set_char(c);
                cell.copy_attrs_from(&attrs);
            }
            self.cursor.position.col += 1;

            // Handle wrap
            if self.cursor.position.col >= self.dims.cols {
                self.cursor.position.col = 0;
                self.cursor.position.row += 1;
                if self.cursor.position.row >= self.dims.rows {
                    self.scroll_up(1);
                    self.cursor.position.row = self.dims.rows - 1;
                }
            }
        }
    }

    /// Move cursor up, stopping at top edge.
    pub fn cursor_up(&mut self, n: u16) {
        self.cursor.position.row = self.cursor.position.row.saturating_sub(n);
    }

    /// Move cursor down, stopping at bottom edge.
    pub fn cursor_down(&mut self, n: u16) {
        self.cursor.position.row = (self.cursor.position.row + n).min(self.dims.rows - 1);
    }

    /// Move cursor left, stopping at left edge.
    pub fn cursor_left(&mut self, n: u16) {
        self.cursor.position.col = self.cursor.position.col.saturating_sub(n);
    }

    /// Move cursor right, stopping at right edge.
    pub fn cursor_right(&mut self, n: u16) {
        self.cursor.position.col = (self.cursor.position.col + n).min(self.dims.cols - 1);
    }

    /// Set absolute cursor position (1-based like ANSI).
    pub fn set_cursor_position(&mut self, row: u16, col: u16) {
        // ANSI positions are 1-based, convert to 0-based
        self.cursor.position.row = row.saturating_sub(1).min(self.dims.rows - 1);
        self.cursor.position.col = col.saturating_sub(1).min(self.dims.cols - 1);
    }

    /// Carriage return - move cursor to column 0.
    pub fn carriage_return(&mut self) {
        self.cursor.position.col = 0;
    }

    /// Newline - move cursor down, scrolling if needed.
    pub fn newline(&mut self) {
        if self.cursor.position.row + 1 >= self.dims.rows {
            self.scroll_up(1);
        } else {
            self.cursor.position.row += 1;
        }
    }

    /// Backspace - move cursor left by one.
    pub fn backspace(&mut self) {
        if self.cursor.position.col > 0 {
            self.cursor.position.col -= 1;
        }
    }

    /// Tab - advance to next tab stop (every 8 columns).
    pub fn tab(&mut self) {
        let next_tab = ((self.cursor.position.col / 8) + 1) * 8;
        self.cursor.position.col = next_tab.min(self.dims.cols - 1);
    }

    /// Scroll the grid up by n lines.
    pub fn scroll_up(&mut self, n: u16) {
        let n = n as usize;
        if n >= self.rows.len() {
            self.clear();
            return;
        }

        // Remove top rows and add new blank rows at bottom
        self.rows.drain(0..n);
        for _ in 0..n {
            self.rows.push(Row::new(self.dims.cols));
        }
        self.mark_all_dirty();
    }

    /// Scroll the grid down by n lines.
    pub fn scroll_down(&mut self, n: u16) {
        let n = n as usize;
        if n >= self.rows.len() {
            self.clear();
            return;
        }

        // Remove bottom rows and add new blank rows at top
        self.rows.truncate(self.rows.len() - n);
        for _ in 0..n {
            self.rows.insert(0, Row::new(self.dims.cols));
        }
        self.mark_all_dirty();
    }

    /// Erase from cursor to end of display.
    pub fn erase_below(&mut self) {
        // Erase from cursor to end of current line
        if let Some(row) = self.rows.get_mut(self.cursor.position.row as usize) {
            for i in self.cursor.position.col..self.dims.cols {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(self.cursor.position.row);

        // Erase all lines below cursor
        for row_idx in (self.cursor.position.row + 1)..self.dims.rows {
            if let Some(row) = self.rows.get_mut(row_idx as usize) {
                row.clear();
            }
            self.mark_dirty(row_idx);
        }
    }

    /// Erase from start of display to cursor.
    pub fn erase_above(&mut self) {
        // Erase all lines above cursor
        for row_idx in 0..self.cursor.position.row {
            if let Some(row) = self.rows.get_mut(row_idx as usize) {
                row.clear();
            }
            self.mark_dirty(row_idx);
        }

        // Erase from start of current line to cursor
        if let Some(row) = self.rows.get_mut(self.cursor.position.row as usize) {
            for i in 0..=self.cursor.position.col {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(self.cursor.position.row);
    }

    /// Erase entire display.
    pub fn clear(&mut self) {
        for row in &mut self.rows {
            row.clear();
        }
        self.mark_all_dirty();
    }

    /// Erase from cursor to end of line.
    pub fn erase_line_right(&mut self) {
        if let Some(row) = self.rows.get_mut(self.cursor.position.row as usize) {
            for i in self.cursor.position.col..self.dims.cols {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(self.cursor.position.row);
    }

    /// Erase from start of line to cursor.
    pub fn erase_line_left(&mut self) {
        if let Some(row) = self.rows.get_mut(self.cursor.position.row as usize) {
            for i in 0..=self.cursor.position.col {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(self.cursor.position.row);
    }

    /// Erase entire current line.
    pub fn erase_line(&mut self) {
        if let Some(row) = self.rows.get_mut(self.cursor.position.row as usize) {
            row.clear();
        }
        self.mark_dirty(self.cursor.position.row);
    }

    /// Resize the grid.
    pub fn resize(&mut self, new_dims: Dimensions) {
        if new_dims == self.dims {
            return;
        }

        // Resize existing rows
        for row in &mut self.rows {
            row.resize(new_dims.cols);
        }

        // Add or remove rows
        while self.rows.len() < new_dims.rows as usize {
            self.rows.push(Row::new(new_dims.cols));
        }
        self.rows.truncate(new_dims.rows as usize);

        // Clamp cursor
        self.cursor.position.col = self.cursor.position.col.min(new_dims.cols.saturating_sub(1));
        self.cursor.position.row = self.cursor.position.row.min(new_dims.rows.saturating_sub(1));

        self.dims = new_dims;
        self.dirty = vec![true; new_dims.rows as usize];
    }

    /// Mark a row as dirty (needs redrawing).
    fn mark_dirty(&mut self, row: u16) {
        if let Some(d) = self.dirty.get_mut(row as usize) {
            *d = true;
        }
    }

    /// Mark all rows as dirty.
    fn mark_all_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = true;
        }
    }

    /// Check if a row is dirty.
    pub fn is_dirty(&self, row: u16) -> bool {
        self.dirty.get(row as usize).copied().unwrap_or(false)
    }

    /// Clear dirty flags after rendering.
    pub fn clear_dirty(&mut self) {
        for d in &mut self.dirty {
            *d = false;
        }
    }

    /// Iterate over all rows.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }
}

impl Index<u16> for Grid {
    type Output = Row;

    fn index(&self, index: u16) -> &Self::Output {
        &self.rows[index as usize]
    }
}

impl IndexMut<u16> for Grid {
    fn index_mut(&mut self, index: u16) -> &mut Self::Output {
        self.mark_dirty(index);
        &mut self.rows[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_grid_has_correct_dimensions() {
        let grid = Grid::new(Dimensions::new(80, 24));
        assert_eq!(grid.dimensions().cols, 80);
        assert_eq!(grid.dimensions().rows, 24);
    }

    #[test]
    fn write_char_advances_cursor() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.write_char('H');
        grid.write_char('i');
        assert_eq!(grid.cursor().position.col, 2);
    }

    #[test]
    fn cursor_movement() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.set_cursor_position(10, 20);
        assert_eq!(grid.cursor().position.row, 9); // 0-indexed
        assert_eq!(grid.cursor().position.col, 19);

        grid.cursor_up(5);
        assert_eq!(grid.cursor().position.row, 4);

        grid.cursor_down(10);
        assert_eq!(grid.cursor().position.row, 14);
    }

    #[test]
    fn scroll_up() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        grid.set_cursor_position(1, 1);
        grid.write_char('A');
        grid.scroll_up(1);
        // First row should now be empty
        assert_eq!(grid.cell(0, 0).unwrap().char(), ' ');
    }

    #[test]
    fn erase_display() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        // Fill grid
        for row in 0..5 {
            for col in 0..10 {
                grid.set_cursor_position(row + 1, col + 1);
                grid.write_char('X');
            }
        }

        // Erase below from middle
        grid.set_cursor_position(3, 5);
        grid.erase_below();

        // Check cells before cursor are unchanged
        assert_eq!(grid.cell(0, 2).unwrap().char(), 'X');
        // Check cells after cursor are cleared
        assert_eq!(grid.cell(8, 2).unwrap().char(), ' ');
        assert_eq!(grid.cell(0, 3).unwrap().char(), ' ');
    }

    #[test]
    fn resize_preserves_content() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        grid.write_char('A');
        grid.resize(Dimensions::new(20, 10));
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert_eq!(grid.dimensions().cols, 20);
        assert_eq!(grid.dimensions().rows, 10);
    }

    #[test]
    fn tab_advances_to_next_stop() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.cursor.position.col = 3;
        grid.tab();
        assert_eq!(grid.cursor.position.col, 8);

        grid.tab();
        assert_eq!(grid.cursor.position.col, 16);
    }

    #[test]
    fn save_restore_cursor() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.set_cursor_position(10, 20);
        grid.cursor_mut().save();

        grid.set_cursor_position(1, 1);
        grid.cursor_mut().restore();

        assert_eq!(grid.cursor().position.row, 9);
        assert_eq!(grid.cursor().position.col, 19);
    }
}
