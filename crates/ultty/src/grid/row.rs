//! Terminal row representation.

use super::cell::Cell;
use std::ops::{Index, IndexMut};

/// A row of terminal cells.
///
/// Provides efficient storage and manipulation of a single line
/// in the terminal grid.
#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    /// Track if this row has been modified
    dirty: bool,
    /// Whether this row is wrapped (continues on next line due to soft wrap)
    wrapped: bool,
}

impl Row {
    /// Create a new row with the given width.
    pub fn new(width: u16) -> Self {
        Self {
            cells: vec![Cell::default(); width as usize],
            dirty: true,
            wrapped: false,
        }
    }

    /// Check if this row is wrapped (soft-wrapped to next line).
    #[inline]
    pub fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    /// Set the wrapped flag.
    #[inline]
    pub fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped = wrapped;
        self.dirty = true;
    }

    /// Get the width of the row.
    #[inline]
    pub fn width(&self) -> u16 {
        self.cells.len() as u16
    }

    /// Get a cell by index.
    #[inline]
    pub fn get(&self, index: u16) -> Option<&Cell> {
        self.cells.get(index as usize)
    }

    /// Get a mutable cell by index.
    #[inline]
    pub fn get_mut(&mut self, index: u16) -> Option<&mut Cell> {
        self.dirty = true;
        self.cells.get_mut(index as usize)
    }

    /// Clear all cells in the row.
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            cell.clear();
        }
        self.wrapped = false;
        self.dirty = true;
    }

    /// Erase all cells per ECMA-48: reset to space, preserve given background.
    pub fn erase(&mut self, bg: super::cell::Color) {
        for cell in &mut self.cells {
            cell.erase(bg);
        }
        self.wrapped = false;
        self.dirty = true;
    }

    /// Resize the row.
    pub fn resize(&mut self, new_width: u16) {
        self.cells.resize(new_width as usize, Cell::default());
        self.dirty = true;
    }

    /// Check if the row is dirty.
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag.
    #[inline]
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Mark the row as dirty.
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Check if the row is blank (all cells are default).
    pub fn is_blank(&self) -> bool {
        self.cells.iter().all(|cell| cell.is_blank())
    }

    /// Iterate over cells.
    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter()
    }

    /// Iterate over cells mutably.
    pub fn cells_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        self.dirty = true;
        self.cells.iter_mut()
    }

    /// Get the text content of the row as a string.
    pub fn text(&self) -> String {
        self.cells.iter().map(|c| c.char()).collect()
    }

    /// Get the trimmed text content (trailing spaces removed).
    pub fn text_trimmed(&self) -> String {
        self.text().trim_end().to_string()
    }

    /// Insert blank cells at position, shifting existing cells right.
    /// Blank cells are filled with the given background color per ECMA-48.
    pub fn insert_cells(&mut self, at: u16, count: u16, bg: super::cell::Color) {
        let at = at as usize;
        let count = count as usize;
        let len = self.cells.len();

        if at >= len || count == 0 {
            return;
        }

        // If inserting at a wide char spacer, clear the orphaned primary cell
        if at > 0 && self.cells[at].is_wide_spacer() {
            self.cells[at - 1].clear();
        }

        // If the cell that will be pushed off-screen is a wide char,
        // clear its spacer to avoid orphaning it
        let pushed = len.saturating_sub(count);
        if pushed < len && self.cells[pushed].is_wide() && pushed + 1 < len {
            self.cells[pushed + 1].clear();
        }

        // Shift cells right using bulk copy
        let src_end = len.saturating_sub(count);
        if at < src_end {
            self.cells.copy_within(at..src_end, at + count);
        }

        // Fill inserted cells with background color
        for i in at..(at + count).min(len) {
            self.cells[i].erase(bg);
        }

        self.dirty = true;
    }

    /// Delete cells at position, shifting remaining cells left.
    /// Vacated cells at the end are filled with the given background color per ECMA-48.
    pub fn delete_cells(&mut self, at: u16, count: u16, bg: super::cell::Color) {
        let at = at as usize;
        let count = count as usize;
        let len = self.cells.len();

        if at >= len || count == 0 {
            return;
        }

        // If deleting starts at a wide char spacer, clear the orphaned primary cell
        if at > 0 && self.cells[at].is_wide_spacer() {
            self.cells[at - 1].clear();
        }

        // If the deletion range ends in the middle of a wide char, clear the spacer
        let end = (at + count).min(len);
        if end < len && self.cells[end].is_wide_spacer() {
            self.cells[end].clear();
        }

        // Shift cells left using bulk copy
        let src_start = (at + count).min(len);
        if src_start < len {
            self.cells.copy_within(src_start..len, at);
        }

        // Fill vacated cells at end with background color
        for i in len.saturating_sub(count)..len {
            self.cells[i].erase(bg);
        }

        self.dirty = true;
    }

    /// Erase cells from position to end of row.
    pub fn erase_right(&mut self, from: u16) {
        for i in from as usize..self.cells.len() {
            self.cells[i].clear();
        }
        self.dirty = true;
    }

    /// Erase cells from start of row to position.
    pub fn erase_left(&mut self, to: u16) {
        for i in 0..=to as usize {
            if i < self.cells.len() {
                self.cells[i].clear();
            }
        }
        self.dirty = true;
    }
}

impl Index<u16> for Row {
    type Output = Cell;

    fn index(&self, index: u16) -> &Self::Output {
        &self.cells[index as usize]
    }
}

impl IndexMut<u16> for Row {
    fn index_mut(&mut self, index: u16) -> &mut Self::Output {
        self.dirty = true;
        &mut self.cells[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_row_is_blank() {
        let row = Row::new(80);
        assert_eq!(row.width(), 80);
        assert!(row.is_blank());
    }

    #[test]
    fn write_and_read_cell() {
        let mut row = Row::new(10);
        row[0].set_char('H');
        row[1].set_char('i');
        assert_eq!(row[0].char(), 'H');
        assert_eq!(row[1].char(), 'i');
    }

    #[test]
    fn text_extraction() {
        let mut row = Row::new(10);
        row[0].set_char('H');
        row[1].set_char('e');
        row[2].set_char('l');
        row[3].set_char('l');
        row[4].set_char('o');

        assert_eq!(row.text_trimmed(), "Hello");
    }

    #[test]
    fn clear_row() {
        let mut row = Row::new(10);
        row[0].set_char('X');
        row[5].set_char('Y');
        row.clear();
        assert!(row.is_blank());
    }

    #[test]
    fn resize_row() {
        let mut row = Row::new(10);
        row[5].set_char('A');

        row.resize(20);
        assert_eq!(row.width(), 20);
        assert_eq!(row[5].char(), 'A');

        row.resize(5);
        assert_eq!(row.width(), 5);
    }

    #[test]
    fn insert_cells() {
        let mut row = Row::new(10);
        row[0].set_char('A');
        row[1].set_char('B');
        row[2].set_char('C');

        row.insert_cells(1, 2, crate::grid::cell::Color::Default);

        assert_eq!(row[0].char(), 'A');
        assert_eq!(row[1].char(), ' ');
        assert_eq!(row[2].char(), ' ');
        assert_eq!(row[3].char(), 'B');
        assert_eq!(row[4].char(), 'C');
    }

    #[test]
    fn delete_cells() {
        let mut row = Row::new(10);
        row[0].set_char('A');
        row[1].set_char('B');
        row[2].set_char('C');
        row[3].set_char('D');

        row.delete_cells(1, 2, crate::grid::cell::Color::Default);

        assert_eq!(row[0].char(), 'A');
        assert_eq!(row[1].char(), 'D');
        assert_eq!(row[2].char(), ' ');
    }

    #[test]
    fn erase_operations() {
        let mut row = Row::new(10);
        for i in 0..10 {
            row[i].set_char((b'A' + i as u8) as char);
        }

        row.erase_right(5);
        assert_eq!(row.text_trimmed(), "ABCDE");

        row.erase_left(2);
        assert_eq!(row[0].char(), ' ');
        assert_eq!(row[1].char(), ' ');
        assert_eq!(row[2].char(), ' ');
        assert_eq!(row[3].char(), 'D');
    }
}
