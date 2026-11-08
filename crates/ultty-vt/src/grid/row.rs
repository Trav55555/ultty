//! Terminal row representation.

use super::cell::{Cell, CellFlags};
use std::ops::{Index, IndexMut};

/// Shell integration marks on a row (OSC 133; ADR 0019).
///
/// Each mark is 0 when unset, otherwise the tag of the command it belongs
/// to: the low bits of a per-terminal command number, from 1 to 255. Three
/// bytes, so they fit in padding rows already have.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowMarks {
    /// Command input starts on this row (OSC 133;B).
    pub input_start: u8,
    /// Command output starts at the beginning of this row (OSC 133;C).
    pub output_start: u8,
    /// The command finished at the beginning of this row (OSC 133;D).
    pub command_end: u8,
}

impl RowMarks {
    /// The tag a command number is stored as: never 0, which means unset.
    pub fn tag(command: u32) -> u8 {
        (command % 255) as u8 + 1
    }

    pub fn is_empty(&self) -> bool {
        *self == RowMarks::default()
    }

    /// These marks, with any unset one taken from `other`.
    pub fn or(self, other: RowMarks) -> RowMarks {
        let pick = |a: u8, b: u8| if a != 0 { a } else { b };
        RowMarks {
            input_start: pick(self.input_start, other.input_start),
            output_start: pick(self.output_start, other.output_start),
            command_end: pick(self.command_end, other.command_end),
        }
    }
}

/// An inline surface's anchor tag on a row (ADR 0020).
///
/// The low bits hold the per-buffer tag, 0 for none; the top bit marks the
/// first row of the reservation, which is where the surface is drawn and
/// what decides whether it is still there. Two bytes, so a compact
/// scrollback row keeps it in padding it already had.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnchorTag(u16);

impl AnchorTag {
    /// No anchor.
    pub const NONE: AnchorTag = AnchorTag(0);

    const FIRST: u16 = 1 << 15;

    /// Tag `tag` (1 to `0x7fff`), on the first row of its reservation when
    /// `first` is set.
    pub fn new(tag: u16, first: bool) -> AnchorTag {
        let tag = tag & !Self::FIRST;
        if first && tag != 0 {
            AnchorTag(tag | Self::FIRST)
        } else {
            AnchorTag(tag)
        }
    }

    /// The tag, 0 for none.
    pub fn tag(self) -> u16 {
        self.0 & !Self::FIRST
    }

    /// Whether this is the first row of the reservation.
    pub fn is_first(self) -> bool {
        self.0 & Self::FIRST != 0
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// A row of terminal cells.
///
/// Provides efficient storage and manipulation of a single line
/// in the terminal grid.
#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    /// Track if this row has been modified
    dirty: bool,
    /// Whether this row is wrapped (continues on next line due to soft wrap)
    wrapped: bool,
    /// Shell integration marks; see [`RowMarks`].
    marks: RowMarks,
    /// Inline surface anchor; see [`AnchorTag`].
    anchor: AnchorTag,
    /// Every cell at or after this column is [`Cell::default`]. It may
    /// overstate the used part of the row, never understate it, so code
    /// that only cares about non-default cells (history encoding, erasing a
    /// row that is already blank) can skip the tail without looking at it.
    used: u16,
    /// Every cell is ASCII in the default style, with no grapheme data, so
    /// the row's content is its characters alone. Like `used`, it may
    /// understate (anything handing out a mutable cell clears it) but never
    /// overstates, so history can copy such a row's text without checking
    /// each cell's style.
    plain: bool,
    /// Codepoints after the base character of a multi-codepoint grapheme
    /// cluster (combining marks, ZWJ sequences, variation selectors), keyed by
    /// column. An entry is only meaningful while the cell at that column has
    /// [`CellFlags::HAS_COMBINING`]; cell writes and erases clear the flag, so
    /// a stale entry is never read. Storing them on the row (rather than by
    /// screen coordinate) keeps them attached through scrolling, history,
    /// and reflow.
    graphemes: GraphemeMap,
}

/// Extra grapheme codepoints by column, sorted by column.
///
/// Output is written left to right, so the cluster being built is nearly
/// always the last entry and a write lands after all of them; lookups and
/// inserts check the end before searching.
#[derive(Debug, Default)]
struct GraphemeMap {
    entries: Vec<(u16, Vec<char>)>,
}

impl Clone for GraphemeMap {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.entries.clone_from(&source.entries);
    }
}

impl GraphemeMap {
    /// The index of `col`'s entry, or where it would be inserted.
    fn find(&self, col: u16) -> Result<usize, usize> {
        match self.entries.last() {
            None => Err(0),
            Some(&(last, _)) if last < col => Err(self.entries.len()),
            Some(&(last, _)) if last == col => Ok(self.entries.len() - 1),
            Some(_) => self.entries.binary_search_by_key(&col, |&(c, _)| c),
        }
    }

    fn get(&self, col: u16) -> Option<&[char]> {
        let index = self.find(col).ok()?;
        Some(&self.entries[index].1)
    }

    /// `col`'s codepoints, inserting an empty entry if it has none.
    fn entry(&mut self, col: u16) -> &mut Vec<char> {
        let index = self.find(col).unwrap_or_else(|index| {
            self.entries.insert(index, (col, Vec::new()));
            index
        });
        &mut self.entries[index].1
    }

    fn insert(&mut self, col: u16, extra: Vec<char>) {
        match self.find(col) {
            Ok(index) => self.entries[index].1 = extra,
            Err(index) => self.entries.insert(index, (col, extra)),
        }
    }

    fn remove(&mut self, col: u16) {
        if let Ok(index) = self.find(col) {
            self.entries.remove(index);
        }
    }

    /// Whether any entry is in `start..end`.
    fn any_in(&self, start: u16, end: u16) -> bool {
        // Writes usually land after every cluster on the row.
        if self.entries.last().is_none_or(|&(c, _)| c < start) {
            return false;
        }
        let first = self.entries.partition_point(|&(c, _)| c < start);
        self.entries.get(first).is_some_and(|&(c, _)| c < end)
    }

    fn remove_range(&mut self, start: u16, end: u16) {
        let first = self.entries.partition_point(|&(c, _)| c < start);
        let past = self.entries.partition_point(|&(c, _)| c < end);
        self.entries.drain(first..past);
    }

    /// Move entries at `from..` by `delta`, dropping any that land outside
    /// `0..width`. A moved entry replaces one already at its new column.
    fn shift(&mut self, from: u16, delta: i32, width: u16) {
        let first = self.entries.partition_point(|&(c, _)| c < from);
        if first == self.entries.len() {
            return;
        }
        let moved: Vec<_> = self.entries.drain(first..).collect();
        for (col, extra) in moved {
            let dest = i32::from(col) + delta;
            if (0..i32::from(width)).contains(&dest) {
                self.insert(dest as u16, extra);
            }
        }
    }

    fn truncate(&mut self, width: u16) {
        let keep = self.entries.partition_point(|&(c, _)| c < width);
        self.entries.truncate(keep);
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Row {
    /// Check the row's structural invariants: every wide cell is followed by
    /// its spacer, every spacer follows a wide cell, and no cell carries more
    /// grapheme codepoints than `max_extra`.
    pub fn check_integrity(&self, max_extra: usize) -> Result<(), String> {
        let cells = &self.cells;
        if self.plain {
            if let Some(col) = cells.iter().position(|cell| {
                let mut style = *cell;
                style.set_char(' ');
                !cell.char().is_ascii() || !style.is_default()
            }) {
                return Err(format!("row is marked plain but cell at col {col} is not"));
            }
        }
        if let Some(col) = cells[(self.used as usize).min(cells.len())..]
            .iter()
            .position(|cell| !cell.is_default())
        {
            return Err(format!(
                "cell at col {} is past the used mark {} but not default",
                self.used as usize + col,
                self.used
            ));
        }
        for (col, cell) in cells.iter().enumerate() {
            if cell.is_wide() && cell.is_wide_spacer() {
                return Err(format!("cell at col {col} is both wide and a spacer"));
            }
            if cell.is_wide() && !cells.get(col + 1).is_some_and(Cell::is_wide_spacer) {
                return Err(format!("wide cell at col {col} has no spacer after it"));
            }
            if cell.is_wide_spacer() && !(col > 0 && cells[col - 1].is_wide()) {
                return Err(format!("spacer at col {col} does not follow a wide cell"));
            }
            if let Some(extra) = self.grapheme_extra(col as u16) {
                if extra.len() > max_extra {
                    return Err(format!(
                        "cell at col {col} holds {} grapheme codepoints (max {max_extra})",
                        extra.len()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Create a new row with the given width.
    pub fn new(width: u16) -> Self {
        Self {
            cells: vec![Cell::default(); width as usize],
            dirty: true,
            wrapped: false,
            marks: RowMarks::default(),
            anchor: AnchorTag::NONE,
            used: 0,
            plain: true,
            graphemes: GraphemeMap::default(),
        }
    }

    /// Write printable ASCII `bytes` starting at `col`, each cell a copy of
    /// `template` with its character replaced. The caller has already
    /// resolved any wide character straddling the span's edges; the span
    /// must fit on the row.
    pub fn write_ascii(&mut self, col: u16, bytes: &[u8], template: Cell) {
        let start = col as usize;
        let end = start + bytes.len();
        self.drop_grapheme_entries(col, end as u16);
        if self.plain {
            let mut style = template;
            style.set_char(' ');
            self.plain = style.is_default();
        }
        for (cell, &byte) in self.cells[start..end].iter_mut().zip(bytes) {
            *cell = template.with_char(byte as char);
        }
        self.mark_used(end);
        self.dirty = true;
    }

    /// Write `c`, taking `width` (1 or 2) columns from `col`, as a copy of
    /// `template` (and a spacer after a wide character), if those columns
    /// hold no part of a wide character and fit on the row. Returns
    /// `false`, having changed nothing, otherwise.
    #[inline]
    pub(crate) fn write_char_in_place(
        &mut self,
        col: u16,
        c: char,
        width: u16,
        template: Cell,
    ) -> bool {
        let start = col as usize;
        let end = start + width as usize;
        let Some(cells) = self.cells.get_mut(start..end) else {
            return false;
        };
        if cells
            .iter()
            .any(|cell| cell.is_wide() || cell.is_wide_spacer())
        {
            return false;
        }
        let mut written = template;
        written.set_char(c);
        written.unset_flag(CellFlags::HAS_COMBINING);
        written.unset_flag(CellFlags::WIDE_CHAR_SPACER);
        if width > 1 {
            written.set_flag(CellFlags::WIDE_CHAR);
            let mut spacer = template;
            spacer.set_char(' ');
            spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
            spacer.unset_flag(CellFlags::WIDE_CHAR);
            cells[1] = spacer;
        } else {
            written.unset_flag(CellFlags::WIDE_CHAR);
        }
        cells[0] = written;
        self.drop_grapheme_entries(col, col + 1);
        self.plain = false;
        self.mark_used(end);
        self.dirty = true;
        true
    }

    /// Make this row a copy of `other`, reusing this row's allocations.
    pub fn copy_from(&mut self, other: &Row) {
        self.cells.clone_from(&other.cells);
        self.graphemes.clone_from(&other.graphemes);
        self.wrapped = other.wrapped;
        self.marks = other.marks;
        self.anchor = other.anchor;
        self.used = other.used;
        self.plain = other.plain;
        self.dirty = true;
    }

    /// Codepoints following the base character of the cluster at `col`, if
    /// the cell holds a multi-codepoint grapheme.
    pub fn grapheme_extra(&self, col: u16) -> Option<&[char]> {
        let cell = self.cells.get(col as usize)?;
        if !cell.has_flag(CellFlags::HAS_COMBINING) {
            return None;
        }
        self.graphemes.get(col)
    }

    /// The full grapheme at `col`: base character plus any extra codepoints.
    pub fn grapheme(&self, col: u16) -> Option<String> {
        let cell = self.cells.get(col as usize)?;
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
    pub(crate) fn with_grapheme_str<R>(
        &self,
        col: u16,
        then: Option<char>,
        f: impl FnOnce(&str) -> R,
    ) -> Option<R> {
        let cell = self.cells.get(col as usize)?;
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

    /// Append a codepoint to the grapheme cluster at `col`.
    ///
    /// At most `cap` extra codepoints are kept per cell; the rest are dropped
    /// so hostile output cannot grow a cell without bound. Returns whether
    /// `c` was kept.
    pub fn push_grapheme_char(&mut self, col: u16, c: char, cap: usize) -> bool {
        let Some(cell) = self.cells.get_mut(col as usize) else {
            return false;
        };
        self.plain = false;
        let extra = self.graphemes.entry(col);
        if !cell.has_flag(CellFlags::HAS_COMBINING) {
            // Whatever is stored for this column belonged to earlier content.
            extra.clear();
            cell.set_flag(CellFlags::HAS_COMBINING);
            self.used = self.used.max(col + 1);
        }
        let kept = extra.len() < cap;
        if kept {
            extra.push(c);
        }
        self.dirty = true;
        kept
    }

    /// Set the extra codepoints of the cluster at `col` (used by reflow).
    pub fn set_grapheme_extra(&mut self, col: u16, extra: &[char]) {
        let Some(cell) = self.cells.get_mut(col as usize) else {
            return;
        };
        if extra.is_empty() {
            cell.unset_flag(CellFlags::HAS_COMBINING);
            self.graphemes.remove(col);
        } else {
            self.plain = false;
            cell.set_flag(CellFlags::HAS_COMBINING);
            self.graphemes.insert(col, extra.to_vec());
            self.mark_used(col as usize + 1);
        }
        self.dirty = true;
    }

    /// Drop grapheme data for columns in `start..end` and clear their flags.
    pub fn clear_graphemes(&mut self, start: u16, end: u16) {
        if start >= end {
            return;
        }
        self.drop_grapheme_entries(start, end);
        let end = (end as usize).min(self.cells.len());
        for cell in self
            .cells
            .get_mut(start as usize..end)
            .into_iter()
            .flatten()
        {
            cell.unset_flag(CellFlags::HAS_COMBINING);
        }
        self.dirty = true;
    }

    /// Remove stored grapheme data for `start..end` without touching cells.
    fn drop_grapheme_entries(&mut self, start: u16, end: u16) {
        if start < end && self.graphemes.any_in(start, end) {
            self.graphemes.remove_range(start, end);
        }
    }

    /// Shift grapheme entries so they follow their cells after a horizontal
    /// move: entries at `from..` move by `delta`, and entries that land
    /// outside the row are dropped.
    fn shift_graphemes(&mut self, from: u16, delta: i32) {
        self.graphemes.shift(from, delta, self.width());
    }

    /// Check if this row is wrapped (soft-wrapped to next line).
    #[inline]
    pub fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    /// Set the wrapped flag.
    #[inline]
    pub fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped = wrapped;
        self.dirty = true;
    }

    /// The row's shell integration marks.
    #[inline]
    pub fn marks(&self) -> RowMarks {
        self.marks
    }

    /// Replace the row's shell integration marks. They do not change what
    /// is drawn, so the row stays clean.
    #[inline]
    pub fn set_marks(&mut self, marks: RowMarks) {
        self.marks = marks;
    }

    /// The row's inline surface anchor tag.
    #[inline]
    pub fn anchor(&self) -> AnchorTag {
        self.anchor
    }

    /// Replace the row's anchor tag. Like marks, it does not change what is
    /// drawn, so the row stays clean.
    #[inline]
    pub fn set_anchor(&mut self, anchor: AnchorTag) {
        self.anchor = anchor;
    }

    /// Get the width of the row.
    #[inline]
    pub fn width(&self) -> u16 {
        self.cells.len() as u16
    }

    /// Get a cell by index.
    #[inline]
    pub fn get(&self, index: u16) -> Option<&Cell> {
        self.cells.get(index as usize)
    }

    /// Get a mutable cell by index.
    #[inline]
    pub fn get_mut(&mut self, index: u16) -> Option<&mut Cell> {
        self.dirty = true;
        self.plain = false;
        self.mark_used(index as usize + 1);
        self.cells.get_mut(index as usize)
    }

    /// Record that cells before `end` may no longer be default.
    #[inline]
    fn mark_used(&mut self, end: usize) {
        let end = end.min(self.cells.len()) as u16;
        self.used = self.used.max(end);
    }

    /// Whether every cell is ASCII in the default style with no grapheme
    /// data. `false` does not mean some cell is not.
    pub(crate) fn is_plain(&self) -> bool {
        self.plain
    }

    /// The cells up to the used mark: every cell after them is default.
    pub(crate) fn used_cells(&self) -> &[Cell] {
        &self.cells[..self.used as usize]
    }

    /// Clear all cells in the row.
    pub fn clear(&mut self) {
        self.erase(super::cell::Color::Default);
    }

    /// Erase all cells per ECMA-48: reset to space, preserve given background.
    pub fn erase(&mut self, bg: super::cell::Color) {
        let mut blank = Cell::default();
        blank.set_bg(bg);
        if blank.is_default() {
            // Cells past the used mark are already default.
            self.cells[..self.used as usize].fill(blank);
            self.used = 0;
            self.plain = true;
        } else {
            self.cells.fill(blank);
            self.used = self.width();
            self.plain = false;
        }
        self.graphemes.clear();
        self.wrapped = false;
        self.marks = RowMarks::default();
        self.anchor = AnchorTag::NONE;
        self.dirty = true;
    }

    /// Erase cells `start..end` per ECMA-48: spaces on background `bg`,
    /// with no grapheme data.
    pub fn erase_range(&mut self, start: u16, end: u16, bg: super::cell::Color) {
        let end = end.min(self.width());
        if start >= end {
            return;
        }
        self.drop_grapheme_entries(start, end);
        let mut blank = Cell::default();
        blank.set_bg(bg);
        if blank.is_default() {
            // Cells past the used mark are already default.
            let fill_end = end.min(self.used);
            if start < fill_end {
                self.cells[start as usize..fill_end as usize].fill(blank);
            }
            if end >= self.used {
                self.used = self.used.min(start);
            }
        } else {
            self.cells[start as usize..end as usize].fill(blank);
            self.mark_used(end as usize);
            self.plain = false;
        }
        self.dirty = true;
    }

    /// Resize the row.
    pub fn resize(&mut self, new_width: u16) {
        self.cells.resize(new_width as usize, Cell::default());
        self.used = self.used.min(new_width);
        self.graphemes.truncate(new_width);
        self.dirty = true;
    }

    /// Check if the row is dirty.
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag.
    #[inline]
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Mark the row as dirty.
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Check if the row is blank (all cells are default).
    pub fn is_blank(&self) -> bool {
        self.cells.iter().all(|cell| cell.is_blank())
    }

    /// Iterate over cells.
    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter()
    }

    /// Iterate over cells mutably.
    pub fn cells_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        self.dirty = true;
        self.plain = false;
        self.used = self.width();
        self.cells.iter_mut()
    }

    /// Get the text content of the row as a string.
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.cells.len());
        for (col, cell) in self.cells.iter().enumerate() {
            text.push(cell.char());
            if let Some(extra) = self.grapheme_extra(col as u16) {
                text.extend(extra);
            }
        }
        text
    }

    /// Text of columns `start..=end` as a user would copy it: each cell's
    /// whole grapheme, with the spacer half of wide characters skipped.
    ///
    /// Unlike [`Row::text`], the result does not map one char per column.
    pub fn copy_text(&self, start: u16, end: u16) -> String {
        let mut text = String::new();
        let end = end.min(self.width().saturating_sub(1));
        for col in start..=end {
            let Some(cell) = self.cells.get(col as usize) else {
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

    /// Get the trimmed text content (trailing spaces removed).
    pub fn text_trimmed(&self) -> String {
        self.text().trim_end().to_string()
    }

    /// Insert blank cells at position, shifting existing cells right.
    /// Blank cells are filled with the given background color per ECMA-48.
    pub fn insert_cells(&mut self, at: u16, count: u16, bg: super::cell::Color) {
        let at = at as usize;
        let count = count as usize;
        let len = self.cells.len();

        if at >= len || count == 0 {
            return;
        }

        // Inserting at a wide char's spacer splits the pair: the primary
        // stays put and the spacer would be shifted away from it, so both
        // halves are cleared.
        if at > 0 && self.cells[at].is_wide_spacer() {
            self.cells[at - 1].clear();
            self.cells[at].clear();
        }

        // The last cell that stays on the row lands in the final column. If
        // it is a wide char, its spacer is the first cell pushed off, so it
        // would be left without one.
        let first_pushed = len.saturating_sub(count);
        if first_pushed > at && self.cells[first_pushed - 1].is_wide() {
            self.cells[first_pushed - 1].clear();
        }

        // Shift cells right using bulk copy
        let src_end = len.saturating_sub(count);
        if at < src_end {
            self.cells.copy_within(at..src_end, at + count);
        }
        self.shift_graphemes(at as u16, count as i32);
        self.used = self.width();
        self.plain = false;

        // Fill inserted cells with background color
        for i in at..(at + count).min(len) {
            self.cells[i].erase(bg);
        }

        self.dirty = true;
    }

    /// Delete cells at position, shifting remaining cells left.
    /// Vacated cells at the end are filled with the given background color per ECMA-48.
    pub fn delete_cells(&mut self, at: u16, count: u16, bg: super::cell::Color) {
        let at = at as usize;
        let count = count as usize;
        let len = self.cells.len();

        if at >= len || count == 0 {
            return;
        }

        // If deleting starts at a wide char spacer, clear the orphaned primary cell
        if at > 0 && self.cells[at].is_wide_spacer() {
            self.cells[at - 1].clear();
        }

        // If the deletion range ends in the middle of a wide char, clear the spacer
        let end = (at + count).min(len);
        if end < len && self.cells[end].is_wide_spacer() {
            self.cells[end].clear();
        }

        // Shift cells left using bulk copy
        let src_start = (at + count).min(len);
        if src_start < len {
            self.cells.copy_within(src_start..len, at);
        }
        self.drop_grapheme_entries(at as u16, src_start as u16);
        self.used = self.width();
        self.plain = false;
        self.shift_graphemes(src_start as u16, -(count as i32));

        // Fill vacated cells at end with background color. A count running
        // past the end of the row vacates only cells from `at` onward.
        for i in len.saturating_sub(count).max(at)..len {
            self.cells[i].erase(bg);
        }

        self.dirty = true;
    }

    /// Erase cells from position to end of row.
    pub fn erase_right(&mut self, from: u16) {
        for i in from as usize..self.cells.len() {
            self.cells[i].clear();
        }
        self.used = self.used.min(from);
        self.clear_graphemes(from, self.width());
        self.dirty = true;
    }

    /// Erase cells from start of row to position.
    pub fn erase_left(&mut self, to: u16) {
        for i in 0..=to as usize {
            if i < self.cells.len() {
                self.cells[i].clear();
            }
        }
        self.clear_graphemes(0, to.saturating_add(1));
        self.dirty = true;
    }
}

impl Index<u16> for Row {
    type Output = Cell;

    fn index(&self, index: u16) -> &Self::Output {
        &self.cells[index as usize]
    }
}

impl IndexMut<u16> for Row {
    fn index_mut(&mut self, index: u16) -> &mut Self::Output {
        self.dirty = true;
        self.plain = false;
        self.mark_used(index as usize + 1);
        &mut self.cells[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::cell::Color;

    #[test]
    fn new_row_is_blank() {
        let row = Row::new(80);
        assert_eq!(row.width(), 80);
        assert!(row.is_blank());
    }

    #[test]
    fn write_and_read_cell() {
        let mut row = Row::new(10);
        row[0].set_char('H');
        row[1].set_char('i');
        assert_eq!(row[0].char(), 'H');
        assert_eq!(row[1].char(), 'i');
    }

    #[test]
    fn text_extraction() {
        let mut row = Row::new(10);
        row[0].set_char('H');
        row[1].set_char('e');
        row[2].set_char('l');
        row[3].set_char('l');
        row[4].set_char('o');

        assert_eq!(row.text_trimmed(), "Hello");
    }

    #[test]
    fn clear_row() {
        let mut row = Row::new(10);
        row[0].set_char('X');
        row[5].set_char('Y');
        row.clear();
        assert!(row.is_blank());
    }

    #[test]
    fn resize_row() {
        let mut row = Row::new(10);
        row[5].set_char('A');

        row.resize(20);
        assert_eq!(row.width(), 20);
        assert_eq!(row[5].char(), 'A');

        row.resize(5);
        assert_eq!(row.width(), 5);
    }

    #[test]
    fn insert_cells() {
        let mut row = Row::new(10);
        row[0].set_char('A');
        row[1].set_char('B');
        row[2].set_char('C');

        row.insert_cells(1, 2, crate::grid::cell::Color::Default);

        assert_eq!(row[0].char(), 'A');
        assert_eq!(row[1].char(), ' ');
        assert_eq!(row[2].char(), ' ');
        assert_eq!(row[3].char(), 'B');
        assert_eq!(row[4].char(), 'C');
    }

    fn wide_at(row: &mut Row, col: usize, c: char) {
        row.cells[col].set_char(c);
        row.cells[col].set_flag(CellFlags::WIDE_CHAR);
        row.cells[col + 1].set_flag(CellFlags::WIDE_CHAR_SPACER);
    }

    #[test]
    fn insert_at_wide_spacer_clears_both_halves() {
        let mut row = Row::new(5);
        wide_at(&mut row, 0, '\u{4e2d}');
        row.insert_cells(1, 1, crate::grid::cell::Color::Default);
        assert!(
            row.check_integrity(16).is_ok(),
            "{:?}",
            row.check_integrity(16)
        );
        assert!(row.cells().all(|c| !c.is_wide() && !c.is_wide_spacer()));
    }

    #[test]
    fn insert_that_pushes_off_a_spacer_clears_its_wide_char() {
        let mut row = Row::new(4);
        row.cells[0].set_char('a');
        wide_at(&mut row, 2, '\u{4e2d}');
        row.insert_cells(0, 1, crate::grid::cell::Color::Default);
        // The wide char would land in the last column with no room for its
        // spacer, so it is cleared instead.
        assert!(
            row.check_integrity(16).is_ok(),
            "{:?}",
            row.check_integrity(16)
        );
        assert_eq!(row[1].char(), 'a');
        assert!(!row[3].is_wide());
    }

    #[test]
    fn delete_past_end_of_row_leaves_cells_before_cursor() {
        let mut row = Row::new(5);
        wide_at(&mut row, 0, '\u{4e2d}');
        row.delete_cells(2, 4, crate::grid::cell::Color::Default);
        assert!(
            row.check_integrity(16).is_ok(),
            "{:?}",
            row.check_integrity(16)
        );
        assert!(row[0].is_wide());
    }

    #[test]
    fn delete_cells() {
        let mut row = Row::new(10);
        row[0].set_char('A');
        row[1].set_char('B');
        row[2].set_char('C');
        row[3].set_char('D');

        row.delete_cells(1, 2, crate::grid::cell::Color::Default);

        assert_eq!(row[0].char(), 'A');
        assert_eq!(row[1].char(), 'D');
        assert_eq!(row[2].char(), ' ');
    }

    #[derive(Debug, Clone)]
    enum MapOp {
        Push(u16, char),
        Insert(u16, Vec<char>),
        Remove(u16),
        RemoveRange(u16, u16),
        Shift(u16, i32, u16),
        Truncate(u16),
    }

    fn map_op() -> impl proptest::strategy::Strategy<Value = MapOp> {
        use proptest::prelude::*;
        let col = 0u16..24;
        prop_oneof![
            (col.clone(), any::<char>()).prop_map(|(c, ch)| MapOp::Push(c, ch)),
            (col.clone(), prop::collection::vec(any::<char>(), 0..3))
                .prop_map(|(c, v)| MapOp::Insert(c, v)),
            col.clone().prop_map(MapOp::Remove),
            (col.clone(), col.clone()).prop_map(|(a, b)| MapOp::RemoveRange(a, b)),
            (col.clone(), -6i32..6, 1u16..24).prop_map(|(f, d, w)| MapOp::Shift(f, d, w)),
            col.prop_map(MapOp::Truncate),
        ]
    }

    proptest::proptest! {
        /// The grapheme map behaves like the ordered map it replaced.
        #[test]
        fn grapheme_map_matches_an_ordered_map(ops in proptest::collection::vec(map_op(), 0..40)) {
            use std::collections::BTreeMap;
            let mut map = GraphemeMap::default();
            let mut model: BTreeMap<u16, Vec<char>> = BTreeMap::new();
            for op in ops {
                match op {
                    MapOp::Push(col, c) => {
                        map.entry(col).push(c);
                        model.entry(col).or_default().push(c);
                    }
                    MapOp::Insert(col, v) => {
                        map.insert(col, v.clone());
                        model.insert(col, v);
                    }
                    MapOp::Remove(col) => {
                        map.remove(col);
                        model.remove(&col);
                    }
                    MapOp::RemoveRange(start, end) => {
                        proptest::prop_assert_eq!(
                            map.any_in(start, end),
                            start < end && model.range(start..end).next().is_some()
                        );
                        if start < end {
                            map.remove_range(start, end);
                            model.retain(|&c, _| c < start || c >= end);
                        }
                    }
                    MapOp::Shift(from, delta, width) => {
                        map.shift(from, delta, width);
                        let moved = model.split_off(&from);
                        for (col, extra) in moved {
                            let dest = i32::from(col) + delta;
                            if (0..i32::from(width)).contains(&dest) {
                                model.insert(dest as u16, extra);
                            }
                        }
                    }
                    MapOp::Truncate(width) => {
                        map.truncate(width);
                        model.retain(|&c, _| c < width);
                    }
                }
                let expected: Vec<(u16, Vec<char>)> = model.clone().into_iter().collect();
                proptest::prop_assert_eq!(&map.entries, &expected);
                for col in 0..24 {
                    proptest::prop_assert_eq!(map.get(col), model.get(&col).map(Vec::as_slice));
                }
            }
        }
    }

    #[test]
    fn grapheme_str_is_the_cluster_then_the_extra_char() {
        let mut row = Row::new(4);
        row[1].set_char('e');
        row.set_grapheme_extra(1, &['\u{301}']);
        let got = row.with_grapheme_str(1, Some('\u{302}'), str::to_string);
        assert_eq!(got.as_deref(), Some("e\u{301}\u{302}"));
        assert_eq!(
            row.with_grapheme_str(0, None, str::to_string).as_deref(),
            Some(" ")
        );
        assert_eq!(row.with_grapheme_str(4, None, str::to_string), None);

        // Longer than the stack buffer.
        let long = ['\u{1F600}'; 30];
        row.set_grapheme_extra(2, &long);
        let expected: String = std::iter::once(' ').chain(long).chain(['x']).collect();
        let got = row.with_grapheme_str(2, Some('x'), str::to_string);
        assert_eq!(got.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn erase_range_blanks_cells_with_the_background() {
        use crate::grid::cell::Color;
        let mut row = Row::new(10);
        row.write_ascii(0, b"abcdef", Cell::default());
        row.set_grapheme_extra(4, &['\u{301}']);

        row.erase_range(2, 5, Color::Default);
        assert_eq!(row.text(), "ab   f    ");
        assert_eq!(row.grapheme_extra(4), None);
        assert_eq!(row.check_integrity(16), Ok(()));

        row.erase_range(4, 10, Color::Default);
        assert_eq!(row.text_trimmed(), "ab");
        assert_eq!(row.check_integrity(16), Ok(()));

        row.erase_range(6, 8, Color::Indexed(4));
        assert_eq!(row[5].bg(), Color::Default);
        assert!((6..8).all(|col| row[col].bg() == Color::Indexed(4)));
        assert_eq!(row[8].bg(), Color::Default);
        assert_eq!(row.check_integrity(16), Ok(()));
    }

    #[test]
    fn a_row_is_plain_until_something_other_than_default_ascii_lands() {
        let mut row = Row::new(10);
        assert!(row.is_plain());
        row.write_ascii(0, b"abc", Cell::default());
        assert!(row.is_plain());
        row.write_ascii(
            3,
            b"d",
            Cell::with_attrs(' ', Color::Default, Color::Named(1), CellFlags::NONE),
        );
        assert!(!row.is_plain());
        row.erase(Color::Default);
        assert!(row.is_plain());
        row.erase(Color::Named(4));
        assert!(!row.is_plain(), "a colored blank is not plain");
        row.clear();
        let _ = row.get_mut(2);
        assert!(!row.is_plain());
        row.clear();
        row.push_grapheme_char(0, '\u{301}', 4);
        assert!(!row.is_plain());
        row.clear();
        row.erase_range(2, 4, Color::Named(2));
        assert!(!row.is_plain());
        row.check_integrity(4).unwrap();
    }

    #[test]
    fn erase_operations() {
        let mut row = Row::new(10);
        for i in 0..10 {
            row[i].set_char((b'A' + i as u8) as char);
        }

        row.erase_right(5);
        assert_eq!(row.text_trimmed(), "ABCDE");

        row.erase_left(2);
        assert_eq!(row[0].char(), ' ');
        assert_eq!(row[1].char(), ' ');
        assert_eq!(row[2].char(), ' ');
        assert_eq!(row[3].char(), 'D');
    }
}
