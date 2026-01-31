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
}

impl Row {
    /// Create a new row with the given width.
    pub fn new(width: u16) -> Self {
        Self {
            cells: vec![Cell::default(); width as usize],
            dirty: true,
        }
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
    pub fn insert_cells(&mut self, at: u16, count: u16) {
        let at = at as usize;
        let count = count as usize;

        // Shift cells to the right
        for i in (at + count..self.cells.len()).rev() {
            self.cells[i] = self.cells[i - count].clone();
        }

        // Clear the inserted cells
        for i in at..at.min(self.cells.len()).saturating_add(count).min(self.cells.len()) {
            self.cells[i].clear();
        }

        self.dirty = true;
    }

    /// Delete cells at position, shifting remaining cells left.
    pub fn delete_cells(&mut self, at: u16, count: u16) {
        let at = at as usize;
        let count = count as usize;

        // Shift cells to the left
        for i in at..self.cells.len().saturating_sub(count) {
            self.cells[i] = self.cells[i + count].clone();
        }

        // Clear the cells at the end
        for i in self.cells.len().saturating_sub(count)..self.cells.len() {
            self.cells[i].clear();
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

        row.insert_cells(1, 2);

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

        row.delete_cells(1, 2);

        assert_eq!(row[0].char(), 'A');
        assert_eq!(row[1].char(), 'D');
        assert_eq!(row[2].char(), ' ');
    }

    #[test]
    fn erase_operations() {
        let mut row = Row::new(10);
        for i in 0..10 {
            row[i].set_char(('A' as u8 + i as u8) as char);
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
