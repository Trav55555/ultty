//! Compact storage for scrollback rows.
//!
//! A screen row holds one 8-byte packed cell per column. Scrollback holds
//! 10,000 rows or more, and every new line was once written into the
//! oldest of them, so saving history stalled on memory
//! (docs/perf/2026-11-06-scrollback-cost.md). A [`CompactRow`] keeps the
//! same content as one UTF-8 character per cell plus runs of identical
//! style, without trailing blank cells: about 100 bytes for a typical
//! output line instead of one cell per column.
//!
//! Rows are encoded when they scroll into history and decoded when
//! something reads them, which is at most a screenful while scrolled back.

use super::cell::CellFlags;
use super::packed::{PackedCell, PackedRow, Style, StyleTable};
use super::row::{AnchorTag, Row, RowMarks};

/// The number of cells up to and including the last one that is not
/// blank in the default style.
///
/// Most rows end in a long blank tail (a short line on a wide screen), so
/// this checks whole blocks of cells from the end and only looks at single
/// cells inside the last block that is not blank.
fn used_len(cells: &[PackedCell]) -> usize {
    const BLOCK: usize = 8;
    let mut end = cells.len();
    while end >= BLOCK
        && cells[end - BLOCK..end]
            .iter()
            .fold(true, |blank, &cell| blank & (cell == PackedCell::BLANK))
    {
        end -= BLOCK;
    }
    cells[..end]
        .iter()
        .rposition(|&cell| cell != PackedCell::BLANK)
        .map_or(0, |last| last + 1)
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
    /// Shell integration marks (ADR 0019); three bytes in padding.
    row_marks: RowMarks,
    /// Inline surface anchor (ADR 0020); two bytes in padding.
    anchor: AnchorTag,
    /// One character per stored cell, as UTF-8, each followed by its extra
    /// grapheme codepoints if it has any. Bytes rather than `String` so
    /// ASCII is a plain push; [`CompactRow::decode`] checks it.
    text: Vec<u8>,
    /// Style runs covering the stored cells in order.
    runs: Vec<Run>,
    /// The columns whose cells are wide, spacers or graphemes, in order,
    /// each with a [`Mark`]. Keeping the codepoints in `text` saves an
    /// allocation per cluster.
    marks: Vec<(u16, Mark)>,
}

/// A cell's structure, which styles leave out: how many extra grapheme
/// codepoints follow its character in the text, and its width bits.
///
/// A wide character followed by its spacer, the way the writers leave
/// them, is [`WIDE_MARK`] on the wide cell alone. Width bits that do not
/// come in such a pair (only direct edits leave them) are stored as they
/// are, and so is a grapheme bit with no codepoints, so encoding is exact
/// for any row.
type Mark = u16;

/// A wide character; the next cell is its spacer.
const WIDE_MARK: Mark = 1 << 15;
/// The width bits are [`RAW_WIDE`] and [`RAW_SPACER`], not implied.
const RAW_MARK: Mark = 1 << 14;
const RAW_WIDE: Mark = 1 << 13;
const RAW_SPACER: Mark = 1 << 12;
/// The grapheme bit with no extra codepoints.
const BARE_GRAPHEME: Mark = 1 << 11;
/// The count of extra codepoints.
const EXTRA_COUNT: Mark = BARE_GRAPHEME - 1;

/// Whether `cells[col]` and `cells[col + 1]` are a wide character and its
/// spacer, with no other width bits.
#[inline]
fn is_pair(cells: &[PackedCell], col: usize) -> bool {
    let wide = cells[col];
    wide.is_wide()
        && !wide.is_spacer()
        && cells
            .get(col + 1)
            .is_some_and(|spacer| spacer.is_spacer() && !spacer.is_wide())
}

impl CompactRow {
    /// Encode `row`, whose styles are in `styles`, reusing this value's
    /// allocations.
    ///
    /// This runs for every line that scrolls off the screen, so it is one
    /// pass over the used cells, finding style runs by comparing keys and
    /// looking each run's style up once.
    pub(super) fn encode_from(&mut self, row: &PackedRow, styles: &StyleTable) {
        self.width = row.width();
        self.wrapped = row.is_wrapped();
        self.row_marks = row.marks();
        self.anchor = row.anchor();
        self.text.clear();
        self.runs.clear();
        self.marks.clear();

        // A uniform row is ASCII in one style, which the row has kept track
        // of: its text is the whole row. In the default style a cell is
        // blank exactly when it holds a space, so trimming the text trims
        // the blank tail; in any other style no cell is blank.
        if row.is_uniform() {
            let cells = row.used_cells();
            self.text.extend(cells.iter().map(|cell| cell.ascii_byte()));
            let key = row.uniform_key();
            let len = if key.is_default() {
                self.text
                    .iter()
                    .rposition(|&b| b != b' ')
                    .map_or(0, |last| last + 1)
            } else {
                self.text.len()
            };
            self.text.truncate(len);
            if len > 0 {
                self.runs.push(Run {
                    len: len as u16,
                    style: styles.style(key),
                });
            }
            return;
        }

        let cells = row.used_cells();
        let cells = &cells[..used_len(cells)];
        self.text.reserve(cells.len());
        let mut col = 0;
        while col < cells.len() {
            let key = cells[col].key();
            let start = col;
            while col < cells.len() && cells[col].key() == key {
                let cell = cells[col];
                self.push_cell(cell);
                col += if cell.has_structure() {
                    self.mark(row, cells, col)
                } else {
                    1
                };
            }
            self.runs.push(Run {
                len: (col - start) as u16,
                style: styles.style(key),
            });
        }
    }

    /// The rest of [`CompactRow::encode_from`] for the cell at `col`, which
    /// has structure bits and whose character is already in the text: its
    /// extra codepoints and its [`Mark`]. Returns how many cells it took:
    /// a wide character takes its spacer with it when they share a style.
    fn mark(&mut self, row: &PackedRow, cells: &[PackedCell], col: usize) -> usize {
        let cell = cells[col];
        let mut mark: Mark = 0;
        if cell.has_grapheme() {
            match row.grapheme_extra(col as u16) {
                Some(extra) if !extra.is_empty() => mark = self.push_extra(extra),
                _ => mark = BARE_GRAPHEME,
            }
        }
        let mut taken = 1;
        if is_pair(cells, col) {
            mark |= WIDE_MARK;
            let spacer = cells[col + 1];
            if spacer.key() == cell.key() && !spacer.has_grapheme() {
                self.push_cell(spacer);
                taken = 2;
            }
        } else if cell.is_wide_part() && !(col > 0 && is_pair(cells, col - 1)) {
            mark |= RAW_MARK;
            if cell.is_wide() {
                mark |= RAW_WIDE;
            }
            if cell.is_spacer() {
                mark |= RAW_SPACER;
            }
        }
        if mark != 0 {
            self.marks.push((col as u16, mark));
        }
        taken
    }

    /// Append a cell's extra codepoints to the text; their [`Mark`].
    fn push_extra(&mut self, extra: &[char]) -> Mark {
        let extra = &extra[..extra.len().min(usize::from(EXTRA_COUNT))];
        for &c in extra {
            self.push_char(c);
        }
        extra.len() as Mark
    }

    /// Append a cell's character to the text. The codepoint came from a
    /// `char`, so it is encoded as it is rather than checked again.
    #[inline]
    fn push_cell(&mut self, cell: PackedCell) {
        let cp = cell.codepoint();
        match cp {
            0..0x80 => self.text.push(cp as u8),
            0x80..0x800 => self
                .text
                .extend_from_slice(&[0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8]),
            0x800..0x10000 => self.text.extend_from_slice(&[
                0xE0 | (cp >> 12) as u8,
                0x80 | (cp >> 6 & 0x3F) as u8,
                0x80 | (cp & 0x3F) as u8,
            ]),
            _ => self.text.extend_from_slice(&[
                0xF0 | (cp >> 18) as u8,
                0x80 | (cp >> 12 & 0x3F) as u8,
                0x80 | (cp >> 6 & 0x3F) as u8,
                0x80 | (cp & 0x3F) as u8,
            ]),
        }
    }

    #[inline]
    fn push_char(&mut self, c: char) {
        if c.is_ascii() {
            self.text.push(c as u8);
        } else {
            let mut utf8 = [0u8; 4];
            self.text
                .extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        }
    }

    /// Bytes this row occupies, including its heap allocations.
    #[cfg(test)]
    pub(super) fn footprint(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.text.capacity()
            + self.runs.capacity() * std::mem::size_of::<Run>()
            + self.marks.capacity() * std::mem::size_of::<(u16, Mark)>()
    }

    /// `row` in compact form.
    #[cfg(test)]
    pub(super) fn encode(row: &Row) -> Self {
        let mut compact = Self::default();
        RowPacker::new().encode(row, &mut compact);
        compact
    }

    /// The row's shell marks, without decoding it.
    pub(super) fn marks(&self) -> RowMarks {
        self.row_marks
    }

    /// The row's anchor tag, without decoding it.
    pub(super) fn anchor(&self) -> AnchorTag {
        self.anchor
    }

    pub(super) fn set_anchor(&mut self, anchor: AnchorTag) {
        self.anchor = anchor;
    }

    /// The full row this was encoded from.
    pub(super) fn decode(&self) -> Row {
        let mut row = Row::new(self.width);
        let text = std::str::from_utf8(&self.text).unwrap_or_default();
        let mut chars = text.chars();
        let mut marks = self.marks.iter().peekable();
        let mut extra = Vec::new();
        let mut col = 0u16;
        let mut spacer = false;
        for run in &self.runs {
            for _ in 0..run.len {
                let c = chars.next().unwrap_or(' ');
                let mut structure = CellFlags::NONE;
                if std::mem::take(&mut spacer) {
                    structure.set(CellFlags::WIDE_CHAR_SPACER);
                }
                let mark = marks.next_if(|&&(at, _)| at == col).map_or(0, |&(_, m)| m);
                if mark & WIDE_MARK != 0 {
                    structure.set(CellFlags::WIDE_CHAR);
                    spacer = true;
                }
                if mark & RAW_WIDE != 0 {
                    structure.set(CellFlags::WIDE_CHAR);
                }
                if mark & RAW_SPACER != 0 {
                    structure.set(CellFlags::WIDE_CHAR_SPACER);
                }
                let len = mark & EXTRA_COUNT;
                if len > 0 || mark & BARE_GRAPHEME != 0 {
                    structure.set(CellFlags::HAS_COMBINING);
                }
                if col < self.width {
                    row[col] = run.style.cell(c, structure);
                }
                if len > 0 {
                    extra.clear();
                    extra.extend(chars.by_ref().take(usize::from(len)));
                    row.set_grapheme_extra(col, &extra);
                }
                col += 1;
            }
        }
        row.set_wrapped(self.wrapped);
        row.set_marks(self.row_marks);
        row.set_anchor(self.anchor);
        row.clear_dirty();
        row
    }
}

/// Encodes owned [`Row`]s, which carry no style ids: each is packed into
/// a scratch row first. Reflow encodes many rows this way, so the scratch
/// row and its style table are reused.
pub(super) struct RowPacker {
    styles: StyleTable,
    row: PackedRow,
}

impl RowPacker {
    pub(super) fn new() -> Self {
        Self {
            styles: StyleTable::new(0),
            row: PackedRow::new(0),
        }
    }

    /// Encode `row` into `compact`, reusing its allocations.
    pub(super) fn encode(&mut self, row: &Row, compact: &mut CompactRow) {
        // Ids only live for one row's encoding, so the table can be emptied
        // whenever it grows.
        if self.styles.is_due() {
            self.styles.reset(0);
        }
        self.row.assign_row(row, &mut self.styles);
        compact.encode_from(&self.row, &self.styles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::cell::{Cell, Color, UnderlineStyle};
    use crate::grid::packed::StyleKey;
    use proptest::prelude::*;

    /// Two rows hold the same content.
    fn same(a: &Row, b: &Row) -> bool {
        a.width() == b.width()
            && a.is_wrapped() == b.is_wrapped()
            && a.marks() == b.marks()
            && a.anchor() == b.anchor()
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
    /// multi-byte characters and grapheme extras, some wide with their
    /// spacers. Some rows also have wide, spacer or combining flags set on
    /// cells directly, which the writer never leaves.
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
            prop_oneof![
                Just(0u16),
                Just(1),
                Just(1 << 2),
                Just(1 << 3),
                Just(1 << 8),
                Just(1 << 9),
                Just(1 << 11)
            ],
            prop_oneof![Just(0u32), Just(7)],
            (
                prop::sample::select(vec![
                    UnderlineStyle::None,
                    UnderlineStyle::Single,
                    UnderlineStyle::Double,
                    UnderlineStyle::Curly,
                    UnderlineStyle::Dotted,
                    UnderlineStyle::Dashed,
                ]),
                color(),
            ),
            prop::option::of(prop::collection::vec(
                prop_oneof![Just('\u{301}'), Just('\u{200d}')],
                1..3,
            )),
            prop::bool::weighted(0.2),
        );
        (
            1u16..60,
            any::<bool>(),
            prop::bool::weighted(0.3),
            (any::<[u8; 3]>(), any::<u16>()),
            prop::collection::vec(cell, 0..60),
        )
            .prop_map(
                |(
                    width,
                    wrapped,
                    raw_flags,
                    ([input_start, output_start, command_end], anchor),
                    cells,
                )| {
                    let mut row = Row::new(width);
                    let mut wide = Vec::new();
                    for (col, (c, fg, bg, flag_bits, link, (ul_style, ul_color), extra, is_wide)) in
                        cells.into_iter().take(width as usize).enumerate()
                    {
                        let col = col as u16;
                        if is_wide {
                            wide.push(col);
                        }
                        let mut flags = CellFlags::NONE;
                        if raw_flags {
                            for (bit, flag) in [
                                (8, CellFlags::WIDE_CHAR),
                                (9, CellFlags::WIDE_CHAR_SPACER),
                                (11, CellFlags::HAS_COMBINING),
                            ] {
                                if flag_bits & (1 << bit) != 0 {
                                    flags.set(flag);
                                }
                            }
                        }
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
                        cell.set_underline_style(ul_style);
                        cell.set_underline_color(ul_color);
                        row[col] = cell;
                        if let Some(extra) = extra {
                            row.set_grapheme_extra(col, &extra);
                        }
                    }
                    // As the writer leaves a wide character: its spacer is a
                    // blank in the same style.
                    for col in wide {
                        if col + 1 < width {
                            row[col].set_flag(CellFlags::WIDE_CHAR);
                            let mut spacer = row[col];
                            spacer.set_char(' ');
                            spacer.unset_flag(CellFlags::WIDE_CHAR);
                            spacer.unset_flag(CellFlags::HAS_COMBINING);
                            spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
                            row[col + 1] = spacer;
                        }
                    }
                    row.set_wrapped(wrapped);
                    row.set_marks(RowMarks {
                        input_start,
                        output_start,
                        command_end,
                    });
                    row.set_anchor(AnchorTag::new(anchor, anchor & 1 != 0));
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

        /// Uniform rows take a shortcut; it must give exactly what
        /// encoding them cell by cell gives.
        #[test]
        fn a_uniform_row_encodes_like_any_other(
            width in 1u16..40,
            style in 0usize..3,
            writes in prop::collection::vec((0u16..40, "[ a-z]{0,40}", any::<bool>()), 0..6),
        ) {
            let mut styles = StyleTable::new(0);
            let mut cell = Cell::default();
            match style {
                0 => {}
                1 => cell.set_fg(Color::GREEN),
                _ => cell.set_bg(Color::rgb(1, 2, 3)),
            }
            let key = styles.pack(&cell).key();
            let mut row = PackedRow::new(width);
            for (col, text, erase) in writes {
                let col = col.min(width - 1);
                if erase {
                    row.erase_range(col, col + text.len() as u16, StyleKey::DEFAULT);
                } else {
                    let len = text.len().min((width - col) as usize);
                    row.write_ascii(col, &text.as_bytes()[..len], key);
                }
            }
            prop_assert!(row.is_uniform() || style != 0, "default-style text stays uniform");
            if row.is_uniform() {
                let mut copy = row.clone();
                copy.set(0, row.get(0).unwrap());
                prop_assert!(!copy.is_uniform());
                let mut compact = CompactRow::default();
                compact.encode_from(&row, &styles);
                let mut general = CompactRow::default();
                general.encode_from(&copy, &styles);
                prop_assert_eq!(&compact, &general);
            }
        }

        #[test]
        fn reusing_a_compact_row_leaves_nothing_behind(a in row(), b in row()) {
            let mut compact = CompactRow::encode(&a);
            RowPacker::new().encode(&b, &mut compact);
            prop_assert_eq!(compact, CompactRow::encode(&b));
        }
    }

    /// Wide characters, their spacers and combining sequences written the
    /// way the grid writes them stay in one style run with the text around
    /// them.
    #[test]
    fn wide_and_combined_cells_do_not_split_style_runs() {
        let mut grid = crate::grid::Grid::new(crate::grid::Dimensions::new(20, 1));
        for c in "日本 e\u{301}語 x".chars() {
            grid.write_char(c);
        }
        let mut compact = CompactRow::default();
        compact.encode_from(&grid.rows[0], &grid.styles);
        assert_eq!(compact.runs.len(), 1);
        assert_eq!(compact.marks.len(), 4, "{:?}", compact.marks);
        assert!(same(&grid.row(0).unwrap().to_row(), &compact.decode()));
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
        assert!(compact.marks.is_empty());
    }

    /// A row of one style and only ASCII base characters takes the
    /// straight-copy path, but combining marks on those characters must
    /// still be kept.
    #[test]
    fn an_all_ascii_row_keeps_its_combining_marks() {
        let mut row = Row::new(3);
        for col in 0..3 {
            row[col] = Cell::new('e');
            row.set_grapheme_extra(col, &['\u{301}']);
        }
        let decoded = CompactRow::encode(&row).decode();
        assert!(same(&row, &decoded), "{row:?}\n!=\n{decoded:?}");
    }
}

#[cfg(test)]
mod size {
    /// Scrollback keeps up to tens of thousands of these. The shell marks
    /// (ADR 0019) and the anchor tag (ADR 0020) fit in padding; keep it
    /// that way. (Screen rows are `PackedRow`s, sized in `packed.rs`.)
    #[test]
    fn rows_do_not_grow() {
        assert_eq!(std::mem::size_of::<super::CompactRow>(), 80);
        assert_eq!(std::mem::size_of::<super::Row>(), 40);
    }
}
