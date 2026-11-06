//! Compact storage for scrollback rows.
//!
//! A screen row is a `Vec<Cell>` of 20 bytes per column. Scrollback held
//! 10,000 of those, far more than fits in L2, and every new line was
//! written into the oldest one, so saving history stalled on memory and
//! halved VT throughput (docs/perf/2026-11-06-scrollback-cost.md). A
//! [`CompactRow`] keeps the same content as one UTF-8 character per cell
//! plus runs of identical style, without trailing blank cells: about 100
//! bytes for a typical output line instead of one cell per column.
//!
//! Rows are encoded when they scroll into history and decoded when
//! something reads them, which is at most a screenful while scrolled back.

use super::cell::{Cell, CellFlags, Color};
use super::row::Row;

/// Everything about a cell except its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Style {
    fg: Color,
    bg: Color,
    flags: CellFlags,
    hyperlink_id: u32,
}

impl Style {
    fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg(),
            bg: cell.bg(),
            flags: cell.flags(),
            hyperlink_id: cell.hyperlink_id(),
        }
    }

    fn cell(self, c: char) -> Cell {
        let mut cell = Cell::with_attrs(c, self.fg, self.bg, self.flags);
        cell.set_hyperlink_id(self.hyperlink_id);
        cell
    }
}

/// `len` consecutive cells with the same style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    len: u16,
    style: Style,
}

/// A row of scrollback in compact form. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompactRow {
    width: u16,
    wrapped: bool,
    /// One character per stored cell, as UTF-8. Bytes rather than `String`
    /// so ASCII is a plain push; [`CompactRow::decode`] checks it.
    text: Vec<u8>,
    /// Style runs covering the stored cells in order.
    runs: Vec<Run>,
    /// Extra grapheme codepoints by column, for cells that have them.
    graphemes: Vec<(u16, Box<[char]>)>,
}

impl CompactRow {
    /// Encode `row`, reusing this value's allocations.
    ///
    /// This runs for every line that scrolls off the screen, so it is one
    /// pass over the used cells with an inner loop per style run.
    pub(super) fn encode_from(&mut self, row: &Row) {
        self.width = row.width();
        self.wrapped = row.is_wrapped();
        self.text.clear();
        self.runs.clear();
        self.graphemes.clear();

        let cells = row.cell_slice();
        let blank = Cell::default();
        let used = cells
            .iter()
            .rposition(|cell| *cell != blank)
            .map_or(0, |last| last + 1);
        let cells = &cells[..used];
        self.text.reserve(used);

        let mut col = 0;
        while col < cells.len() {
            let style = Style::of(&cells[col]);
            let start = col;
            while col < cells.len() && Style::of(&cells[col]) == style {
                let cell = &cells[col];
                let c = cell.char();
                if c.is_ascii() {
                    self.text.push(c as u8);
                } else {
                    let mut utf8 = [0u8; 4];
                    self.text
                        .extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
                }
                if cell.has_flag(CellFlags::HAS_COMBINING) {
                    if let Some(extra) = row.grapheme_extra(col as u16) {
                        self.graphemes.push((col as u16, extra.into()));
                    }
                }
                col += 1;
            }
            self.runs.push(Run {
                len: (col - start) as u16,
                style,
            });
        }
    }

    /// Bytes this row occupies, including its heap allocations.
    #[cfg(test)]
    pub(super) fn footprint(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.text.capacity()
            + self.runs.capacity() * std::mem::size_of::<Run>()
            + self.graphemes.capacity() * std::mem::size_of::<(u16, Box<[char]>)>()
    }

    /// `row` in compact form.
    pub(super) fn encode(row: &Row) -> Self {
        let mut compact = Self::default();
        compact.encode_from(row);
        compact
    }

    /// The full row this was encoded from.
    pub(super) fn decode(&self) -> Row {
        let mut row = Row::new(self.width);
        let text = std::str::from_utf8(&self.text).unwrap_or_default();
        let mut chars = text.chars();
        let mut col = 0u16;
        for run in &self.runs {
            for _ in 0..run.len {
                let c = chars.next().unwrap_or(' ');
                if col < self.width {
                    row[col] = run.style.cell(c);
                }
                col += 1;
            }
        }
        for (col, extra) in &self.graphemes {
            row.set_grapheme_extra(*col, extra);
        }
        row.set_wrapped(self.wrapped);
        row.clear_dirty();
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Two rows hold the same content.
    fn same(a: &Row, b: &Row) -> bool {
        a.width() == b.width()
            && a.is_wrapped() == b.is_wrapped()
            && a.cells().eq(b.cells())
            && (0..a.width()).all(|col| a.grapheme_extra(col) == b.grapheme_extra(col))
    }

    fn color() -> impl Strategy<Value = Color> {
        prop_oneof![
            Just(Color::Default),
            (0u8..16).prop_map(Color::Named),
            any::<u8>().prop_map(Color::Indexed),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::rgb(r, g, b)),
        ]
    }

    /// A row of plausible cells: mostly default, some styled, some with
    /// multi-byte characters and grapheme extras.
    fn row() -> impl Strategy<Value = Row> {
        let cell = (
            prop_oneof![
                Just(' '),
                Just('a'),
                Just('Z'),
                Just('é'),
                Just('中'),
                Just('😀')
            ],
            color(),
            color(),
            prop_oneof![Just(0u16), Just(1), Just(1 << 2), Just(1 << 3)],
            prop_oneof![Just(0u32), Just(7)],
            prop::option::of(prop::collection::vec(
                prop_oneof![Just('\u{301}'), Just('\u{200d}')],
                1..3,
            )),
        );
        (1u16..60, any::<bool>(), prop::collection::vec(cell, 0..60)).prop_map(
            |(width, wrapped, cells)| {
                let mut row = Row::new(width);
                for (col, (c, fg, bg, flag_bits, link, extra)) in
                    cells.into_iter().take(width as usize).enumerate()
                {
                    let col = col as u16;
                    let mut flags = CellFlags::NONE;
                    if flag_bits & 1 != 0 {
                        flags.set(CellFlags::BOLD);
                    }
                    if flag_bits & (1 << 2) != 0 {
                        flags.set(CellFlags::ITALIC);
                    }
                    if flag_bits & (1 << 3) != 0 {
                        flags.set(CellFlags::UNDERLINE);
                    }
                    let mut cell = Cell::with_attrs(c, fg, bg, flags);
                    cell.set_hyperlink_id(link);
                    row[col] = cell;
                    if let Some(extra) = extra {
                        row.set_grapheme_extra(col, &extra);
                    }
                }
                row.set_wrapped(wrapped);
                row
            },
        )
    }

    proptest! {
        #[test]
        fn encode_then_decode_returns_the_same_row(row in row()) {
            let decoded = CompactRow::encode(&row).decode();
            prop_assert!(same(&row, &decoded), "{row:?}\n!=\n{decoded:?}");
        }

        #[test]
        fn reusing_a_compact_row_leaves_nothing_behind(a in row(), b in row()) {
            let mut compact = CompactRow::encode(&a);
            compact.encode_from(&b);
            prop_assert_eq!(compact, CompactRow::encode(&b));
        }
    }

    #[test]
    fn a_typical_line_is_small() {
        let mut row = Row::new(120);
        for (col, c) in "the quick brown fox jumps over the lazy dog"
            .chars()
            .enumerate()
        {
            row[col as u16] = Cell::new(c);
        }
        let compact = CompactRow::encode(&row);
        assert_eq!(compact.runs.len(), 1);
        assert_eq!(compact.text.len(), 43);
        assert!(compact.graphemes.is_empty());
    }
}
