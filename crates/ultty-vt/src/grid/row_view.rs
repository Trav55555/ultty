//! A read-only view of one row of a [`Buffer`](super::Buffer).
//!
//! Readers (the renderer, selection, search, the control socket) go through
//! [`RowView`] rather than holding a [`Row`], so the storage behind a row can
//! change without changing them (ADR 0022). Cells come out by value; a caller
//! that truly needs an owned [`Row`] asks for one with [`RowView::to_row`].

use std::borrow::Cow;

use super::cell::Color;
use super::packed::{PackedRow, Style, StyleTable};
use super::{AnchorTag, Cell, CellFlags, Row, RowMarks};

/// One row of a buffer: a screen row, or a scrollback row.
///
/// A screen row is read in place from its packed cells; a scrollback row is
/// decoded when the view is made, so hold a view only as long as needed.
#[derive(Debug)]
pub struct RowView<'a> {
    inner: Inner<'a>,
}

#[derive(Debug)]
enum Inner<'a> {
    /// A screen row and the table its style ids are in.
    Screen {
        row: &'a PackedRow,
        styles: &'a StyleTable,
    },
    Row(Cow<'a, Row>),
}

/// A view of an owned [`Row`] snapshot, for code (and tests) that holds one.
impl<'a> From<&'a Row> for RowView<'a> {
    fn from(row: &'a Row) -> Self {
        Self {
            inner: Inner::Row(Cow::Borrowed(row)),
        }
    }
}

impl<'a> RowView<'a> {
    pub(super) fn owned(row: Row) -> Self {
        Self {
            inner: Inner::Row(Cow::Owned(row)),
        }
    }

    pub(super) fn screen(row: &'a PackedRow, styles: &'a StyleTable) -> Self {
        Self {
            inner: Inner::Screen { row, styles },
        }
    }

    /// The number of columns.
    pub fn width(&self) -> u16 {
        match &self.inner {
            Inner::Screen { row, .. } => row.width(),
            Inner::Row(row) => row.width(),
        }
    }

    /// The cell at `col`, or `None` past the end of the row.
    #[inline]
    pub fn cell(&self, col: u16) -> Option<Cell> {
        match &self.inner {
            Inner::Screen { row, styles } => Some(styles.unpack(row.get(col)?)),
            Inner::Row(row) => row.get(col).copied(),
        }
    }

    /// Every cell, left to right, by value.
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        (0..self.width()).filter_map(|col| self.cell(col))
    }

    /// The character at each column, without the rest of the cell.
    fn chars(&self) -> impl Iterator<Item = char> + '_ {
        let (packed, cells) = match &self.inner {
            Inner::Screen { row, .. } => (row.cells(), &[][..]),
            Inner::Row(row) => (&[][..], row.cells_slice()),
        };
        packed
            .iter()
            .map(|cell| cell.char())
            .chain(cells.iter().map(Cell::char))
    }

    /// Codepoints following the base character of the cluster at `col`, if
    /// the cell holds a multi-codepoint grapheme.
    pub fn grapheme_extra(&self, col: u16) -> Option<&[char]> {
        match &self.inner {
            Inner::Screen { row, .. } => row.grapheme_extra(col),
            Inner::Row(row) => row.grapheme_extra(col),
        }
    }

    /// Whether the row soft-wraps onto the next one.
    pub fn is_wrapped(&self) -> bool {
        match &self.inner {
            Inner::Screen { row, .. } => row.is_wrapped(),
            Inner::Row(row) => row.is_wrapped(),
        }
    }

    /// The row's shell integration marks.
    pub fn marks(&self) -> RowMarks {
        match &self.inner {
            Inner::Screen { row, .. } => row.marks(),
            Inner::Row(row) => row.marks(),
        }
    }

    /// The row's inline surface anchor.
    pub fn anchor(&self) -> AnchorTag {
        match &self.inner {
            Inner::Screen { row, .. } => row.anchor(),
            Inner::Row(row) => row.anchor(),
        }
    }

    /// Whether every cell is a blank (see [`Cell::is_blank`]).
    pub fn is_blank(&self) -> bool {
        self.cells().all(|cell| cell.is_blank())
    }

    /// The row's text, one char per column plus grapheme extras.
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(usize::from(self.width()));
        for (col, c) in self.chars().enumerate() {
            text.push(c);
            if let Some(extra) = self.grapheme_extra(col as u16) {
                text.extend(extra);
            }
        }
        text
    }

    /// [`RowView::text`] with trailing spaces removed.
    pub fn text_trimmed(&self) -> String {
        let mut text = self.text();
        text.truncate(text.trim_end().len());
        text
    }

    /// Text of columns `start..=end` as a user would copy it: each cell's
    /// whole grapheme, with the spacer half of wide characters skipped.
    ///
    /// Unlike [`RowView::text`], the result does not map one char per
    /// column.
    pub fn copy_text(&self, start: u16, end: u16) -> String {
        let mut text = String::new();
        let end = end.min(self.width().saturating_sub(1));
        for col in start..=end {
            let Some(cell) = self.cell(col) else {
                break;
            };
            if cell.is_wide_spacer() {
                continue;
            }
            text.push(cell.char());
            if let Some(extra) = self.grapheme_extra(col) {
                text.extend(extra);
            }
        }
        text
    }

    /// Whether `other` shows the same: the same width, cells and grapheme
    /// clusters. Wrapping, marks and anchor tags are not compared.
    pub fn eq_row(&self, other: &Row) -> bool {
        self.width() == other.width()
            && self.cells().eq(other.cells().copied())
            && self
                .cells()
                .enumerate()
                .filter(|(_, cell)| cell.has_flag(CellFlags::HAS_COMBINING))
                .all(|(col, _)| self.grapheme_extra(col as u16) == other.grapheme_extra(col as u16))
    }

    /// An owned copy of the row.
    pub fn to_row(&self) -> Row {
        match &self.inner {
            Inner::Screen { .. } => {
                let mut row = Row::new(0);
                self.copy_into(&mut row);
                row
            }
            Inner::Row(row) => row.as_ref().clone(),
        }
    }

    /// Make `dst` a copy of the row, reusing `dst`'s allocations.
    pub fn copy_into(&self, dst: &mut Row) {
        match &self.inner {
            Inner::Screen { row, styles } => dst.assign(
                row.cells().iter().map(|&cell| styles.unpack(cell)),
                (0..row.width()).filter_map(|col| Some((col, row.grapheme_extra(col)?))),
                row.is_wrapped(),
                row.marks(),
                row.anchor(),
            ),
            Inner::Row(row) => dst.copy_from(row),
        }
    }
}

/// A screen row being changed, from [`super::Grid::row_mut`]. Cells go in
/// and out by value; styles are interned in the grid's table as they go in.
#[derive(Debug)]
pub struct RowMut<'a> {
    row: &'a mut PackedRow,
    styles: &'a mut StyleTable,
}

impl<'a> RowMut<'a> {
    /// The caller has collected the table if it was due: the handle
    /// interns without collecting.
    pub(super) fn new(row: &'a mut PackedRow, styles: &'a mut StyleTable) -> Self {
        Self { row, styles }
    }

    /// The row as it is now, to read.
    pub fn view(&self) -> RowView<'_> {
        RowView::screen(self.row, self.styles)
    }

    /// The number of columns.
    pub fn width(&self) -> u16 {
        self.row.width()
    }

    /// The cell at `col`, or `None` past the end of the row.
    pub fn cell(&self, col: u16) -> Option<Cell> {
        Some(self.styles.unpack(self.row.get(col)?))
    }

    /// Set the cell at `col`, as a direct edit. Returns whether there is
    /// such a cell.
    pub fn set_cell(&mut self, col: u16, cell: Cell) -> bool {
        self.update(col, |old| *old = cell)
    }

    /// Change the cell at `col` in place with `f`, as a direct edit.
    /// Returns whether there is such a cell.
    pub fn update(&mut self, col: u16, f: impl FnOnce(&mut Cell)) -> bool {
        let Some(mut cell) = self.cell(col) else {
            return false;
        };
        f(&mut cell);
        let packed = self.styles.pack(&cell);
        self.row.set(col, packed);
        true
    }

    /// Whether the row soft-wraps onto the next one.
    pub fn is_wrapped(&self) -> bool {
        self.row.is_wrapped()
    }

    pub fn set_wrapped(&mut self, wrapped: bool) {
        self.row.set_wrapped(wrapped);
    }

    /// The row's shell integration marks.
    pub fn marks(&self) -> RowMarks {
        self.row.marks()
    }

    /// Replace the row's shell integration marks.
    pub fn set_marks(&mut self, marks: RowMarks) {
        self.row.set_marks(marks);
    }

    /// The row's inline surface anchor.
    pub fn anchor(&self) -> AnchorTag {
        self.row.anchor()
    }

    /// Replace the row's inline surface anchor.
    pub fn set_anchor(&mut self, anchor: AnchorTag) {
        self.row.set_anchor(anchor);
    }

    /// Insert `count` blank cells at `at`, on background `bg` (ECMA-48),
    /// shifting cells right.
    pub fn insert_cells(&mut self, at: u16, count: u16, bg: Color) {
        let key = self.styles.key(&Style::erased(bg));
        self.row.insert_cells(at, count, key);
    }

    /// Delete `count` cells at `at`, shifting cells left and filling the
    /// end with blanks on background `bg` (ECMA-48).
    pub fn delete_cells(&mut self, at: u16, count: u16, bg: Color) {
        let key = self.styles.key(&Style::erased(bg));
        self.row.delete_cells(at, count, key);
    }

    /// Make the row a copy of `row`: cells, graphemes, wrap, marks and
    /// anchor.
    pub(super) fn assign(&mut self, row: &Row) {
        self.row.assign_row(row, self.styles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_of(text: &str) -> Row {
        let mut row = Row::new(8);
        row.write_ascii(0, text.as_bytes(), Cell::default());
        row
    }

    #[test]
    fn cells_come_out_by_value_and_stop_at_the_width() {
        let row = row_of("ab");
        let view = RowView::from(&row);
        assert_eq!(view.cell(1).map(|cell| cell.char()), Some('b'));
        assert_eq!(view.cell(8), None);
    }

    #[test]
    fn eq_row_compares_graphemes_but_not_wrapping() {
        let mut row = row_of("e");
        row.set_grapheme_extra(0, &['\u{301}']);
        let mut wrapped = row.clone();
        wrapped.set_wrapped(true);
        assert!(RowView::from(&row).eq_row(&wrapped));

        let mut other = row.clone();
        other.set_grapheme_extra(0, &['\u{300}']);
        assert!(!RowView::owned(row).eq_row(&other));
    }
}
