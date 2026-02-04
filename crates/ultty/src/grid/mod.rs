//! Terminal grid and buffer management.
//! Terminal grid and cell management.
//!
//! This module provides the core data structures for terminal emulation:
//! - [`Cell`] - A single character cell with styling
//! - [`Row`] - A row of cells
//! - [`Grid`] - The visible terminal grid
//! - [`Buffer`] - Grid with scrollback history
//! - [`Terminal`] - Full terminal state with alternate screen and selection
//! - [`Handler`] - Applies ANSI parser actions to the terminal
//! - [`ImageStore`] - Storage for inline images (Sixel, etc.)
//!
//! # Example
//!
//! ```
//! use ultty::grid::{Grid, Dimensions, Terminal};
//!
//! // Create a terminal
//! let mut term = Terminal::new(Dimensions::new(80, 24));
//!
//! // Write some text
//! for c in "Hello, World!".chars() {
//!     term.grid_mut().write_char(c);
//! }
//!
//! // Check the content
//! assert_eq!(term.grid().row(0).unwrap().text_trimmed(), "Hello, World!");
//! ```

mod buffer;
mod cell;
mod handler;
mod image;
mod row;
mod term_handler;
mod terminal;

pub use buffer::Buffer;
pub use cell::{Cell, CellFlags, Color};
pub use handler::Handler;
pub use image::{decode_sixel, ImageId, ImageStore, TerminalImage};
pub use row::Row;
pub use term_handler::TerminalHandler;
pub use terminal::{
    MouseButton, MouseEncoding, MouseMode, MouseModifiers, Selection, SelectionMode,
    SelectionPoint, ShellIntegration, Terminal, TerminalModes,
};

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
    pub blinking: bool,
    /// Saved cursor for ESC 7 / ESC 8
    saved: Option<CursorPosition>,
    /// Pending wrap flag - cursor is at end of line, next char will wrap
    /// This implements standard terminal "autowrap pending" behavior where
    /// the cursor stays at the last column until the next character is typed.
    pub wrap_pending: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            position: CursorPosition::origin(),
            visible: true,
            shape: CursorShape::Block,
            blinking: true, // Default is blinking
            saved: None,
            wrap_pending: false,
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

/// Scroll region bounds (inclusive, 0-indexed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRegion {
    pub top: u16,
    pub bottom: u16,
}

impl ScrollRegion {
    pub fn new(top: u16, bottom: u16) -> Self {
        // Ensure top <= bottom to prevent underflow in region_height calculations
        let (top, bottom) = if top <= bottom {
            (top, bottom)
        } else {
            (bottom, top)
        };
        Self { top, bottom }
    }

    pub fn contains(&self, row: u16) -> bool {
        row >= self.top && row <= self.bottom
    }
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
    /// Scroll region (None means full screen)
    scroll_region: Option<ScrollRegion>,
    /// Combining characters storage: (row, col) -> additional chars
    /// Only cells with HAS_COMBINING flag have entries here
    combining: std::collections::HashMap<(u16, u16), Vec<char>>,
    /// Custom tab stops (column numbers where tabs stop)
    /// Initialized to default every-8-columns pattern
    tab_stops: std::collections::BTreeSet<u16>,
    /// Auto-wrap mode (DECAWM) — when false, characters at the right margin
    /// overwrite the last column instead of wrapping to the next line.
    auto_wrap: bool,
}

impl Grid {
    /// Create a new grid with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        let rows = (0..dims.rows).map(|_| Row::new(dims.cols)).collect();
        let dirty = vec![true; dims.rows as usize];

        let tab_stops: std::collections::BTreeSet<u16> = (0..dims.cols).step_by(8).collect();

        Self {
            rows,
            dims,
            cursor: Cursor::default(),
            attrs: Cell::default(),
            dirty,
            scroll_region: None,
            combining: std::collections::HashMap::new(),
            tab_stops,
            auto_wrap: true,
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

    /// Set auto-wrap mode (DECAWM).
    pub fn set_auto_wrap(&mut self, enabled: bool) {
        self.auto_wrap = enabled;
    }

    /// Get auto-wrap mode (DECAWM).
    pub fn auto_wrap(&self) -> bool {
        self.auto_wrap
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

    /// Get mutable access to current text attributes.
    pub fn attrs_mut(&mut self) -> &mut Cell {
        &mut self.attrs
    }

    /// Set the current hyperlink ID for new characters.
    pub fn set_hyperlink_id(&mut self, id: u32) {
        self.attrs.set_hyperlink_id(id);
    }

    // === Combining characters (grapheme clusters) ===

    /// Add a combining character to a cell.
    fn add_combining_char(&mut self, row: u16, col: u16, c: char) {
        // Set the flag on the cell
        if let Some(cell) = self.cell_mut(col, row) {
            cell.set_flag(CellFlags::HAS_COMBINING);
        }

        // Add to the combining map
        self.combining.entry((row, col)).or_default().push(c);
    }

    /// Get combining characters for a cell.
    pub fn combining_chars(&self, row: u16, col: u16) -> Option<&[char]> {
        self.combining.get(&(row, col)).map(|v| v.as_slice())
    }

    /// Get the full grapheme for a cell (base char + combining chars).
    pub fn grapheme(&self, row: u16, col: u16) -> Option<String> {
        let cell = self.cell(col, row)?;
        let base = cell.char();
        if cell.has_flag(CellFlags::HAS_COMBINING) {
            if let Some(combining) = self.combining_chars(row, col) {
                let mut s = String::with_capacity(1 + combining.len());
                s.push(base);
                for &c in combining {
                    s.push(c);
                }
                return Some(s);
            }
        }
        Some(base.to_string())
    }

    /// Clear combining characters for a cell.
    fn clear_combining(&mut self, row: u16, col: u16) {
        self.combining.remove(&(row, col));
    }

    /// Clear all combining characters for a range of rows.
    fn clear_combining_rows(&mut self, start_row: u16, end_row: u16) {
        self.combining
            .retain(|&(row, _), _| row < start_row || row > end_row);
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
        use unicode_width::UnicodeWidthChar;

        // Get character width (0 for control chars, 1 for normal, 2 for wide)
        let width = c.width().unwrap_or(0) as u16;

        if width == 0 {
            // Zero-width character - could be combining mark or control char
            // Try to attach combining characters to the previous cell
            let col = self.cursor.position.col;
            let row = self.cursor.position.row;
            if col > 0 || self.cursor.wrap_pending {
                // Attach to previous cell (which is at col-1, or last col if wrap pending)
                let attach_col = if self.cursor.wrap_pending {
                    self.dims.cols.saturating_sub(1)
                } else {
                    col - 1
                };
                self.add_combining_char(row, attach_col, c);
            } else if row > 0 {
                // Previous row, last column
                self.add_combining_char(row - 1, self.dims.cols.saturating_sub(1), c);
            }
            // Otherwise ignore (no previous cell to attach to)
            return;
        }

        // If wrap is pending, perform the wrap now before writing
        if self.cursor.wrap_pending {
            // Mark current row as wrapped (soft wrap)
            let row = self.cursor.position.row;
            if let Some(r) = self.rows.get_mut(row as usize) {
                r.set_wrapped(true);
            }
            // Move to next line, respecting scroll region
            let bottom = self
                .scroll_region
                .map(|r| r.bottom)
                .unwrap_or(self.dims.rows.saturating_sub(1));
            self.cursor.position.col = 0;
            self.cursor.position.row += 1;
            if self.cursor.position.row > bottom {
                self.scroll_up(1);
                self.cursor.position.row = bottom;
            }
            self.cursor.wrap_pending = false;
        }

        let col = self.cursor.position.col;
        let row = self.cursor.position.row;

        if row >= self.dims.rows {
            return;
        }

        // Wide character can never fit in a terminal narrower than its width
        if width > 1 && self.dims.cols < width {
            // Replace with a space to avoid infinite scroll loop
            if let Some(cell) = self.cell_mut(col, row) {
                cell.set_char(' ');
            }
            self.cursor.position.col = col + 1;
            if self.cursor.position.col >= self.dims.cols {
                self.cursor.wrap_pending = true;
            }
            self.mark_dirty(row);
            return;
        }

        // Check if wide char would wrap (needs 2 columns but only 1 left)
        if width > 1 && col + width > self.dims.cols {
            // Mark current row as wrapped (soft wrap)
            if let Some(r) = self.row_mut(row) {
                r.set_wrapped(true);
            }
            // Wrap to next line before writing, respecting scroll region
            let bottom = self
                .scroll_region
                .map(|r| r.bottom)
                .unwrap_or(self.dims.rows.saturating_sub(1));
            self.cursor.position.col = 0;
            self.cursor.position.row += 1;
            if self.cursor.position.row > bottom {
                self.scroll_up(1);
                self.cursor.position.row = bottom;
            }
        }

        let col = self.cursor.position.col;
        let row = self.cursor.position.row;

        // Copy attrs first to avoid borrow conflict
        let attrs = self.attrs;

        // Clear any combining characters from previous content
        self.clear_combining(row, col);

        // Write the character
        if let Some(cell) = self.cell_mut(col, row) {
            cell.set_char(c);
            cell.copy_attrs_from(&attrs);
            cell.unset_flag(CellFlags::HAS_COMBINING);

            // Handle wide characters
            if width > 1 {
                cell.set_flag(CellFlags::WIDE_CHAR);
            } else {
                cell.unset_flag(CellFlags::WIDE_CHAR);
            }
            cell.unset_flag(CellFlags::WIDE_CHAR_SPACER);
        }

        // If wide character, put spacer in next cell
        if width > 1 && col + 1 < self.dims.cols {
            if let Some(spacer) = self.cell_mut(col + 1, row) {
                spacer.set_char(' ');
                spacer.copy_attrs_from(&attrs);
                spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
                spacer.unset_flag(CellFlags::WIDE_CHAR);
            }
        }

        // Advance cursor by character width
        self.cursor.position.col += width;

        // Check if we're now at or past the last column
        if self.cursor.position.col >= self.dims.cols {
            // Keep cursor at last valid column
            self.cursor.position.col = self.dims.cols.saturating_sub(1);
            // Only set pending wrap if auto-wrap (DECAWM) is enabled;
            // when off, subsequent chars overwrite the last column
            if self.auto_wrap {
                self.cursor.wrap_pending = true;
            }
        }
    }

    /// Move cursor up, stopping at top edge.
    pub fn cursor_up(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.row = self.cursor.position.row.saturating_sub(n);
    }

    /// Move cursor down, stopping at bottom edge.
    pub fn cursor_down(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.row = (self.cursor.position.row + n).min(self.dims.rows.saturating_sub(1));
    }

    /// Move cursor left, stopping at left edge.
    pub fn cursor_left(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.col = self.cursor.position.col.saturating_sub(n);
    }

    /// Move cursor right, stopping at right edge.
    pub fn cursor_right(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.col = (self.cursor.position.col + n).min(self.dims.cols.saturating_sub(1));
    }

    /// Set absolute cursor position (1-based like ANSI).
    pub fn set_cursor_position(&mut self, row: u16, col: u16) {
        self.cursor.wrap_pending = false;
        // ANSI positions are 1-based, convert to 0-based
        self.cursor.position.row = row.saturating_sub(1).min(self.dims.rows.saturating_sub(1));
        self.cursor.position.col = col.saturating_sub(1).min(self.dims.cols.saturating_sub(1));
    }

    /// Carriage return - move cursor to column 0.
    pub fn carriage_return(&mut self) {
        // Clear pending wrap - CR should NOT cause a line break, it just
        // moves to column 0 of the current line
        self.cursor.wrap_pending = false;
        self.cursor.position.col = 0;
    }

    /// Newline - move cursor down, scrolling if needed.
    pub fn newline(&mut self) {
        self.cursor.wrap_pending = false;
        let bottom = self
            .scroll_region
            .map(|r| r.bottom)
            .unwrap_or(self.dims.rows.saturating_sub(1));

        if self.cursor.position.row >= bottom {
            self.scroll_up(1);
        } else {
            self.cursor.position.row += 1;
        }
    }

    /// Backspace - move cursor left by one.
    pub fn backspace(&mut self) {
        self.cursor.wrap_pending = false;
        if self.cursor.position.col > 0 {
            self.cursor.position.col -= 1;
        }
    }

    /// Tab - advance to next tab stop.
    pub fn tab(&mut self) {
        self.cursor.wrap_pending = false;
        let current = self.cursor.position.col;
        if let Some(&next) = self.tab_stops.range((current + 1)..).next() {
            self.cursor.position.col = next.min(self.dims.cols.saturating_sub(1));
        } else {
            self.cursor.position.col = self.dims.cols.saturating_sub(1);
        }
    }

    /// Set a tab stop at the current cursor column.
    pub fn set_tab_stop(&mut self) {
        self.tab_stops.insert(self.cursor.position.col);
    }

    /// Clear tab stop(s).
    pub fn clear_tab_stop(&mut self, mode: u8) {
        match mode {
            0 => {
                self.tab_stops.remove(&self.cursor.position.col);
            }
            3 => {
                self.tab_stops.clear();
            }
            _ => {}
        }
    }

    /// Reset tab stops to default (every 8 columns).
    pub fn reset_tab_stops(&mut self) {
        self.tab_stops = (0..self.dims.cols).step_by(8).collect();
    }

    /// Set the scroll region (1-based top and bottom, inclusive).
    /// Pass (1, rows) to reset to full screen.
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        self.cursor.wrap_pending = false;
        // Convert from 1-based to 0-based
        let top = top.saturating_sub(1);
        let bottom = bottom.saturating_sub(1).min(self.dims.rows.saturating_sub(1));

        if top >= bottom {
            // Invalid region, reset to full screen
            self.scroll_region = None;
        } else if top == 0 && bottom >= self.dims.rows.saturating_sub(1) {
            // Full screen, no need to track
            self.scroll_region = None;
        } else {
            self.scroll_region = Some(ScrollRegion::new(top, bottom));
        }

        // Move cursor to home position (top-left of scroll region if origin mode)
        // For now, just move to absolute top-left
        self.cursor.position.row = top;
        self.cursor.position.col = 0;
    }

    /// Get the current scroll region.
    pub fn scroll_region(&self) -> Option<ScrollRegion> {
        self.scroll_region
    }

    /// Get the effective scroll region (returns full screen if none set).
    pub fn effective_scroll_region(&self) -> ScrollRegion {
        self.scroll_region
            .unwrap_or(ScrollRegion::new(0, self.dims.rows.saturating_sub(1)))
    }

    /// Scroll the grid up by n lines (within scroll region).
    pub fn scroll_up(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(region.top) + 1) as usize;

        // Clear combining chars for the entire scroll region
        self.clear_combining_rows(region.top, region.bottom);

        if n >= region_height {
            // Clear the entire scroll region
            for row_idx in region.top..=region.bottom {
                if let Some(row) = self.rows.get_mut(row_idx as usize) {
                    row.clear();
                }
                self.mark_dirty(row_idx);
            }
            return;
        }

        // Shift rows up within the region
        for row_idx in (region.top as usize)..=(region.bottom as usize - n) {
            let src_row = row_idx + n;
            self.rows[row_idx] = self.rows[src_row].clone();
        }

        // Clear the bottom n rows of the region
        for i in 0..n {
            let row_idx = region.bottom as usize - i;
            self.rows[row_idx] = Row::new(self.dims.cols);
        }

        // Mark affected rows dirty
        for row_idx in region.top..=region.bottom {
            self.mark_dirty(row_idx);
        }
    }

    /// Scroll the grid down by n lines (within scroll region).
    pub fn scroll_down(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(region.top) + 1) as usize;

        // Clear combining chars for the entire scroll region
        self.clear_combining_rows(region.top, region.bottom);

        if n >= region_height {
            // Clear the entire scroll region
            for row_idx in region.top..=region.bottom {
                if let Some(row) = self.rows.get_mut(row_idx as usize) {
                    row.clear();
                }
                self.mark_dirty(row_idx);
            }
            return;
        }

        // Shift rows down within the region (reverse to avoid overwriting)
        for row_idx in ((region.top as usize + n)..=region.bottom as usize).rev() {
            let src_row = row_idx - n;
            self.rows[row_idx] = self.rows[src_row].clone();
        }

        // Clear the top n rows of the region
        for i in 0..n {
            let row_idx = region.top as usize + i;
            self.rows[row_idx] = Row::new(self.dims.cols);
        }

        // Mark affected rows dirty
        for row_idx in region.top..=region.bottom {
            self.mark_dirty(row_idx);
        }
    }

    /// Insert n blank lines at the cursor position (IL).
    /// Content from cursor row to bottom of scroll region shifts down.
    pub fn insert_lines(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let cursor_row = self.cursor.position.row;

        // Only operate if cursor is within scroll region
        if cursor_row < region.top || cursor_row > region.bottom {
            return;
        }

        // Clear combining chars for affected rows
        self.clear_combining_rows(cursor_row, region.bottom);

        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(cursor_row) + 1) as usize;

        if n >= region_height {
            // Clear from cursor to bottom of region
            for row_idx in cursor_row..=region.bottom {
                if let Some(row) = self.rows.get_mut(row_idx as usize) {
                    row.clear();
                }
                self.mark_dirty(row_idx);
            }
            return;
        }

        // Move rows down (from bottom to top to avoid overwriting)
        for i in (0..(region_height - n)).rev() {
            let src_row = cursor_row as usize + i;
            let dst_row = src_row + n;

            if dst_row <= region.bottom as usize {
                let src = self.rows[src_row].clone();
                self.rows[dst_row] = src;
            }
        }

        // Clear the n rows at cursor position
        for i in 0..n {
            let row_idx = cursor_row as usize + i;
            if row_idx <= region.bottom as usize {
                self.rows[row_idx] = Row::new(self.dims.cols);
            }
        }

        // Mark affected rows dirty
        for row_idx in cursor_row..=region.bottom {
            self.mark_dirty(row_idx);
        }
    }

    /// Delete n lines at the cursor position (DL).
    /// Content below cursor shifts up, blank lines appear at bottom of scroll region.
    pub fn delete_lines(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let cursor_row = self.cursor.position.row;

        // Only operate if cursor is within scroll region
        if cursor_row < region.top || cursor_row > region.bottom {
            return;
        }

        // Clear combining chars for affected rows
        self.clear_combining_rows(cursor_row, region.bottom);

        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(cursor_row) + 1) as usize;

        if n >= region_height {
            // Clear from cursor to bottom of region
            for row_idx in cursor_row..=region.bottom {
                if let Some(row) = self.rows.get_mut(row_idx as usize) {
                    row.clear();
                }
                self.mark_dirty(row_idx);
            }
            return;
        }

        // Move rows up
        for i in 0..(region_height - n) {
            let src_row = cursor_row as usize + n + i;
            let dst_row = cursor_row as usize + i;

            if src_row <= region.bottom as usize {
                let src = self.rows[src_row].clone();
                self.rows[dst_row] = src;
            }
        }

        // Clear the bottom n rows of the region
        for i in 0..n {
            let row_idx = region.bottom as usize - i;
            if row_idx >= cursor_row as usize {
                self.rows[row_idx] = Row::new(self.dims.cols);
            }
        }

        // Mark affected rows dirty
        for row_idx in cursor_row..=region.bottom {
            self.mark_dirty(row_idx);
        }
    }

    /// Erase from cursor to end of display.
    pub fn erase_below(&mut self) {
        let r = self.cursor.position.row;
        // Clear combining chars: partial cursor row (cursor_col..end) + all rows below
        for col in self.cursor.position.col..self.dims.cols {
            self.combining.remove(&(r, col));
        }
        if r + 1 < self.dims.rows {
            self.clear_combining_rows(r + 1, self.dims.rows.saturating_sub(1));
        }

        // Erase from cursor to end of current line
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in self.cursor.position.col..self.dims.cols {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(r);

        // Erase all lines below cursor
        for row_idx in (r + 1)..self.dims.rows {
            if let Some(row) = self.rows.get_mut(row_idx as usize) {
                row.clear();
            }
            self.mark_dirty(row_idx);
        }
    }

    /// Erase from start of display to cursor.
    pub fn erase_above(&mut self) {
        let r = self.cursor.position.row;
        // Clear combining chars: all rows above cursor + partial cursor row (0..=cursor_col)
        if r > 0 {
            self.clear_combining_rows(0, r - 1);
        }
        for col in 0..=self.cursor.position.col {
            self.combining.remove(&(r, col));
        }

        // Erase all lines above cursor
        for row_idx in 0..r {
            if let Some(row) = self.rows.get_mut(row_idx as usize) {
                row.clear();
            }
            self.mark_dirty(row_idx);
        }

        // Erase from start of current line to cursor
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in 0..=self.cursor.position.col {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(r);
    }

    /// Erase entire display.
    pub fn clear(&mut self) {
        self.combining.clear();
        for row in &mut self.rows {
            row.clear();
        }
        self.mark_all_dirty();
    }

    /// Erase from cursor to end of line.
    pub fn erase_line_right(&mut self) {
        let r = self.cursor.position.row;
        // Clear combining chars for erased cells on this row
        for col in self.cursor.position.col..self.dims.cols {
            self.combining.remove(&(r, col));
        }
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in self.cursor.position.col..self.dims.cols {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
            // Erasing to end of line means this line ends here (hard break),
            // so clear the wrapped flag. This is important for proper reflow
            // when resizing - content after an erase-to-EOL should not be
            // considered a continuation of this line.
            row.set_wrapped(false);
        }
        self.mark_dirty(r);
    }

    /// Erase from start of line to cursor.
    pub fn erase_line_left(&mut self) {
        let r = self.cursor.position.row;
        // Clear combining chars for erased cells on this row
        for col in 0..=self.cursor.position.col {
            self.combining.remove(&(r, col));
        }
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in 0..=self.cursor.position.col {
                if let Some(cell) = row.get_mut(i) {
                    cell.clear();
                }
            }
        }
        self.mark_dirty(r);
    }

    /// Erase entire current line.
    pub fn erase_line(&mut self) {
        let r = self.cursor.position.row;
        self.clear_combining_rows(r, r);
        if let Some(row) = self.rows.get_mut(r as usize) {
            row.clear();
        }
        self.mark_dirty(r);
    }

    /// Resize the grid with reflow.
    pub fn resize(&mut self, new_dims: Dimensions) {
        if new_dims == self.dims {
            return;
        }

        let old_cols = self.dims.cols;
        let new_cols = new_dims.cols;

        // Perform reflow if width changed
        if old_cols != new_cols {
            self.reflow(new_cols);
        }

        // Add or remove rows
        while self.rows.len() < new_dims.rows as usize {
            self.rows.push(Row::new(new_cols));
        }
        self.rows.truncate(new_dims.rows as usize);

        // Clamp cursor
        self.cursor.position.col = self
            .cursor
            .position
            .col
            .min(new_dims.cols.saturating_sub(1));
        self.cursor.position.row = self
            .cursor
            .position
            .row
            .min(new_dims.rows.saturating_sub(1));

        // Reset scroll region — apps that need one will re-set it after resize
        self.scroll_region = None;

        // Extend tab stops to cover any new columns
        let old_cols = self.dims.cols;
        if new_dims.cols > old_cols {
            for col in (old_cols..new_dims.cols).filter(|c| c % 8 == 0) {
                self.tab_stops.insert(col);
            }
        }

        self.dims = new_dims;
        self.dirty = vec![true; new_dims.rows as usize];
    }

    /// Reflow lines to a new width.
    ///
    /// This handles soft-wrapped lines by joining them together and re-wrapping
    /// to the new width. Hard line breaks (actual newlines) are preserved.
    fn reflow(&mut self, new_cols: u16) {
        use crate::grid::cell::Cell;

        if self.rows.is_empty() {
            return;
        }

        // Combining chars are keyed by (row, col) which become invalid after reflow
        self.combining.clear();

        // Collect all logical lines (sequences of wrapped rows form one logical line)
        let mut logical_lines: Vec<Vec<Cell>> = Vec::new();
        let mut current_line: Vec<Cell> = Vec::new();

        for row in self.rows.drain(..) {
            // Append this row's non-blank cells to current logical line
            for cell in row.cells() {
                current_line.push(*cell);
            }

            // If this row wasn't wrapped, this logical line ends here
            if !row.is_wrapped() {
                // Trim trailing empty cells
                while current_line.last().is_some_and(|c| c.is_blank()) {
                    current_line.pop();
                }
                logical_lines.push(std::mem::take(&mut current_line));
            }
        }

        // Don't forget any remaining cells
        if !current_line.is_empty() {
            while current_line.last().is_some_and(|c| c.is_blank()) {
                current_line.pop();
            }
            logical_lines.push(current_line);
        }

        // Now re-wrap each logical line to new width
        for line in logical_lines {
            if line.is_empty() {
                // Empty line - just add an empty row
                self.rows.push(Row::new(new_cols));
                continue;
            }

            let mut pos = 0;
            while pos < line.len() {
                let mut new_row = Row::new(new_cols);
                let mut col = 0u16;

                while col < new_cols && pos < line.len() {
                    let cell = &line[pos];

                    // Skip spacer cells from the old layout — they will be
                    // regenerated when we place the wide char below.
                    if cell.is_wide_spacer() {
                        pos += 1;
                        continue;
                    }

                    // A wide char needs two columns. If only one column
                    // remains on this row, leave it blank and wrap the
                    // wide char to the next row.
                    if cell.is_wide() && col + 1 >= new_cols {
                        break;
                    }

                    if let Some(dest) = new_row.get_mut(col) {
                        *dest = *cell;
                    }
                    col += 1;
                    pos += 1;

                    // After placing a wide char, insert its spacer cell.
                    if cell.is_wide() {
                        if let Some(spacer) = new_row.get_mut(col) {
                            spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
                        }
                        col += 1;
                    }
                }

                // If there's more content, mark this row as wrapped
                if pos < line.len() {
                    new_row.set_wrapped(true);
                }

                self.rows.push(new_row);
            }
        }
    }

    /// Set rows directly (used by Buffer during reflow).
    pub fn set_rows(&mut self, rows: Vec<Row>, new_dims: Dimensions) {
        self.rows = rows;
        self.dims = new_dims;

        // Ensure correct number of rows
        while self.rows.len() < new_dims.rows as usize {
            self.rows.push(Row::new(new_dims.cols));
        }
        self.rows.truncate(new_dims.rows as usize);

        // Clamp cursor position
        self.cursor.position.col = self
            .cursor
            .position
            .col
            .min(new_dims.cols.saturating_sub(1));
        self.cursor.position.row = self
            .cursor
            .position
            .row
            .min(new_dims.rows.saturating_sub(1));

        // Mark all dirty
        self.dirty = vec![true; new_dims.rows as usize];
    }

    /// Prepend rows at the top (used when growing viewport to pull from history).
    pub fn prepend_rows(&mut self, rows: Vec<Row>) {
        let count = rows.len();
        if count == 0 {
            return;
        }

        // Insert at beginning, then truncate from end
        for row in rows.into_iter().rev() {
            self.rows.insert(0, row);
        }
        self.rows.truncate(self.dims.rows as usize);

        // Adjust cursor position down by the number of inserted rows
        self.cursor.position.row = self
            .cursor
            .position
            .row
            .saturating_add(count.min(u16::MAX as usize) as u16)
            .min(self.dims.rows.saturating_sub(1));

        // Mark all dirty
        self.mark_all_dirty();
    }

    /// Remove rows from the top (used when shrinking viewport).
    pub fn remove_top_rows(&mut self, count: usize) {
        if count == 0 || count >= self.rows.len() {
            return;
        }

        // Remove from the beginning
        self.rows.drain(..count);

        // Adjust cursor position up
        self.cursor.position.row = self
            .cursor
            .position
            .row
            .saturating_sub(count.min(u16::MAX as usize) as u16);

        // Mark all dirty
        self.mark_all_dirty();
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

    /// Check if any row is dirty.
    pub fn has_dirty(&self) -> bool {
        self.dirty.iter().any(|&d| d)
    }

    /// Get indices of dirty rows.
    pub fn dirty_rows(&self) -> impl Iterator<Item = u16> + '_ {
        self.dirty
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| if d { Some(i as u16) } else { None })
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
    fn reflow_on_narrow() {
        // Create a 10-column grid and write 15 characters
        let mut grid = Grid::new(Dimensions::new(10, 5));
        for c in "Hello World!!!!".chars() {
            grid.write_char(c);
        }
        // Content should be on rows 0 and 1 (wrapped at col 10)
        assert!(grid.row(0).unwrap().is_wrapped()); // First row wrapped

        // Now widen to 20 columns
        grid.resize(Dimensions::new(20, 5));
        // Content should now fit on one row
        let row0_text = grid.row(0).unwrap().text_trimmed();
        assert_eq!(row0_text, "Hello World!!!!");
        assert!(!grid.row(0).unwrap().is_wrapped());
    }

    #[test]
    fn reflow_on_widen() {
        // Create a 20-column grid with content
        let mut grid = Grid::new(Dimensions::new(20, 5));
        for c in "Hello World!!!!".chars() {
            grid.write_char(c);
        }
        assert_eq!(grid.row(0).unwrap().text_trimmed(), "Hello World!!!!");

        // Narrow to 10 columns - should wrap
        grid.resize(Dimensions::new(10, 5));
        // Content should now be split across two rows
        assert!(grid.row(0).unwrap().is_wrapped());
        assert_eq!(grid.row(0).unwrap().text_trimmed(), "Hello Worl");
        assert_eq!(grid.row(1).unwrap().text_trimmed(), "d!!!!");
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
    fn custom_tab_stops() {
        let mut grid = Grid::new(Dimensions::new(80, 24));

        grid.clear_tab_stop(3);
        grid.cursor.position.col = 5;
        grid.set_tab_stop();
        grid.cursor.position.col = 12;
        grid.set_tab_stop();
        grid.cursor.position.col = 20;
        grid.set_tab_stop();

        grid.cursor.position.col = 0;
        grid.tab();
        assert_eq!(grid.cursor.position.col, 5);

        grid.tab();
        assert_eq!(grid.cursor.position.col, 12);

        grid.clear_tab_stop(0);
        grid.tab();
        assert_eq!(grid.cursor.position.col, 20);

        grid.reset_tab_stops();
        grid.cursor.position.col = 5;
        grid.tab();
        assert_eq!(grid.cursor.position.col, 8);
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

    #[test]
    fn scroll_region_basic() {
        let mut grid = Grid::new(Dimensions::new(10, 10));

        // Set scroll region to rows 3-7 (1-indexed, so 2-6 in 0-indexed)
        grid.set_scroll_region(3, 7);

        let region = grid.scroll_region().unwrap();
        assert_eq!(region.top, 2);
        assert_eq!(region.bottom, 6);
    }

    #[test]
    fn scroll_region_full_screen_reset() {
        let mut grid = Grid::new(Dimensions::new(10, 10));

        // Set a scroll region
        grid.set_scroll_region(3, 7);
        assert!(grid.scroll_region().is_some());

        // Reset to full screen
        grid.set_scroll_region(1, 10);
        assert!(grid.scroll_region().is_none());
    }

    #[test]
    fn scroll_up_in_region() {
        let mut grid = Grid::new(Dimensions::new(5, 10));

        // Write content on each row
        for row in 0..10 {
            grid.cursor.position.row = row;
            grid.cursor.position.col = 0;
            grid.write_char((b'A' + row as u8) as char); // A, B, C, etc.
        }

        // Set scroll region to rows 3-7 (1-indexed: rows 2-6 in 0-indexed)
        grid.set_scroll_region(3, 7);

        // Scroll up within region
        grid.scroll_up(1);

        // Rows outside region should be unchanged
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A'); // Row 0 unchanged
        assert_eq!(grid.cell(0, 1).unwrap().char(), 'B'); // Row 1 unchanged

        // Rows inside region shifted up
        assert_eq!(grid.cell(0, 2).unwrap().char(), 'D'); // Was row 3, now row 2
        assert_eq!(grid.cell(0, 5).unwrap().char(), 'G'); // Was row 6, now row 5

        // Bottom of region should be blank
        assert_eq!(grid.cell(0, 6).unwrap().char(), ' '); // Cleared

        // Row below region unchanged
        assert_eq!(grid.cell(0, 7).unwrap().char(), 'H');
    }

    #[test]
    fn scroll_down_in_region() {
        let mut grid = Grid::new(Dimensions::new(5, 10));

        // Write content on each row
        for row in 0..10 {
            grid.cursor.position.row = row;
            grid.cursor.position.col = 0;
            grid.write_char((b'A' + row as u8) as char);
        }

        // Set scroll region to rows 3-7 (1-indexed: rows 2-6 in 0-indexed)
        grid.set_scroll_region(3, 7);

        // Scroll down within region
        grid.scroll_down(1);

        // Rows outside region should be unchanged
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert_eq!(grid.cell(0, 1).unwrap().char(), 'B');

        // Top of region should be blank
        assert_eq!(grid.cell(0, 2).unwrap().char(), ' '); // Cleared

        // Rows inside region shifted down
        assert_eq!(grid.cell(0, 3).unwrap().char(), 'C'); // Was row 2, now row 3
        assert_eq!(grid.cell(0, 6).unwrap().char(), 'F'); // Was row 5, now row 6

        // Row below region unchanged
        assert_eq!(grid.cell(0, 7).unwrap().char(), 'H');
    }

    #[test]
    fn newline_respects_scroll_region() {
        let mut grid = Grid::new(Dimensions::new(5, 10));

        // Write content
        for row in 0..10 {
            grid.cursor.position.row = row;
            grid.cursor.position.col = 0;
            grid.write_char((b'A' + row as u8) as char);
        }

        // Set scroll region to rows 3-7 (1-indexed: rows 2-6 in 0-indexed)
        grid.set_scroll_region(3, 7);

        // Move cursor to bottom of scroll region
        grid.cursor.position.row = 6;
        grid.cursor.position.col = 0;

        // Newline should scroll within region
        grid.newline();

        // Cursor should still be at bottom of region
        assert_eq!(grid.cursor.position.row, 6);

        // Content above region unchanged
        assert_eq!(grid.cell(0, 1).unwrap().char(), 'B');

        // Region scrolled up
        assert_eq!(grid.cell(0, 2).unwrap().char(), 'D'); // Was C, now D

        // Bottom of region cleared
        assert_eq!(grid.cell(0, 6).unwrap().char(), ' ');

        // Below region unchanged
        assert_eq!(grid.cell(0, 7).unwrap().char(), 'H');
    }

    #[test]
    fn combining_characters() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        // Write 'e' followed by combining acute accent (é)
        grid.write_char('e');
        grid.write_char('\u{0301}'); // Combining acute accent

        // Base character should be at col 0
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'e');
        assert!(grid.cell(0, 0).unwrap().has_flag(CellFlags::HAS_COMBINING));

        // Cursor should still be at col 1 (combining char doesn't advance cursor)
        assert_eq!(grid.cursor().position.col, 1);

        // Full grapheme should include both characters
        let grapheme = grid.grapheme(0, 0).unwrap();
        assert_eq!(grapheme, "e\u{0301}");
    }

    #[test]
    fn combining_char_cleared_on_overwrite() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        // Write 'e' with combining accent
        grid.write_char('e');
        grid.write_char('\u{0301}');

        assert!(grid.cell(0, 0).unwrap().has_flag(CellFlags::HAS_COMBINING));

        // Go back and overwrite
        grid.cursor.position.col = 0;
        grid.write_char('X');

        // Combining chars should be cleared
        assert!(!grid.cell(0, 0).unwrap().has_flag(CellFlags::HAS_COMBINING));
        assert_eq!(grid.grapheme(0, 0).unwrap(), "X");
    }

    #[test]
    fn dirty_tracking() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        // New grid should have all rows dirty
        assert!(grid.has_dirty());
        assert!(grid.is_dirty(0));
        assert!(grid.is_dirty(4));

        // Clear dirty flags
        grid.clear_dirty();
        assert!(!grid.has_dirty());
        assert!(!grid.is_dirty(0));

        // Writing a character marks that row dirty
        grid.write_char('A');
        assert!(grid.has_dirty());
        assert!(grid.is_dirty(0));
        assert!(!grid.is_dirty(1));

        // Clear and verify
        grid.clear_dirty();
        assert!(!grid.has_dirty());

        // Accessing a cell mutably marks row dirty
        let _ = grid.cell_mut(5, 2);
        assert!(grid.is_dirty(2));
        assert!(!grid.is_dirty(0));
    }

    #[test]
    fn dirty_rows_iterator() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        grid.clear_dirty();

        // Mark specific rows
        let _ = grid.cell_mut(0, 1);
        let _ = grid.cell_mut(0, 3);

        let dirty: Vec<u16> = grid.dirty_rows().collect();
        assert_eq!(dirty, vec![1, 3]);
    }

    #[test]
    fn insert_lines_at_cursor() {
        let mut grid = Grid::new(Dimensions::new(5, 6));

        for row in 0..6 {
            grid.cursor.position.row = row;
            grid.cursor.position.col = 0;
            grid.write_char((b'A' + row as u8) as char);
        }

        grid.cursor.position.row = 2;
        grid.cursor.position.col = 0;

        grid.insert_lines(2);

        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert_eq!(grid.cell(0, 1).unwrap().char(), 'B');
        assert_eq!(grid.cell(0, 2).unwrap().char(), ' ');
        assert_eq!(grid.cell(0, 3).unwrap().char(), ' ');
        assert_eq!(grid.cell(0, 4).unwrap().char(), 'C');
        assert_eq!(grid.cell(0, 5).unwrap().char(), 'D');
    }

    #[test]
    fn delete_lines_at_cursor() {
        let mut grid = Grid::new(Dimensions::new(5, 6));

        for row in 0..6 {
            grid.cursor.position.row = row;
            grid.cursor.position.col = 0;
            grid.write_char((b'A' + row as u8) as char);
        }

        grid.cursor.position.row = 2;
        grid.cursor.position.col = 0;

        grid.delete_lines(2);

        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert_eq!(grid.cell(0, 1).unwrap().char(), 'B');
        assert_eq!(grid.cell(0, 2).unwrap().char(), 'E');
        assert_eq!(grid.cell(0, 3).unwrap().char(), 'F');
        assert_eq!(grid.cell(0, 4).unwrap().char(), ' ');
        assert_eq!(grid.cell(0, 5).unwrap().char(), ' ');
    }

    #[test]
    fn wrap_respects_scroll_region() {
        let mut grid = Grid::new(Dimensions::new(5, 10));

        // Write content on each row
        for row in 0..10 {
            grid.cursor.position.row = row;
            grid.cursor.position.col = 0;
            grid.write_char((b'A' + row as u8) as char);
        }

        // Set scroll region to rows 3-7 (1-indexed)
        grid.set_scroll_region(3, 7);

        // Write chars to fill past the right margin inside the region
        // Cursor is at row 2 (top of region) after set_scroll_region
        grid.cursor.position.row = 6; // bottom of 0-indexed region
        grid.cursor.position.col = 0;

        // Fill to trigger wrap at end of row
        for _ in 0..5 {
            grid.write_char('X');
        }
        // Now wrap_pending is true. Writing another char should scroll
        // within the region, NOT escape to row 7.
        grid.write_char('Z');

        // Cursor should still be at the bottom of the scroll region
        assert_eq!(grid.cursor().position.row, 6);
        // Row below region should be unchanged
        assert_eq!(grid.cell(0, 7).unwrap().char(), 'H');
    }

    #[test]
    fn resize_resets_scroll_region() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.set_scroll_region(5, 20);
        assert!(grid.scroll_region().is_some());

        grid.resize(Dimensions::new(80, 10));
        assert!(grid.scroll_region().is_none());
    }

    #[test]
    fn resize_extends_tab_stops() {
        let mut grid = Grid::new(Dimensions::new(40, 5));
        // Default: tabs at 0, 8, 16, 24, 32
        assert!(grid.tab_stops.contains(&32));
        assert!(!grid.tab_stops.contains(&40));

        // Widen to 80 columns
        grid.resize(Dimensions::new(80, 5));
        // Should now have tabs at 40, 48, 56, 64, 72
        assert!(grid.tab_stops.contains(&40));
        assert!(grid.tab_stops.contains(&48));
        assert!(grid.tab_stops.contains(&72));
    }

    #[test]
    fn combining_chars_cleared_on_scroll_up() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        // Write 'e' with combining accent on row 2
        grid.cursor.position.row = 2;
        grid.cursor.position.col = 0;
        grid.write_char('e');
        grid.write_char('\u{0301}');
        assert!(grid.combining_chars(2, 0).is_some());

        // Scroll up - combining chars should be cleaned
        grid.scroll_up(1);
        // Old combining entry at (2, 0) should be gone
        assert!(grid.combining_chars(2, 0).is_none());
    }

    #[test]
    fn combining_chars_cleared_on_erase() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        // Write 'e' with combining accent on row 0
        grid.write_char('e');
        grid.write_char('\u{0301}');
        assert!(grid.combining_chars(0, 0).is_some());

        // Erase line right should clear combining
        grid.cursor.position.col = 0;
        grid.erase_line_right();
        assert!(grid.combining_chars(0, 0).is_none());
    }

    #[test]
    fn combining_chars_cleared_on_clear() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        grid.write_char('e');
        grid.write_char('\u{0301}');
        assert!(grid.combining_chars(0, 0).is_some());

        grid.clear();
        assert!(grid.combining.is_empty());
    }

    #[test]
    fn auto_wrap_off_prevents_wrap() {
        let mut grid = Grid::new(Dimensions::new(5, 3));
        grid.set_auto_wrap(false);

        // Write 6 chars in 5-column grid
        for c in "ABCDEF".chars() {
            grid.write_char(c);
        }

        // Should NOT have wrapped — last column overwritten
        assert_eq!(grid.cursor().position.row, 0);
        assert_eq!(grid.cursor().position.col, 4);
        // Last column should have 'F' (overwritten 'E')
        assert_eq!(grid.cell(4, 0).unwrap().char(), 'F');
        // Row 1 should be empty
        assert_eq!(grid.cell(0, 1).unwrap().char(), ' ');
    }

    #[test]
    fn auto_wrap_on_wraps_normally() {
        let mut grid = Grid::new(Dimensions::new(5, 3));
        // auto_wrap defaults to true

        // Write 6 chars in 5-column grid
        for c in "ABCDEF".chars() {
            grid.write_char(c);
        }

        // Should have wrapped to row 1
        assert_eq!(grid.cursor().position.row, 1);
        assert_eq!(grid.cell(0, 1).unwrap().char(), 'F');
    }

    #[test]
    fn zero_dimension_grid_does_not_panic() {
        let mut grid = Grid::new(Dimensions::new(0, 0));

        // None of these should panic with 0-dimension grid
        grid.cursor_down(1);
        grid.cursor_right(1);
        grid.cursor_up(1);
        grid.cursor_left(1);
        grid.set_cursor_position(1, 1);
        grid.newline();
        grid.tab();
        grid.write_char('A');

        let region = grid.effective_scroll_region();
        assert_eq!(region.top, 0);
        assert_eq!(region.bottom, 0);
    }

    #[test]
    fn reflow_wide_char_at_boundary() {
        // 4-column grid: write "A" + wide char '中' + "B"
        let mut grid = Grid::new(Dimensions::new(4, 3));
        grid.write_char('A');
        grid.write_char('中'); // takes cols 1-2
        grid.write_char('B'); // col 3

        // Reflow to 2 columns — after placing 'A' at col 0,
        // '中' needs 2 cols but only 1 remains, so it wraps.
        grid.reflow(2);

        // Row 0: 'A' then blank (wide char doesn't fit)
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert!(!grid.cell(1, 0).unwrap().is_wide());
        // Row 1: '中' + spacer
        assert!(grid.cell(0, 1).unwrap().is_wide());
        assert!(grid.cell(1, 1).unwrap().is_wide_spacer());
        // Row 2: 'B'
        assert_eq!(grid.cell(0, 2).unwrap().char(), 'B');
    }
}
