//! Terminal grid and buffer management.
//! Terminal grid and cell management.
//!
//! This module provides the core data structures for terminal emulation:
//! - [`Cell`] - A single character cell with styling
//! - [`Row`] - A row of cells
//! - [`Grid`] - The visible terminal grid
//! - [`Buffer`] - Grid with scrollback history
//! - [`Terminal`] - Full terminal state with alternate screen and selection
//! - [`TerminalHandler`] - Applies ANSI parser actions to the terminal
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
mod image;
mod row;
mod term_handler;
mod terminal;

pub use buffer::Buffer;
pub use cell::{Cell, CellFlags, Color};
pub use image::{decode_sixel, ImageId, ImageStore, TerminalImage};
pub use row::Row;
pub use term_handler::TerminalHandler;
pub use terminal::{
    MouseButton, MouseEncoding, MouseMode, MouseModifiers, Selection, SelectionMode,
    SelectionPoint, ShellIntegration, Terminal, TerminalModes,
};

use std::ops::{Index, IndexMut};

/// Maximum number of combining marks stored for a single cell.
///
/// PTY output can attach an unbounded run of zero-width marks to one cell;
/// anything beyond this many is dropped.
pub const MAX_COMBINING_PER_CELL: usize = 8;

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

/// Saved cursor state for DECSC/DECRC.
#[derive(Debug, Clone, Copy)]
struct SavedCursorState {
    position: CursorPosition,
    attrs: Cell,
    auto_wrap: bool,
    origin_mode: bool,
}

/// Terminal cursor state.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub position: CursorPosition,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
    /// Saved cursor state for ESC 7 / ESC 8 (position + attributes)
    saved: Option<SavedCursorState>,
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
    /// Save cursor position only (used internally; prefer Grid::save_cursor for full DECSC).
    pub fn save(&mut self) {
        self.saved = Some(SavedCursorState {
            position: self.position,
            attrs: Cell::default(),
            auto_wrap: true,
            origin_mode: false,
        });
    }

    /// Restore cursor position only (used internally; prefer Grid::restore_cursor for full DECRC).
    pub fn restore(&mut self) {
        if let Some(state) = self.saved {
            self.position = state.position;
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
    /// Insert/Replace mode (IRM) — when true, characters insert rather than overwrite
    insert_mode: bool,
    /// Last printed character (for REP — CSI b)
    last_char: char,
    /// Incremented every time cell content changes (writes, scrolls, erases,
    /// resizes). Pure cursor moves do not bump it. Used to invalidate
    /// selections that refer to content which no longer exists.
    content_version: u64,
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
            insert_mode: false,
            last_char: ' ',
            content_version: 0,
        }
    }

    /// Get grid dimensions.
    pub fn dimensions(&self) -> Dimensions {
        self.dims
    }

    /// Version counter that changes whenever cell content is mutated.
    ///
    /// Cursor movement and viewport scrolling leave it untouched.
    pub fn content_version(&self) -> u64 {
        self.content_version
    }

    /// Get cursor state.
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    /// Get mutable cursor state.
    pub fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// Save cursor state (DECSC): position, text attributes, auto-wrap, and origin mode.
    pub fn save_cursor(&mut self, origin_mode: bool) {
        self.cursor.saved = Some(SavedCursorState {
            position: self.cursor.position,
            attrs: self.attrs,
            auto_wrap: self.auto_wrap,
            origin_mode,
        });
    }

    /// Restore cursor state (DECRC): position, text attributes, auto-wrap, and origin mode.
    /// Returns the saved origin_mode value if a state was restored, so the caller
    /// can update TerminalModes accordingly.
    pub fn restore_cursor(&mut self) -> Option<bool> {
        if let Some(state) = self.cursor.saved {
            // The grid may have shrunk since DECSC; never restore out of bounds
            self.cursor.position = CursorPosition::new(
                state.position.col.min(self.dims.cols.saturating_sub(1)),
                state.position.row.min(self.dims.rows.saturating_sub(1)),
            );
            self.cursor.wrap_pending = false;
            self.attrs = state.attrs;
            self.auto_wrap = state.auto_wrap;
            Some(state.origin_mode)
        } else {
            None
        }
    }

    /// Set auto-wrap mode (DECAWM).
    pub fn set_auto_wrap(&mut self, enabled: bool) {
        self.auto_wrap = enabled;
    }

    /// Set insert/replace mode (IRM).
    pub fn set_insert_mode(&mut self, enabled: bool) {
        self.insert_mode = enabled;
    }

    /// Get insert/replace mode (IRM).
    pub fn insert_mode(&self) -> bool {
        self.insert_mode
    }

    /// Get auto-wrap mode (DECAWM).
    pub fn auto_wrap(&self) -> bool {
        self.auto_wrap
    }

    /// Fill entire grid with a character (used by DECALN — ESC # 8).
    pub fn fill_with_char(&mut self, c: char) {
        let cols = self.dims.cols;
        let rows = self.dims.rows;
        for row_idx in 0..rows {
            if let Some(row) = self.row_mut(row_idx) {
                for col in 0..cols {
                    row[col].set_char(c);
                }
                row.mark_dirty();
            }
        }
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

        // Add to the combining map, dropping marks beyond the per-cell cap
        let marks = self.combining.entry((row, col)).or_default();
        if marks.len() < MAX_COMBINING_PER_CELL {
            marks.push(c);
        }
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

    /// Clear combining characters for a range of cells in one row.
    fn clear_combining_range(&mut self, row: u16, start_col: u16, end_col: u16) {
        self.combining
            .retain(|&(r, c), _| r != row || c < start_col || c >= end_col);
        if let Some(row_cells) = self.rows.get_mut(row as usize) {
            for col in start_col..end_col {
                if let Some(cell) = row_cells.get_mut(col) {
                    cell.unset_flag(CellFlags::HAS_COMBINING);
                }
            }
        }
        self.mark_dirty(row);
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
            self.wrap_pending_cursor();
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
            self.last_char = ' ';
            self.advance_cursor(1);
            return;
        }

        // Check if wide char would wrap (needs 2 columns but only 1 left)
        if width > 1 && col + width > self.dims.cols {
            // Wrap to next line before writing, respecting scroll region
            self.wrap_pending_cursor();
        }

        let col = self.cursor.position.col;
        let row = self.cursor.position.row;

        // Copy attrs first to avoid borrow conflict
        let attrs = self.attrs;

        // Clear any combining characters from previous content
        self.clear_combining(row, col);

        // Insert mode (IRM): shift existing cells right before writing
        if self.insert_mode {
            let bg = attrs.bg();
            if let Some(r) = self.row_mut(row) {
                r.insert_cells(col, width, bg);
            }
        }

        // Clean up orphaned wide-char parts before overwriting
        let is_wide = self.cell(col, row).is_some_and(|c| c.is_wide());
        let is_spacer = self.cell(col, row).is_some_and(|c| c.is_wide_spacer());
        if is_wide && col + 1 < self.dims.cols {
            if let Some(spacer) = self.cell_mut(col + 1, row) {
                spacer.clear();
            }
        }
        if is_spacer && col > 0 {
            if let Some(primary) = self.cell_mut(col - 1, row) {
                primary.clear();
            }
        }

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

        // Track last printed char for REP (CSI b)
        self.last_char = c;

        self.advance_cursor(width);
    }

    /// Advance the cursor after printing `width` columns, parking it on the
    /// last column with a pending wrap when it would run off the row.
    fn advance_cursor(&mut self, width: u16) {
        self.cursor.position.col = self.cursor.position.col.saturating_add(width);

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

    /// Write a contiguous printable ASCII span at the cursor position.
    pub fn write_ascii_span(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        if self.insert_mode || !self.auto_wrap {
            for &byte in bytes {
                self.write_char(byte as char);
            }
            return;
        }

        let mut remaining = bytes;
        while !remaining.is_empty() {
            if self.cursor.wrap_pending {
                self.wrap_pending_cursor();
            }

            let row = self.cursor.position.row;
            let col = self.cursor.position.col;
            if row >= self.dims.rows || self.dims.cols == 0 {
                return;
            }

            let available = self.dims.cols.saturating_sub(col).max(1) as usize;
            let write_len = available.min(remaining.len());
            let end_col = col + write_len as u16;
            let attrs = self.attrs;

            self.clear_combining_range(row, col, end_col);
            self.clear_wide_orphans_for_ascii_span(row, col, end_col);

            if let Some(row_cells) = self.rows.get_mut(row as usize) {
                for (offset, &byte) in remaining[..write_len].iter().enumerate() {
                    if let Some(cell) = row_cells.get_mut(col + offset as u16) {
                        cell.set_char(byte as char);
                        cell.copy_attrs_from(&attrs);
                        cell.unset_flag(CellFlags::HAS_COMBINING);
                        cell.unset_flag(CellFlags::WIDE_CHAR);
                        cell.unset_flag(CellFlags::WIDE_CHAR_SPACER);
                    }
                }
            }
            self.mark_dirty(row);

            self.last_char = remaining[write_len - 1] as char;
            remaining = &remaining[write_len..];

            if end_col >= self.dims.cols {
                self.cursor.position.col = self.dims.cols.saturating_sub(1);
                self.cursor.wrap_pending = true;
            } else {
                self.cursor.position.col = end_col;
            }
        }
    }

    /// Perform a deferred auto-wrap: mark the current row soft-wrapped and
    /// move the cursor to column 0 of the next line (scrolling if needed).
    fn wrap_pending_cursor(&mut self) {
        let row = self.cursor.position.row;
        if let Some(r) = self.rows.get_mut(row as usize) {
            r.set_wrapped(true);
        }

        self.cursor.position.col = 0;
        self.line_feed();
        self.cursor.wrap_pending = false;
    }

    /// Move the cursor down one line, scrolling the region when the cursor
    /// sits on its bottom margin (IND semantics).
    ///
    /// A cursor outside the scroll region moves down without scrolling and
    /// stops at the last screen row, matching `cursor_down`.
    fn line_feed(&mut self) {
        let region = self.effective_scroll_region();
        let row = self.cursor.position.row;
        if row == region.bottom {
            self.scroll_up(1);
        } else if row < region.bottom {
            self.cursor.position.row = row + 1;
        } else {
            self.cursor.position.row = (row + 1).min(self.dims.rows.saturating_sub(1));
        }
    }

    fn clear_wide_orphans_for_ascii_span(&mut self, row: u16, start_col: u16, end_col: u16) {
        if start_col > 0
            && self
                .cell(start_col, row)
                .is_some_and(|cell| cell.is_wide_spacer())
        {
            if let Some(primary) = self.cell_mut(start_col - 1, row) {
                primary.clear();
            }
        }

        if end_col < self.dims.cols
            && self
                .cell(end_col, row)
                .is_some_and(|cell| cell.is_wide_spacer())
        {
            if let Some(spacer) = self.cell_mut(end_col, row) {
                spacer.clear();
            }
        }

        for col in start_col..end_col {
            let is_wide = self.cell(col, row).is_some_and(|cell| cell.is_wide());
            let is_spacer = self
                .cell(col, row)
                .is_some_and(|cell| cell.is_wide_spacer());
            if is_wide && col + 1 < self.dims.cols {
                if let Some(spacer) = self.cell_mut(col + 1, row) {
                    spacer.clear();
                }
            }
            if is_spacer && col > 0 {
                if let Some(primary) = self.cell_mut(col - 1, row) {
                    primary.clear();
                }
            }
        }
    }

    /// Move cursor up, stopping at scroll region top if inside region.
    pub fn cursor_up(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        let region = self.effective_scroll_region();
        let top = if self.cursor.position.row >= region.top
            && self.cursor.position.row <= region.bottom
        {
            region.top
        } else {
            0
        };
        self.cursor.position.row = self.cursor.position.row.saturating_sub(n).max(top);
    }

    /// Move cursor down, stopping at scroll region bottom if inside region.
    pub fn cursor_down(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        let region = self.effective_scroll_region();
        let bottom = if self.cursor.position.row >= region.top
            && self.cursor.position.row <= region.bottom
        {
            region.bottom
        } else {
            self.dims.rows.saturating_sub(1)
        };
        self.cursor.position.row = (self.cursor.position.row + n).min(bottom);
    }

    /// Move cursor left, stopping at left edge.
    pub fn cursor_left(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.col = self.cursor.position.col.saturating_sub(n);
    }

    /// Move cursor right, stopping at right edge.
    pub fn cursor_right(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.col =
            (self.cursor.position.col + n).min(self.dims.cols.saturating_sub(1));
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
        self.line_feed();
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

    /// Repeat the last printed character N times (REP — CSI b).
    pub fn repeat_last_char(&mut self, n: u16) {
        let c = self.last_char;
        for _ in 0..n {
            self.write_char(c);
        }
    }

    /// Move cursor forward N tab stops (CHT — CSI I).
    pub fn forward_tab(&mut self, n: u16) {
        for _ in 0..n {
            self.tab();
        }
    }

    /// Move cursor backward N tab stops (CBT — CSI Z).
    pub fn backward_tab(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        let mut current = self.cursor.position.col;
        for _ in 0..n {
            if let Some(&prev) = self.tab_stops.range(..current).next_back() {
                current = prev;
            } else {
                current = 0;
                break;
            }
        }
        self.cursor.position.col = current;
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
        // 0 means "default": top defaults to 1, bottom defaults to screen height
        let top = if top == 0 { 1 } else { top };
        let bottom = if bottom == 0 { self.dims.rows } else { bottom };
        // Convert from 1-based to 0-based
        let top = top.saturating_sub(1);
        let bottom = bottom
            .saturating_sub(1)
            .min(self.dims.rows.saturating_sub(1));

        if top >= bottom {
            // Invalid region, reset to full screen
            self.scroll_region = None;
        } else if top == 0 && bottom >= self.dims.rows.saturating_sub(1) {
            // Full screen, no need to track
            self.scroll_region = None;
        } else {
            self.scroll_region = Some(ScrollRegion::new(top, bottom));
        }

        // DECSTBM homes cursor to absolute (0,0)
        self.cursor.position.row = 0;
        self.cursor.position.col = 0;
    }

    /// Get the current scroll region.
    pub fn scroll_region(&self) -> Option<ScrollRegion> {
        self.scroll_region
    }

    /// Get the effective scroll region (returns full screen if none set).
    ///
    /// The result is always clamped to the current grid height so callers can
    /// index rows with it even if a stale region outlives a resize.
    pub fn effective_scroll_region(&self) -> ScrollRegion {
        let last_row = self.dims.rows.saturating_sub(1);
        match self.scroll_region {
            Some(region) => {
                let bottom = region.bottom.min(last_row);
                let top = region.top.min(bottom);
                ScrollRegion::new(top, bottom)
            }
            None => ScrollRegion::new(0, last_row),
        }
    }

    /// Scroll the grid up by n lines (within scroll region).
    pub fn scroll_up(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let bg = self.attrs.bg();
        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(region.top) + 1) as usize;

        // Clear combining chars for the entire scroll region
        self.clear_combining_rows(region.top, region.bottom);

        if n >= region_height {
            // Clear the entire scroll region
            for row_idx in region.top..=region.bottom {
                if let Some(row) = self.rows.get_mut(row_idx as usize) {
                    row.erase(bg);
                }
                self.mark_dirty(row_idx);
            }
            return;
        }

        let start = region.top as usize;
        let end = region.bottom as usize + 1;
        self.rows[start..end].rotate_left(n);

        // Erase the bottom n rows of the region with current bg
        for i in 0..n {
            let row_idx = region.bottom as usize - i;
            self.rows[row_idx].erase(bg);
        }

        // Mark affected rows dirty
        for row_idx in region.top..=region.bottom {
            self.mark_dirty(row_idx);
        }
    }

    /// Scroll the grid down by n lines (within scroll region).
    pub fn scroll_down(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let bg = self.attrs.bg();
        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(region.top) + 1) as usize;

        // Clear combining chars for the entire scroll region
        self.clear_combining_rows(region.top, region.bottom);

        if n >= region_height {
            // Clear the entire scroll region
            for row_idx in region.top..=region.bottom {
                if let Some(row) = self.rows.get_mut(row_idx as usize) {
                    row.erase(bg);
                }
                self.mark_dirty(row_idx);
            }
            return;
        }

        let start = region.top as usize;
        let end = region.bottom as usize + 1;
        self.rows[start..end].rotate_right(n);

        // Erase the top n rows of the region with current bg
        for i in 0..n {
            let row_idx = region.top as usize + i;
            self.rows[row_idx].erase(bg);
        }

        // Mark affected rows dirty
        for row_idx in region.top..=region.bottom {
            self.mark_dirty(row_idx);
        }
    }

    /// Insert n blank lines at the cursor position (IL).
    /// Content from cursor row to bottom of scroll region shifts down.
    pub fn insert_lines(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        let region = self.effective_scroll_region();
        let bg = self.attrs.bg();
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
                    row.erase(bg);
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

        // Erase the n rows at cursor position with current bg
        for i in 0..n {
            let row_idx = cursor_row as usize + i;
            if row_idx <= region.bottom as usize {
                let mut row = Row::new(self.dims.cols);
                row.erase(bg);
                self.rows[row_idx] = row;
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
        self.cursor.wrap_pending = false;
        let region = self.effective_scroll_region();
        let bg = self.attrs.bg();
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
                    row.erase(bg);
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

        // Erase the bottom n rows of the region with current bg
        for i in 0..n {
            let row_idx = region.bottom as usize - i;
            if row_idx >= cursor_row as usize {
                let mut row = Row::new(self.dims.cols);
                row.erase(bg);
                self.rows[row_idx] = row;
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
        let bg = self.attrs.bg();
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
                    cell.erase(bg);
                }
            }
        }
        self.mark_dirty(r);

        // Erase all lines below cursor
        for row_idx in (r + 1)..self.dims.rows {
            if let Some(row) = self.rows.get_mut(row_idx as usize) {
                row.erase(bg);
            }
            self.mark_dirty(row_idx);
        }
    }

    /// Erase from start of display to cursor.
    pub fn erase_above(&mut self) {
        let r = self.cursor.position.row;
        let bg = self.attrs.bg();
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
                row.erase(bg);
            }
            self.mark_dirty(row_idx);
        }

        // Erase from start of current line to cursor
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in 0..=self.cursor.position.col {
                if let Some(cell) = row.get_mut(i) {
                    cell.erase(bg);
                }
            }
        }
        self.mark_dirty(r);
    }

    /// Erase entire display.
    pub fn clear(&mut self) {
        let bg = self.attrs.bg();
        self.combining.clear();
        for row in &mut self.rows {
            row.erase(bg);
        }
        self.mark_all_dirty();
    }

    /// Erase from cursor to end of line.
    pub fn erase_line_right(&mut self) {
        let r = self.cursor.position.row;
        let bg = self.attrs.bg();
        // Clear combining chars for erased cells on this row
        for col in self.cursor.position.col..self.dims.cols {
            self.combining.remove(&(r, col));
        }
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in self.cursor.position.col..self.dims.cols {
                if let Some(cell) = row.get_mut(i) {
                    cell.erase(bg);
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
        let bg = self.attrs.bg();
        // Clear combining chars for erased cells on this row
        for col in 0..=self.cursor.position.col {
            self.combining.remove(&(r, col));
        }
        if let Some(row) = self.rows.get_mut(r as usize) {
            for i in 0..=self.cursor.position.col {
                if let Some(cell) = row.get_mut(i) {
                    cell.erase(bg);
                }
            }
        }
        self.mark_dirty(r);
    }

    /// Erase entire current line.
    pub fn erase_line(&mut self) {
        let r = self.cursor.position.row;
        let bg = self.attrs.bg();
        self.clear_combining_rows(r, r);
        if let Some(row) = self.rows.get_mut(r as usize) {
            row.erase(bg);
        }
        self.mark_dirty(r);
    }

    /// Resize the grid without reflow: rows are truncated or padded in place.
    ///
    /// Width-changing resizes with reflow go through [`Buffer::resize`], which
    /// re-wraps history and grid together and hands the result to
    /// [`Grid::set_rows`].
    pub fn resize(&mut self, new_dims: Dimensions) {
        if new_dims == self.dims {
            return;
        }

        let new_cols = new_dims.cols;

        if new_cols != self.dims.cols {
            // Cell coordinates beyond the new width are gone
            self.combining.retain(|&(_, col), _| col < new_cols);
            for row in &mut self.rows {
                row.resize(new_cols);
            }
        }

        // Add or remove rows
        while self.rows.len() < new_dims.rows as usize {
            self.rows.push(Row::new(new_cols));
        }
        self.rows.truncate(new_dims.rows as usize);
        self.combining.retain(|&(row, _), _| row < new_dims.rows);

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

        // Update tab stops for new width
        let old_cols = self.dims.cols;
        if new_dims.cols > old_cols {
            // Extend tab stops to cover any new columns
            for col in (old_cols..new_dims.cols).filter(|c| c % 8 == 0) {
                self.tab_stops.insert(col);
            }
        } else if new_dims.cols < old_cols {
            // Remove tab stops beyond new width
            self.tab_stops.retain(|&col| col < new_dims.cols);
        }

        self.dims = new_dims;
        self.dirty = vec![true; new_dims.rows as usize];
        self.content_version = self.content_version.wrapping_add(1);
    }

    /// Set rows directly (used by Buffer during reflow).
    pub fn set_rows(&mut self, rows: Vec<Row>, new_dims: Dimensions) {
        self.rows = rows;
        self.dims = new_dims;

        // Row indices changed, so any scroll region is meaningless now.
        // Apps that need one re-set it after SIGWINCH (xterm does the same).
        self.scroll_region = None;
        // Combining marks are keyed by (row, col) which the reflow invalidated
        self.combining.clear();

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
        self.content_version = self.content_version.wrapping_add(1);
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
        self.content_version = self.content_version.wrapping_add(1);
        if let Some(d) = self.dirty.get_mut(row as usize) {
            *d = true;
        }
    }

    /// Mark all rows as dirty.
    fn mark_all_dirty(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
        self.dirty.fill(true);
    }

    /// Check if a row is dirty.
    pub fn is_dirty(&self, row: u16) -> bool {
        self.dirty.get(row as usize).copied().unwrap_or(false)
    }

    /// Clear dirty flags after rendering.
    pub fn clear_dirty(&mut self) {
        self.dirty.fill(false);
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
    fn write_ascii_span_advances_cursor() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.write_ascii_span(b"Hello");

        assert_eq!(grid.row(0).unwrap().text_trimmed(), "Hello");
        assert_eq!(grid.cursor().position.col, 5);
    }

    #[test]
    fn write_ascii_span_wraps_across_rows() {
        let mut grid = Grid::new(Dimensions::new(5, 3));
        grid.write_ascii_span(b"abcdef");

        assert_eq!(grid.row(0).unwrap().text(), "abcde");
        assert!(grid.row(0).unwrap().is_wrapped());
        assert_eq!(grid.row(1).unwrap().text_trimmed(), "f");
        assert_eq!(grid.cursor().position.row, 1);
        assert_eq!(grid.cursor().position.col, 1);
    }

    #[test]
    fn write_ascii_span_insert_mode_falls_back_to_character_semantics() {
        let mut grid = Grid::new(Dimensions::new(6, 2));
        grid.write_ascii_span(b"ABC");
        grid.set_cursor_position(1, 1);
        grid.set_insert_mode(true);

        grid.write_ascii_span(b"XY");

        assert_eq!(grid.row(0).unwrap().text(), "XYABC ");
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
    fn resize_narrower_truncates_rows_without_reflow() {
        let mut grid = Grid::new(Dimensions::new(20, 5));
        for c in "Hello World!!!!".chars() {
            grid.write_char(c);
        }

        grid.resize(Dimensions::new(10, 5));

        assert_eq!(grid.row(0).unwrap().width(), 10);
        assert_eq!(grid.row(0).unwrap().text_trimmed(), "Hello Worl");
        assert_eq!(grid.row(1).unwrap().text_trimmed(), "");
        assert!(!grid.row(0).unwrap().is_wrapped());
    }

    #[test]
    fn resize_wider_pads_rows() {
        let mut grid = Grid::new(Dimensions::new(5, 2));
        grid.write_char('A');

        grid.resize(Dimensions::new(8, 3));

        assert_eq!(grid.row(0).unwrap().width(), 8);
        assert_eq!(grid.row(2).unwrap().width(), 8);
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert_eq!(grid.cell(7, 2).unwrap().char(), ' ');
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
        grid.save_cursor(false);

        grid.set_cursor_position(1, 1);
        grid.restore_cursor();

        assert_eq!(grid.cursor().position.row, 9);
        assert_eq!(grid.cursor().position.col, 19);
    }

    #[test]
    fn restore_cursor_clamps_to_current_dimensions() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        grid.set_cursor_position(5, 9);
        grid.save_cursor(false);

        let rows: Vec<Row> = (0..2).map(|_| Row::new(4)).collect();
        grid.set_rows(rows, Dimensions::new(4, 2));
        grid.restore_cursor();

        assert_eq!(grid.cursor().position.row, 1);
        assert_eq!(grid.cursor().position.col, 3);
    }

    #[test]
    fn save_restore_cursor_preserves_attrs() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.set_cursor_position(5, 10);
        grid.attrs_mut().set_flag(CellFlags::BOLD);
        grid.attrs_mut().set_flag(CellFlags::ITALIC);
        grid.save_cursor(false);

        // Change position and attrs
        grid.set_cursor_position(1, 1);
        grid.reset_attrs();

        assert!(!grid.attrs().has_flag(CellFlags::BOLD));

        grid.restore_cursor();

        assert_eq!(grid.cursor().position.row, 4);
        assert_eq!(grid.cursor().position.col, 9);
        assert!(grid.attrs().has_flag(CellFlags::BOLD));
        assert!(grid.attrs().has_flag(CellFlags::ITALIC));
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
    fn resize_shrink_removes_tab_stops_beyond_width() {
        let mut grid = Grid::new(Dimensions::new(80, 5));
        // Default: tabs at 0, 8, 16, ..., 72
        assert!(grid.tab_stops.contains(&72));
        assert!(grid.tab_stops.contains(&40));

        // Shrink to 20 columns
        grid.resize(Dimensions::new(20, 5));
        // Tab stops beyond col 20 should be removed
        assert!(grid.tab_stops.contains(&0));
        assert!(grid.tab_stops.contains(&8));
        assert!(grid.tab_stops.contains(&16));
        assert!(!grid.tab_stops.contains(&24));
        assert!(!grid.tab_stops.contains(&40));
        assert!(!grid.tab_stops.contains(&72));
    }

    #[test]
    fn backward_tab_moves_to_previous_stop() {
        let mut grid = Grid::new(Dimensions::new(80, 5));
        grid.cursor.position.col = 20;
        grid.backward_tab(1);
        assert_eq!(grid.cursor.position.col, 16);
    }

    #[test]
    fn backward_tab_multiple_stops() {
        let mut grid = Grid::new(Dimensions::new(80, 5));
        grid.cursor.position.col = 25;
        // 25 → 24 → 16 → 8
        grid.backward_tab(3);
        assert_eq!(grid.cursor.position.col, 8);
    }

    #[test]
    fn backward_tab_at_start_stays_at_zero() {
        let mut grid = Grid::new(Dimensions::new(80, 5));
        grid.cursor.position.col = 3;
        // Only one tab stop (0) before col 3
        grid.backward_tab(5);
        assert_eq!(grid.cursor.position.col, 0);
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
    fn newline_below_scroll_region_moves_to_screen_bottom_without_scrolling() {
        let mut grid = Grid::new(Dimensions::new(10, 24));
        for row in 0..10u16 {
            grid.set_cursor_position(row + 1, 1);
            grid.write_char((b'a' + row as u8) as char);
        }
        // DECSTBM 1..10 (rows 0..9), then move below the region.
        grid.set_scroll_region(1, 10);
        grid.set_cursor_position(20, 1);
        assert_eq!(grid.cursor().position.row, 19);

        grid.newline();
        grid.newline();
        grid.newline();
        assert_eq!(grid.cursor().position.row, 22);

        // Further newlines clamp at the last screen row and never scroll.
        grid.newline();
        grid.newline();
        assert_eq!(grid.cursor().position.row, 23);

        for row in 0..10u16 {
            assert_eq!(
                grid.cell(0, row).unwrap().char(),
                (b'a' + row as u8) as char,
                "row {row} inside the region must be untouched"
            );
        }
    }

    #[test]
    fn autowrap_below_scroll_region_does_not_scroll_region() {
        let mut grid = Grid::new(Dimensions::new(4, 6));
        grid.set_cursor_position(1, 1);
        grid.write_char('T');
        grid.set_scroll_region(1, 2);
        grid.set_cursor_position(6, 1);
        for c in "abcdefgh".chars() {
            grid.write_char(c);
        }

        // Wrapped past the last screen row: the cursor stays on it and the
        // region above is untouched.
        assert_eq!(grid.cursor().position.row, 5);
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'T');
    }

    #[test]
    fn effective_scroll_region_is_clamped_to_current_height() {
        let mut grid = Grid::new(Dimensions::new(80, 40));
        grid.set_scroll_region(32, 40);
        assert_eq!(grid.effective_scroll_region(), ScrollRegion::new(31, 39));

        // Simulate a reflow-driven height change that bypasses `resize`.
        let rows: Vec<Row> = (0..20).map(|_| Row::new(60)).collect();
        grid.set_rows(rows, Dimensions::new(60, 20));

        assert_eq!(grid.scroll_region(), None);
        assert_eq!(grid.effective_scroll_region(), ScrollRegion::new(0, 19));
        // Exercise the paths that index by region bounds
        grid.scroll_up(1);
        grid.scroll_down(1);
        grid.newline();
    }

    #[test]
    fn wide_char_on_one_column_grid_keeps_cursor_in_bounds() {
        let mut grid = Grid::new(Dimensions::new(1, 1));
        grid.write_char('漢');
        assert_eq!(grid.cursor().position.col, 0);
        assert!(grid.cursor().wrap_pending);
        grid.write_char('字');
        assert_eq!(grid.cursor().position.col, 0);
        assert_eq!(grid.cursor().position.row, 0);
    }

    #[test]
    fn combining_marks_are_capped_per_cell() {
        let mut grid = Grid::new(Dimensions::new(10, 2));
        grid.write_char('e');
        for _ in 0..(MAX_COMBINING_PER_CELL * 4) {
            grid.write_char('\u{0301}');
        }

        let marks = grid.combining_chars(0, 0).unwrap();
        assert_eq!(marks.len(), MAX_COMBINING_PER_CELL);
    }

    #[test]
    fn content_version_changes_on_writes_but_not_cursor_moves() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        let v0 = grid.content_version();

        grid.cursor_down(2);
        grid.cursor_right(3);
        grid.set_cursor_position(1, 1);
        grid.carriage_return();
        assert_eq!(grid.content_version(), v0);

        grid.write_char('x');
        let v1 = grid.content_version();
        assert_ne!(v1, v0);

        grid.scroll_up(1);
        assert_ne!(grid.content_version(), v1);
    }

    #[test]
    fn decstbm_default_bottom_uses_screen_height() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // CSI 5 r — top=5, bottom=0 (default = screen height)
        grid.set_scroll_region(5, 0);
        let region = grid.scroll_region().unwrap();
        assert_eq!(region.top, 4); // 5-1 = 4 (0-based)
        assert_eq!(region.bottom, 23); // 24-1 = 23 (0-based)
    }

    #[test]
    fn decstbm_bare_resets_to_full_screen() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // First set a region
        grid.set_scroll_region(5, 10);
        assert!(grid.scroll_region().is_some());
        // CSI r — top=0, bottom=0 (defaults to full screen)
        grid.set_scroll_region(0, 0);
        assert!(grid.scroll_region().is_none());
        // Cursor should be at home (0,0)
        assert_eq!(grid.cursor().position.row, 0);
        assert_eq!(grid.cursor().position.col, 0);
    }

    #[test]
    fn decstbm_homes_cursor_to_origin() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        grid.set_cursor_position(10, 20);
        grid.set_scroll_region(5, 15);
        // Cursor should be at absolute (0, 0) when DECOM is off
        assert_eq!(grid.cursor().position.row, 0);
        assert_eq!(grid.cursor().position.col, 0);
    }

    #[test]
    fn write_char_clears_orphaned_wide_spacer() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Write a wide char at col 0
        grid.write_char('中');
        assert!(grid.cell(0, 0).unwrap().is_wide());
        assert!(grid.cell(1, 0).unwrap().is_wide_spacer());
        // Overwrite the wide char with a narrow char (1-based: row=1, col=1)
        grid.set_cursor_position(1, 1);
        grid.write_char('A');
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert!(!grid.cell(0, 0).unwrap().is_wide());
        // The spacer at col 1 should be cleared
        assert!(!grid.cell(1, 0).unwrap().is_wide_spacer());
    }

    #[test]
    fn write_char_clears_orphaned_wide_primary() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Write a wide char at col 0
        grid.write_char('中');
        // Overwrite the spacer (col 1) with a narrow char (1-based: row=1, col=2)
        grid.set_cursor_position(1, 2);
        grid.write_char('B');
        assert_eq!(grid.cell(1, 0).unwrap().char(), 'B');
        assert!(!grid.cell(1, 0).unwrap().is_wide_spacer());
        // The primary at col 0 should have WIDE_CHAR cleared
        assert!(!grid.cell(0, 0).unwrap().is_wide());
    }

    #[test]
    fn insert_mode_shifts_cells_right() {
        let mut grid = Grid::new(Dimensions::new(10, 1));
        grid.write_char('A');
        grid.write_char('B');
        grid.write_char('C');
        // Move cursor to col 1 (1-based: row=1, col=2) and enable insert mode
        grid.set_cursor_position(1, 2);
        grid.set_insert_mode(true);
        grid.write_char('X');
        // Should be A X B C
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert_eq!(grid.cell(1, 0).unwrap().char(), 'X');
        assert_eq!(grid.cell(2, 0).unwrap().char(), 'B');
        assert_eq!(grid.cell(3, 0).unwrap().char(), 'C');
    }
}
