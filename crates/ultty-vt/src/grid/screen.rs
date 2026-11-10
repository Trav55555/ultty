//! The screen page (ADR 0022 §2 and §4, migration slice 4).
//!
//! Every screen row's packed cells live in one allocation, a slot of `cols`
//! cells each, and screen row `r` shows slot `slots[r]`. Scrolling, scroll
//! regions, IL and DL rotate a sub-slice of `slots` (two bytes a row) and
//! clear the slots that come in, so moving rows copies no cells and touches
//! no heap. Each slot has a [`RowHead`]: what [`super::Row`] keeps besides
//! its cells (`used`, `uniform` and its style, wrap, marks, anchor).
//!
//! Grapheme extras live in a [`GraphemeStore`] per slot, keyed by column,
//! so they move with their slot.
//!
//! [`ScreenRow`] and [`ScreenRowMut`] are one slot borrowed with the store:
//! the row operations the grid uses.

use super::packed::{PackedCell, StyleKey, StyleTable};
use super::row::{AnchorTag, Row, RowMarks};

/// What a slot keeps besides its cells.
#[derive(Debug, Clone, Copy)]
pub(super) struct RowHead {
    /// The style of a uniform row; meaningless otherwise.
    uniform_key: StyleKey,
    /// Every cell at or after this column is [`PackedCell::BLANK`]. May
    /// overstate, never understates.
    used: u16,
    flags: u8,
    marks: RowMarks,
    anchor: AnchorTag,
}

impl RowHead {
    const WRAPPED: u8 = 1 << 0;
    /// Every cell before `used` is ASCII with no structure bits, in style
    /// `uniform_key`. Like `used`, it may understate (anything writing a
    /// cell directly clears it) but never overstates, so history can copy
    /// such a row's text and style it once.
    const UNIFORM: u8 = 1 << 1;
    /// The store may hold extras for this slot. May overstate.
    const GRAPHEMES: u8 = 1 << 2;

    const BLANK: RowHead = RowHead {
        uniform_key: StyleKey::DEFAULT,
        used: 0,
        flags: Self::UNIFORM,
        marks: RowMarks {
            input_start: 0,
            output_start: 0,
            command_end: 0,
        },
        anchor: AnchorTag::NONE,
    };

    #[inline]
    fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
}

/// One entry of a [`GraphemeStore`]: `len` codepoints from `start`.
#[derive(Debug, Clone, Copy)]
struct Extra {
    col: u16,
    len: u8,
    start: u32,
}

/// Extra codepoints of one slot's multi-codepoint graphemes, by column.
///
/// `index` is sorted by column, and every entry's codepoints are a run of
/// `chars`. A codepoint is appended in place when its cluster's run ends
/// `chars`, which is the case while a cluster is being printed; otherwise
/// the run moves to the end and the old one becomes garbage. Garbage is
/// compacted away in place once it outgrows the live codepoints. Erasing
/// the slot empties both vectors and keeps their capacity, so a slot that
/// has held graphemes holds them again without allocating.
///
/// An entry counts only while its cell has the grapheme bit; entries for
/// other cells may linger until their cells are cleared.
///
/// ADR 0022 §4 drew one store per page, sorted by cell index. With slot
/// rotation a slot's cells sit anywhere in that order, so writing or
/// erasing a row moved the index of every row after it: the combining
/// corpus took twice the instructions. One store per slot keeps every
/// change to a row inside the row.
#[derive(Debug, Clone, Default)]
pub(super) struct GraphemeStore {
    index: Vec<Extra>,
    chars: Vec<char>,
    garbage: usize,
}

impl GraphemeStore {
    /// Garbage below this many codepoints is never compacted.
    const MIN_GARBAGE: usize = 32;

    /// The index of `col`'s entry, or where it would be inserted.
    #[inline]
    fn find(&self, col: u16) -> Result<usize, usize> {
        match self.index.last() {
            None => Err(0),
            Some(last) if last.col < col => Err(self.index.len()),
            Some(last) if last.col == col => Ok(self.index.len() - 1),
            Some(_) => self.index.binary_search_by_key(&col, |extra| extra.col),
        }
    }

    fn get(&self, col: u16) -> Option<&[char]> {
        let extra = self.index[self.find(col).ok()?];
        let start = extra.start as usize;
        Some(&self.chars[start..start + usize::from(extra.len)])
    }

    /// Drop every entry.
    #[inline]
    fn clear(&mut self) {
        self.index.clear();
        self.chars.clear();
        self.garbage = 0;
    }

    /// Drop the entries for columns in `start..end`.
    #[inline]
    fn remove_range(&mut self, start: u16, end: u16) {
        // Writes usually land after every cluster on the row.
        if self.index.last().is_none_or(|last| last.col < start) {
            return;
        }
        self.remove_range_slow(start, end);
    }

    #[inline(never)]
    fn remove_range_slow(&mut self, start: u16, end: u16) {
        let first = self.index.partition_point(|extra| extra.col < start);
        let past = first + self.index[first..].partition_point(|extra| extra.col < end);
        self.drop_entries(first..past);
    }

    fn drop_entries(&mut self, entries: std::ops::Range<usize>) {
        if entries.is_empty() {
            return;
        }
        if entries.len() == self.index.len() {
            self.clear();
            return;
        }
        self.garbage += self.index[entries.clone()]
            .iter()
            .map(|extra| usize::from(extra.len))
            .sum::<usize>();
        self.index.drain(entries);
    }

    /// Append `c` to `col`'s codepoints, emptying them first when `fresh`
    /// (they belonged to earlier content). At most `cap` are kept; returns
    /// whether `c` was.
    fn push(&mut self, col: u16, c: char, cap: usize, fresh: bool) -> bool {
        let at = match self.find(col) {
            Ok(at) => at,
            Err(at) => {
                let start = self.chars.len() as u32;
                self.index.insert(at, Extra { col, len: 0, start });
                at
            }
        };
        if fresh {
            let extra = &mut self.index[at];
            self.garbage += usize::from(extra.len);
            extra.len = 0;
            extra.start = self.chars.len() as u32;
        }
        let len = usize::from(self.index[at].len);
        if len >= cap.min(usize::from(u8::MAX)) {
            return false;
        }
        if self.garbage >= Self::MIN_GARBAGE && self.garbage > self.chars.len() / 2 {
            self.compact();
        }
        let extra = self.index[at];
        let start = extra.start as usize;
        if start + len != self.chars.len() {
            // Not the run that ends `chars`: move it to the end.
            self.chars.extend_from_within(start..start + len);
            self.garbage += len;
            self.index[at].start = (self.chars.len() - len) as u32;
        }
        self.chars.push(c);
        self.index[at].len += 1;
        true
    }

    /// Make `col`'s codepoints `chars` (at most 255 of them).
    fn insert(&mut self, col: u16, chars: &[char]) {
        let chars = &chars[..chars.len().min(usize::from(u8::MAX))];
        let extra = Extra {
            col,
            len: chars.len() as u8,
            start: self.chars.len() as u32,
        };
        self.chars.extend_from_slice(chars);
        match self.find(col) {
            Ok(at) => {
                self.garbage += usize::from(self.index[at].len);
                self.index[at] = extra;
            }
            Err(at) => self.index.insert(at, extra),
        }
    }

    /// Move the entries for columns at or after `from` by `delta`, dropping
    /// those that land outside `0..width`.
    fn shift(&mut self, from: u16, delta: i32, width: u16) {
        let first = self.index.partition_point(|extra| extra.col < from);
        for extra in &mut self.index[first..] {
            // Out-of-range columns are dropped below, so clamp rather than
            // wrap.
            extra.col = (i32::from(extra.col) + delta).clamp(0, i32::from(width)) as u16;
        }
        let keep = first + self.index[first..].partition_point(|extra| extra.col < width);
        let past = self.index.len();
        self.drop_entries(keep..past);
    }

    /// Move the live runs to the front of `chars`, in place: in the order
    /// they start, each run moves down or stays.
    fn compact(&mut self) {
        self.index.sort_unstable_by_key(|extra| extra.start);
        let mut next = 0;
        for extra in &mut self.index {
            let start = extra.start as usize;
            let len = usize::from(extra.len);
            self.chars.copy_within(start..start + len, next);
            extra.start = next as u32;
            next += len;
        }
        self.chars.truncate(next);
        self.garbage = 0;
        self.index.sort_unstable_by_key(|extra| extra.col);
    }

    fn check_integrity(&self) -> Result<(), String> {
        if self.index.windows(2).any(|pair| pair[0].col >= pair[1].col) {
            return Err("grapheme index is not sorted by column".into());
        }
        let live: usize = self.index.iter().map(|extra| usize::from(extra.len)).sum();
        if live + self.garbage != self.chars.len() {
            return Err(format!(
                "grapheme store holds {} codepoints, {live} live and {} garbage",
                self.chars.len(),
                self.garbage
            ));
        }
        if let Some(extra) = self
            .index
            .iter()
            .find(|extra| extra.start as usize + usize::from(extra.len) > self.chars.len())
        {
            return Err(format!("grapheme entry {extra:?} runs past the store"));
        }
        Ok(())
    }
}

/// The screen: every row's cells in one allocation, shown through `slots`.
#[derive(Debug, Clone)]
pub(super) struct ScreenPage {
    cols: u16,
    /// `slots.len() * cols` cells; slot `s` is `s * cols..(s + 1) * cols`.
    cells: Vec<PackedCell>,
    /// Screen row `r` shows slot `slots[r]`.
    slots: Vec<u16>,
    /// Per slot, together so that one bounds check reaches both.
    heads: Vec<(RowHead, GraphemeStore)>,
}

impl ScreenPage {
    /// A page of blank rows.
    pub(super) fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            cells: vec![PackedCell::BLANK; usize::from(cols) * usize::from(rows)],
            slots: (0..rows).collect(),
            heads: vec![(RowHead::BLANK, GraphemeStore::default()); usize::from(rows)],
        }
    }

    #[inline]
    pub(super) fn cols(&self) -> u16 {
        self.cols
    }

    #[inline]
    pub(super) fn rows(&self) -> u16 {
        self.slots.len() as u16
    }

    /// The number of cells, for sizing the style table's collection.
    pub(super) fn cell_count(&self) -> usize {
        self.cells.len()
    }

    #[inline]
    fn slot_parts(&self, row: u16) -> Option<(usize, usize)> {
        let slot = usize::from(*self.slots.get(usize::from(row))?);
        Some((slot, slot * usize::from(self.cols)))
    }

    /// Screen row `row`.
    #[inline]
    pub(super) fn row(&self, row: u16) -> Option<ScreenRow<'_>> {
        let (slot, start) = self.slot_parts(row)?;
        let (head, graphemes) = &self.heads[slot];
        Some(ScreenRow {
            cells: &self.cells[start..start + usize::from(self.cols)],
            head,
            graphemes,
        })
    }

    /// Screen row `row`, to change.
    #[inline]
    pub(super) fn row_mut(&mut self, row: u16) -> Option<ScreenRowMut<'_>> {
        let (slot, start) = self.slot_parts(row)?;
        let (head, graphemes) = &mut self.heads[slot];
        Some(ScreenRowMut {
            cells: &mut self.cells[start..start + usize::from(self.cols)],
            head,
            graphemes,
        })
    }

    /// Every row, top to bottom.
    pub(super) fn iter(&self) -> impl Iterator<Item = ScreenRow<'_>> {
        (0..self.rows()).filter_map(|row| self.row(row))
    }

    /// Erase every row in `rows` with blanks in style `key`.
    pub(super) fn erase_rows(&mut self, rows: std::ops::Range<u16>, key: StyleKey) {
        for row in rows {
            if let Some(mut r) = self.row_mut(row) {
                r.erase(key);
            }
        }
    }

    /// Move rows `top..=bottom` up by `n` (less than the range's height):
    /// the top `n` leave, and the bottom `n` come in erased in style `key`.
    pub(super) fn rotate_up(&mut self, top: u16, bottom: u16, n: u16, key: StyleKey) {
        let slots = &mut self.slots[usize::from(top)..usize::from(bottom) + 1];
        if n == 1 {
            // A line feed on the bottom margin. `rotate_left` costs about a
            // hundred instructions on a screen's worth of slots; this is a
            // short `memmove`.
            let first = slots[0];
            slots.copy_within(1.., 0);
            slots[slots.len() - 1] = first;
            if let Some(mut row) = self.row_mut(bottom) {
                row.erase(key);
            }
        } else {
            slots.rotate_left(usize::from(n));
            self.erase_rows(bottom + 1 - n..bottom + 1, key);
        }
    }

    /// Move rows `top..=bottom` down by `n` (less than the range's height):
    /// the bottom `n` leave, and the top `n` come in erased in style `key`.
    pub(super) fn rotate_down(&mut self, top: u16, bottom: u16, n: u16, key: StyleKey) {
        let slots = &mut self.slots[usize::from(top)..usize::from(bottom) + 1];
        if n == 1 {
            // A reverse index on the top margin, or one inserted line.
            let last = slots[slots.len() - 1];
            slots.copy_within(..slots.len() - 1, 1);
            slots[0] = last;
            if let Some(mut row) = self.row_mut(top) {
                row.erase(key);
            }
        } else {
            slots.rotate_right(usize::from(n));
            self.erase_rows(top..top + n, key);
        }
    }

    /// The page at `cols` by `rows`: each row cut or padded with blanks
    /// at its end, rows dropped from or added at the bottom.
    pub(super) fn resized(&self, cols: u16, rows: u16) -> Self {
        let mut page = Self::new(cols, rows);
        for (row, from) in self.iter().take(usize::from(rows)).enumerate() {
            page.row_mut(row as u16)
                .expect("the new page has this row")
                .copy_from(from);
        }
        page
    }

    /// Call `f` on every style key the page holds: each uniform row's and
    /// each used cell's.
    pub(super) fn for_each_key(&self, mut f: impl FnMut(StyleKey)) {
        let cols = usize::from(self.cols);
        for (slot, (head, _)) in self.heads.iter().enumerate() {
            if head.flags & RowHead::UNIFORM != 0 {
                f(head.uniform_key);
            }
            let start = slot * cols;
            for cell in &self.cells[start..start + usize::from(head.used)] {
                f(cell.key());
            }
        }
    }

    /// Replace every style key the page holds with `f` of it, and forget
    /// the uniform style of rows that are not uniform.
    pub(super) fn remap_keys(&mut self, f: impl Fn(StyleKey) -> StyleKey) {
        let cols = usize::from(self.cols);
        for (slot, (head, _)) in self.heads.iter_mut().enumerate() {
            head.uniform_key = if head.flags & RowHead::UNIFORM != 0 {
                f(head.uniform_key)
            } else {
                StyleKey::DEFAULT
            };
            let start = slot * cols;
            for cell in &mut self.cells[start..start + usize::from(head.used)] {
                let key = cell.key();
                let new = f(key);
                if new != key {
                    *cell = cell.with_key(new);
                }
            }
        }
    }

    /// Check the page's own invariants and every row's (see
    /// [`ScreenRow::check_integrity`]).
    pub(super) fn check_integrity(
        &self,
        max_extra: usize,
        styles: &StyleTable,
    ) -> Result<(), String> {
        let rows = self.slots.len();
        if self.heads.len() != rows || self.cells.len() != rows * usize::from(self.cols) {
            return Err(format!(
                "page of {rows} rows has {} heads and {} cells at {} columns",
                self.heads.len(),
                self.cells.len(),
                self.cols
            ));
        }
        let mut seen = vec![false; rows];
        for &slot in &self.slots {
            match seen.get_mut(usize::from(slot)) {
                Some(seen) if !*seen => *seen = true,
                _ => return Err(format!("slot {slot} is shown twice or is out of range")),
            }
        }
        for (index, row) in self.iter().enumerate() {
            row.check_integrity(max_extra, styles)
                .map_err(|e| format!("row {index}: {e}"))?;
        }
        Ok(())
    }
}

/// One screen row, borrowed from its page.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScreenRow<'a> {
    cells: &'a [PackedCell],
    head: &'a RowHead,
    graphemes: &'a GraphemeStore,
}

impl<'a> ScreenRow<'a> {
    #[inline]
    pub(super) fn width(self) -> u16 {
        self.cells.len() as u16
    }

    #[inline]
    pub(super) fn cells(self) -> &'a [PackedCell] {
        self.cells
    }

    #[inline]
    pub(super) fn get(self, col: u16) -> Option<PackedCell> {
        self.cells.get(usize::from(col)).copied()
    }

    /// The cells up to the used mark: every cell after them is blank.
    #[inline]
    pub(super) fn used_cells(self) -> &'a [PackedCell] {
        &self.cells[..usize::from(self.head.used)]
    }

    /// Whether every used cell is ASCII with no structure in
    /// [`ScreenRow::uniform_key`]. `false` does not mean some cell is not.
    #[inline]
    pub(super) fn is_uniform(self) -> bool {
        self.head.flags & RowHead::UNIFORM != 0
    }

    #[inline]
    pub(super) fn uniform_key(self) -> StyleKey {
        self.head.uniform_key
    }

    #[inline]
    pub(super) fn is_wrapped(self) -> bool {
        self.head.flags & RowHead::WRAPPED != 0
    }

    #[inline]
    pub(super) fn marks(self) -> RowMarks {
        self.head.marks
    }

    #[inline]
    pub(super) fn anchor(self) -> AnchorTag {
        self.head.anchor
    }

    /// Codepoints following the base character of the cluster at `col`, if
    /// the cell holds a multi-codepoint grapheme.
    #[inline]
    pub(super) fn grapheme_extra(self, col: u16) -> Option<&'a [char]> {
        if !self.get(col)?.has_grapheme() {
            return None;
        }
        self.graphemes.get(col)
    }

    /// The full grapheme at `col`: base character plus any extra codepoints.
    pub(super) fn grapheme(self, col: u16) -> Option<String> {
        let cell = self.get(col)?;
        let mut s = String::new();
        s.push(cell.char());
        if let Some(extra) = self.grapheme_extra(col) {
            s.extend(extra);
        }
        Some(s)
    }

    /// Run `f` on the grapheme at `col` followed by `then`, as one string.
    /// Clusters of the length a cell can hold are assembled on the stack,
    /// so asking about the cluster under the cursor does not allocate.
    pub(super) fn with_grapheme_str<R>(
        self,
        col: u16,
        then: Option<char>,
        f: impl FnOnce(&str) -> R,
    ) -> Option<R> {
        let cell = self.get(col)?;
        let extra = self.grapheme_extra(col).unwrap_or(&[]);
        let chars = std::iter::once(cell.char())
            .chain(extra.iter().copied())
            .chain(then);
        let mut buf = [0u8; 4 * 20];
        let mut len = 0;
        for c in chars.clone() {
            if len + c.len_utf8() > buf.len() {
                return Some(f(&chars.collect::<String>()));
            }
            len += c.encode_utf8(&mut buf[len..]).len();
        }
        Some(match std::str::from_utf8(&buf[..len]) {
            Ok(s) => f(s),
            // Whole characters were encoded, so this does not happen.
            Err(_) => f(&chars.collect::<String>()),
        })
    }

    /// Check the row's invariants: those of [`Row::check_integrity`], plus
    /// a valid style key in every cell, a uniform row that is uniform, and
    /// grapheme data behind every grapheme bit.
    pub(super) fn check_integrity(
        self,
        max_extra: usize,
        styles: &StyleTable,
    ) -> Result<(), String> {
        let cells = self.cells;
        let used = usize::from(self.head.used).min(cells.len());
        if usize::from(self.head.used) > cells.len() {
            return Err(format!(
                "used mark {} is past the row's {} cells",
                self.head.used,
                cells.len()
            ));
        }
        if let Some(col) = cells[used..]
            .iter()
            .position(|&cell| cell != PackedCell::BLANK)
        {
            return Err(format!(
                "cell at col {} is past the used mark {used} but not blank",
                used + col
            ));
        }
        if self.is_uniform() {
            styles
                .check_key(self.head.uniform_key)
                .map_err(|e| format!("uniform style: {e}"))?;
            if let Some(col) = cells[..used]
                .iter()
                .position(|cell| !cell.is_uniform_ascii() || cell.key() != self.head.uniform_key)
            {
                return Err(format!(
                    "row is marked uniform but cell at col {col} ({:?}) is not",
                    cells[col]
                ));
            }
        }
        self.graphemes.check_integrity()?;
        for (col, &cell) in cells.iter().enumerate() {
            styles
                .check_key(cell.key())
                .map_err(|e| format!("cell at col {col}: {e}"))?;
            if cell.is_wide() && cell.is_spacer() {
                return Err(format!("cell at col {col} is both wide and a spacer"));
            }
            if cell.is_wide() && !cells.get(col + 1).is_some_and(|next| next.is_spacer()) {
                return Err(format!("wide cell at col {col} has no spacer after it"));
            }
            if cell.is_spacer() && !(col > 0 && cells[col - 1].is_wide()) {
                return Err(format!("spacer at col {col} does not follow a wide cell"));
            }
            if cell.has_grapheme() {
                if self.head.flags & RowHead::GRAPHEMES == 0 {
                    return Err(format!(
                        "cell at col {col} has the grapheme bit in a row marked as having none"
                    ));
                }
                match self.graphemes.get(col as u16) {
                    None | Some([]) => {
                        return Err(format!(
                            "cell at col {col} has the grapheme bit but no extra codepoints"
                        ));
                    }
                    Some(extra) if extra.len() > max_extra => {
                        return Err(format!(
                            "cell at col {col} holds {} grapheme codepoints (max {max_extra})",
                            extra.len()
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(())
    }
}

/// One screen row, borrowed from its page to change.
#[derive(Debug)]
pub(super) struct ScreenRowMut<'a> {
    cells: &'a mut [PackedCell],
    head: &'a mut RowHead,
    graphemes: &'a mut GraphemeStore,
}

impl ScreenRowMut<'_> {
    /// The row as it is now, to read.
    #[inline]
    pub(super) fn as_ref(&self) -> ScreenRow<'_> {
        ScreenRow {
            cells: self.cells,
            head: self.head,
            graphemes: self.graphemes,
        }
    }

    #[inline]
    pub(super) fn width(&self) -> u16 {
        self.cells.len() as u16
    }

    #[inline]
    pub(super) fn get(&self, col: u16) -> Option<PackedCell> {
        self.cells.get(usize::from(col)).copied()
    }

    #[inline]
    pub(super) fn is_wrapped(&self) -> bool {
        self.head.flags & RowHead::WRAPPED != 0
    }

    #[inline]
    pub(super) fn set_wrapped(&mut self, wrapped: bool) {
        self.head.set_flag(RowHead::WRAPPED, wrapped);
    }

    #[inline]
    pub(super) fn marks(&self) -> RowMarks {
        self.head.marks
    }

    #[inline]
    pub(super) fn set_marks(&mut self, marks: RowMarks) {
        self.head.marks = marks;
    }

    #[inline]
    pub(super) fn anchor(&self) -> AnchorTag {
        self.head.anchor
    }

    #[inline]
    pub(super) fn set_anchor(&mut self, anchor: AnchorTag) {
        self.head.anchor = anchor;
    }

    /// Record that cells before `end` may no longer be blank.
    #[inline]
    fn mark_used(&mut self, end: usize) {
        let end = end.min(self.cells.len()) as u16;
        self.head.used = self.head.used.max(end);
    }

    /// Set the cell at `col`, if there is one, as a direct edit: the row
    /// is no longer known to be uniform.
    #[inline]
    pub(super) fn set(&mut self, col: u16, cell: PackedCell) {
        if let Some(slot) = self.cells.get_mut(usize::from(col)) {
            *slot = cell;
            self.head.flags &= !RowHead::UNIFORM;
            self.mark_used(usize::from(col) + 1);
        }
    }

    /// Write printable ASCII `bytes` from `col` in style `key`. The caller
    /// has resolved any wide character straddling the span's edges (see
    /// [`ScreenRowMut::clear_ascii_edges`]); the span must fit on the row.
    #[inline]
    pub(super) fn write_ascii(&mut self, col: u16, bytes: &[u8], key: StyleKey) {
        let start = usize::from(col);
        let end = start + bytes.len();
        self.drop_grapheme_entries(col, end as u16);
        let used = usize::from(self.head.used);
        if start == 0 && end >= used {
            // Every used cell is about to be in `key`.
            self.head.flags |= RowHead::UNIFORM;
            self.head.uniform_key = key;
        } else if self.head.uniform_key != key || (start > used && !key.is_default()) {
            // Cells in another style before or after the span, or blanks
            // between the used mark and the span.
            self.head.flags &= !RowHead::UNIFORM;
        }
        let base = key.base();
        for (cell, &byte) in self.cells[start..end].iter_mut().zip(bytes) {
            *cell = PackedCell::ascii(base, byte);
        }
        self.head.used = self.head.used.max(end as u16);
    }

    /// Blank the halves of wide characters an ASCII span over `start..end`
    /// would cut: the character before a spacer at `start`, and a spacer
    /// at `end`. Every cell in between is overwritten, so a wide character
    /// can only be split at the two edges.
    #[inline]
    pub(super) fn clear_ascii_edges(&mut self, start: u16, end: u16) {
        if start > 0 && self.get(start).is_some_and(PackedCell::is_spacer) {
            self.set(start - 1, PackedCell::BLANK);
        }
        if self.get(end).is_some_and(PackedCell::is_spacer) {
            self.set(end, PackedCell::BLANK);
        }
    }

    /// Write `c`, taking `width` (1 or 2) columns from `col`, in style
    /// `key` (and a spacer after a wide character), if those columns hold
    /// no part of a wide character and fit on the row. Returns `false`,
    /// having changed nothing, otherwise.
    #[inline]
    pub(super) fn write_char_in_place(
        &mut self,
        col: u16,
        c: char,
        width: u16,
        key: StyleKey,
    ) -> bool {
        let start = usize::from(col);
        let end = start + usize::from(width);
        let Some(cells) = self.cells.get_mut(start..end) else {
            return false;
        };
        if cells.iter().any(|cell| cell.is_wide_part()) {
            return false;
        }
        if width > 1 {
            cells[0] = PackedCell::wide(c, key);
            cells[1] = PackedCell::spacer(key);
        } else {
            cells[0] = PackedCell::new(c, key);
        }
        self.drop_grapheme_entries(col, col + 1);
        self.head.flags &= !RowHead::UNIFORM;
        self.mark_used(end);
        true
    }

    /// Codepoints following the base character of the cluster at `col`, if
    /// the cell holds a multi-codepoint grapheme.
    #[inline]
    pub(super) fn grapheme_extra(&self, col: u16) -> Option<&[char]> {
        if !self.get(col)?.has_grapheme() {
            return None;
        }
        self.graphemes.get(col)
    }

    /// Append a codepoint to the grapheme cluster at `col`. At most `cap`
    /// extra codepoints are kept per cell. Returns whether `c` was kept.
    pub(super) fn push_grapheme_char(&mut self, col: u16, c: char, cap: usize) -> bool {
        let Some(cell) = self.cells.get_mut(usize::from(col)) else {
            return false;
        };
        self.head.flags &= !RowHead::UNIFORM;
        self.head.flags |= RowHead::GRAPHEMES;
        // Whatever is stored for a cell without the bit belonged to earlier
        // content.
        let fresh = !cell.has_grapheme();
        if fresh {
            *cell = cell.with_grapheme();
            self.head.used = self.head.used.max(col + 1);
        }
        self.graphemes.push(col, c, cap, fresh)
    }

    /// Drop grapheme data for columns in `start..end` and clear their bits.
    pub(super) fn clear_graphemes(&mut self, start: u16, end: u16) {
        if start >= end {
            return;
        }
        self.drop_grapheme_entries(start, end);
        let end = usize::from(end).min(self.cells.len());
        for cell in self
            .cells
            .get_mut(usize::from(start)..end)
            .into_iter()
            .flatten()
        {
            *cell = cell.without_grapheme();
        }
    }

    /// Remove stored grapheme data for `start..end` without touching cells.
    #[inline]
    fn drop_grapheme_entries(&mut self, start: u16, end: u16) {
        if start < end && self.head.flags & RowHead::GRAPHEMES != 0 {
            self.graphemes.remove_range(start, end);
        }
    }

    /// Drop every grapheme entry of the row.
    #[inline]
    fn drop_all_graphemes(&mut self) {
        if self.head.flags & RowHead::GRAPHEMES != 0 {
            self.graphemes.clear();
            self.head.flags &= !RowHead::GRAPHEMES;
        }
    }

    /// Erase the row per ECMA-48 with blanks in style `key` (the erase
    /// style, background only): no grapheme data, marks, anchor or wrap.
    #[inline]
    pub(super) fn erase(&mut self, key: StyleKey) {
        self.drop_all_graphemes();
        if key.is_default() {
            // Cells past the used mark are already blank.
            self.cells[..usize::from(self.head.used)].fill(PackedCell::BLANK);
            self.head.used = 0;
        } else {
            self.cells.fill(PackedCell::blank(key));
            self.head.used = self.width();
        }
        // Uniform, no longer wrapped and no graphemes, with no marks or
        // anchor: every scroll erases a row.
        *self.head = RowHead {
            uniform_key: key,
            used: self.head.used,
            ..RowHead::BLANK
        };
    }

    /// Erase cells `start..end` per ECMA-48: blanks in style `key`, with no
    /// grapheme data.
    pub(super) fn erase_range(&mut self, start: u16, end: u16, key: StyleKey) {
        let end = end.min(self.width());
        if start >= end {
            return;
        }
        self.drop_grapheme_entries(start, end);
        let used = self.head.used;
        if key.is_default() {
            // Cells past the used mark are already blank.
            let fill_end = end.min(used);
            if start < fill_end {
                self.cells[usize::from(start)..usize::from(fill_end)].fill(PackedCell::BLANK);
            }
            if end >= used {
                self.head.used = used.min(start);
            } else if !self.head.uniform_key.is_default() {
                // Default blanks inside a row of another style.
                self.head.flags &= !RowHead::UNIFORM;
            }
        } else {
            if start == 0 && end >= used {
                // Every used cell is about to be a blank in `key`; the
                // grapheme entries in range are gone, and no cell past
                // `end` is used, so no wide or grapheme state remains.
                self.head.flags |= RowHead::UNIFORM;
                self.head.uniform_key = key;
            } else if self.head.uniform_key != key || start > used {
                self.head.flags &= !RowHead::UNIFORM;
            }
            self.cells[usize::from(start)..usize::from(end)].fill(PackedCell::blank(key));
            self.mark_used(usize::from(end));
        }
    }

    /// Fill the row with `c` in the default style, as DECALN does.
    pub(super) fn fill_with_char(&mut self, c: char) {
        self.erase(StyleKey::DEFAULT);
        self.cells.fill(PackedCell::new(c, StyleKey::DEFAULT));
        self.head.used = self.width();
        self.head.flags &= !RowHead::UNIFORM;
    }

    /// Insert `count` blanks in style `key` at `at`, shifting cells right.
    pub(super) fn insert_cells(&mut self, at: u16, count: u16, key: StyleKey) {
        let at = usize::from(at);
        let count = usize::from(count);
        let len = self.cells.len();
        if at >= len || count == 0 {
            return;
        }

        // Inserting at a wide char's spacer splits the pair: the primary
        // stays put and the spacer would be shifted away from it, so both
        // halves are cleared.
        if at > 0 && self.cells[at].is_spacer() {
            self.cells[at - 1] = PackedCell::BLANK;
            self.cells[at] = PackedCell::BLANK;
        }

        // The last cell that stays on the row lands in the final column. If
        // it is a wide char, its spacer is the first cell pushed off, so it
        // would be left without one.
        let first_pushed = len.saturating_sub(count);
        if first_pushed > at && self.cells[first_pushed - 1].is_wide() {
            self.cells[first_pushed - 1] = PackedCell::BLANK;
        }

        let src_end = len.saturating_sub(count);
        if at < src_end {
            self.cells.copy_within(at..src_end, at + count);
        }
        if self.head.flags & RowHead::GRAPHEMES != 0 {
            self.graphemes.shift(at as u16, count as i32, len as u16);
        }
        self.head.used = self.width();
        self.head.flags &= !RowHead::UNIFORM;
        self.cells[at..(at + count).min(len)].fill(PackedCell::blank(key));
    }

    /// Delete `count` cells at `at`, shifting the rest left and filling the
    /// end with blanks in style `key`.
    pub(super) fn delete_cells(&mut self, at: u16, count: u16, key: StyleKey) {
        let at = usize::from(at);
        let count = usize::from(count);
        let len = self.cells.len();
        if at >= len || count == 0 {
            return;
        }

        // Deleting from a wide char's spacer orphans its primary.
        if at > 0 && self.cells[at].is_spacer() {
            self.cells[at - 1] = PackedCell::BLANK;
        }
        // Ending in the middle of a wide char orphans its spacer.
        let end = (at + count).min(len);
        if end < len && self.cells[end].is_spacer() {
            self.cells[end] = PackedCell::BLANK;
        }

        let src_start = (at + count).min(len);
        if src_start < len {
            self.cells.copy_within(src_start..len, at);
        }
        self.drop_grapheme_entries(at as u16, src_start as u16);
        if self.head.flags & RowHead::GRAPHEMES != 0 {
            self.graphemes
                .shift(src_start as u16, -(count as i32), len as u16);
        }
        self.head.used = self.width();
        self.head.flags &= !RowHead::UNIFORM;

        // A count running past the end of the row vacates only cells from
        // `at` onward.
        self.cells[len.saturating_sub(count).max(at)..len].fill(PackedCell::blank(key));
    }

    /// Set the row's uniform flag and style from its used cells.
    fn settle_uniform(&mut self) {
        let used = &self.cells[..usize::from(self.head.used)];
        let key = used.first().map_or(StyleKey::DEFAULT, |cell| cell.key());
        let uniform = used
            .iter()
            .all(|cell| cell.is_uniform_ascii() && cell.key() == key);
        self.head.uniform_key = key;
        self.head.set_flag(RowHead::UNIFORM, uniform);
    }

    /// Make this row hold `row`'s cells (cut or padded to this row's
    /// width), graphemes, wrap, marks and anchor, interning its styles in
    /// `styles` (which never collects).
    pub(super) fn assign_row(&mut self, row: &Row, styles: &mut StyleTable) {
        self.drop_all_graphemes();
        let width = usize::from(row.width());
        for (packed, cell) in self.cells.iter_mut().zip(row.cells()) {
            *packed = styles.pack(cell);
        }
        if width < self.cells.len() {
            self.cells[width..].fill(PackedCell::BLANK);
        }
        for col in 0..self.width() {
            if self.cells[usize::from(col)].has_grapheme() {
                if let Some(extra) = row.grapheme_extra(col) {
                    self.graphemes.insert(col, extra);
                    self.head.flags |= RowHead::GRAPHEMES;
                }
            }
        }
        self.head.used = self
            .cells
            .iter()
            .rposition(|&cell| cell != PackedCell::BLANK)
            .map_or(0, |last| last as u16 + 1);
        self.settle_uniform();
        self.set_wrapped(row.is_wrapped());
        self.head.marks = row.marks();
        self.head.anchor = row.anchor();
    }

    /// Make this row a copy of `from` (in the same style table), cut or
    /// padded with blanks to this row's width. A cut is only a cut: it may
    /// leave half of a wide character, as resizing without reflow always
    /// has.
    pub(super) fn copy_from(&mut self, from: ScreenRow<'_>) {
        self.drop_all_graphemes();
        let width = self.cells.len().min(from.cells.len());
        self.cells[..width].copy_from_slice(&from.cells[..width]);
        self.cells[width..].fill(PackedCell::BLANK);
        let mut flags = from.head.flags & (RowHead::WRAPPED | RowHead::UNIFORM);
        for col in 0..width as u16 {
            if let Some(extra) = from.grapheme_extra(col) {
                self.graphemes.insert(col, extra);
                flags |= RowHead::GRAPHEMES;
            }
        }
        *self.head = RowHead {
            used: from.head.used.min(width as u16),
            flags,
            ..*from.head
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::cell::{CellFlags, Color};
    use crate::grid::packed::Style;

    /// A page of one row `width` wide, standing in for a lone row.
    fn row_page(width: u16) -> ScreenPage {
        ScreenPage::new(width, 1)
    }

    fn row(page: &mut ScreenPage) -> ScreenRowMut<'_> {
        page.row_mut(0).unwrap()
    }

    fn integrity(page: &ScreenPage, table: &StyleTable) -> Result<(), String> {
        page.check_integrity(16, table)
    }

    #[test]
    fn a_row_is_uniform_until_something_else_lands() {
        let mut table = StyleTable::new(0);
        let key = table.key(&Style::of(&{
            let mut cell = crate::grid::Cell::default();
            cell.set_fg(Color::Named(2));
            cell
        }));
        let mut page = row_page(10);
        let mut row = row(&mut page);
        row.write_ascii(0, b"abc", key);
        row.write_ascii(3, b"de", key);
        assert!(row.as_ref().is_uniform());
        row.erase_range(4, 10, StyleKey::DEFAULT);
        assert!(row.as_ref().is_uniform(), "erasing the tail keeps it");
        row.write_ascii(6, b"x", key);
        assert!(
            !row.as_ref().is_uniform(),
            "blanks before the span are another style"
        );

        row.write_ascii(0, b"0123456789", StyleKey::DEFAULT);
        assert!(
            row.as_ref().is_uniform(),
            "rewriting every used cell makes it uniform"
        );
        row.write_ascii(2, b"q", key);
        assert!(!row.as_ref().is_uniform());

        row.erase(key);
        assert!(row.as_ref().is_uniform());
        assert_eq!(row.as_ref().uniform_key(), key);
        assert!(row.write_char_in_place(0, 'z', 1, key));
        assert!(!row.as_ref().is_uniform(), "any other write gives up on it");
    }

    #[test]
    fn insert_at_a_spacer_clears_both_halves() {
        let mut page = row_page(6);
        let mut row = row(&mut page);
        row.write_ascii(0, b"ab", StyleKey::DEFAULT);
        assert!(row.write_char_in_place(2, '中', 2, StyleKey::DEFAULT));
        row.insert_cells(3, 1, StyleKey::DEFAULT);
        assert_eq!(row.get(2), Some(PackedCell::BLANK));
        integrity(&page, &StyleTable::new(0)).unwrap();
    }

    #[test]
    fn insert_that_pushes_off_a_spacer_clears_its_wide_char() {
        let mut page = row_page(4);
        let mut row = row(&mut page);
        row.write_ascii(0, b"ab", StyleKey::DEFAULT);
        assert!(row.write_char_in_place(2, '中', 2, StyleKey::DEFAULT));
        row.insert_cells(0, 1, StyleKey::DEFAULT);
        assert_eq!(row.get(3), Some(PackedCell::BLANK));
        integrity(&page, &StyleTable::new(0)).unwrap();
    }

    #[test]
    fn delete_from_a_spacer_or_into_a_wide_char_clears_the_orphan() {
        let mut page = row_page(6);
        let mut row = row(&mut page);
        assert!(row.write_char_in_place(0, '中', 2, StyleKey::DEFAULT));
        assert!(row.write_char_in_place(2, '文', 2, StyleKey::DEFAULT));
        row.delete_cells(1, 2, StyleKey::DEFAULT);
        integrity(&page, &StyleTable::new(0)).unwrap();
        assert!(page
            .row(0)
            .unwrap()
            .cells()
            .iter()
            .all(|cell| !cell.is_wide_part()));
    }

    #[test]
    fn integrity_catches_a_bad_key_and_a_bare_grapheme_bit() {
        let table = StyleTable::new(0);
        let mut page = row_page(4);
        row(&mut page).set(0, PackedCell::new('a', StyleKey::from_bits(5)));
        assert!(integrity(&page, &table).is_err(), "id past the table");

        let mut page = row_page(4);
        row(&mut page).set(
            0,
            PackedCell::new('a', StyleKey::DEFAULT).with_structure(CellFlags::HAS_COMBINING),
        );
        assert!(integrity(&page, &table).is_err(), "bit with no extras");
        let mut page = row_page(4);
        let mut r = row(&mut page);
        r.set(0, PackedCell::new('a', StyleKey::DEFAULT));
        r.push_grapheme_char(0, '\u{301}', 16);
        integrity(&page, &table).unwrap();

        let mut page = row_page(4);
        let mut r = row(&mut page);
        r.write_ascii(0, b"ab", StyleKey::DEFAULT);
        r.cells[1] = PackedCell::new('é', StyleKey::DEFAULT);
        assert!(integrity(&page, &table).is_err(), "uniform but not ASCII");
    }

    #[test]
    fn grapheme_str_is_the_cluster_then_the_extra_char() {
        let mut page = row_page(4);
        let mut r = row(&mut page);
        r.set(1, PackedCell::new('e', StyleKey::DEFAULT));
        r.push_grapheme_char(1, '\u{301}', 16);
        let row = page.row(0).unwrap();
        let got = row.with_grapheme_str(1, Some('\u{302}'), str::to_string);
        assert_eq!(got.as_deref(), Some("e\u{301}\u{302}"));
        assert_eq!(
            row.with_grapheme_str(0, None, str::to_string).as_deref(),
            Some(" ")
        );
        assert_eq!(row.with_grapheme_str(4, None, str::to_string), None);

        // Longer than the stack buffer.
        let long = ['\u{1F600}'; 30];
        let mut r = page.row_mut(0).unwrap();
        for c in long {
            r.push_grapheme_char(2, c, long.len());
        }
        let expected: String = std::iter::once(' ').chain(long).chain(['x']).collect();
        let got = page
            .row(0)
            .unwrap()
            .with_grapheme_str(2, Some('x'), str::to_string);
        assert_eq!(got.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn erasing_a_blank_row_from_column_zero_leaves_it_uniform() {
        let mut table = StyleTable::new(0);
        let key = table.key(&Style::erased(Color::Indexed(4)));
        assert!(!key.is_default());
        for end in [3, 6] {
            let mut page = row_page(6);
            row(&mut page).erase_range(0, end, key);
            let r = page.row(0).unwrap();
            assert!(r.is_uniform(), "erase to {end}");
            assert_eq!(r.uniform_key(), key);
            integrity(&page, &table).unwrap();
        }
    }

    #[test]
    fn erasing_part_of_a_styled_row_is_not_uniform() {
        let mut table = StyleTable::new(0);
        let key = table.key(&Style::erased(Color::Indexed(4)));
        let mut page = row_page(6);
        let mut r = row(&mut page);
        r.set(4, PackedCell::new('x', StyleKey::DEFAULT));
        r.erase_range(0, 2, key);
        integrity(&page, &table).unwrap();
        assert!(!page.row(0).unwrap().is_uniform());
    }

    /// Rows `0..rows` of `page` as text, with each cell's extras.
    fn texts(page: &ScreenPage) -> Vec<String> {
        page.iter()
            .map(|row| {
                (0..row.width())
                    .map(|col| row.grapheme(col).unwrap())
                    .collect()
            })
            .collect()
    }

    fn page_of(lines: &[&str]) -> ScreenPage {
        let cols = 4;
        let mut page = ScreenPage::new(cols, lines.len() as u16);
        for (index, line) in lines.iter().enumerate() {
            let mut row = page.row_mut(index as u16).unwrap();
            let mut col = 0;
            for c in line.chars() {
                if c == '\u{301}' {
                    row.push_grapheme_char(col - 1, c, 16);
                } else {
                    row.set(col, PackedCell::new(c, StyleKey::DEFAULT));
                    col += 1;
                }
            }
        }
        page
    }

    #[test]
    fn rotating_moves_rows_with_their_graphemes_and_clears_the_new_ones() {
        let table = StyleTable::new(0);
        let mut page = page_of(&["a", "be\u{301}", "c", "d"]);
        page.rotate_up(0, 3, 1, StyleKey::DEFAULT);
        assert_eq!(texts(&page), ["be\u{301}  ", "c   ", "d   ", "    "]);
        assert_eq!(page.row(0).unwrap().grapheme(1).unwrap(), "e\u{301}");
        integrity(&page, &table).unwrap();

        page.rotate_down(0, 2, 2, StyleKey::DEFAULT);
        assert_eq!(texts(&page), ["    ", "    ", "be\u{301}  ", "    "]);
        integrity(&page, &table).unwrap();

        page.rotate_up(1, 3, 2, StyleKey::DEFAULT);
        assert_eq!(texts(&page), ["    ", "    ", "    ", "    "]);
        integrity(&page, &table).unwrap();
        assert!(
            page.heads.iter().all(|(_, store)| store.index.is_empty()),
            "the cluster went with its row"
        );
    }

    #[test]
    fn shifting_cells_moves_their_graphemes() {
        let table = StyleTable::new(0);
        let mut page = page_of(&["xa\u{301}b", "ye\u{301}"]);
        page.row_mut(0)
            .unwrap()
            .insert_cells(0, 1, StyleKey::DEFAULT);
        assert_eq!(page.row(0).unwrap().grapheme(2).unwrap(), "a\u{301}");
        page.row_mut(0)
            .unwrap()
            .insert_cells(0, 2, StyleKey::DEFAULT);
        assert_eq!(page.row(0).unwrap().grapheme_extra(4), None);
        assert_eq!(page.row(1).unwrap().grapheme(1).unwrap(), "e\u{301}");
        integrity(&page, &table).unwrap();

        let mut page = page_of(&["xa\u{301}b", "ye\u{301}"]);
        page.row_mut(0)
            .unwrap()
            .delete_cells(0, 1, StyleKey::DEFAULT);
        assert_eq!(page.row(0).unwrap().grapheme(0).unwrap(), "a\u{301}");
        page.row_mut(0)
            .unwrap()
            .delete_cells(0, 1, StyleKey::DEFAULT);
        assert_eq!(texts(&page)[0], "b   ");
        assert_eq!(page.row(1).unwrap().grapheme(1).unwrap(), "e\u{301}");
        integrity(&page, &table).unwrap();
    }

    #[test]
    fn graphemes_written_out_of_order_are_compacted() {
        let table = StyleTable::new(0);
        let mut page = ScreenPage::new(8, 2);
        // Grow two clusters of one row in turn, so each push moves one of
        // them, until the moves leave enough garbage to compact.
        for round in 0..190 {
            for col in [1, 3] {
                let mut r = page.row_mut(1).unwrap();
                if round == 0 {
                    r.set(col, PackedCell::new('e', StyleKey::DEFAULT));
                }
                r.push_grapheme_char(col, '\u{301}', 16);
                if round % 20 == 19 {
                    // Start the cluster over.
                    r.clear_graphemes(col, col + 1);
                    r.set(col, PackedCell::new('e', StyleKey::DEFAULT));
                }
            }
            integrity(&page, &table).unwrap();
            let store = &page.heads[1].1;
            assert!(store.chars.len() <= 2 * 16 + 2 * GraphemeStore::MIN_GARBAGE);
        }
        let row = page.row(1).unwrap();
        // Ten pushes since the last start-over at round 179.
        assert_eq!(row.grapheme_extra(1), Some(&['\u{301}'; 10][..]));
        assert_eq!(row.grapheme_extra(3), Some(&['\u{301}'; 10][..]));
    }

    #[test]
    fn resizing_keeps_rows_in_screen_order() {
        let table = StyleTable::new(0);
        let mut page = page_of(&["ab", "ce\u{301}", "d"]);
        page.rotate_up(0, 2, 1, StyleKey::DEFAULT);
        let wider = page.resized(6, 4);
        assert_eq!(
            texts(&wider),
            ["ce\u{301}    ", "d     ", "      ", "      "]
        );
        integrity(&wider, &table).unwrap();
        let narrower = page.resized(1, 1);
        assert_eq!(texts(&narrower), ["c"]);
        integrity(&narrower, &table).unwrap();
    }

    #[test]
    fn heads_stay_small() {
        // A page keeps one head per screen row.
        assert_eq!(std::mem::size_of::<RowHead>(), 16);
    }
}
