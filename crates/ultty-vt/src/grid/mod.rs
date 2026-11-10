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
//! use ultty_vt::grid::{Grid, Dimensions, Terminal};
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
mod char_class;
mod cluster;
mod history;
mod image;
mod palette;
#[cfg(feature = "page-proto")]
#[doc(hidden)]
pub mod proto;
mod row;
mod row_view;
mod term_handler;
mod terminal;

pub use buffer::{Anchor, AnchorRefusal, Buffer, DEFAULT_SCROLLBACK, MAX_ANCHORS};
pub use cell::{Cell, CellFlags, Color, UnderlineStyle};
pub use image::{decode_sixel, ImageId, ImageStore, TerminalImage};
pub use palette::{format_color_spec, Palette, Rgb, ThemeColors};
pub use row::{AnchorTag, Row, RowMarks};
pub use row_view::RowView;
pub use term_handler::TerminalHandler;
pub use terminal::{
    AnchorEvent, LastCommand, MouseButton, MouseEncoding, MouseMode, MouseModifiers, Selection,
    SelectionMode, SelectionPoint, ShellIntegration, Terminal, TerminalModes,
    DEFAULT_TERMINAL_NAME, MAX_ANCHOR_EVENTS, MAX_QUEUED_REFUSALS,
};

use std::ops::{Index, IndexMut};

/// Maximum number of combining marks stored for a single cell.
///
/// PTY output can attach an unbounded run of zero-width marks to one cell;
/// anything beyond this many is dropped.
pub const MAX_COMBINING_PER_CELL: usize = 16;

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
    /// The `content_version` at which each row last changed. Unlike `dirty`,
    /// the renderer never resets it, so a selection can tell whether its own
    /// rows changed rather than whether anything on screen did.
    row_versions: Vec<u64>,
    /// Scroll region (None means full screen)
    scroll_region: Option<ScrollRegion>,
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
    /// Grapheme cluster processing (DEC mode 2027). When on, a codepoint that
    /// continues the previously printed cluster (ZWJ sequences, emoji
    /// modifiers, flags, variation selectors, spacing marks) joins that cell
    /// instead of taking new columns, and the cluster takes the width Unicode
    /// assigns the whole sequence.
    grapheme_clustering: bool,
    /// Where the last printed grapheme starts, and the cursor and content
    /// state right after printing it. A following codepoint may only join
    /// that cluster while both are unchanged.
    last_print: Option<LastPrint>,
}

/// Position of the most recently printed grapheme, see [`Grid::write_char`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LastPrint {
    base_row: u16,
    base_col: u16,
    cursor: CursorPosition,
    wrap_pending: bool,
    content_version: u64,
    /// How the cluster ends, as far as joining it goes.
    tail: cluster::Tail,
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
            row_versions: vec![0; dims.rows as usize],
            scroll_region: None,
            tab_stops,
            auto_wrap: true,
            insert_mode: false,
            last_char: ' ',
            content_version: 0,
            grapheme_clustering: true,
            last_print: None,
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

    /// Whether any row in `rows` changed after `version` (a value previously
    /// read from [`Grid::content_version`]). Rows outside the grid count as
    /// unchanged.
    pub fn rows_changed_since(&self, version: u64, rows: std::ops::RangeInclusive<u16>) -> bool {
        let start = *rows.start() as usize;
        let end = (*rows.end() as usize).min(self.row_versions.len().saturating_sub(1));
        // A u64 counter bumped once per mutation does not wrap in practice.
        start <= end && self.row_versions[start..=end].iter().any(|&v| v > version)
    }

    /// Get cursor state.
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    /// Get mutable cursor state.
    pub fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// Put the saved cursor back to its power-on state (DECSTR): home,
    /// default attributes, autowrap on, origin mode off.
    pub fn reset_saved_cursor(&mut self) {
        self.cursor.saved = Some(SavedCursorState {
            position: CursorPosition::origin(),
            attrs: Cell::default(),
            auto_wrap: true,
            origin_mode: false,
        });
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
                // Start from blank cells so no wide-char, spacer, grapheme,
                // or style flags from earlier content survive the fill.
                row.clear();
                for col in 0..cols {
                    row[col].set_char(c);
                }
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
        if let Some(r) = self.rows.get_mut(row as usize) {
            r.push_grapheme_char(col, c, MAX_COMBINING_PER_CELL);
        }
        self.mark_dirty(row);
    }

    /// Get combining characters for a cell.
    pub fn combining_chars(&self, row: u16, col: u16) -> Option<&[char]> {
        self.rows.get(row as usize)?.grapheme_extra(col)
    }

    /// Get the full grapheme for a cell (base char + combining chars).
    pub fn grapheme(&self, row: u16, col: u16) -> Option<String> {
        self.rows.get(row as usize)?.grapheme(col)
    }

    /// Clear combining characters for a range of cells in one row.
    fn clear_combining_range(&mut self, row: u16, start_col: u16, end_col: u16) {
        if let Some(r) = self.rows.get_mut(row as usize) {
            r.clear_graphemes(start_col, end_col);
        }
        self.mark_dirty(row);
    }

    /// Check the grid's structural invariants: row count and widths match
    /// the dimensions, the cursor and scroll region are in bounds, and every
    /// row is internally consistent (see [`Row::check_integrity`]).
    pub fn check_integrity(&self) -> Result<(), String> {
        let Dimensions { cols, rows } = self.dims;
        if self.rows.len() != rows as usize {
            return Err(format!(
                "grid has {} rows, dimensions say {rows}",
                self.rows.len()
            ));
        }
        if self.dirty.len() != rows as usize {
            return Err(format!(
                "dirty map has {} rows, grid has {rows}",
                self.dirty.len()
            ));
        }
        if self.row_versions.len() != rows as usize {
            return Err(format!(
                "row version map has {} rows, grid has {rows}",
                self.row_versions.len()
            ));
        }
        let pos = self.cursor.position;
        if rows > 0 && pos.row >= rows {
            return Err(format!("cursor row {} outside {rows} rows", pos.row));
        }
        if cols > 0 && pos.col >= cols {
            return Err(format!("cursor col {} outside {cols} cols", pos.col));
        }
        if let Some(region) = self.scroll_region {
            if region.top > region.bottom || region.bottom >= rows {
                return Err(format!("scroll region {region:?} outside {rows} rows"));
            }
        }
        for (index, row) in self.rows.iter().enumerate() {
            if row.width() != cols {
                return Err(format!(
                    "row {index} is {} wide, grid is {cols}",
                    row.width()
                ));
            }
            row.check_integrity(MAX_COMBINING_PER_CELL)
                .map_err(|e| format!("row {index}: {e}"))?;
        }
        Ok(())
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

    /// Whether grapheme cluster processing (mode 2027) is enabled.
    pub fn grapheme_clustering(&self) -> bool {
        self.grapheme_clustering
    }

    /// Enable or disable grapheme cluster processing (mode 2027).
    pub fn set_grapheme_clustering(&mut self, enabled: bool) {
        self.grapheme_clustering = enabled;
        self.last_print = None;
    }

    /// The last printed grapheme, if nothing has moved the cursor or
    /// changed the grid since it was printed.
    fn last_print(&self) -> Option<LastPrint> {
        let last = self.last_print?;
        let unchanged = last.cursor == self.cursor.position
            && last.wrap_pending == self.cursor.wrap_pending
            && last.content_version == self.content_version;
        unchanged.then_some(last)
    }

    /// The cell holding the last printed grapheme, if nothing has moved the
    /// cursor or changed the grid since it was printed.
    fn cluster_base(&self) -> Option<(u16, u16)> {
        self.last_print().map(|last| (last.base_row, last.base_col))
    }

    /// Record that a grapheme starting at `(row, col)` and ending in a
    /// codepoint of `tail` was just printed.
    fn record_print(&mut self, row: u16, col: u16, tail: cluster::Tail) {
        self.last_print = Some(LastPrint {
            base_row: row,
            base_col: col,
            cursor: self.cursor.position,
            wrap_pending: self.cursor.wrap_pending,
            content_version: self.content_version,
            tail,
        });
    }

    /// Whether `c` joins the previously printed grapheme cluster rather than
    /// starting a new one. Such a codepoint never advances to a new cell,
    /// so callers must treat it as zero-width when deciding to wrap.
    pub fn continues_cluster(&self, c: char) -> bool {
        self.continues_cluster_with(c, cluster::CharProps::of(c))
    }

    /// [`Grid::continues_cluster`] for `c` with properties `props`.
    #[inline]
    fn continues_cluster_with(&self, c: char, props: cluster::CharProps) -> bool {
        // Nothing below U+0300 extends a cluster (except CR LF, which is
        // never printed), so the common case never segments.
        if !self.grapheme_clustering || c < '\u{300}' {
            return false;
        }
        let Some(last) = self.last_print() else {
            return false;
        };
        match cluster::joins_tail(last.tail, props.class()) {
            Some(joins) => joins,
            None => self.continues_cluster_slow(last, c),
        }
    }

    /// Whether `c` joins the cluster `last` started, from the whole
    /// cluster rather than how it ends.
    #[inline(never)]
    fn continues_cluster_slow(&self, last: LastPrint, c: char) -> bool {
        use unicode_segmentation::UnicodeSegmentation;

        let (row, col) = (last.base_row, last.base_col);
        let Some(r) = self.rows.get(row as usize) else {
            return false;
        };
        let Some(base) = r.get(col) else {
            return false;
        };
        if let Some(joins) = cluster::joins(base.char(), r.grapheme_extra(col).unwrap_or(&[]), c) {
            return joins;
        }
        r.with_grapheme_str(col, Some(c), |cluster| {
            cluster.graphemes(true).nth(1).is_none()
        })
        .unwrap_or(false)
    }

    /// Append `c` to the cluster at `(row, col)`, widening the cell to two
    /// columns when the sequence now has emoji presentation (VS16, flag
    /// pairs, ZWJ sequences) and there is room on the row.
    fn extend_cluster(&mut self, row: u16, col: u16, c: char) {
        // A codepoint past the cap is dropped, leaving the cluster's end as
        // it was.
        let old_tail = self
            .last_print
            .map_or(cluster::Tail::Other, |last| last.tail);
        let Some(r) = self.rows.get_mut(row as usize) else {
            return;
        };
        let tail = if r.push_grapheme_char(col, c, MAX_COMBINING_PER_CELL) {
            old_tail.then(c)
        } else {
            old_tail
        };
        let was_wide = r.get(col).is_some_and(|cell| cell.is_wide());
        let room = col.saturating_add(1) < self.dims.cols;
        let at_cursor = !self.cursor.wrap_pending && self.cursor.position.col == col + 1;
        let widens = !was_wide
            && room
            && at_cursor
            && r.get(col).is_some_and(|cell| {
                let extra = r.grapheme_extra(col).unwrap_or_default();
                let cluster = std::iter::once(cell.char()).chain(extra.iter().copied());
                unicode_width::char_iter_width(cluster) >= 2
            });

        if widens {
            let next = col + 1;
            // The spacer overwrites whatever was at `next`; a wide char there
            // would leave its own spacer orphaned.
            if self.cell(next, row).is_some_and(|cell| cell.is_wide()) {
                if let Some(orphan) = self.cell_mut(next + 1, row) {
                    orphan.clear();
                }
            }
            self.clear_combining_range(row, next, next + 1);
            let base = self.cell(col, row).copied();
            if let Some(mut base) = base {
                base.set_flag(CellFlags::WIDE_CHAR);
                if let Some(cell) = self.cell_mut(col, row) {
                    *cell = base;
                }
                if let Some(spacer) = self.cell_mut(next, row) {
                    spacer.set_char(' ');
                    spacer.copy_attrs_from(&base);
                    spacer.unset_flag(CellFlags::WIDE_CHAR);
                    spacer.unset_flag(CellFlags::HAS_COMBINING);
                    spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
                }
            }
            self.advance_cursor(1);
        }
        self.mark_dirty(row);
        self.record_print(row, col, tail);
    }

    /// Write a character at the cursor position and advance.
    pub fn write_char(&mut self, c: char) {
        let width = self.print_width(c);
        self.write_measured_char(c, width);
    }

    /// The columns printing `c` takes: `None` when it joins the previous
    /// grapheme cluster, else its width (0 for control characters and
    /// combining marks that start no cluster, 1 normally, 2 for wide).
    pub(super) fn print_width(&self, c: char) -> Option<u16> {
        let props = cluster::CharProps::of(c);
        if self.continues_cluster_with(c, props) {
            None
        } else {
            Some(props.width())
        }
    }

    /// [`Grid::write_char`] with `width` from [`Grid::print_width`] for the
    /// current state, so a caller that needed it first does not pay twice.
    pub(super) fn write_measured_char(&mut self, c: char, width: Option<u16>) {
        if !self.write_in_row(c, width) {
            self.write_measured_char_slow(c, width);
        }
    }

    /// Print `c`, `width` columns from [`Grid::print_width`], when that is
    /// only writing its cells at the cursor: it starts a cluster of one or
    /// two columns that fit on the row, no wrap is pending, insert mode is
    /// off and it overwrites no part of a wide character. Returns `false`,
    /// having changed nothing, otherwise.
    ///
    /// Most printed characters are this case; the result is what
    /// [`Grid::write_measured_char_slow`] would do.
    #[inline]
    pub(super) fn write_in_row(&mut self, c: char, width: Option<u16>) -> bool {
        let Some(width @ 1..=2) = width else {
            return false;
        };
        let CursorPosition { col, row } = self.cursor.position;
        if self.cursor.wrap_pending || self.insert_mode || col + width > self.dims.cols {
            return false;
        }
        let Some(r) = self.rows.get_mut(row as usize) else {
            return false;
        };
        if !r.write_char_in_place(col, c, width, self.attrs) {
            return false;
        }
        self.mark_dirty(row);
        self.last_char = c;
        self.advance_cursor(width);
        self.record_print(row, col, cluster::Tail::start(c));
        true
    }

    /// [`Grid::write_measured_char`] for any character in any state.
    pub(super) fn write_measured_char_slow(&mut self, c: char, width: Option<u16>) {
        let Some(width) = width else {
            if let Some((row, col)) = self.cluster_base() {
                self.extend_cluster(row, col, c);
            }
            return;
        };

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

        // A wide char with too few columns left wraps first, respecting the
        // scroll region. Without autowrap it has nowhere to go and is
        // dropped, as xterm does, rather than split or moved to a new row.
        if width > 1 && col + width > self.dims.cols {
            if !self.auto_wrap {
                return;
            }
            self.wrap_pending_cursor();
        }

        let col = self.cursor.position.col;
        let row = self.cursor.position.row;

        let attrs = self.attrs;
        let cols = self.dims.cols;
        let insert_mode = self.insert_mode;
        // Everything below touches only this row, so it is marked dirty once.
        self.mark_dirty(row);
        let Some(r) = self.rows.get_mut(row as usize) else {
            return;
        };

        // Clear any combining characters from previous content
        r.clear_graphemes(col, col.saturating_add(1));

        // Insert mode (IRM): shift existing cells right before writing
        if insert_mode {
            r.insert_cells(col, width, attrs.bg());
        }

        // Clean up orphaned wide-char parts before overwriting
        let is_wide = r.get(col).is_some_and(|c| c.is_wide());
        let is_spacer = r.get(col).is_some_and(|c| c.is_wide_spacer());
        if is_wide && col + 1 < cols {
            if let Some(spacer) = r.get_mut(col + 1) {
                spacer.clear();
            }
        }
        if is_spacer && col > 0 {
            if let Some(primary) = r.get_mut(col - 1) {
                primary.clear();
            }
        }
        // A wide char's spacer overwrites the next cell; if that cell starts
        // another wide char, that char's own spacer would be orphaned.
        if width > 1 && col + 2 < cols && r.get(col + 1).is_some_and(|c| c.is_wide()) {
            if let Some(orphan) = r.get_mut(col + 2) {
                orphan.clear();
            }
        }

        // Write the character
        if let Some(cell) = r.get_mut(col) {
            let mut written = attrs;
            written.set_char(c);
            written.unset_flag(CellFlags::HAS_COMBINING);
            if width > 1 {
                written.set_flag(CellFlags::WIDE_CHAR);
            } else {
                written.unset_flag(CellFlags::WIDE_CHAR);
            }
            written.unset_flag(CellFlags::WIDE_CHAR_SPACER);
            *cell = written;
        }

        // If wide character, put spacer in next cell
        if width > 1 && col + 1 < cols {
            if let Some(spacer) = r.get_mut(col + 1) {
                let mut written = attrs;
                written.set_char(' ');
                written.set_flag(CellFlags::WIDE_CHAR_SPACER);
                written.unset_flag(CellFlags::WIDE_CHAR);
                *spacer = written;
            }
        }

        // Track last printed char for REP (CSI b)
        self.last_char = c;

        self.advance_cursor(width);
        self.record_print(row, col, cluster::Tail::start(c));
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
            let mut template = self.attrs;
            template.unset_flag(CellFlags::HAS_COMBINING);
            template.unset_flag(CellFlags::WIDE_CHAR);
            template.unset_flag(CellFlags::WIDE_CHAR_SPACER);

            self.clear_wide_orphans_for_ascii_span(row, col, end_col);
            if let Some(row_cells) = self.rows.get_mut(row as usize) {
                row_cells.write_ascii(col, &remaining[..write_len], template);
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
            // Printable ASCII is plain.
            self.record_print(row, end_col - 1, cluster::Tail::Settled);
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

        // Every cell in start_col..end_col is overwritten, so a wide char
        // can only be split at the two edges handled above.
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
        self.cursor.position.row = self.cursor.position.row.saturating_add(n).min(bottom);
    }

    /// Move cursor left, stopping at left edge.
    pub fn cursor_left(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.col = self.cursor.position.col.saturating_sub(n);
    }

    /// Move cursor right, stopping at right edge.
    pub fn cursor_right(&mut self, n: u16) {
        self.cursor.wrap_pending = false;
        self.cursor.position.col = self
            .cursor
            .position
            .col
            .saturating_add(n)
            .min(self.dims.cols.saturating_sub(1));
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

    /// Fit the tab stops to a new width: new columns get the default stop
    /// every 8 columns, as in xterm, and stops beyond the width go. Stops in
    /// the columns both widths share are kept, custom or not.
    fn resize_tab_stops(&mut self, new_cols: u16) {
        let old_cols = self.dims.cols;
        if new_cols > old_cols {
            self.tab_stops
                .extend((old_cols..new_cols).filter(|c| c % 8 == 0));
        } else {
            self.tab_stops.retain(|&col| col < new_cols);
        }
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
            // xterm ignores a region of less than two lines, cursor and all.
            return;
        }
        if top == 0 && bottom >= self.dims.rows.saturating_sub(1) {
            // Full screen, no need to track
            self.scroll_region = None;
        } else {
            self.scroll_region = Some(ScrollRegion::new(top, bottom));
        }

        // DECSTBM homes cursor to absolute (0,0)
        self.cursor.position.row = 0;
        self.cursor.position.col = 0;
    }

    /// Drop the scroll region without moving the cursor (DECSTR, RIS).
    pub fn reset_scroll_region(&mut self) {
        self.scroll_region = None;
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

        self.mark_rows_dirty(region.top..=region.bottom);
    }

    /// Scroll the grid down by n lines (within scroll region).
    pub fn scroll_down(&mut self, n: u16) {
        let region = self.effective_scroll_region();
        let bg = self.attrs.bg();
        let n = n as usize;
        let region_height = (region.bottom.saturating_sub(region.top) + 1) as usize;

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

        self.mark_rows_dirty(region.top..=region.bottom);
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

        self.mark_rows_dirty(cursor_row..=region.bottom);
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

        self.mark_rows_dirty(cursor_row..=region.bottom);
    }

    /// Erase from cursor to end of display.
    pub fn erase_below(&mut self) {
        let r = self.cursor.position.row;
        let bg = self.attrs.bg();
        self.erase_cells(r, self.cursor.position.col, self.dims.cols);

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

        // Erase all lines above cursor
        for row_idx in 0..r {
            if let Some(row) = self.rows.get_mut(row_idx as usize) {
                row.erase(bg);
            }
            self.mark_dirty(row_idx);
        }

        // Erase from start of current line to cursor
        self.erase_cells(r, 0, self.cursor.position.col.saturating_add(1));
    }

    /// Erase cells `start..end` of `row` with the current background.
    ///
    /// A wide char that straddles either boundary is erased whole, so the
    /// row never keeps half of one (xterm behaves the same way).
    pub fn erase_cells(&mut self, row: u16, start: u16, end: u16) {
        let cols = self.dims.cols;
        let end = end.min(cols);
        if start >= end {
            return;
        }
        let mut start = start;
        let mut end = end;
        if start > 0 && self.cell(start, row).is_some_and(|c| c.is_wide_spacer()) {
            start -= 1;
        }
        if end < cols && self.cell(end - 1, row).is_some_and(|c| c.is_wide()) {
            end += 1;
        }
        let bg = self.attrs.bg();
        if let Some(r) = self.rows.get_mut(row as usize) {
            r.erase_range(start, end, bg);
        }
        self.mark_dirty(row);
    }

    /// Erase entire display.
    pub fn clear(&mut self) {
        let bg = self.attrs.bg();
        for row in &mut self.rows {
            row.erase(bg);
        }
        self.mark_all_dirty();
    }

    /// Erase from cursor to end of line.
    pub fn erase_line_right(&mut self) {
        let r = self.cursor.position.row;
        self.erase_cells(r, self.cursor.position.col, self.dims.cols);
        if let Some(row) = self.rows.get_mut(r as usize) {
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
        self.erase_cells(r, 0, self.cursor.position.col.saturating_add(1));
    }

    /// Erase entire current line.
    pub fn erase_line(&mut self) {
        let r = self.cursor.position.row;
        let bg = self.attrs.bg();
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
            for row in &mut self.rows {
                row.resize(new_cols);
            }
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

        self.resize_tab_stops(new_dims.cols);
        self.dims = new_dims;
        self.dirty = vec![true; new_dims.rows as usize];
        self.content_version = self.content_version.wrapping_add(1);
        self.row_versions = vec![self.content_version; new_dims.rows as usize];
    }

    /// Set rows directly (used by Buffer during reflow).
    pub fn set_rows(&mut self, rows: Vec<Row>, new_dims: Dimensions) {
        self.rows = rows;
        self.resize_tab_stops(new_dims.cols);
        self.dims = new_dims;

        // Row indices changed, so any scroll region is meaningless now.
        // Apps that need one re-set it after SIGWINCH (xterm does the same).
        self.scroll_region = None;

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
        self.row_versions = vec![self.content_version; new_dims.rows as usize];
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
        if let Some(v) = self.row_versions.get_mut(row as usize) {
            *v = self.content_version;
        }
        if let Some(d) = self.dirty.get_mut(row as usize) {
            *d = true;
        }
    }

    /// Mark `rows` dirty as one change: they all get the same new version.
    fn mark_rows_dirty(&mut self, rows: std::ops::RangeInclusive<u16>) {
        self.content_version = self.content_version.wrapping_add(1);
        let start = *rows.start() as usize;
        let end = (*rows.end() as usize + 1).min(self.dirty.len());
        if start < end {
            self.row_versions[start..end].fill(self.content_version);
            self.dirty[start..end].fill(true);
        }
    }

    /// A row to rewrite in place: it is redrawn, but the content version
    /// stays (see [`Buffer::replace_row`]). A grapheme printed before can
    /// no longer be joined.
    fn row_mut_in_place(&mut self, index: u16) -> Option<&mut Row> {
        *self.dirty.get_mut(index as usize)? = true;
        self.last_print = None;
        self.rows.get_mut(index as usize)
    }

    /// Mark all rows as dirty.
    fn mark_all_dirty(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
        self.dirty.fill(true);
        self.row_versions.fill(self.content_version);
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
    fn combining_chars_move_with_scrolled_rows() {
        let mut grid = Grid::new(Dimensions::new(10, 5));

        // Write 'e' with combining accent on row 2
        grid.cursor.position.row = 2;
        grid.cursor.position.col = 0;
        grid.write_char('e');
        grid.write_char('\u{0301}');
        assert!(grid.combining_chars(2, 0).is_some());

        // Scroll up - the mark moves with its row instead of being dropped
        grid.scroll_up(1);
        assert_eq!(grid.combining_chars(1, 0), Some(&['\u{0301}'][..]));
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
        assert!(grid.combining_chars(0, 0).is_none());
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

    /// A codepoint dropped at the cap does not end the cluster: what joins
    /// next depends on the last one stored.
    #[test]
    fn a_codepoint_dropped_at_the_cap_leaves_the_cluster_end_alone() {
        let mut grid = Grid::new(Dimensions::new(10, 2));
        grid.write_char('\u{1F600}');
        for _ in 1..MAX_COMBINING_PER_CELL {
            grid.write_char('\u{0301}');
        }
        grid.write_char('\u{200D}');
        // Joins, but there is no room left for it.
        grid.write_char('\u{0302}');
        assert_eq!(
            grid.combining_chars(0, 0).unwrap().len(),
            MAX_COMBINING_PER_CELL
        );
        // The stored cluster ends in ZWJ, so a pictograph joins it (GB11).
        assert!(grid.continues_cluster('\u{1F603}'));
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

    #[derive(Debug, Clone)]
    enum PrintOp {
        Print(char),
        MoveTo(u16, u16),
        InsertMode(bool),
        AutoWrap(bool),
        Bold(bool),
    }

    fn print_op() -> impl proptest::strategy::Strategy<Value = PrintOp> {
        use proptest::prelude::*;
        prop_oneof![
            8 => prop::sample::select(vec![
                'a', ' ', '\u{E9}', '\u{4E2D}', '\u{65E5}', '\u{1F600}', '\u{2764}', '\u{FE0F}',
                '\u{301}', '\u{200D}', '\u{1F1EF}', '\u{1F1F5}', '\u{915}', '\u{94D}',
                '\u{93F}', '\u{1100}', '\u{1161}', '\u{AD}', '\u{17D8}',
            ])
            .prop_map(PrintOp::Print),
            1 => (0u16..8, 0u16..3).prop_map(|(col, row)| PrintOp::MoveTo(col, row)),
            1 => any::<bool>().prop_map(PrintOp::InsertMode),
            1 => any::<bool>().prop_map(PrintOp::AutoWrap),
            1 => any::<bool>().prop_map(PrintOp::Bold),
        ]
    }

    proptest::proptest! {
        /// Printing through the in-place shortcut leaves the grid exactly as
        /// the general path does.
        #[test]
        fn printing_in_place_matches_the_general_path(
            cols in 1u16..8,
            ops in proptest::collection::vec(print_op(), 0..60),
        ) {
            let dims = Dimensions::new(cols, 3);
            let mut fast = Grid::new(dims);
            let mut general = Grid::new(dims);
            for op in ops {
                for (grid, shortcut) in [(&mut fast, true), (&mut general, false)] {
                    match op {
                        PrintOp::Print(c) => {
                            let width = grid.print_width(c);
                            if shortcut {
                                grid.write_measured_char(c, width);
                            } else {
                                grid.write_measured_char_slow(c, width);
                            }
                        }
                        PrintOp::MoveTo(col, row) => grid.set_cursor_position(row + 1, col + 1),
                        PrintOp::InsertMode(on) => grid.set_insert_mode(on),
                        PrintOp::AutoWrap(on) => grid.set_auto_wrap(on),
                        PrintOp::Bold(on) => {
                            if on {
                                grid.attrs_mut().set_flag(CellFlags::BOLD);
                            } else {
                                grid.attrs_mut().unset_flag(CellFlags::BOLD);
                            }
                        }
                    }
                }
                proptest::prop_assert_eq!(fast.cursor.position, general.cursor.position);
                proptest::prop_assert_eq!(fast.cursor.wrap_pending, general.cursor.wrap_pending);
                proptest::prop_assert_eq!(fast.last_char, general.last_char);
                proptest::prop_assert_eq!(fast.last_print, general.last_print);
                for (a, b) in fast.rows.iter().zip(&general.rows) {
                    proptest::prop_assert!(a.cells().eq(b.cells()), "{a:?}\n!=\n{b:?}");
                    for col in 0..cols {
                        proptest::prop_assert_eq!(a.grapheme_extra(col), b.grapheme_extra(col));
                    }
                    proptest::prop_assert_eq!(a.used_cells().len(), b.used_cells().len());
                }
                fast.check_integrity().map_err(proptest::test_runner::TestCaseError::fail)?;
            }
        }
    }
}
