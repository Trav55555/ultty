//! Frozen copy of `grid/history.rs` (ADR 0022 slice 2); see `reference/mod.rs`.
//! Do not edit except to keep it compiling against the shared types.

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
use super::row::{AnchorTag, Row, RowMarks};

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
        self.row_marks = row.marks();
        self.anchor = row.anchor();
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
        row.set_marks(self.row_marks);
        row.set_anchor(self.anchor);
        row.clear_dirty();
        row
    }
}
