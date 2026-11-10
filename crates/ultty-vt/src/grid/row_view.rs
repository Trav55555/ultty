//! A read-only view of one row of a [`Buffer`](super::Buffer).
//!
//! Readers (the renderer, selection, search, the control socket) go through
//! [`RowView`] rather than holding a [`Row`], so the storage behind a row can
//! change without changing them (ADR 0022). Cells come out by value; a caller
//! that truly needs an owned [`Row`] asks for one with [`RowView::to_row`].

use std::borrow::Cow;

use super::{AnchorTag, Cell, CellFlags, Row, RowMarks};

/// One row of a buffer: a screen row, or a scrollback row.
///
/// Today a screen row is borrowed and a scrollback row is decoded when the
/// view is made, so hold a view only as long as needed.
#[derive(Debug)]
pub struct RowView<'a> {
    row: Cow<'a, Row>,
}

/// A view of an owned [`Row`] snapshot, for code (and tests) that holds one.
impl<'a> From<&'a Row> for RowView<'a> {
    fn from(row: &'a Row) -> Self {
        Self {
            row: Cow::Borrowed(row),
        }
    }
}

impl<'a> RowView<'a> {
    pub(super) fn owned(row: Row) -> Self {
        Self {
            row: Cow::Owned(row),
        }
    }

    /// The number of columns.
    pub fn width(&self) -> u16 {
        self.row.width()
    }

    /// The cell at `col`, or `None` past the end of the row.
    #[inline]
    pub fn cell(&self, col: u16) -> Option<Cell> {
        self.row.get(col).copied()
    }

    /// Every cell, left to right, by value.
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.row.cells().copied()
    }

    /// Codepoints following the base character of the cluster at `col`, if
    /// the cell holds a multi-codepoint grapheme.
    pub fn grapheme_extra(&self, col: u16) -> Option<&[char]> {
        self.row.grapheme_extra(col)
    }

    /// Whether the row soft-wraps onto the next one.
    pub fn is_wrapped(&self) -> bool {
        self.row.is_wrapped()
    }

    /// The row's shell integration marks.
    pub fn marks(&self) -> RowMarks {
        self.row.marks()
    }

    /// The row's inline surface anchor.
    pub fn anchor(&self) -> AnchorTag {
        self.row.anchor()
    }

    /// The row's text, one char per column plus grapheme extras.
    pub fn text(&self) -> String {
        self.row.text()
    }

    /// [`RowView::text`] with trailing spaces removed.
    pub fn text_trimmed(&self) -> String {
        self.row.text_trimmed()
    }

    /// Text of columns `start..=end` as a user would copy it; see
    /// [`Row::copy_text`].
    pub fn copy_text(&self, start: u16, end: u16) -> String {
        self.row.copy_text(start, end)
    }

    /// Whether `other` shows the same: the same width, cells and grapheme
    /// clusters. Wrapping, marks and anchor tags are not compared.
    pub fn eq_row(&self, other: &Row) -> bool {
        let row: &Row = &self.row;
        row.width() == other.width()
            && row.cells().eq(other.cells())
            && row
                .cells()
                .enumerate()
                .filter(|(_, cell)| cell.has_flag(CellFlags::HAS_COMBINING))
                .all(|(col, _)| row.grapheme_extra(col as u16) == other.grapheme_extra(col as u16))
    }

    /// An owned copy of the row.
    pub fn to_row(&self) -> Row {
        self.row.as_ref().clone()
    }

    /// Make `dst` a copy of the row, reusing `dst`'s allocations.
    pub fn copy_into(&self, dst: &mut Row) {
        dst.copy_from(&self.row);
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
