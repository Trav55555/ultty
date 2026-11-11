//! Scrollback in pages (ADR 0022 §3, migration slice 5).
//!
//! Rows that scroll off the screen are appended to the newest of a list of
//! pages, [`PAGE_ROWS`] rows each. A page keeps a head per row and the rows'
//! cells in one of three forms, chosen per row:
//!
//! - **Ascii**, one byte a cell: ASCII with no structure bits, all in one
//!   style. A uniform screen row (`ScreenRow::is_uniform`) is a byte copy.
//! - **Chars**, four bytes a cell (the codepoint and the width bits): any
//!   row without grapheme extras whose cells are all in one style, which
//!   is how CJK output arrives. Without it a CJK row costs 8 bytes a cell,
//!   about 1.7 times what it did before pages (ADR 0022 §9, Risks, and open
//!   question 2).
//! - **Cells**, the packed cells themselves, with their grapheme extras.
//!
//! Every form stores the cells up to the last one that is not blank in the
//! default style, so a short line on a wide screen stays short. Style keys
//! that are not inline are table ids of the page's own style list; a cached
//! map from the screen table's ids (`History::remap`) makes that a lookup
//! per style run rather than per cell.
//!
//! Pages are append-only; eviction is per row and frees a page once its
//! last row is gone, and a freed page is kept to be the next new one, so a
//! full scrollback allocates nothing per row. [`HistoryRow`] reads a row in
//! place, without decoding it.

use std::collections::VecDeque;
use std::fmt;

use super::cell::{Cell, CellFlags};
use super::packed::{PackedCell, Style, StyleKey, StyleTable, Summary};
use super::row::{AnchorTag, Row, RowMarks};
use super::screen::{ScreenPage, ScreenRow};

/// Rows per history page (ADR 0022 §3; open question 4).
pub(super) const PAGE_ROWS: usize = 256;

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

/// How a row's cells are stored; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Ascii,
    Chars,
    Cells,
}

/// A history row's head: everything but its cells.
#[derive(Debug, Clone, Copy)]
struct Head {
    /// Ascii and Chars: the row's style key, in the page's ids
    /// ([`StyleKey::bits`]). Cells: where its grapheme extras are,
    /// `extras[start..start + count]`, as `start | count << 32`.
    aux: u64,
    /// Where the row's cells start in the vector its form uses.
    offset: u32,
    /// Cells stored; every cell from here to `width` is blank.
    len: u16,
    width: u16,
    flags: u8,
    /// Shell integration marks (ADR 0019).
    marks: RowMarks,
    /// Inline surface anchor (ADR 0020).
    anchor: AnchorTag,
}

impl Head {
    const FORM: u8 = 0b11;
    const CHARS: u8 = 1;
    const CELLS: u8 = 2;
    const WRAPPED: u8 = 1 << 2;

    #[inline]
    fn form(&self) -> Form {
        match self.flags & Self::FORM {
            0 => Form::Ascii,
            Self::CHARS => Form::Chars,
            _ => Form::Cells,
        }
    }

    /// The style of a one-style row, in the page's ids.
    #[inline]
    fn key(&self) -> StyleKey {
        StyleKey::from_bits(self.aux)
    }

    /// A Cells row's entries in the page's `extras`.
    #[inline]
    fn extras(&self) -> std::ops::Range<usize> {
        let start = (self.aux & u64::from(u32::MAX)) as usize;
        start..start + (self.aux >> 32) as usize
    }

    #[inline]
    fn is_wrapped(&self) -> bool {
        self.flags & Self::WRAPPED != 0
    }
}

/// A Cells row's [`Head::aux`] for extras `start..start + count`; no
/// extras point at none.
fn extras_aux(start: usize, count: usize) -> u64 {
    if count == 0 {
        0
    } else {
        start as u64 | (count as u64) << 32
    }
}

/// The extra codepoints of one grapheme in a Cells row:
/// `extra_chars[start..start + len]`.
#[derive(Debug, Clone, Copy)]
struct Extra {
    col: u16,
    len: u8,
    start: u32,
}

/// Up to [`PAGE_ROWS`] rows, appended in place.
#[derive(Debug, Clone)]
struct Page {
    heads: Vec<Head>,
    ascii: Vec<u8>,
    chars: Vec<u32>,
    cells: Vec<PackedCell>,
    /// Grapheme extras of Cells rows, each row's sorted by column.
    extras: Vec<Extra>,
    extra_chars: Vec<char>,
    /// The styles of the page's keys that are not inline; entry 0 is the
    /// default style.
    styles: Vec<Style>,
    /// Payload bytes no head points at any more (left by rewrites and by
    /// rows taken back off the end).
    garbage: usize,
}

impl Page {
    fn new() -> Self {
        Self {
            heads: Vec::with_capacity(PAGE_ROWS),
            ascii: Vec::new(),
            chars: Vec::new(),
            cells: Vec::new(),
            extras: Vec::new(),
            extra_chars: Vec::new(),
            styles: vec![Style::default()],
            garbage: 0,
        }
    }

    /// Empty the page for reuse, keeping its allocations.
    fn clear(&mut self) {
        self.heads.clear();
        self.ascii.clear();
        self.chars.clear();
        self.cells.clear();
        self.extras.clear();
        self.extra_chars.clear();
        self.styles.truncate(1);
        self.garbage = 0;
    }

    /// The page is full: give back what its vectors over-allocated while
    /// growing, and what a recycled page kept from content of another
    /// shape.
    fn shrink(&mut self) {
        self.ascii.shrink_to_fit();
        self.chars.shrink_to_fit();
        self.cells.shrink_to_fit();
        self.extras.shrink_to_fit();
        self.extra_chars.shrink_to_fit();
        self.styles.shrink_to_fit();
    }

    #[inline]
    fn row(&self, index: usize) -> HistoryRow<'_> {
        let head = &self.heads[index];
        let style = match head.form() {
            Form::Ascii | Form::Chars => head.key().style_in(&self.styles),
            Form::Cells => Style::default(),
        };
        HistoryRow {
            page: self,
            head,
            style,
        }
    }

    /// Store `row`'s cells, whose keys `map` turns into this page's (it is
    /// handed the page's style list), and return its head. The head is
    /// not added.
    #[inline]
    fn append(
        &mut self,
        row: ScreenRow<'_>,
        map: &mut impl FnMut(&mut Vec<Style>, StyleKey) -> StyleKey,
    ) -> Head {
        let mut head = Head {
            aux: 0,
            offset: 0,
            len: 0,
            width: row.width(),
            flags: if row.is_wrapped() { Head::WRAPPED } else { 0 },
            marks: row.marks(),
            anchor: row.anchor(),
        };
        if !row.is_uniform() {
            self.append_cells(row, &mut head, map);
            return head;
        }

        // A uniform row is ASCII in one style, which the row has kept track
        // of. In the default style a cell is blank exactly when it holds a
        // space, so trimming the spaces trims the blank tail; in any other
        // style no cell is blank.
        let mut used = row.used_cells();
        let key = row.uniform_key();
        if key.is_default() {
            while let [rest @ .., last] = used {
                if *last != PackedCell::BLANK {
                    break;
                }
                used = rest;
            }
        }
        // A row with no payload points at none, so that taking later rows
        // off the end leaves no offset past it.
        if !used.is_empty() {
            head.offset = self.ascii.len() as u32;
            head.len = used.len() as u16;
            self.ascii.extend(used.iter().map(|cell| cell.ascii_byte()));
            if !key.is_default() {
                head.aux = map(&mut self.styles, key).bits();
            }
        }
        head
    }

    /// [`Page::append`] for a row that is not uniform: it may still be
    /// ASCII in one style (printed a character at a time), or else it takes
    /// the Chars or the Cells form.
    #[inline(never)]
    fn append_cells(
        &mut self,
        row: ScreenRow<'_>,
        head: &mut Head,
        map: &mut impl FnMut(&mut Vec<Style>, StyleKey) -> StyleKey,
    ) {
        let used = row.used_cells();
        let cells = &used[..used_len(used)];
        let summary = Summary::of(cells);
        let key = cells.first().map_or(StyleKey::DEFAULT, |cell| cell.key());
        head.len = cells.len() as u16;
        if summary.one_key && summary.ascii {
            head.offset = self.ascii.len() as u32;
            self.ascii
                .extend(cells.iter().map(|cell| cell.ascii_byte()));
            if !cells.is_empty() {
                head.aux = map(&mut self.styles, key).bits();
            }
        } else if summary.one_key && !summary.graphemes {
            head.flags |= Head::CHARS;
            head.offset = self.chars.len() as u32;
            self.chars.extend(cells.iter().map(|cell| cell.low_bits()));
            head.aux = map(&mut self.styles, key).bits();
        } else {
            head.flags |= Head::CELLS;
            let start = self.cells.len();
            head.offset = start as u32;
            self.cells.extend_from_slice(cells);
            let (mut last, mut mapped) = (StyleKey::DEFAULT, StyleKey::DEFAULT);
            for cell in &mut self.cells[start..] {
                let key = cell.key();
                if key != last {
                    last = key;
                    mapped = map(&mut self.styles, key);
                }
                *cell = cell.with_key(mapped);
            }
            let first = self.extras.len();
            let mut count = 0;
            if summary.graphemes {
                for (col, cell) in cells.iter().enumerate() {
                    if !cell.has_grapheme() {
                        continue;
                    }
                    if let Some(extra) = row.grapheme_extra(col as u16) {
                        let extra = &extra[..extra.len().min(usize::from(u8::MAX))];
                        if !extra.is_empty() {
                            self.extras.push(Extra {
                                col: col as u16,
                                len: extra.len() as u8,
                                start: self.extra_chars.len() as u32,
                            });
                            self.extra_chars.extend_from_slice(extra);
                            count += 1;
                        }
                    }
                }
            }
            head.aux = extras_aux(first, count);
        }
        if head.len == 0 {
            head.offset = 0;
        }
    }

    /// The payload bytes `head` points at.
    fn payload_bytes(&self, head: &Head) -> usize {
        let len = usize::from(head.len);
        match head.form() {
            Form::Ascii => len,
            Form::Chars => 4 * len,
            Form::Cells => {
                let extras = &self.extras[head.extras()];
                8 * len
                    + std::mem::size_of_val(extras)
                    + extras
                        .iter()
                        .map(|extra| 4 * usize::from(extra.len))
                        .sum::<usize>()
            }
        }
    }

    /// All payload bytes, live or not.
    fn total_bytes(&self) -> usize {
        self.ascii.len()
            + 4 * self.chars.len()
            + 8 * self.cells.len()
            + self.extras.len() * std::mem::size_of::<Extra>()
            + 4 * self.extra_chars.len()
    }

    /// Remove the last row. Its payload is given back when it is the last
    /// thing stored, which it is unless a rewrite came after it.
    fn pop_head(&mut self) {
        let Some(head) = self.heads.pop() else {
            return;
        };
        let start = head.offset as usize;
        let end = start + usize::from(head.len);
        let at_end = match head.form() {
            Form::Ascii => self.ascii.len() == end,
            Form::Chars => self.chars.len() == end,
            Form::Cells => {
                let extras = head.extras();
                self.cells.len() == end && (extras.is_empty() || self.extras.len() == extras.end)
            }
        };
        if !at_end {
            self.garbage += self.payload_bytes(&head);
            return;
        }
        match head.form() {
            Form::Ascii => self.ascii.truncate(start),
            Form::Chars => self.chars.truncate(start),
            Form::Cells => {
                self.cells.truncate(start);
                let extras = head.extras();
                if !extras.is_empty() {
                    self.extra_chars
                        .truncate(self.extras[extras.start].start as usize);
                    self.extras.truncate(extras.start);
                }
            }
        }
    }

    /// Make row `index` hold `row`, whose keys are ids of `screen`. The
    /// new cells are appended and the old ones left as garbage, which is
    /// compacted away once it outgrows the live payload.
    fn rewrite(&mut self, index: usize, row: ScreenRow<'_>, screen: &StyleTable) {
        let old = self.heads[index];
        self.garbage += self.payload_bytes(&old);
        // Not the cached remap, which belongs to the newest page and the
        // grid's table: look each style up in the page's list, so repeated
        // rewrites do not grow it.
        let head = self.append(row, &mut |styles, key| match key.table_id() {
            None | Some(0) => key,
            Some(_) => {
                let style = screen.style(key);
                let id = match styles.iter().skip(1).position(|s| *s == style) {
                    Some(at) => at + 1,
                    None => {
                        styles.push(style);
                        styles.len() - 1
                    }
                };
                StyleKey::from_id(id as u32)
            }
        });
        self.heads[index] = head;
        const MIN_GARBAGE: usize = 4096;
        if self.garbage > MIN_GARBAGE && 2 * self.garbage > self.total_bytes() {
            self.compact();
        }
    }

    /// Copy every row's payload to fresh vectors, dropping the garbage.
    fn compact(&mut self) {
        let mut ascii = Vec::with_capacity(self.ascii.len());
        let mut chars = Vec::with_capacity(self.chars.len());
        let mut cells = Vec::with_capacity(self.cells.len());
        let mut extras = Vec::with_capacity(self.extras.len());
        let mut extra_chars = Vec::with_capacity(self.extra_chars.len());
        for head in &mut self.heads {
            let range = head.offset as usize..head.offset as usize + usize::from(head.len);
            match head.form() {
                Form::Ascii => {
                    head.offset = ascii.len() as u32;
                    ascii.extend_from_slice(&self.ascii[range]);
                }
                Form::Chars => {
                    head.offset = chars.len() as u32;
                    chars.extend_from_slice(&self.chars[range]);
                }
                Form::Cells => {
                    head.offset = cells.len() as u32;
                    cells.extend_from_slice(&self.cells[range]);
                    let first = extras.len();
                    for extra in &self.extras[head.extras()] {
                        let from = extra.start as usize;
                        extras.push(Extra {
                            start: extra_chars.len() as u32,
                            ..*extra
                        });
                        extra_chars.extend_from_slice(
                            &self.extra_chars[from..from + usize::from(extra.len)],
                        );
                    }
                    head.aux = extras_aux(first, extras.len() - first);
                }
            }
        }
        self.ascii = ascii;
        self.chars = chars;
        self.cells = cells;
        self.extras = extras;
        self.extra_chars = extra_chars;
        self.garbage = 0;
    }

    /// Heap bytes the page holds.
    #[cfg(test)]
    fn heap_bytes(&self) -> usize {
        self.heads.capacity() * std::mem::size_of::<Head>()
            + self.ascii.capacity()
            + self.chars.capacity() * 4
            + self.cells.capacity() * 8
            + self.extras.capacity() * std::mem::size_of::<Extra>()
            + self.extra_chars.capacity() * 4
            + self.styles.capacity() * std::mem::size_of::<Style>()
    }

    /// Every head points at a payload inside the page, in keys the page can
    /// hand out, and every grapheme extra belongs to a cell with the bit.
    fn check_integrity(&self, max_extra: usize) -> Result<(), String> {
        if self.styles.first() != Some(&Style::default()) {
            return Err("page style 0 is not the default style".into());
        }
        if self.garbage > self.total_bytes() {
            return Err(format!(
                "page counts {} bytes of garbage in {} bytes",
                self.garbage,
                self.total_bytes()
            ));
        }
        for (index, head) in self.heads.iter().enumerate() {
            self.check_head(head, max_extra)
                .map_err(|e| format!("page row {index}: {e}"))?;
        }
        Ok(())
    }

    fn check_head(&self, head: &Head, max_extra: usize) -> Result<(), String> {
        if head.len > head.width {
            return Err(format!(
                "{} cells stored in a row of {}",
                head.len, head.width
            ));
        }
        let range = head.offset as usize..head.offset as usize + usize::from(head.len);
        let stored = match head.form() {
            Form::Ascii => self.ascii.len(),
            Form::Chars => self.chars.len(),
            Form::Cells => self.cells.len(),
        };
        if range.end > stored {
            return Err(format!(
                "{:?} cells {range:?} past the page's {stored}",
                head.form()
            ));
        }
        match head.form() {
            Form::Ascii => {
                if !self.ascii[range].is_ascii() {
                    return Err("Ascii row holds a byte that is not ASCII".into());
                }
                head.key().check_in(&self.styles)?;
            }
            Form::Chars => {
                for &bits in &self.chars[range] {
                    let cell = PackedCell::from_low_bits(bits, StyleKey::DEFAULT);
                    if char::from_u32(cell.codepoint()).is_none() || cell.has_grapheme() {
                        return Err(format!("Chars row holds a bad cell {bits:#x}"));
                    }
                }
                head.key().check_in(&self.styles)?;
            }
            Form::Cells => {
                let cells = &self.cells[range];
                for cell in cells {
                    cell.key().check_in(&self.styles)?;
                }
                let extras = head.extras();
                if extras.end > self.extras.len() {
                    return Err(format!(
                        "extras {extras:?} past the page's {}",
                        self.extras.len()
                    ));
                }
                let extras = &self.extras[extras];
                if extras.windows(2).any(|pair| pair[0].col >= pair[1].col) {
                    return Err("grapheme extras are not sorted by column".into());
                }
                for extra in extras {
                    let chars = extra.start as usize..extra.start as usize + usize::from(extra.len);
                    if chars.end > self.extra_chars.len() {
                        return Err(format!("extra {extra:?} runs past the page's codepoints"));
                    }
                    if extra.len == 0 || usize::from(extra.len) > max_extra {
                        return Err(format!("extra {extra:?} holds a bad number of codepoints"));
                    }
                    if !cells
                        .get(usize::from(extra.col))
                        .is_some_and(|cell| cell.has_grapheme())
                    {
                        return Err(format!("extra {extra:?} is for a cell without the bit"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One scrollback row, read in place from its page.
#[derive(Clone, Copy)]
pub(super) struct HistoryRow<'a> {
    page: &'a Page,
    head: &'a Head,
    /// The one-style forms' style, looked up once.
    style: Style,
}

impl fmt::Debug for HistoryRow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryRow")
            .field("form", &self.head.form())
            .field("len", &self.head.len)
            .field("width", &self.head.width)
            .finish_non_exhaustive()
    }
}

impl<'a> HistoryRow<'a> {
    #[inline]
    pub(super) fn width(self) -> u16 {
        self.head.width
    }

    #[inline]
    pub(super) fn is_wrapped(self) -> bool {
        self.head.is_wrapped()
    }

    #[inline]
    pub(super) fn marks(self) -> RowMarks {
        self.head.marks
    }

    #[inline]
    pub(super) fn anchor(self) -> AnchorTag {
        self.head.anchor
    }

    /// Where stored cell `col` is in its form's vector, or `None` past the
    /// stored cells.
    #[inline]
    fn stored(self, col: u16) -> Option<usize> {
        (col < self.head.len).then(|| self.head.offset as usize + usize::from(col))
    }

    /// The cell at `col`, or `None` past the end of the row.
    #[inline]
    pub(super) fn cell(self, col: u16) -> Option<Cell> {
        if col >= self.head.width {
            return None;
        }
        let Some(at) = self.stored(col) else {
            return Some(Cell::default());
        };
        Some(match self.head.form() {
            Form::Ascii => self
                .style
                .cell(char::from(self.page.ascii[at]), CellFlags::NONE),
            Form::Chars => {
                let cell = PackedCell::from_low_bits(self.page.chars[at], StyleKey::DEFAULT);
                self.style.cell(cell.char(), cell.structural_flags())
            }
            Form::Cells => {
                let cell = self.page.cells[at];
                cell.key()
                    .style_in(&self.page.styles)
                    .cell(cell.char(), cell.structural_flags())
            }
        })
    }

    /// The row's grapheme extras, by column.
    #[inline]
    fn extras(self) -> &'a [Extra] {
        match self.head.form() {
            Form::Cells => &self.page.extras[self.head.extras()],
            Form::Ascii | Form::Chars => &[],
        }
    }

    #[inline]
    fn extra_chars(self, extra: &Extra) -> &'a [char] {
        let start = extra.start as usize;
        &self.page.extra_chars[start..start + usize::from(extra.len)]
    }

    /// Codepoints following the base character of the cluster at `col`, if
    /// the cell holds a multi-codepoint grapheme.
    #[inline]
    pub(super) fn grapheme_extra(self, col: u16) -> Option<&'a [char]> {
        let extras = self.extras();
        if extras.is_empty() {
            return None;
        }
        let at = extras.binary_search_by_key(&col, |extra| extra.col).ok()?;
        Some(self.extra_chars(&extras[at]))
    }

    /// The character at each column, without the rest of the cell or any
    /// grapheme extras.
    pub(super) fn chars(self) -> impl Iterator<Item = char> + 'a {
        let range =
            self.head.offset as usize..self.head.offset as usize + usize::from(self.head.len);
        let (ascii, chars, cells) = match self.head.form() {
            Form::Ascii => (&self.page.ascii[range], &[][..], &[][..]),
            Form::Chars => (&[][..], &self.page.chars[range], &[][..]),
            Form::Cells => (&[][..], &[][..], &self.page.cells[range]),
        };
        ascii
            .iter()
            .map(|&b| char::from(b))
            .chain(
                chars
                    .iter()
                    .map(|&bits| PackedCell::from_low_bits(bits, StyleKey::DEFAULT).char()),
            )
            .chain(cells.iter().map(|cell| cell.char()))
            .chain(std::iter::repeat_n(
                ' ',
                usize::from(self.head.width - self.head.len),
            ))
    }

    /// Append the row's text, one char per column plus grapheme extras.
    pub(super) fn push_text(self, text: &mut String) {
        let range =
            self.head.offset as usize..self.head.offset as usize + usize::from(self.head.len);
        match self.head.form() {
            Form::Ascii => text.extend(self.page.ascii[range].iter().map(|&b| char::from(b))),
            Form::Chars => text.extend(
                self.page.chars[range]
                    .iter()
                    .map(|&bits| PackedCell::from_low_bits(bits, StyleKey::DEFAULT).char()),
            ),
            Form::Cells => {
                let mut extras = self.extras().iter().peekable();
                for (col, cell) in self.page.cells[range].iter().enumerate() {
                    text.push(cell.char());
                    if let Some(extra) = extras.next_if(|extra| usize::from(extra.col) == col) {
                        text.extend(self.extra_chars(extra));
                    }
                }
            }
        }
        text.extend(std::iter::repeat_n(
            ' ',
            usize::from(self.head.width - self.head.len),
        ));
    }

    /// Make `dst` a copy of the row, reusing `dst`'s allocations.
    pub(super) fn copy_into(self, dst: &mut Row) {
        dst.assign(
            (0..self.width()).filter_map(|col| self.cell(col)),
            self.extras()
                .iter()
                .map(|extra| (extra.col, self.extra_chars(extra))),
            self.is_wrapped(),
            self.marks(),
            self.anchor(),
        );
        dst.clear_dirty();
    }
}

/// Scrollback: rows in pages, oldest first, with per-row eviction.
///
/// Every page but the last holds [`PAGE_ROWS`] rows, so row `i` is in page
/// `(front + i) / PAGE_ROWS`. The buffer decides when to evict; this only
/// stores.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), derive(Default))]
pub(super) struct History {
    pages: VecDeque<Page>,
    /// Rows of `pages[0]` already evicted.
    front: usize,
    len: usize,
    /// The last page freed, to be the next one started.
    spare: Option<Page>,
    /// Screen table id to the newest page's id (0 for none yet), for the
    /// table and numbering in `remap_stamp`.
    remap: Vec<u32>,
    remap_stamp: Option<(u64, u64)>,
    /// Rows per page in tests, which can make pages tiny
    /// (`test_support::page_rows`) so short streams cross page boundaries.
    #[cfg(test)]
    page_rows: usize,
}

#[cfg(test)]
impl Default for History {
    fn default() -> Self {
        Self {
            pages: VecDeque::new(),
            front: 0,
            len: 0,
            spare: None,
            remap: Vec::new(),
            remap_stamp: None,
            #[cfg(test)]
            page_rows: test_support::forced_page_rows().unwrap_or(PAGE_ROWS),
        }
    }
}

/// The newest page's key for `key`, a key of `screen`: inline keys and the
/// default are the same everywhere; a table id is looked up in `remap`, and
/// its style added to the page (`styles`) the first time.
#[inline]
fn remapped(
    remap: &mut Vec<u32>,
    styles: &mut Vec<Style>,
    screen: &StyleTable,
    key: StyleKey,
) -> StyleKey {
    match key.table_id() {
        None | Some(0) => key,
        Some(id) => match remap.get(id) {
            Some(&mapped) if mapped != 0 => StyleKey::from_id(mapped),
            _ => add_style(remap, styles, screen, key, id),
        },
    }
}

/// [`remapped`]'s miss: append the style to the page.
#[inline(never)]
fn add_style(
    remap: &mut Vec<u32>,
    styles: &mut Vec<Style>,
    screen: &StyleTable,
    key: StyleKey,
    id: usize,
) -> StyleKey {
    let mapped = styles.len() as u32;
    styles.push(screen.style(key));
    if remap.len() <= id {
        remap.resize(id + 1, 0);
    }
    remap[id] = mapped;
    StyleKey::from_id(mapped)
}

/// Start a new newest page in `pages`, from `spare` if there is one, and
/// return it.
#[inline(never)]
fn start_page<'a>(pages: &'a mut VecDeque<Page>, spare: &mut Option<Page>) -> &'a mut Page {
    if let Some(full) = pages.back_mut() {
        full.shrink();
    }
    pages.push_back(spare.take().unwrap_or_else(Page::new));
    pages.back_mut().expect("a page was just added")
}

impl History {
    /// Rows per page.
    #[cfg(not(test))]
    #[inline(always)]
    fn page_rows(&self) -> usize {
        PAGE_ROWS
    }

    /// Rows per page: [`PAGE_ROWS`], unless the test shrank it.
    #[cfg(test)]
    fn page_rows(&self) -> usize {
        self.page_rows
    }

    /// The number of rows.
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// The page and the index in it of row `index`.
    #[inline]
    fn locate(&self, index: usize) -> Option<(usize, usize)> {
        (index < self.len).then(|| {
            let at = self.front + index;
            (at / self.page_rows(), at % self.page_rows())
        })
    }

    #[inline]
    fn head(&self, index: usize) -> Option<&Head> {
        let (page, row) = self.locate(index)?;
        Some(&self.pages[page].heads[row])
    }

    /// Row `index`, 0 being the oldest.
    #[inline]
    pub(super) fn row(&self, index: usize) -> Option<HistoryRow<'_>> {
        let (page, row) = self.locate(index)?;
        Some(self.pages[page].row(row))
    }

    /// Row `index`'s shell marks.
    pub(super) fn marks(&self, index: usize) -> Option<RowMarks> {
        Some(self.head(index)?.marks)
    }

    /// Row `index`'s anchor tag.
    pub(super) fn anchor(&self, index: usize) -> Option<AnchorTag> {
        Some(self.head(index)?.anchor)
    }

    /// Replace row `index`'s anchor tag.
    pub(super) fn set_anchor(&mut self, index: usize, anchor: AnchorTag) {
        if let Some((page, row)) = self.locate(index) {
            self.pages[page].heads[row].anchor = anchor;
        }
    }

    /// Take every anchor tag off.
    pub(super) fn clear_anchors(&mut self) {
        for head in self.pages.iter_mut().flat_map(|page| &mut page.heads) {
            head.anchor = AnchorTag::NONE;
        }
    }

    /// Append screen row `row`, whose style keys are ids of `screen`.
    #[inline]
    pub(super) fn push(&mut self, row: ScreenRow<'_>, screen: &StyleTable) {
        let stamp = screen.stamp();
        let page_rows = self.page_rows();
        let page = match self.pages.back_mut() {
            Some(page) if page.heads.len() < page_rows => page,
            _ => {
                // The remap is for the newest page.
                self.remap_stamp = None;
                start_page(&mut self.pages, &mut self.spare)
            }
        };
        if self.remap_stamp != Some(stamp) {
            self.remap.clear();
            self.remap_stamp = Some(stamp);
        }
        let remap = &mut self.remap;
        let head = page.append(row, &mut |styles, key| remapped(remap, styles, screen, key));
        page.heads.push(head);
        self.len += 1;
    }

    /// Append an owned row, packing it with `packer` first.
    pub(super) fn push_row(&mut self, row: &Row, packer: &mut RowPacker) {
        let (packed, styles) = packer.pack(row);
        self.push(packed, styles);
    }

    fn recycle(&mut self, mut page: Page) {
        page.clear();
        self.spare = Some(page);
    }

    /// Evict the oldest row.
    #[inline]
    pub(super) fn pop_front(&mut self) {
        if self.len == 0 {
            return;
        }
        self.front += 1;
        self.len -= 1;
        if self.front >= self.page_rows() || self.len == 0 {
            self.free_front_page();
        }
    }

    /// The first page's rows are all evicted: free it.
    #[inline(never)]
    fn free_front_page(&mut self) {
        if self
            .pages
            .front()
            .is_some_and(|page| self.front >= page.heads.len())
        {
            if let Some(page) = self.pages.pop_front() {
                self.recycle(page);
            }
            self.front = 0;
            if self.pages.is_empty() {
                self.remap_stamp = None;
            }
        }
    }

    /// Evict the oldest `count` rows.
    pub(super) fn drop_front(&mut self, count: usize) {
        for _ in 0..count.min(self.len) {
            self.pop_front();
        }
    }

    /// Remove the newest row.
    fn pop_back(&mut self) {
        if self.len == 0 {
            return;
        }
        let single = self.pages.len() == 1;
        let Some(page) = self.pages.back_mut() else {
            return;
        };
        page.pop_head();
        self.len -= 1;
        let live = page.heads.len() - if single { self.front } else { 0 };
        if live == 0 {
            if let Some(page) = self.pages.pop_back() {
                self.recycle(page);
            }
            if single {
                self.front = 0;
            }
            self.remap_stamp = None;
        }
    }

    /// Keep the oldest `len` rows.
    pub(super) fn truncate(&mut self, len: usize) {
        while self.len > len {
            self.pop_back();
        }
    }

    /// Drop every row.
    pub(super) fn clear(&mut self) {
        while let Some(page) = self.pages.pop_front() {
            self.recycle(page);
        }
        self.front = 0;
        self.len = 0;
        self.remap_stamp = None;
    }

    /// Make row `index` hold `row` (packed with `packer`), keeping its place.
    pub(super) fn replace(&mut self, index: usize, row: &Row, packer: &mut RowPacker) {
        let Some((page, at)) = self.locate(index) else {
            return;
        };
        let (packed, styles) = packer.pack(row);
        self.pages[page].rewrite(at, packed, styles);
    }

    /// Hand every row to `f`, oldest first, freeing each page once its
    /// rows are done.
    pub(super) fn drain(mut self, mut f: impl FnMut(HistoryRow<'_>)) {
        let mut front = self.front;
        while let Some(page) = self.pages.pop_front() {
            for index in front..page.heads.len() {
                f(page.row(index));
            }
            front = 0;
        }
    }

    /// Heap bytes the pages hold, the spare included.
    #[cfg(test)]
    pub(super) fn heap_bytes(&self) -> usize {
        self.pages
            .iter()
            .chain(&self.spare)
            .map(Page::heap_bytes)
            .sum::<usize>()
            + self.pages.capacity() * std::mem::size_of::<Page>()
            + self.remap.capacity() * 4
    }

    /// Check the pages' invariants: the row count, full pages before the
    /// last, and every page's own (see `Page::check_integrity`).
    pub(super) fn check_integrity(&self, max_extra: usize) -> Result<(), String> {
        let stored: usize = self.pages.iter().map(|page| page.heads.len()).sum();
        if stored.checked_sub(self.front) != Some(self.len) && !(stored == 0 && self.len == 0) {
            return Err(format!(
                "history holds {stored} rows from {} on, but counts {}",
                self.front, self.len
            ));
        }
        if let Some(first) = self.pages.front() {
            if self.front >= first.heads.len() {
                return Err(format!(
                    "the first page's {} rows are all evicted ({})",
                    first.heads.len(),
                    self.front
                ));
            }
        } else if self.front != 0 {
            return Err(format!(
                "no pages, but {} rows evicted from the first",
                self.front
            ));
        }
        let pages = self.pages.len();
        for (index, page) in self.pages.iter().enumerate() {
            if index + 1 < pages && page.heads.len() != self.page_rows() {
                return Err(format!(
                    "page {index} of {pages} holds {} rows, not {}",
                    page.heads.len(),
                    self.page_rows()
                ));
            }
            page.check_integrity(max_extra)
                .map_err(|e| format!("history page {index}: {e}"))?;
        }
        Ok(())
    }
}

/// Packs owned [`Row`]s, which carry no style ids, into a one-row screen
/// page, so they can be stored like screen rows. Reflow packs many rows
/// this way, so the page and its style table are reused.
pub(super) struct RowPacker {
    styles: StyleTable,
    /// One row, as wide as the last row packed.
    page: ScreenPage,
}

impl RowPacker {
    pub(super) fn new() -> Self {
        Self {
            styles: StyleTable::new(0),
            page: ScreenPage::new(0, 1),
        }
    }

    /// `row` packed, and the table its keys are in.
    pub(super) fn pack(&mut self, row: &Row) -> (ScreenRow<'_>, &StyleTable) {
        // Ids only live for one row, so the table can be emptied whenever
        // it grows.
        if self.styles.is_due() {
            self.styles.reset(0);
        }
        if self.page.cols() != row.width() {
            self.page = ScreenPage::new(row.width(), 1);
        }
        let mut packed = self.page.row_mut(0).expect("the page has one row");
        packed.assign_row(row, &mut self.styles);
        let packed = self.page.row(0).expect("the page has one row");
        (packed, &self.styles)
    }
}

/// Test-only control over the page size, so short streams cross pages.
#[cfg(test)]
pub(super) mod test_support {
    use std::cell::Cell;

    thread_local! {
        static ROWS: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(in crate::grid) fn forced_page_rows() -> Option<usize> {
        ROWS.with(Cell::get)
    }

    /// While the guard lives, histories created on this thread keep `rows`
    /// rows a page.
    pub(in crate::grid) fn page_rows(rows: usize) -> Guard {
        ROWS.with(|forced| forced.set(Some(rows.max(1))));
        Guard
    }

    pub(in crate::grid) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            ROWS.with(|forced| forced.set(None));
        }
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

    /// Row `index` of `history` as an owned row.
    fn owned(history: &History, index: usize) -> Row {
        let mut row = Row::new(0);
        history.row(index).unwrap().copy_into(&mut row);
        row
    }

    /// `row`'s text as `RowView::text` gives it for a row of the screen.
    fn text_of(row: &Row) -> String {
        let mut text = String::new();
        for (col, cell) in row.cells().enumerate() {
            text.push(cell.char());
            if let Some(extra) = row.grapheme_extra(col as u16) {
                text.extend(extra);
            }
        }
        text
    }

    /// The form row `index` was stored in.
    fn form(history: &History, index: usize) -> Form {
        history.head(index).unwrap().form()
    }

    proptest! {
        /// `push(row).view().to_row() == row`, for every row the generator
        /// makes, including the flags only direct edits leave; and the
        /// in-place readers agree with the copy.
        #[test]
        fn a_pushed_row_reads_back_the_same(rows in prop::collection::vec(row(), 1..8)) {
            let mut history = History::default();
            let mut packer = RowPacker::new();
            for row in &rows {
                history.push_row(row, &mut packer);
            }
            history.check_integrity(16).unwrap();
            for (index, row) in rows.iter().enumerate() {
                let back = owned(&history, index);
                prop_assert!(same(row, &back), "{row:?}\n!=\n{back:?}");
                let view = history.row(index).unwrap();
                for col in 0..=row.width() {
                    prop_assert_eq!(view.cell(col), row.get(col).copied());
                    prop_assert_eq!(view.grapheme_extra(col), row.grapheme_extra(col));
                }
                let mut text = String::new();
                view.push_text(&mut text);
                prop_assert_eq!(text, text_of(row));
            }
        }

        /// Uniform rows take a shortcut; it must give exactly what storing
        /// them cell by cell gives.
        #[test]
        fn a_uniform_row_is_stored_like_any_other(
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
            let mut page = ScreenPage::new(width, 1);
            let mut row = page.row_mut(0).unwrap();
            for (col, text, erase) in writes {
                let col = col.min(width - 1);
                if erase {
                    row.erase_range(col, col + text.len() as u16, StyleKey::DEFAULT);
                } else {
                    let len = text.len().min((width - col) as usize);
                    row.write_ascii(col, &text.as_bytes()[..len], key);
                }
            }
            let row = page.row(0).unwrap();
            prop_assert!(row.is_uniform() || style != 0, "default-style text stays uniform");
            if row.is_uniform() {
                let mut copy = page.clone();
                let first = row.get(0).unwrap();
                copy.row_mut(0).unwrap().set(0, first);
                let copy = copy.row(0).unwrap();
                prop_assert!(!copy.is_uniform());
                let mut history = History::default();
                history.push(row, &styles);
                history.push(copy, &styles);
                prop_assert_eq!(form(&history, 0), Form::Ascii);
                prop_assert_eq!(form(&history, 1), Form::Ascii);
                let (a, b) = (history.head(0).unwrap(), history.head(1).unwrap());
                prop_assert_eq!(a.len, b.len);
                prop_assert!(same(&owned(&history, 0), &owned(&history, 1)));
            }
        }
    }

    /// A screen of `cols` by one row holding `text`, printed the way the
    /// grid prints it.
    fn grid_row(cols: u16, text: &str) -> crate::grid::Grid {
        let mut grid = crate::grid::Grid::new(crate::grid::Dimensions::new(cols, 1));
        for c in text.chars() {
            grid.write_char(c);
        }
        grid
    }

    fn push_grid(history: &mut History, grid: &crate::grid::Grid) {
        history.push(grid.page.row(0).unwrap(), &grid.styles);
    }

    #[test]
    fn each_row_takes_the_smallest_form_that_holds_it() {
        let mut history = History::default();
        let lines = [
            ("the quick brown fox", Form::Ascii, 19),
            ("日本語のテキスト", Form::Chars, 16),
            ("日本 e\u{301}語 x", Form::Cells, 10),
            ("", Form::Ascii, 0),
        ];
        for (text, ..) in lines {
            push_grid(&mut history, &grid_row(40, text));
        }
        history.check_integrity(16).unwrap();
        for (index, (text, want, len)) in lines.into_iter().enumerate() {
            assert_eq!(form(&history, index), want, "{text:?}");
            assert_eq!(
                usize::from(history.head(index).unwrap().len),
                len,
                "{text:?}"
            );
            let grid = grid_row(40, text);
            assert!(same(
                &owned(&history, index),
                &grid.row(0).unwrap().to_row()
            ));
        }
        // Printing a character at a time clears the uniform flag, but an
        // ASCII row in one style is still stored a byte a cell.
        let grid = grid_row(40, "x");
        assert!(!grid.page.row(0).unwrap().is_uniform());
        push_grid(&mut history, &grid);
        assert_eq!(form(&history, 4), Form::Ascii);
    }

    /// Per-row eviction, the spare page, truncation and draining, on pages
    /// of whatever size the test has set.
    fn evict_reuse_truncate_drain() {
        let mut history = History::default();
        let page = history.page_rows();
        let mut packer = RowPacker::new();
        let row_of = |n: usize| {
            let mut row = Row::new(8);
            row.write_ascii(0, format!("{n:>8}").as_bytes(), Cell::default());
            row
        };
        for n in 0..3 * page {
            history.push_row(&row_of(n), &mut packer);
        }
        assert_eq!(history.pages.len(), 3);
        for n in 0..page + 1 {
            history.pop_front();
            history.check_integrity(16).unwrap();
            assert_eq!(history.len(), 3 * page - n - 1);
        }
        assert_eq!(history.pages.len(), 2, "the first page went");
        assert!(history.spare.is_some(), "and was kept for reuse");
        let first = history.row(0).unwrap();
        let mut text = String::new();
        first.push_text(&mut text);
        assert_eq!(text.trim(), (page + 1).to_string());

        // The oldest row is the second page's second, so keeping up to
        // `page - 1` rows leaves them all in that page.
        let keep = (page - 1).min(10);
        history.truncate(keep);
        history.check_integrity(16).unwrap();
        assert_eq!(history.len(), keep);
        assert_eq!(history.pages.len(), 1);
        history.push_row(&row_of(7), &mut packer);
        history.check_integrity(16).unwrap();
        assert_eq!(owned(&history, keep).cells().nth(7).unwrap().char(), '7');

        let dropped = keep / 2;
        history.drop_front(dropped);
        history.check_integrity(16).unwrap();
        let mut seen = Vec::new();
        history.clone().drain(|row| {
            let mut text = String::new();
            row.push_text(&mut text);
            seen.push(text.trim().to_string());
        });
        let want: Vec<String> = (page + 1 + dropped..page + 1 + keep)
            .map(|n| n.to_string())
            .chain(["7".to_string()])
            .collect();
        assert_eq!(seen, want);

        history.clear();
        history.check_integrity(16).unwrap();
        assert_eq!(history.len(), 0);
        assert!(history.row(0).is_none());
    }

    #[test]
    fn rows_are_evicted_one_at_a_time_and_pages_reused() {
        evict_reuse_truncate_drain();
        for rows in [2, 3, 4, 7] {
            let _rows = test_support::page_rows(rows);
            evict_reuse_truncate_drain();
        }
    }

    #[test]
    fn a_full_history_scrolls_without_allocating_pages() {
        let mut history = History::default();
        let grid = grid_row(40, "steady state");
        for _ in 0..4 * PAGE_ROWS {
            if history.len() >= 2 * PAGE_ROWS {
                history.pop_front();
            }
            push_grid(&mut history, &grid);
        }
        let bytes = history.heap_bytes();
        for _ in 0..8 * PAGE_ROWS {
            history.pop_front();
            push_grid(&mut history, &grid);
        }
        assert_eq!(history.heap_bytes(), bytes);
    }

    #[test]
    fn styles_are_remapped_into_each_page() {
        let mut history = History::default();
        let mut grid = crate::grid::Grid::new(crate::grid::Dimensions::new(10, 1));
        grid.attrs_mut().set_fg(Color::rgb(1, 2, 3));
        grid.write_ascii_span(b"rgb");
        grid.attrs_mut().set_fg(Color::Indexed(9));
        grid.write_ascii_span(b"idx");
        for _ in 0..PAGE_ROWS + 1 {
            push_grid(&mut history, &grid);
        }
        history.check_integrity(16).unwrap();
        let want = grid.row(0).unwrap().to_row();
        assert!(same(&owned(&history, 0), &want));
        assert!(same(&owned(&history, PAGE_ROWS), &want));
        // One truecolor style per page, found again through the remap.
        assert_eq!(history.pages[0].styles.len(), 2);
        assert_eq!(history.pages[1].styles.len(), 2);
    }

    #[test]
    fn rewriting_a_row_keeps_its_place_and_compacts_garbage() {
        let mut history = History::default();
        let mut packer = RowPacker::new();
        let wide = |c: char| {
            let mut row = Row::new(60);
            for col in 0..60 {
                let mut cell = Cell::new(c);
                cell.set_fg(Color::rgb(col as u8, 0, 0));
                row[col] = cell;
            }
            row
        };
        for _ in 0..4 {
            history.push_row(&wide('a'), &mut packer);
        }
        let page = &history.pages[0];
        let before = page.total_bytes();
        for round in 0..100 {
            let c = if round % 2 == 0 { 'b' } else { 'c' };
            history.replace(1, &wide(c), &mut packer);
            history.check_integrity(16).unwrap();
            assert!(same(&owned(&history, 1), &wide(c)));
            assert!(same(&owned(&history, 2), &wide('a')));
        }
        let page = &history.pages[0];
        assert!(
            page.total_bytes() <= 3 * before,
            "{} bytes",
            page.total_bytes()
        );
        assert_eq!(
            page.styles.len(),
            61,
            "rewrites find the page's styles again"
        );
    }

    #[test]
    fn compacting_keeps_grapheme_extras_with_their_rows() {
        let mut history = History::default();
        let mut packer = RowPacker::new();
        let texts = [
            "a\u{301}b c\u{302}\u{303}",
            "plain",
            "日本 e\u{301}語 x",
            "👍\u{1f3fd} 🏳\u{fe0f}\u{200d}🌈",
        ];
        for text in texts {
            push_grid(&mut history, &grid_row(20, text));
        }
        // Rewrite the first and third rows so their old payloads, extras
        // included, become garbage in the middle of the page.
        let rewritten = [
            (0, "x\u{308}\u{30a} y\u{301}"),
            (2, "z\u{301}\u{302}\u{303}"),
        ];
        let mut want: Vec<Row> = texts
            .iter()
            .map(|text| grid_row(20, text).row(0).unwrap().to_row())
            .collect();
        for (index, text) in rewritten {
            let row = grid_row(20, text).row(0).unwrap().to_row();
            history.replace(index, &row, &mut packer);
            want[index] = row;
        }
        let page = &history.pages[0];
        assert!(page.garbage > 0);
        let live: usize = page.heads.iter().map(|head| page.payload_bytes(head)).sum();
        assert!(page.total_bytes() > live);

        history.pages[0].compact();
        history.check_integrity(16).unwrap();
        let page = &history.pages[0];
        assert_eq!(page.garbage, 0);
        assert_eq!(page.total_bytes(), live, "only live payload is left");
        for (index, want) in want.iter().enumerate() {
            assert!(same(&owned(&history, index), want), "row {index}");
            let row = history.row(index).unwrap();
            for col in 0..want.width() {
                assert_eq!(
                    row.grapheme_extra(col),
                    want.grapheme_extra(col),
                    "row {index} col {col}"
                );
            }
        }
    }

    #[test]
    fn taking_rows_back_off_the_end_gives_their_payload_back() {
        let mut history = History::default();
        let grid = grid_row(40, "日本 e\u{301}語 x");
        for _ in 0..3 {
            push_grid(&mut history, &grid);
        }
        let page = &history.pages[0];
        let (cells, extras, chars) = (page.cells.len(), page.extras.len(), page.extra_chars.len());
        history.truncate(2);
        let page = &history.pages[0];
        assert_eq!(page.cells.len(), cells / 3 * 2);
        assert_eq!(page.extras.len(), extras / 3 * 2);
        assert_eq!(page.extra_chars.len(), chars / 3 * 2);
        assert_eq!(page.garbage, 0);
        history.check_integrity(16).unwrap();
    }

    #[test]
    fn a_typical_line_is_small() {
        let mut history = History::default();
        let grid = grid_row(120, "the quick brown fox jumps over the lazy dog");
        push_grid(&mut history, &grid);
        assert_eq!(history.pages[0].ascii.len(), 43);
        assert_eq!(history.pages[0].styles.len(), 1);
    }

    #[test]
    fn heads_stay_small() {
        // Scrollback keeps tens of thousands of these.
        assert_eq!(std::mem::size_of::<Head>(), 24);
    }
}
