//! Compact storage for scrollback rows.
//!
//! A screen row is a `Vec<Cell>` of 24 bytes per column. Scrollback held
//! 10,000 of those, far more than fits in L2, and every new line was
//! written into the oldest one, so saving history stalled on memory and
//! halved VT throughput (docs/perf/2026-11-06-scrollback-cost.md). A
//! [`CompactRow`] keeps the same content as one UTF-8 character per cell
//! plus runs of identical style, without trailing blank cells: about 100
//! bytes for a typical output line instead of one cell per column.
//!
//! Rows are encoded when they scroll into history and decoded when
//! something reads them, which is at most a screenful while scrolled back.

use super::cell::{Cell, CellFlags, PackedColor};
use super::row::Row;

/// Everything about a cell except its character. Colors stay packed so
/// comparing styles, done for every cell, is integer comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Style {
    fg: PackedColor,
    bg: PackedColor,
    underline_color: PackedColor,
    flags: CellFlags,
    hyperlink_id: u32,
}

impl Style {
    /// `cell`'s style without the flags in `implied`.
    fn of(cell: &Cell, implied: CellFlags) -> Self {
        let mut flags = cell.flags();
        flags.unset(implied);
        Self {
            fg: cell.packed_fg(),
            bg: cell.packed_bg(),
            underline_color: cell.packed_underline_color(),
            flags,
            hyperlink_id: cell.hyperlink_id(),
        }
    }

    fn cell(self, c: char) -> Cell {
        Cell::from_packed(
            c,
            self.fg,
            self.bg,
            self.underline_color,
            self.flags,
            self.hyperlink_id,
        )
    }
}

/// The number of cells up to and including the last one that is not
/// [`Cell::default`].
///
/// Most rows end in a long blank tail (a short line on a wide screen), so
/// this checks whole blocks of cells from the end with a branch-free test
/// and only looks at single cells inside the last block that is not blank.
fn used_len(cells: &[Cell]) -> usize {
    const BLOCK: usize = 8;
    let mut end = cells.len();
    while end >= BLOCK
        && cells[end - BLOCK..end]
            .iter()
            .fold(true, |blank, cell| blank & cell.is_default())
    {
        end -= BLOCK;
    }
    cells[..end]
        .iter()
        .rposition(|cell| !cell.is_default())
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
    /// One character per stored cell, as UTF-8, each followed by its extra
    /// grapheme codepoints if it has any. Bytes rather than `String` so
    /// ASCII is a plain push; [`CompactRow::decode`] checks it.
    text: Vec<u8>,
    /// Style runs covering the stored cells in order.
    runs: Vec<Run>,
    /// The columns whose cells are wide or have extra grapheme codepoints,
    /// in order, each with a [`Mark`]. Keeping the codepoints in `text`
    /// saves an allocation per cluster.
    marks: Vec<(u16, Mark)>,
}

/// The flags a row's structure implies: a wide cell is followed by its
/// spacer and a cell has `HAS_COMBINING` when it has extra codepoints.
/// Leaving them out of the styles keeps a wide character, its spacer and
/// a combining sequence in the run around them; [`Mark`]s restore them.
const IMPLIED: CellFlags = CellFlags::WIDE_CHAR
    .union(CellFlags::WIDE_CHAR_SPACER)
    .union(CellFlags::HAS_COMBINING);

/// What a row stores about one cell besides its character and style: how
/// many extra grapheme codepoints follow the character in the text, and
/// whether it is a wide character whose spacer is the next cell.
type Mark = u16;

/// The [`Mark`] bit for a wide character; the rest is the codepoint count.
const WIDE_MARK: Mark = 1 << 15;

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
        self.marks.clear();

        // Plain output in the default style, which the row has kept track
        // of: its text is the whole row, and a cell is default exactly when
        // it holds a space, so trimming the text trims the blank tail.
        if row.is_plain() {
            let cells = row.used_cells();
            self.text.extend(cells.iter().map(|cell| cell.char() as u8));
            let len = self
                .text
                .iter()
                .rposition(|&b| b != b' ')
                .map_or(0, |last| last + 1);
            self.text.truncate(len);
            if len > 0 {
                self.runs.push(Run {
                    len: len as u16,
                    style: Style::of(&Cell::default(), CellFlags::NONE),
                });
            }
            return;
        }

        let cells = row.used_cells();
        let cells = &cells[..used_len(cells)];
        self.text.reserve(cells.len());

        // Plain output: one style and only ASCII on the whole row. The text
        // is then a straight copy, a fraction of the per-cell loop below.
        // Other rows give up at their first style change or non-ASCII cell.
        if let Some(first) = cells.first() {
            let style = Style::of(first, CellFlags::NONE);
            if !style.flags.contains(CellFlags::HAS_COMBINING)
                && cells
                    .iter()
                    .all(|cell| cell.char().is_ascii() && Style::of(cell, CellFlags::NONE) == style)
            {
                self.text.extend(cells.iter().map(|cell| cell.char() as u8));
                self.runs.push(Run {
                    len: cells.len() as u16,
                    style,
                });
                return;
            }
        }

        // Rows whose wide cells, spacers and combining flags are as the
        // writer leaves them have those flags implied by their marks. Any
        // other row (only reachable by editing cells directly) keeps them in
        // its styles, so encoding stays exact.
        if !self.encode_cells::<true>(row, cells) {
            self.text.clear();
            self.runs.clear();
            self.marks.clear();
            self.encode_cells::<false>(row, cells);
        }
    }

    /// Encode `cells` cell by cell, leaving the [`IMPLIED`] flags out of
    /// the styles if `IMPLY`. Returns `false`, with this row half written,
    /// when a cell's implied flags do not follow from its marks.
    #[inline(never)]
    fn encode_cells<const IMPLY: bool>(&mut self, row: &Row, cells: &[Cell]) -> bool {
        let implied = if IMPLY { IMPLIED } else { CellFlags::NONE };
        let mut col = 0;
        while col < cells.len() {
            let style = Style::of(&cells[col], implied);
            let start = col;
            while col < cells.len() && Style::of(&cells[col], implied) == style {
                let cell = &cells[col];
                self.push_char(cell.char());
                let kind = cell.flags().intersection(IMPLIED);
                if !kind.is_empty() {
                    // The common cases inline: a wide character with its
                    // spacer, and a character with extra codepoints.
                    if IMPLY && kind == CellFlags::WIDE_CHAR {
                        if let Some(spacer) = cells.get(col + 1).filter(|next| {
                            next.flags().intersection(IMPLIED) == CellFlags::WIDE_CHAR_SPACER
                                && Style::of(next, implied) == style
                        }) {
                            self.push_char(spacer.char());
                            self.marks.push((col as u16, WIDE_MARK));
                            col += 2;
                            continue;
                        }
                    } else if kind == CellFlags::HAS_COMBINING {
                        if let Some(extra) = row
                            .grapheme_extra(col as u16)
                            .filter(|extra| !extra.is_empty())
                        {
                            let mark = self.push_extra(extra);
                            self.marks.push((col as u16, mark));
                            col += 1;
                            continue;
                        }
                    }
                    if !self.mark::<IMPLY>(row, cells, col) {
                        return false;
                    }
                }
                col += 1;
            }
            self.runs.push(Run {
                len: (col - start) as u16,
                style,
            });
        }
        true
    }

    /// The rest of [`CompactRow::encode_cells`] for the cell at `col`, whose
    /// character is already in the text: its extra codepoints and, if
    /// `IMPLY`, whether it is wide. Returns `false` when its implied flags
    /// do not follow from its marks.
    #[cold]
    fn mark<const IMPLY: bool>(&mut self, row: &Row, cells: &[Cell], col: usize) -> bool {
        let cell = &cells[col];
        let mut mark: Mark = 0;
        if cell.has_flag(CellFlags::HAS_COMBINING) {
            match row.grapheme_extra(col as u16) {
                Some(extra) if !extra.is_empty() => mark = self.push_extra(extra),
                _ if IMPLY => return false,
                _ => {}
            }
        }
        if IMPLY {
            if cell.is_wide() {
                if !cells.get(col + 1).is_some_and(Cell::is_wide_spacer) {
                    return false;
                }
                mark |= WIDE_MARK;
            }
            if cell.is_wide_spacer() && !(col > 0 && cells[col - 1].is_wide()) {
                return false;
            }
        }
        if mark != 0 {
            self.marks.push((col as u16, mark));
        }
        true
    }

    /// Append a cell's extra codepoints to the text; their [`Mark`].
    fn push_extra(&mut self, extra: &[char]) -> Mark {
        let extra = &extra[..extra.len().min(usize::from(!WIDE_MARK))];
        for &c in extra {
            self.push_char(c);
        }
        extra.len() as Mark
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
        let mut marks = self.marks.iter().peekable();
        let mut extra = Vec::new();
        let mut col = 0u16;
        let mut spacer = false;
        for run in &self.runs {
            for _ in 0..run.len {
                let c = chars.next().unwrap_or(' ');
                let mut cell = run.style.cell(c);
                if std::mem::take(&mut spacer) {
                    cell.set_flag(CellFlags::WIDE_CHAR_SPACER);
                }
                let mark = marks.next_if(|&&(at, _)| at == col).map_or(0, |&(_, m)| m);
                if mark & WIDE_MARK != 0 {
                    cell.set_flag(CellFlags::WIDE_CHAR);
                    spacer = true;
                }
                if col < self.width {
                    row[col] = cell;
                }
                let len = mark & !WIDE_MARK;
                if len > 0 {
                    extra.clear();
                    extra.extend(chars.by_ref().take(usize::from(len)));
                    row.set_grapheme_extra(col, &extra);
                }
                col += 1;
            }
        }
        row.set_wrapped(self.wrapped);
        row.clear_dirty();
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::cell::{Color, UnderlineStyle};
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
            prop::collection::vec(cell, 0..60),
        )
            .prop_map(|(width, wrapped, raw_flags, cells)| {
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
                row
            })
    }

    proptest! {
        #[test]
        fn encode_then_decode_returns_the_same_row(row in row()) {
            let decoded = CompactRow::encode(&row).decode();
            prop_assert!(same(&row, &decoded), "{row:?}\n!=\n{decoded:?}");
        }

        /// Rows that keep track of being plain take a shortcut; it must
        /// give exactly what encoding them cell by cell gives.
        #[test]
        fn a_plain_row_encodes_like_any_other(
            width in 1u16..40,
            writes in prop::collection::vec((0u16..40, "[ a-z]{0,40}", any::<bool>()), 0..6),
        ) {
            let mut row = Row::new(width);
            for (col, text, erase) in writes {
                let col = col.min(width - 1);
                if erase {
                    row.erase_range(col, col + text.len() as u16, Color::Default);
                } else {
                    let len = text.len().min((width - col) as usize);
                    row.write_ascii(col, &text.as_bytes()[..len], Cell::default());
                }
            }
            prop_assert!(row.is_plain());
            let mut copy = row.clone();
            copy[0] = row[0];
            prop_assert!(!copy.is_plain());
            let compact = CompactRow::encode(&row);
            prop_assert_eq!(&compact, &CompactRow::encode(&copy));
            prop_assert!(same(&row, &compact.decode()));
        }

        #[test]
        fn reusing_a_compact_row_leaves_nothing_behind(a in row(), b in row()) {
            let mut compact = CompactRow::encode(&a);
            compact.encode_from(&b);
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
        let row = grid.row(0).unwrap();
        let compact = CompactRow::encode(row);
        assert_eq!(compact.runs.len(), 1);
        assert_eq!(compact.marks.len(), 4);
        assert!(same(row, &compact.decode()));
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
