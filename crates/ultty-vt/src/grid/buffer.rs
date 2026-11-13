//! Terminal buffer with scrollback history.

use std::collections::{BTreeMap, VecDeque};

use super::history::{History, RowPacker};
use super::{
    AnchorTag, Cell, CellFlags, Dimensions, Grid, ImageStore, Row, RowMarks, RowView, ScrollRegion,
};

mod images;

/// Default scrollback buffer size (lines).
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// Most inline surface anchors a buffer holds at once (ADR 0020). Tags run
/// from 1 to this.
pub const MAX_ANCHORS: usize = 1024;

/// An inline surface anchor placed in the text (ADR 0020): which session's
/// surface it is for and how many rows it reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub session_id: u64,
    pub surface_id: u32,
    pub rows: u16,
}

/// Why an anchor reserved nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorRefusal {
    /// Anchors live on the main screen only.
    AlternateScreen,
    /// The buffer already holds [`MAX_ANCHORS`] anchors.
    TableFull,
    /// The cursor is outside the scroll region, or the region is shorter
    /// than the reservation, so its rows could not all stay on the screen.
    NoRoom,
}

/// A live anchor and where its first row was last seen.
#[derive(Debug, Clone, Copy)]
struct AnchorEntry {
    anchor: Anchor,
    /// Which placement this is: anchors placed later have larger serials,
    /// so a tag reused for an identical anchor is still told apart.
    serial: u64,
    /// The first row's line number: lines evicted from history before it
    /// plus its absolute index. Scrolling and eviction leave it right;
    /// anything else that moves rows makes it a stale hint, which lookups
    /// check against the row's tag. The row is the truth.
    first_line: usize,
    /// All its rows are in scrollback and were found intact there, so
    /// only eviction can still take it.
    settled: bool,
}

/// Terminal buffer with scrollback history.
///
/// This wraps a [`Grid`] and maintains a history of scrolled-off lines
/// that can be viewed by scrolling back.
#[derive(Debug, Clone)]
pub struct Buffer {
    /// The visible grid
    grid: Grid,
    /// Scrollback history (oldest first), in pages that store each row in
    /// the smallest form that holds it; see [`History`].
    history: History,
    /// Maximum history size
    max_history: usize,
    /// Current scroll offset (0 = at bottom, >0 = scrolled up)
    scroll_offset: usize,
    /// Inline surface anchors by tag - 1 (ADR 0020); `None` is a free tag.
    /// At most [`MAX_ANCHORS`] long, with no trailing free entries.
    anchors: Vec<Option<AnchorEntry>>,
    /// Lines dropped from the front of history so far, cleared ones
    /// included, so `evicted + absolute index` numbers a line for good.
    evicted: usize,
    /// Anchors a reflow pushed out of scrollback, for the next sweep to
    /// report.
    lost_in_reflow: Vec<(u16, u64, Anchor)>,
    /// The serial the next placed anchor gets.
    next_anchor_serial: u64,
    /// Rows rewritten in place ([`Buffer::replace_row`]) so far.
    rewrites: u64,
    /// The rewrite count after each line was last rewritten, by line
    /// number (`evicted` plus absolute index). Lines evicted since are
    /// pruned.
    rewritten_lines: BTreeMap<usize, u64>,
    /// Inline images (Sixel) placed on this screen. Each screen keeps its
    /// own, as foot does (`normal.sixel_images`, `alt.sixel_images`).
    images: ImageStore,
    /// Rows looked at for anchor bookkeeping, so tests can bound its cost.
    #[cfg(test)]
    anchor_visits: std::cell::Cell<usize>,
}

/// The first row of anchor `anchor` is now line `line` (as numbered in
/// [`AnchorEntry::first_line`]).
#[cold]
fn note_first_row(anchors: &mut [Option<AnchorEntry>], anchor: AnchorTag, line: usize) {
    if let Some(Some(entry)) = anchors.get_mut(usize::from(anchor.tag()) - 1) {
        entry.first_line = line;
    }
}

/// A cell being re-wrapped, plus the extra codepoints of its grapheme.
type ReflowCell = (Cell, Option<Vec<char>>);

fn push_reflow_cells(line: &mut Vec<ReflowCell>, row: &RowView<'_>) {
    for col in 0..row.width() {
        let Some(cell) = row.cell(col) else {
            break;
        };
        let extra = row.grapheme_extra(col).map(<[char]>::to_vec);
        line.push((cell, extra));
    }
}

/// Where [`Buffer::reflow_all`] puts rewrapped rows: the last screenful
/// whole, for the grid, and older rows appended to history pages as they
/// leave it, keeping at most `max_history`.
struct Rewrap {
    cols: u16,
    screen_rows: usize,
    max_history: usize,
    history: History,
    screen: VecDeque<Row>,
    /// Packs the rows that go to history.
    packer: RowPacker,
    /// Rows made so far, including those dropped past the history limit.
    total: usize,
    /// Logical lines finished so far (empty ones still pending included).
    lines: usize,
    /// The cursor's logical line and its offset in it, once seen.
    cursor: Option<(usize, usize)>,
    /// The cursor's new absolute row and column, once placed.
    cursor_at: Option<(usize, usize)>,
    /// Empty, unmarked lines not yet emitted: dropped if nothing follows.
    pending_empty: usize,
    /// Images whose top row is in a logical line not yet laid out, oldest
    /// first: the line's index, the image's offset in it, and its slot.
    watched: VecDeque<(usize, usize, usize)>,
    /// Where each watched image went: its slot and the index among the rows
    /// made of the row holding its top-left corner.
    placed: Vec<(usize, usize)>,
}

impl Rewrap {
    fn new(cols: u16, screen_rows: usize, max_history: usize) -> Self {
        Self {
            cols,
            screen_rows,
            max_history,
            history: History::default(),
            screen: VecDeque::with_capacity(screen_rows + 1),
            packer: RowPacker::new(),
            total: 0,
            lines: 0,
            cursor: None,
            cursor_at: None,
            pending_empty: 0,
            watched: VecDeque::new(),
            placed: Vec::new(),
        }
    }

    /// The image in `slot` has its top-left corner `offset` cells into the
    /// logical line being gathered.
    fn watch_image(&mut self, slot: usize, offset: usize) {
        self.watched.push_back((self.lines, offset, slot));
    }

    /// Place the images watched in logical line `index`, whose rows are
    /// `start..self.total`, on the row their corner wrapped to.
    fn place_images(&mut self, index: usize, start: usize) {
        let rows = (self.total - start).max(1);
        let cols = usize::from(self.cols).max(1);
        while self.watched.front().is_some_and(|w| w.0 == index) {
            let Some((_, offset, slot)) = self.watched.pop_front() else {
                break;
            };
            self.placed
                .push((slot, start + (offset / cols).min(rows - 1)));
        }
    }

    /// The cursor is in the logical line being gathered, `offset` cells in.
    /// Only the first call counts.
    fn set_cursor(&mut self, offset: usize) {
        if self.cursor.is_none() {
            self.cursor = Some((self.lines, offset));
        }
    }

    fn push_row(&mut self, row: Row) {
        self.total += 1;
        self.screen.push_back(row);
        if self.screen.len() > self.screen_rows {
            let Some(old) = self.screen.pop_front() else {
                return;
            };
            if self.max_history > 0 {
                if self.history.len() >= self.max_history {
                    self.history.pop_front();
                }
                self.history.push_row(&old, &mut self.packer);
            }
        }
    }

    /// Finish a logical line: trim trailing blanks and lay it out, leaving
    /// `line` empty for the next one. Trailing empty lines without marks
    /// are held back and dropped if nothing follows them.
    fn finish_line(&mut self, line: &mut Vec<ReflowCell>, marks: RowMarks) {
        while line.last().is_some_and(|(c, _)| c.is_blank()) {
            line.pop();
        }
        let index = self.lines;
        self.lines += 1;
        // A line holding an image's corner is content even if blank.
        let holds_image = self.watched.back().is_some_and(|w| w.0 == index);
        if line.is_empty() && marks.is_empty() && !holds_image {
            self.pending_empty += 1;
            return;
        }
        let first_pending = index - self.pending_empty;
        for pending in first_pending..index {
            self.lay_out(pending, &[], RowMarks::default());
        }
        self.pending_empty = 0;
        self.lay_out(index, line, marks);
        line.clear();
    }

    /// Lay out a row carrying an anchor tag (ADR 0020): a logical line of
    /// its own that never wraps, clipped or padded to the new width, with
    /// its tag and marks. A wide character cut in half by the clip is
    /// blanked. Returns the row's index among the rows made.
    fn finish_tagged(&mut self, row: &Row) -> usize {
        let index = self.lines;
        self.lines += 1;
        for pending in index - self.pending_empty..index {
            self.lay_out(pending, &[], RowMarks::default());
        }
        self.pending_empty = 0;

        let mut new_row = row.clone();
        let cols = self.cols;
        if cols > 0 && cols < new_row.width() {
            let last = new_row[cols - 1];
            if last.is_wide() {
                new_row.erase_range(cols - 1, cols, last.bg());
            }
        }
        new_row.resize(cols);
        new_row.set_wrapped(false);
        if let Some((line, col)) = self.cursor {
            if line == index {
                self.cursor_at = Some((self.total, col.min(usize::from(cols.saturating_sub(1)))));
            }
        }
        let made = self.total;
        self.push_row(new_row);
        self.place_images(index, made);
        made
    }

    /// Wrap logical line `index` to the new width, placing the cursor if it
    /// is in this line.
    fn lay_out(&mut self, index: usize, line: &[ReflowCell], marks: RowMarks) {
        let start = self.total;
        self.lay_out_rows(index, line, marks);
        self.place_images(index, start);
    }

    fn lay_out_rows(&mut self, index: usize, line: &[ReflowCell], marks: RowMarks) {
        let cursor_col = match self.cursor {
            // Allow the cursor at len(): one past the last character.
            Some((cursor_line, col)) if cursor_line == index => Some(col.min(line.len())),
            _ => None,
        };
        let new_cols = self.cols;
        if line.is_empty() || new_cols == 0 {
            // An empty line, or a zero-width row where nothing can be
            // placed: one row per logical line keeps row counts meaningful.
            if cursor_col.is_some() {
                self.cursor_at = Some((self.total, 0));
            }
            let mut row = Row::new(new_cols);
            row.set_marks(marks);
            self.push_row(row);
            return;
        }

        let mut pos = 0;
        while pos < line.len() {
            let row_start = pos;
            let mut new_row = Row::new(new_cols);
            let mut col = 0u16;

            while col < new_cols && pos < line.len() {
                let (cell, extra) = &line[pos];

                // Skip spacer cells from old layout — regenerated when placing wide chars
                if cell.is_wide_spacer() {
                    pos += 1;
                    continue;
                }

                // Wide char needs 2 columns; if only 1 remains, wrap to next row
                if cell.is_wide() && col + 1 >= new_cols {
                    if col > 0 {
                        break;
                    }
                    // The row is empty and the wide char can never fit
                    // (single-column terminal): emit a blank placeholder
                    // so `pos` always advances and the loop terminates.
                    if let Some(dest) = new_row.get_mut(col) {
                        *dest = *cell;
                        dest.set_char(' ');
                        dest.unset_flag(CellFlags::WIDE_CHAR);
                        dest.unset_flag(CellFlags::HAS_COMBINING);
                    }
                    col += 1;
                    pos += 1;
                    continue;
                }

                if let Some(dest) = new_row.get_mut(col) {
                    *dest = *cell;
                }
                if let Some(extra) = extra {
                    new_row.set_grapheme_extra(col, extra);
                }
                col += 1;
                pos += 1;

                // After placing a wide char, insert its spacer cell
                if cell.is_wide() {
                    if let Some(spacer) = new_row.get_mut(col) {
                        spacer.set_flag(CellFlags::WIDE_CHAR_SPACER);
                    }
                    col += 1;
                }
            }

            if let Some(cursor_col) = cursor_col {
                if self.cursor_at.is_none() && cursor_col >= row_start && cursor_col < pos {
                    self.cursor_at = Some((self.total, cursor_col - row_start));
                }
            }
            if pos < line.len() {
                new_row.set_wrapped(true);
            }
            if row_start == 0 {
                new_row.set_marks(marks);
            }
            self.push_row(new_row);
        }

        // Cursor at exact end of logical line
        if let Some(cursor_col) = cursor_col {
            if self.cursor_at.is_none() {
                let col = cursor_col
                    .saturating_sub(line.len().saturating_sub(line.len() % new_cols as usize));
                self.cursor_at = Some((self.total.saturating_sub(1), col));
            }
        }
    }
}

/// Which tagged rows a reflow keeps tagged (ADR 0020): a live anchor's
/// first row and the `rows - 1` rows right after it that carry its tag.
/// Any other tag is stale and dropped, so the rows rewrap as text.
struct AnchorRuns {
    /// Each tag's reserved rows, if its anchor is live.
    rows: Vec<Option<u16>>,
    /// Where each tag's first row went, as an index among the rows made.
    made_at: Vec<Option<usize>>,
    /// The run being followed: its tag and how many rows it has left.
    run: Option<(u16, u16)>,
}

impl AnchorRuns {
    fn new(anchors: &[Option<AnchorEntry>]) -> Self {
        Self {
            rows: anchors
                .iter()
                .map(|entry| entry.map(|e| e.anchor.rows))
                .collect(),
            made_at: vec![None; anchors.len()],
            run: None,
        }
    }

    /// Whether the next row, tagged `anchor`, stays tagged. A first row
    /// kept here must be [`AnchorRuns::placed`] once laid out.
    fn keep(&mut self, anchor: AnchorTag) -> bool {
        let tag = anchor.tag();
        let index = usize::from(tag).wrapping_sub(1);
        if anchor.is_first() {
            if let Some(Some(rows)) = self.rows.get(index) {
                if self.made_at[index].is_none() {
                    self.run = Some((tag, rows - 1));
                    return true;
                }
            }
        } else if let Some((run_tag, left)) = self.run {
            if run_tag == tag && left > 0 {
                self.run = Some((tag, left - 1));
                return true;
            }
        }
        self.run = None;
        false
    }

    /// Anchor `tag`'s first row became row `made` of the reflow.
    fn placed(&mut self, tag: u16, made: usize) {
        self.made_at[usize::from(tag) - 1] = Some(made);
    }
}

impl Buffer {
    /// Restore a validated viewer's absolute line origin before anchor hints.
    pub(super) fn restore_view_line_origin(&mut self, evicted: usize) {
        self.evicted = evicted;
    }

    /// Restore validated viewer anchor locations without reserving or erasing rows.
    pub(super) fn restore_view_anchors(&mut self, anchors: &[(u16, u64, Anchor, usize)]) {
        self.anchors.clear();
        for &(tag, serial, anchor, line) in anchors {
            self.anchors
                .resize(self.anchors.len().max(usize::from(tag)), None);
            self.anchors[usize::from(tag) - 1] = Some(AnchorEntry {
                anchor,
                serial,
                first_line: line,
                settled: false,
            });
            self.next_anchor_serial = self.next_anchor_serial.max(serial.saturating_add(1));
        }
    }

    /// Check the buffer's invariants: the grid's own (see
    /// [`Grid::check_integrity`]), scrollback within its limit, a viewport
    /// offset that points into history, and consistent history rows.
    pub fn check_integrity(&self) -> Result<(), String> {
        self.grid.check_integrity()?;
        if self.history.len() > self.max_history {
            return Err(format!(
                "history holds {} rows, limit is {}",
                self.history.len(),
                self.max_history
            ));
        }
        if self.scroll_offset > self.history.len() {
            return Err(format!(
                "viewport offset {} beyond {} history rows",
                self.scroll_offset,
                self.history.len()
            ));
        }
        self.history
            .check_integrity(super::MAX_COMBINING_PER_CELL)?;
        for index in 0..self.history.len() {
            let row = self
                .history
                .row(index)
                .ok_or_else(|| format!("history row {index} is missing"))?;
            RowView::history(row)
                .to_row()
                .check_integrity(super::MAX_COMBINING_PER_CELL)
                .map_err(|e| format!("history row {index}: {e}"))?;
        }
        self.check_anchors()
    }

    /// Every first row belongs to a live anchor, and no anchor has two.
    /// Other tagged rows may be stale (see [`Buffer::strip_anchor_rows`]).
    fn check_anchors(&self) -> Result<(), String> {
        if self.anchors.len() > MAX_ANCHORS {
            return Err(format!(
                "anchor table holds {} entries, limit is {MAX_ANCHORS}",
                self.anchors.len()
            ));
        }
        let mut first_seen = vec![false; self.anchors.len()];
        for line in 0..self.total_lines() {
            let Some(anchor) = self.anchor_at(line).filter(|a| a.is_first()) else {
                continue;
            };
            let index = usize::from(anchor.tag()) - 1;
            if !self.anchors.get(index).is_some_and(Option::is_some) {
                return Err(format!(
                    "line {line} is the first row of tag {}, which no anchor has",
                    anchor.tag()
                ));
            }
            if first_seen[index] {
                return Err(format!("anchor {} has two first rows", anchor.tag()));
            }
            first_seen[index] = true;
        }
        Ok(())
    }

    /// Create a new buffer with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        Self::with_scrollback(dims, DEFAULT_SCROLLBACK)
    }

    /// Create a new buffer with custom scrollback size.
    pub fn with_scrollback(dims: Dimensions, max_history: usize) -> Self {
        Self {
            grid: Grid::new(dims),
            history: History::default(),
            max_history,
            scroll_offset: 0,
            anchors: Vec::new(),
            evicted: 0,
            lost_in_reflow: Vec::new(),
            next_anchor_serial: 1,
            rewrites: 0,
            rewritten_lines: BTreeMap::new(),
            images: ImageStore::new(),
            #[cfg(test)]
            anchor_visits: std::cell::Cell::new(0),
        }
    }

    /// This screen's inline images.
    pub fn images(&self) -> &ImageStore {
        &self.images
    }

    /// Mutable access to this screen's inline images.
    pub fn images_mut(&mut self) -> &mut ImageStore {
        &mut self.images
    }

    /// How many rows have been evicted from the front of history so far.
    ///
    /// `evicted_lines() + absolute index` numbers a row for good, so a
    /// reader holding absolute indices can tell how far they have shifted.
    pub fn evicted_lines(&self) -> usize {
        self.evicted
    }

    /// Get the underlying grid.
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Get mutable access to the grid.
    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// Get grid dimensions.
    pub fn dimensions(&self) -> Dimensions {
        self.grid.dimensions()
    }

    /// Get total lines (visible + history).
    pub fn total_lines(&self) -> usize {
        self.history.len() + self.grid.dimensions().rows as usize
    }

    /// Get current scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Check if scrolled back from bottom.
    pub fn is_scrolled(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Scroll up by n lines.
    pub fn scroll_up(&mut self, n: usize) {
        let max_offset = self.history.len();
        self.scroll_offset = (self.scroll_offset + n).min(max_offset);
    }

    /// Scroll down by n lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Absolute index of the row at the top of the view (0 is the oldest
    /// scrollback row), whichever way the view is scrolled.
    pub fn top_row(&self) -> usize {
        self.history.len() - self.scroll_offset.min(self.history.len())
    }

    /// Scroll so that the row at `absolute_index` is at the top of the view,
    /// or as close as the buffer allows: a row in the live screen cannot be
    /// scrolled to the top, so the view goes to the bottom.
    pub fn scroll_to_row(&mut self, absolute_index: usize) {
        self.scroll_offset = self.history.len().saturating_sub(absolute_index);
    }

    /// The nearest row above the top of the view that has shell input
    /// starting on it (OSC 133;B), if any.
    pub fn prompt_row_above_view(&self) -> Option<usize> {
        (0..self.top_row())
            .rev()
            .find(|&index| self.has_prompt_at(index))
    }

    /// The nearest row below the top of the view that has shell input
    /// starting on it, if any.
    pub fn prompt_row_below_view(&self) -> Option<usize> {
        (self.top_row() + 1..self.total_lines()).find(|&index| self.has_prompt_at(index))
    }

    fn has_prompt_at(&self, index: usize) -> bool {
        self.marks_at(index).is_some_and(|m| m.input_start != 0)
    }

    /// Scroll to top of history.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = self.history.len();
    }

    /// Scroll to bottom (most recent).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Copy the top `n` grid rows into history.
    ///
    /// Once history is full, the oldest row is evicted for each one, and
    /// pages freed that way are reused, so steady-state scrolling allocates
    /// nothing.
    fn save_top_rows(&mut self, n: usize) {
        if self.max_history == 0 {
            return;
        }
        for index in 0..n.min(self.grid.dimensions().rows as usize) as u16 {
            let Some(row) = self.grid.page.row(index) else {
                break;
            };
            if self.history.len() >= self.max_history {
                self.evicted += 1;
                self.history.pop_front();
            }
            if row.anchor().is_first() {
                // As `note_row_to_history`, on the fields the row does not
                // borrow.
                let line = self.evicted + self.history.len();
                note_first_row(&mut self.anchors, row.anchor(), line);
            }
            self.history.push(row, &self.grid.styles);
        }
        self.prune_evicted_images();
    }

    /// Push lines from grid top into history (called when grid scrolls up).
    pub fn push_to_history(&mut self, rows: Vec<Row>) {
        if self.max_history == 0 {
            return; // No history storage configured (e.g., alternate screen)
        }
        let mut packer = RowPacker::new();
        for row in rows {
            if self.history.len() >= self.max_history {
                self.evicted += 1;
                self.history.pop_front();
            }
            self.note_row_to_history(row.anchor());
            self.history.push_row(&row, &mut packer);
        }
        self.prune_evicted_images();
    }

    /// Get a row at the given visual index (accounting for scroll).
    ///
    /// Index 0 is the top of the visible area. Screen and scrollback rows
    /// alike are read in place.
    pub fn visible_row(&self, visual_index: u16) -> Option<RowView<'_>> {
        let total_history = self.history.len();
        let visible_rows = self.grid.dimensions().rows as usize;

        // Calculate which row this maps to
        if self.scroll_offset > 0 {
            // We're scrolled back
            let history_start = total_history.saturating_sub(self.scroll_offset);
            let absolute_index = history_start + visual_index as usize;

            if absolute_index < total_history {
                // This row is in history
                self.history.row(absolute_index).map(RowView::history)
            } else {
                // This row is in the visible grid
                let grid_index = absolute_index - total_history;
                if grid_index < visible_rows {
                    self.grid.row(grid_index as u16)
                } else {
                    None
                }
            }
        } else {
            // Not scrolled, show grid directly
            self.grid.row(visual_index)
        }
    }

    /// Get a row by absolute index (for searching all content).
    ///
    /// Index 0 is the oldest history row.
    /// Indices beyond history access the current grid.
    /// Like [`Buffer::visible_row`], scrollback rows are read in place.
    pub fn row_at(&self, absolute_index: usize) -> Option<RowView<'_>> {
        let history_len = self.history.len();
        if absolute_index < history_len {
            self.history.row(absolute_index).map(RowView::history)
        } else {
            let grid_index = absolute_index - history_len;
            self.grid.row(grid_index as u16)
        }
    }

    /// The shell marks of the row at `absolute_index` (as for
    /// [`Buffer::row_at`]), from the row's head.
    pub fn marks_at(&self, absolute_index: usize) -> Option<RowMarks> {
        let history_len = self.history.len();
        if absolute_index < history_len {
            self.history.marks(absolute_index)
        } else {
            let grid_index = absolute_index - history_len;
            let row = u16::try_from(grid_index)
                .ok()
                .and_then(|i| self.grid.page.row(i));
            row.map(|row| row.marks())
        }
    }

    /// Replace the cells of the row at `absolute_index` (as for
    /// [`Buffer::row_at`]) with those of `row`, as writing an inline
    /// surface does (ADR 0020). The row keeps its anchor tag and shell
    /// marks, does not wrap, and is clipped or padded to the buffer's
    /// width; a scrollback row is rewritten in its page. Returns whether the
    /// row changed: one that already holds those cells, or that does not
    /// exist, is left alone.
    ///
    /// A rewrite is not output: the content version stays, so selections
    /// are kept unless they cover the row (see
    /// [`Buffer::rows_rewritten_since`]).
    pub fn replace_row(&mut self, absolute_index: usize, row: &Row) -> bool {
        let cols = self.grid.dimensions().cols;
        let history_len = self.history.len();
        let mut replaced = row.clone();
        replaced.resize(cols);
        replaced.set_wrapped(false);
        if absolute_index < history_len {
            let Some(old) = self.history.row(absolute_index) else {
                return false;
            };
            replaced.set_marks(old.marks());
            replaced.set_anchor(old.anchor());
            if RowView::history(old).eq_row(&replaced) && !old.is_wrapped() {
                return false;
            }
            self.history
                .replace(absolute_index, &replaced, &mut RowPacker::new());
        } else {
            let Ok(index) = u16::try_from(absolute_index - history_len) else {
                return false;
            };
            let Some(mut target) = self.grid.row_mut_in_place(index) else {
                return false;
            };
            if target.view().eq_row(&replaced) && !target.is_wrapped() {
                return false;
            }
            replaced.set_marks(target.marks());
            replaced.set_anchor(target.anchor());
            target.assign(&replaced);
        }
        self.note_rewrite(self.evicted + absolute_index);
        true
    }

    /// Record that line `line` (as numbered in `rewritten_lines`) was
    /// rewritten in place, forgetting lines evicted since.
    ///
    /// With no history (`max_history` 0) rows scrolled off are not counted
    /// in `evicted`, so a line's number can come to name another row. That
    /// costs at most a selection over that row being dropped, and the map
    /// holds at most a screenful of such lines.
    fn note_rewrite(&mut self, line: usize) {
        self.rewrites += 1;
        self.rewritten_lines.insert(line, self.rewrites);
        while self
            .rewritten_lines
            .first_key_value()
            .is_some_and(|(&line, _)| line < self.evicted)
        {
            self.rewritten_lines.pop_first();
        }
    }

    /// How many rows were rewritten in place so far, for
    /// [`Buffer::rows_rewritten_since`].
    pub fn rewrite_count(&self) -> u64 {
        self.rewrites
    }

    /// Whether any of the rows at absolute indices `rows` was rewritten in
    /// place after [`Buffer::rewrite_count`] returned `since`.
    pub fn rows_rewritten_since(&self, since: u64, rows: std::ops::RangeInclusive<usize>) -> bool {
        if since == self.rewrites {
            return false;
        }
        let lines = self.evicted + rows.start()..=self.evicted + rows.end();
        self.rewritten_lines
            .range(lines)
            .any(|(_, &count)| count > since)
    }

    /// The anchor tag of the row at `absolute_index` (as for
    /// [`Buffer::row_at`]), from the row's head.
    pub fn anchor_at(&self, absolute_index: usize) -> Option<AnchorTag> {
        #[cfg(test)]
        self.anchor_visits.set(self.anchor_visits.get() + 1);
        let history_len = self.history.len();
        if absolute_index < history_len {
            self.history.anchor(absolute_index)
        } else {
            let grid_index = absolute_index - history_len;
            let row = u16::try_from(grid_index)
                .ok()
                .and_then(|i| self.grid.page.row(i));
            row.map(|row| row.anchor())
        }
    }

    /// Reserve `anchor.rows` rows at the cursor for an inline surface
    /// (ADR 0020): scroll them in as `rows - 1` line feeds would, tag each,
    /// and put the cursor back on the first. Returns the tag and the
    /// placement's serial (see [`Buffer::anchor_serial`]).
    ///
    /// Anchors whose first row is gone are not freed here; call
    /// [`Buffer::sweep_anchors`] first so their tags can be reused.
    pub fn place_anchor(&mut self, anchor: Anchor) -> Result<(u16, u64), AnchorRefusal> {
        let region = self.grid.effective_scroll_region();
        let cursor_row = self.grid.cursor().position.row;
        let height = region.bottom - region.top + 1;
        if anchor.rows == 0 || anchor.rows > height || !region.contains(cursor_row) {
            return Err(AnchorRefusal::NoRoom);
        }
        let index = match self.anchors.iter().position(Option::is_none) {
            Some(index) => index,
            None if self.anchors.len() < MAX_ANCHORS => {
                self.anchors.push(None);
                self.anchors.len() - 1
            }
            None => return Err(AnchorRefusal::TableFull),
        };
        let tag = index as u16 + 1;

        for _ in 1..anchor.rows {
            self.newline();
        }
        let last = self.grid.cursor().position.row;
        let first = last + 1 - anchor.rows;
        for row in first..=last {
            if let Some(mut row_ref) = self.grid.row_mut(row) {
                row_ref.set_anchor(AnchorTag::new(tag, row == first));
            }
        }
        // Back on the first row, with no wrap pending: fallback text
        // printed at the last column must not wrap out of the reservation.
        let cursor = self.grid.cursor_mut();
        cursor.position.row = first;
        cursor.wrap_pending = false;
        let serial = self.next_anchor_serial;
        self.next_anchor_serial += 1;
        self.anchors[index] = Some(AnchorEntry {
            anchor,
            serial,
            first_line: self.evicted + self.history.len() + usize::from(first),
            settled: false,
        });
        Ok((tag, serial))
    }

    /// The serial of the live anchor tagged `tag`: placements number up
    /// from 1 in the order they happened, and a tag freed and reused gets
    /// a new one.
    pub fn anchor_serial(&self, tag: u16) -> Option<u64> {
        let index = usize::from(tag).checked_sub(1)?;
        Some(self.anchors.get(index)?.as_ref()?.serial)
    }

    /// The anchor tagged `tag`, if it is live.
    pub fn anchor(&self, tag: u16) -> Option<Anchor> {
        let index = usize::from(tag).checked_sub(1)?;
        Some(self.anchors.get(index)?.as_ref()?.anchor)
    }

    /// Live anchors and their tags, in tag order.
    pub fn anchors(&self) -> impl Iterator<Item = (u16, Anchor)> + '_ {
        self.anchors
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| Some((index as u16 + 1, entry.as_ref()?.anchor)))
    }

    /// Where `entry`'s hint says its first row is, as an absolute index;
    /// negative once that line was evicted.
    fn hinted_line(&self, entry: &AnchorEntry) -> isize {
        entry.first_line as isize - self.evicted as isize
    }

    /// The absolute index (as for [`Buffer::row_at`]) of the first row of
    /// anchor `tag`, or `None` if that row is gone.
    ///
    /// Scrollback rows move only by eviction, which the hint counts, and by
    /// reflow, which rewrites it, and a first row going into scrollback
    /// sets it. So a hint that misses can only be stale for a row that
    /// moved on the grid (insert or delete line, scroll regions), and only
    /// the grid is searched: a lookup costs at most a screenful.
    pub fn anchor_line(&self, tag: u16) -> Option<usize> {
        let index = usize::from(tag).checked_sub(1)?;
        let entry = self.anchors.get(index)?.as_ref()?;
        let first = AnchorTag::new(tag, true);
        if let Ok(hint) = usize::try_from(self.hinted_line(entry)) {
            if self.anchor_at(hint) == Some(first) {
                return Some(hint);
            }
        }
        let history_len = self.history.len();
        (history_len..self.total_lines()).find(|&line| self.anchor_at(line) == Some(first))
    }

    /// Whether the `rows` rows from `first` are all reserved for `tag`: the
    /// first row and then `rows - 1` rows carrying the tag.
    fn run_intact(&self, first: usize, tag: u16, rows: u16) -> bool {
        let rest = AnchorTag::new(tag, false);
        (1..usize::from(rows)).all(|offset| self.anchor_at(first + offset) == Some(rest))
    }

    /// Find every live anchor's first row, refreshing where it was seen.
    /// Anchors that lost a reserved row (erased, evicted, cleared, moved
    /// apart by insert or delete line, pushed out of scrollback by reflow)
    /// are dropped: their tags are freed and taken off their rows, and they
    /// are returned.
    ///
    /// An anchor whose rows are all in scrollback cannot change but by
    /// eviction, so once checked there it costs one row per sweep; the
    /// others cost their rows plus, when they moved, a screenful.
    pub fn sweep_anchors(&mut self) -> Vec<(u16, u64, Anchor)> {
        let mut lost = std::mem::take(&mut self.lost_in_reflow);
        for index in 0..self.anchors.len() {
            let Some(entry) = self.anchors[index] else {
                continue;
            };
            let tag = index as u16 + 1;
            let rows = entry.anchor.rows;
            let found = self.anchor_line(tag);
            let intact = match found {
                Some(_) if entry.settled => true,
                Some(line) => self.run_intact(line, tag, rows),
                None => false,
            };
            if intact {
                let line = found.unwrap_or_default();
                let settled = line + usize::from(rows) <= self.history.len();
                if let Some(entry) = &mut self.anchors[index] {
                    entry.first_line = self.evicted + line;
                    entry.settled = settled;
                }
            } else {
                let from = found.map_or_else(|| self.hinted_line(&entry), |line| line as isize);
                self.anchors[index] = None;
                self.strip_anchor_rows(tag, from, rows);
                lost.push((tag, entry.serial, entry.anchor));
            }
        }
        self.trim_anchor_table();
        lost
    }

    /// Drop anchor `tag` and take its tag off its rows, as when its surface
    /// is closed. Returns it if it was live.
    pub fn release_anchor(&mut self, tag: u16) -> Option<Anchor> {
        let index = usize::from(tag).checked_sub(1)?;
        let entry = (*self.anchors.get(index)?)?;
        let from = self
            .anchor_line(tag)
            .map_or_else(|| self.hinted_line(&entry), |line| line as isize);
        self.anchors[index] = None;
        self.strip_anchor_rows(tag, from, entry.anchor.rows);
        self.trim_anchor_table();
        Some(entry.anchor)
    }

    /// Drop every anchor and take all tags off rows, as a full reset does.
    /// Returns the anchors that were live. This walks all of scrollback; a
    /// reset clears it first.
    pub fn drop_all_anchors(&mut self) -> Vec<(u16, u64, Anchor)> {
        let mut live = std::mem::take(&mut self.lost_in_reflow);
        live.extend(
            self.anchors
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    let entry = entry.as_ref()?;
                    Some((index as u16 + 1, entry.serial, entry.anchor))
                }),
        );
        self.anchors.clear();
        self.history.clear_anchors();
        self.strip_grid_tags(|_| true);
        live
    }

    /// Take tag `tag` off the scrollback rows an anchor whose first row is
    /// (or was) at absolute index `from` reserved, and off any grid row.
    /// Rows the anchor lost track of elsewhere in scrollback (pulled apart
    /// by insert line before scrolling off) keep the stale tag; it marks no
    /// first row, so nothing reads it, and the next reflow drops it.
    fn strip_anchor_rows(&mut self, tag: u16, from: isize, rows: u16) {
        let history_len = self.history.len() as isize;
        let start = from.clamp(0, history_len) as usize;
        let end = from.saturating_add(rows as isize).clamp(0, history_len) as usize;
        for line in start..end {
            #[cfg(test)]
            self.anchor_visits.set(self.anchor_visits.get() + 1);
            if self.history.anchor(line).is_some_and(|a| a.tag() == tag) {
                self.history.set_anchor(line, AnchorTag::NONE);
            }
        }
        self.strip_grid_tags(|t| t == tag);
    }

    /// Clear the anchor tag of every grid row whose tag `drop` picks.
    fn strip_grid_tags(&mut self, drop: impl Fn(u16) -> bool) {
        // `row_mut` marks a row dirty, so only for rows that change.
        for index in 0..self.grid.dimensions().rows {
            let picked = self
                .grid
                .row(index)
                .is_some_and(|row| !row.anchor().is_none() && drop(row.anchor().tag()));
            if picked {
                if let Some(mut row) = self.grid.row_mut(index) {
                    row.set_anchor(AnchorTag::NONE);
                }
            }
        }
    }

    /// After a reflow, point each anchor's hint at where its first row went
    /// (`made_at`, counting `dropped` rows that fell past the history
    /// limit). Anchors whose first row was dropped or not found are lost,
    /// and reported by the next sweep.
    fn place_anchors_after_reflow(&mut self, made_at: &[Option<usize>], dropped: usize) {
        for (index, &made) in made_at.iter().enumerate() {
            let Some(entry) = self.anchors[index] else {
                continue;
            };
            match made {
                Some(made) if made >= dropped => {
                    if let Some(entry) = &mut self.anchors[index] {
                        entry.first_line = self.evicted + made - dropped;
                        entry.settled = false;
                    }
                }
                _ => {
                    let tag = index as u16 + 1;
                    let from = made.map_or(isize::MIN / 2, |made| made as isize - dropped as isize);
                    self.anchors[index] = None;
                    self.strip_anchor_rows(tag, from, entry.anchor.rows);
                    self.lost_in_reflow.push((tag, entry.serial, entry.anchor));
                }
            }
        }
        self.trim_anchor_table();
    }

    /// Rows that went back from scrollback to the grid can be erased
    /// again, so anchors that were settled there must be checked again.
    fn unsettle_anchors(&mut self) {
        for entry in self.anchors.iter_mut().flatten() {
            entry.settled = false;
        }
    }

    /// A row tagged `anchor` is about to become scrollback line
    /// `history.len()`: if it is a first row, that is where its anchor is.
    fn note_row_to_history(&mut self, anchor: AnchorTag) {
        if anchor.is_first() {
            let line = self.evicted + self.history.len();
            note_first_row(&mut self.anchors, anchor, line);
        }
    }

    fn trim_anchor_table(&mut self) {
        while self.anchors.last().is_some_and(Option::is_none) {
            self.anchors.pop();
        }
    }

    /// The text of rows `start..end` (absolute indices), from column
    /// `first_col` of the first row. A soft-wrapped row runs on into the
    /// next; other rows end in a newline. Trailing blanks and blank lines
    /// at the end are dropped.
    pub fn text_of_rows(&self, start: usize, end: usize, first_col: u16) -> String {
        let mut text = String::new();
        for index in start..end.min(self.total_lines()) {
            let Some(row) = self.row_at(index) else {
                break;
            };
            let from = if index == start { first_col } else { 0 };
            if row.is_wrapped() {
                text.push_str(&row.copy_text(from, row.width()));
            } else {
                text.push_str(row.copy_text(from, row.width()).trim_end());
                text.push('\n');
            }
        }
        text.truncate(text.trim_end().len());
        text
    }

    /// Clear scrollback history.
    pub fn clear_history(&mut self) {
        // Images in scrollback go with it (foot's `term_erase` for ED 3).
        let top = self.screen_top_line();
        self.images.retain_mut(|image| image.line >= top);
        self.evicted += self.history.len();
        self.history.clear();
        self.scroll_offset = 0;
    }

    /// Get history length.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// How many lines of history the buffer keeps.
    pub fn max_history(&self) -> usize {
        self.max_history
    }

    /// Keep at most `max_history` lines of history from now on, dropping
    /// the oldest lines beyond it. Returns how many lines were dropped.
    pub fn set_max_history(&mut self, max_history: usize) -> usize {
        self.max_history = max_history;
        let excess = self.history.len().saturating_sub(max_history);
        self.history.drop_front(excess);
        self.evicted += excess;
        self.prune_evicted_images();
        self.scroll_offset = self.scroll_offset.min(self.history.len());
        excess
    }

    /// Resize the buffer with proper reflow.
    pub fn resize(&mut self, new_dims: Dimensions) {
        let old_dims = self.grid.dimensions();

        if new_dims == old_dims {
            return;
        }
        // Reflow renumbers lines, and changes the content version anyway.
        self.rewritten_lines.clear();

        // If width changed, we need to reflow both history and grid together
        if new_dims.cols != old_dims.cols {
            self.reflow_all(new_dims);
        } else {
            // Only height changed - handle specially
            self.resize_height(new_dims);
        }

        self.scroll_offset = self.scroll_offset.min(self.history.len());
    }

    /// Handle resize when only height changes.
    fn resize_height(&mut self, new_dims: Dimensions) {
        let old_rows = self.grid.dimensions().rows as usize;
        let new_rows = new_dims.rows as usize;

        if new_rows < old_rows {
            // Shrinking. Blank rows below the cursor go first (the grid
            // resize below truncates them), as in xterm: otherwise a
            // half-empty screen would scroll its top lines into history
            // just because the window got shorter. Whatever is still too
            // much moves from the top into history.
            let cursor_row = self.grid.cursor().position.row as usize;
            let image_bottom = self
                .images
                .images()
                .filter_map(|image| image.line.checked_add(usize::from(image.cell_height)))
                .max()
                .unwrap_or(0)
                .saturating_sub(self.screen_top_line())
                .min(old_rows);
            let blank_below = (cursor_row + 1..old_rows)
                .rev()
                .take_while(|&row| {
                    row >= image_bottom
                        && self
                            .grid
                            .row(row as u16)
                            .is_some_and(|row| row.is_blank() && row.anchor().is_none())
                })
                .count();
            let rows_to_move = (old_rows - new_rows).saturating_sub(blank_below);
            self.shift_images_for_scroll_up(
                ScrollRegion {
                    top: 0,
                    bottom: old_rows.saturating_sub(1) as u16,
                },
                rows_to_move,
            );
            self.save_top_rows(rows_to_move);
            // Remove those rows from the grid (shift everything up)
            self.grid.remove_top_rows(rows_to_move);
        }

        self.grid.resize(new_dims);

        if new_rows > old_rows {
            // Growing - try to pull rows back from history
            let rows_to_pull = (new_rows - old_rows).min(self.history.len());
            if rows_to_pull > 0 {
                // Pull from end of history (most recent)
                let start = self.history.len() - rows_to_pull;
                let pulled: Vec<Row> = (start..self.history.len())
                    .filter_map(|index| self.history.row(index))
                    .map(|row| RowView::history(row).to_row())
                    .collect();
                self.history.truncate(start);
                // Insert at beginning of grid
                self.grid.prepend_rows(pulled);
                self.unsettle_anchors();
            }
        }
    }

    /// Reflow all content (history + visible grid) to new dimensions.
    ///
    /// Tracks cursor position through the reflow so it stays on the same
    /// logical content after re-wrapping.
    fn reflow_all(&mut self, new_dims: Dimensions) {
        let new_rows = new_dims.rows as usize;
        let old_cursor_row = self.grid.cursor().position.row as usize;
        let old_cursor_col = self.grid.cursor().position.col as usize;

        // Each logical line is rewrapped as soon as it is complete, and rows
        // older than a screenful go to new history pages as they are made,
        // while the old pages are freed as their rows are read, so the
        // scrollback is never held unpacked all at once.
        let mut out = Rewrap::new(new_dims.cols, new_rows, self.max_history);
        // Images follow the row that holds their top-left corner.
        let mut images = images::ImagesInReflow::new(self);
        let mut old_index = 0;

        // Each cell carries the extra codepoints of its grapheme cluster so
        // reflow keeps them. Shell marks of a logical line, gathered from
        // its rows, go on the first row of its new layout (ADR 0019).
        let mut current_line: Vec<ReflowCell> = Vec::new();
        let mut current_marks = RowMarks::default();

        // A row a live anchor reserved ends the logical line before it and
        // is one of its own (ADR 0020); stale tags are dropped. `open` is
        // whether a soft-wrapped line is being gathered, which a reserved
        // row must finish first even if it has no cells.
        let mut runs = AnchorRuns::new(&self.anchors);
        let mut open = false;
        let finish_open = |out: &mut Rewrap,
                           line: &mut Vec<ReflowCell>,
                           marks: &mut RowMarks,
                           open: &mut bool| {
            if std::mem::take(open) {
                out.finish_line(line, std::mem::take(marks));
            }
        };

        let history_rows = self.history.len();
        std::mem::take(&mut self.history).drain(|row| {
            let row = RowView::history(row);
            let this_row = old_index;
            old_index += 1;
            if runs.keep(row.anchor()) {
                finish_open(&mut out, &mut current_line, &mut current_marks, &mut open);
                images.visit_row(this_row, &mut out, 0);
                let made = out.finish_tagged(&row.to_row());
                if row.anchor().is_first() {
                    runs.placed(row.anchor().tag(), made);
                }
                return;
            }
            // Laid out as text from its cells, so a stale tag goes with it.
            images.visit_row(this_row, &mut out, current_line.len());
            push_reflow_cells(&mut current_line, &row);
            current_marks = current_marks.or(row.marks());
            open = row.is_wrapped();
            if !open {
                out.finish_line(&mut current_line, std::mem::take(&mut current_marks));
            }
        });

        // Grid rows, skipping trailing blank ones. A blank row with shell
        // marks (output that has not started yet) or an anchor tag (a
        // reservation not yet printed) counts as content; a stale tag does
        // not.
        let grid_rows: Vec<Row> = self.grid.rows().map(|row| row.to_row()).collect();
        let kept: Vec<bool> = grid_rows
            .iter()
            .map(|row| runs.keep(row.anchor()))
            .collect();
        let image_on: Vec<bool> = (0..grid_rows.len())
            .map(|idx| images.on_row(history_rows + idx))
            .collect();
        let has_content = |idx: usize| {
            let row = &grid_rows[idx];
            !row.is_blank() || !row.marks().is_empty() || kept[idx] || image_on[idx]
        };
        let last_content_row = (0..grid_rows.len()).rposition(has_content).unwrap_or(0);

        for (idx, row) in grid_rows.iter().enumerate() {
            if idx > last_content_row && !has_content(idx) {
                if idx == old_cursor_row {
                    out.set_cursor(0);
                }
                continue;
            }
            if kept[idx] {
                finish_open(&mut out, &mut current_line, &mut current_marks, &mut open);
                if idx == old_cursor_row {
                    out.set_cursor(old_cursor_col);
                }
                images.visit_row(history_rows + idx, &mut out, 0);
                let made = out.finish_tagged(row);
                if row.anchor().is_first() {
                    runs.placed(row.anchor().tag(), made);
                }
                continue;
            }
            if idx == old_cursor_row {
                out.set_cursor(current_line.len() + old_cursor_col);
            }
            images.visit_row(history_rows + idx, &mut out, current_line.len());
            push_reflow_cells(&mut current_line, &RowView::from(row));
            current_marks = current_marks.or(row.marks());
            open = row.is_wrapped();
            if !open {
                out.finish_line(&mut current_line, std::mem::take(&mut current_marks));
            }
        }
        if !current_line.is_empty() {
            out.finish_line(&mut current_line, current_marks);
        }

        let Rewrap {
            history,
            screen,
            total,
            cursor_at,
            placed,
            ..
        } = out;
        // Rows made but dropped past the history limit.
        let dropped = total - history.len() - screen.len();
        self.evicted += dropped;
        self.history = history;
        let mut rows: Vec<Row> = screen.into();
        while rows.len() < new_rows {
            rows.push(Row::new(new_dims.cols));
        }
        // Rows before the screen, including any dropped past the history
        // limit, which absolute cursor rows count.
        let split_point = total.saturating_sub(new_rows);
        self.grid.set_rows(rows, new_dims);
        self.place_anchors_after_reflow(&runs.made_at, dropped);
        images.restore(self, &placed, dropped);

        if let Some((abs_row, col)) = cursor_at {
            let grid_row = abs_row.saturating_sub(split_point);
            self.grid.cursor_mut().position.row =
                (grid_row as u16).min(new_dims.rows.saturating_sub(1));
            self.grid.cursor_mut().position.col = (col as u16).min(new_dims.cols.saturating_sub(1));
        }
    }

    /// Write a character, saving to history if scrolling occurs.
    #[inline]
    pub fn write_char(&mut self, c: char) {
        self.scroll_offset = 0;
        // A character that fits on the row with no wrap pending writes only
        // there, so it cannot scroll.
        if let Err(width) = self.grid.print_in_row(c) {
            self.write_char_slow(c, width);
        }
    }

    /// [`Buffer::write_char`] for what [`Grid::print_in_row`] left, `width`
    /// columns as it returned them.
    #[inline(never)]
    fn write_char_slow(&mut self, c: char, width: Option<u16>) {
        // A codepoint that joins the previous grapheme cluster takes no new
        // column, so it cannot scroll.
        if width.is_none() {
            self.grid.write_measured_char_slow(c, width);
            return;
        }

        let cursor_col = self.grid.cursor().position.col;
        let cursor_row = self.grid.cursor().position.row;
        let wrap_pending = self.grid.cursor().wrap_pending;
        let auto_wrap = self.grid.auto_wrap();
        let scroll_bottom = self.grid.effective_scroll_region().bottom;
        let max_col = self.grid.dimensions().cols;
        // A codepoint that joins the previous grapheme cluster takes no new
        // column, so it can never wrap or scroll.
        let char_width = width.unwrap_or(0);

        // Determine if this write will cause scrolling.
        // With pending wrap behavior:
        // - If wrap_pending is true, the next non-zero-width char triggers deferred wrap
        // - If wrap_pending is false and we're writing to the last column, that only sets
        //   wrap_pending but doesn't actually scroll (scroll happens on NEXT char)
        // - Wide chars that don't fit (width > 1 and col + width > cols) wrap before writing
        // - If auto_wrap is off, no wrapping ever happens
        // A wide char that can never fit (grid narrower than the char) is
        // written as a placeholder without wrapping, so it never scrolls.
        let wide_wraps =
            char_width > 1 && max_col >= char_width && cursor_col + char_width > max_col;
        let will_scroll = if char_width == 0 || !auto_wrap {
            false
        } else if wrap_pending || wide_wraps {
            cursor_row == scroll_bottom
        } else {
            false
        };

        if will_scroll {
            self.save_scrolled_rows(1);
        }

        // Saving rows leaves the grid as it was, so the width still holds.
        self.grid.write_measured_char_slow(c, width);
    }

    /// Write a contiguous printable ASCII span, saving scrolled rows to history.
    pub fn write_ascii_span(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        if self.grid.insert_mode() || !self.grid.auto_wrap() {
            for &byte in bytes {
                self.write_char(byte as char);
            }
            return;
        }

        self.scroll_offset = 0;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            if self.grid.cursor().wrap_pending
                && self.grid.cursor().position.row == self.grid.effective_scroll_region().bottom
            {
                self.save_scrolled_rows(1);
            }

            let cols = self.grid.dimensions().cols;
            if cols == 0 {
                return;
            }

            let col = self.grid.cursor().position.col;
            let chunk_len = if self.grid.cursor().wrap_pending {
                1
            } else {
                (cols.saturating_sub(col).max(1) as usize).min(remaining.len())
            };
            self.grid.write_ascii_span(&remaining[..chunk_len]);
            remaining = &remaining[chunk_len..];
        }
    }

    /// Before the scroll region scrolls up `n` lines, keep the lines that
    /// leave it. As in xterm, lines are saved whenever the region's top
    /// margin is the top of the screen, so a tool that keeps a status line
    /// below its output (apt, progress bars) still fills the scrollback.
    /// Lines leaving a region that starts lower are discarded.
    ///
    /// Every scroll up goes through here, so this is also where the images
    /// of the screen follow their rows (see `images.rs`).
    fn save_scrolled_rows(&mut self, n: usize) {
        let region = self.grid.effective_scroll_region();
        self.shift_images_for_scroll_up(region, n);
        if region.top == 0 {
            let height = usize::from(region.bottom - region.top) + 1;
            self.save_top_rows(n.min(height));
        }
    }

    /// Scroll the scroll region up, keeping lines that leave the top of the
    /// screen in history.
    pub fn scroll_grid_up(&mut self, n: u16) {
        self.save_scrolled_rows(n as usize);
        self.grid.scroll_up(n);
    }

    /// Scroll the scroll region down `n` rows (SD, reverse index), with
    /// the images on it.
    pub fn scroll_grid_down(&mut self, n: u16) {
        let region = self.grid.effective_scroll_region();
        self.shift_images_down(region.top, region.bottom, n);
        self.grid.scroll_down(n);
    }

    /// Insert `n` blank lines at the cursor (IL), with the images below it.
    pub fn insert_lines(&mut self, n: u16) {
        let region = self.grid.effective_scroll_region();
        let row = self.grid.cursor().position.row;
        if row >= region.top && row <= region.bottom {
            self.shift_images_down(row, region.bottom, n);
        }
        self.grid.insert_lines(n);
    }

    /// Delete `n` lines at the cursor (DL), with the images on them.
    pub fn delete_lines(&mut self, n: u16) {
        let region = self.grid.effective_scroll_region();
        let row = self.grid.cursor().position.row;
        if row >= region.top && row <= region.bottom {
            self.shift_images_up(row, region.bottom, n);
        }
        self.grid.delete_lines(n);
    }

    /// Newline - scroll to history if at bottom.
    pub fn newline(&mut self) {
        self.grid.cursor_mut().wrap_pending = false;
        let cursor_row = self.grid.cursor().position.row;
        let scroll_bottom = self.grid.effective_scroll_region().bottom;

        if cursor_row == scroll_bottom {
            self.scroll_grid_up(1);
        } else {
            self.grid.newline();
        }
    }

    // === Damage tracking ===

    /// Check if any visible content is dirty.
    pub fn has_dirty(&self) -> bool {
        self.grid.has_dirty()
    }

    /// Clear dirty flags after rendering.
    pub fn clear_dirty(&mut self) {
        self.grid.clear_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_dropped_once_its_last_line_leaves_scrollback() {
        use crate::grid::{ImageId, TerminalImage};
        let mut buf = Buffer::with_scrollback(Dimensions::new(10, 3), 2);
        buf.images_mut().add(TerminalImage {
            id: ImageId(0),
            width: 8,
            height: 32,
            data: vec![0; 8 * 32 * 4],
            start_col: 0,
            line: 0,
            cell_width: 1,
            cell_height: 2,
        });
        buf.grid_mut().cursor_mut().position.row = 2;
        // Lines 0 and 1 leave the screen, into scrollback that holds two.
        for _ in 0..2 {
            buf.newline();
        }
        assert_eq!(buf.images().len(), 1);
        // Line 2 pushes line 0 out of scrollback; the image still has line 1.
        buf.newline();
        assert_eq!(buf.images().len(), 1);
        // Line 3 pushes out line 1 too.
        buf.newline();
        assert!(buf.images().is_empty());
    }

    #[test]
    fn new_buffer_empty_history() {
        let buf = Buffer::new(Dimensions::new(80, 24));
        assert_eq!(buf.history_len(), 0);
        assert_eq!(buf.scroll_offset(), 0);
        assert!(!buf.is_scrolled());
    }

    #[test]
    fn scroll_up_down() {
        let mut buf = Buffer::new(Dimensions::new(80, 24));

        // Add some history
        for i in 0..50 {
            let mut row = Row::new(80);
            row[0].set_char((b'A' + (i % 26)) as char);
            buf.push_to_history(vec![row]);
        }

        assert_eq!(buf.history_len(), 50);

        buf.scroll_up(10);
        assert_eq!(buf.scroll_offset(), 10);
        assert!(buf.is_scrolled());

        buf.scroll_down(5);
        assert_eq!(buf.scroll_offset(), 5);

        buf.scroll_to_bottom();
        assert_eq!(buf.scroll_offset(), 0);

        buf.scroll_to_top();
        assert_eq!(buf.scroll_offset(), 50);
    }

    #[test]
    fn history_limit() {
        let mut buf = Buffer::with_scrollback(Dimensions::new(10, 5), 100);

        // Add more than max history
        for _ in 0..150 {
            buf.push_to_history(vec![Row::new(10)]);
        }

        assert_eq!(buf.history_len(), 100);
    }

    #[test]
    fn lowering_the_history_limit_drops_the_oldest_lines() {
        let mut buf = Buffer::with_scrollback(Dimensions::new(10, 2), 100);
        for i in 0..50 {
            let mut row = Row::new(10);
            row[0].set_char(char::from(b'0' + (i % 10) as u8));
            buf.push_to_history(vec![row]);
        }
        buf.scroll_up(40);

        assert_eq!(buf.set_max_history(20), 30);

        assert_eq!(buf.history_len(), 20);
        assert_eq!(buf.max_history(), 20);
        // Line 30 is now the oldest.
        assert_eq!(buf.row_at(0).unwrap().cell(0).unwrap().char(), '0');
        assert_eq!(buf.row_at(1).unwrap().cell(0).unwrap().char(), '1');
        assert_eq!(buf.scroll_offset(), 20);
        assert!(buf.check_integrity().is_ok());

        // Raising the limit keeps what is there and lets history grow.
        assert_eq!(buf.set_max_history(30), 0);
        for _ in 0..15 {
            buf.push_to_history(vec![Row::new(10)]);
        }
        assert_eq!(buf.history_len(), 30);
    }

    #[test]
    fn write_ascii_span_saves_to_history_on_scroll() {
        let mut buf = Buffer::new(Dimensions::new(5, 2));
        buf.grid_mut()
            .row_mut(0)
            .unwrap()
            .update(0, |cell| cell.set_char('A'));
        buf.grid_mut()
            .row_mut(1)
            .unwrap()
            .update(0, |cell| cell.set_char('B'));
        buf.grid_mut().set_cursor_position(2, 1);

        buf.write_ascii_span(b"abcdef");

        assert_eq!(buf.history_len(), 1);
        assert_eq!(buf.row_at(0).unwrap().cell(0).unwrap().char(), 'A');
        assert_eq!(buf.grid().row(0).unwrap().text(), "abcde");
        assert_eq!(buf.grid().row(1).unwrap().text_trimmed(), "f");
    }

    #[test]
    fn scrolled_back_rows_read_exactly_as_written() {
        // What the renderer, copy, and search see through visible_row and
        // row_at for rows that went through compact scrollback.
        let dims = Dimensions::new(20, 4);
        let mut buf = Buffer::new(dims);
        let mut expected: Vec<Row> = Vec::new();
        for i in 0..12u8 {
            {
                let grid = buf.grid_mut();
                grid.attrs_mut().set_fg(super::super::Color::Indexed(i));
                grid.attrs_mut().set_bg(if i % 2 == 0 {
                    super::super::Color::Default
                } else {
                    super::super::Color::rgb(i, 2, 3)
                });
                for c in format!("{i:02} é中x").chars() {
                    grid.write_char(c);
                }
            }
            let row = buf.grid().cursor().position.row;
            buf.grid_mut().row_mut(row).unwrap().set_wrapped(i % 3 == 0);
            expected.push(
                buf.grid()
                    .row(buf.grid().cursor().position.row)
                    .unwrap()
                    .to_row(),
            );
            buf.grid_mut().carriage_return();
            buf.newline();
        }
        // 12 lines written, 4 on screen: the first 9 are in history.
        assert_eq!(buf.history_len(), 9);
        for (index, want) in expected.iter().take(9).enumerate() {
            let got = buf.row_at(index).unwrap();
            assert!(got.eq_row(want), "row_at({index})");
            assert_eq!(got.is_wrapped(), want.is_wrapped(), "row_at({index}) wrap");
        }
        buf.scroll_up(9);
        for visual in 0..4u16 {
            let got = buf.visible_row(visual).unwrap();
            assert!(
                got.eq_row(&expected[visual as usize]),
                "visible_row({visual}) after scrolling back"
            );
        }
    }

    #[test]
    fn a_full_scrollback_of_typical_output_stays_small() {
        // 10,000 lines of 84 characters with some color, as `cat` of a log
        // would leave them, on a 300-column screen. As full rows this was
        // 300 cells x 20 bytes = 6 KB a line, about 57 MiB.
        let mut buf = Buffer::new(Dimensions::new(300, 75));
        let line =
            b"the quick brown fox jumps over the lazy dog 0123456789 ABCDEF green 0123456789 end";
        for i in 0..(DEFAULT_SCROLLBACK + 75) {
            if i % 3 == 0 {
                buf.grid_mut()
                    .attrs_mut()
                    .set_fg(super::super::Color::Named(2));
            } else {
                buf.grid_mut()
                    .attrs_mut()
                    .set_fg(super::super::Color::Default);
            }
            buf.write_ascii_span(line);
            buf.grid_mut().carriage_return();
            buf.newline();
        }
        assert_eq!(buf.history_len(), DEFAULT_SCROLLBACK);
        // Measured 2026-11-06: 2.42 MB, about 240 bytes a line, as
        // `CompactRow`s; 2026-11-10 in pages: see the memory bench.
        let bytes = buf.history.heap_bytes();
        let full_rows = DEFAULT_SCROLLBACK * 300 * std::mem::size_of::<Cell>();
        assert!(
            bytes * 20 < full_rows,
            "scrollback holds {bytes} bytes for {DEFAULT_SCROLLBACK} lines; full rows were {full_rows}"
        );
    }

    fn one_row_anchor(surface_id: u32) -> Anchor {
        Anchor {
            session_id: 1,
            surface_id,
            rows: 1,
        }
    }

    fn three_row_anchor(surface_id: u32) -> Anchor {
        Anchor {
            rows: 3,
            ..one_row_anchor(surface_id)
        }
    }

    /// `lines` lines of text, scrolled through `buf`.
    fn fill(buf: &mut Buffer, lines: usize) {
        for _ in 0..lines {
            buf.write_ascii_span(b"some output");
            buf.grid_mut().carriage_return();
            buf.newline();
        }
    }

    #[test]
    fn anchor_bookkeeping_costs_a_screenful_not_the_scrollback() {
        // Each anchor lands on the previous one's first row, so every
        // sweep finds one lost: with a full scrollback, finding and
        // stripping it must look at the grid and the anchor's rows only.
        let mut buf = Buffer::with_scrollback(Dimensions::new(80, 24), 10_000);
        fill(&mut buf, 10_024);
        buf.anchor_visits.set(0);
        let mut lost = 0;
        for surface in 0..1000 {
            lost += buf.sweep_anchors().len();
            buf.place_anchor(one_row_anchor(surface)).unwrap();
        }
        lost += buf.sweep_anchors().len();
        assert_eq!(lost, 999);
        let per_anchor = buf.anchor_visits.get() / 1000;
        assert!(
            per_anchor <= 2 * 24,
            "{per_anchor} rows looked at per anchor"
        );
    }

    #[test]
    fn settled_anchors_cost_one_row_per_sweep() {
        let mut buf = Buffer::with_scrollback(Dimensions::new(80, 24), 10_000);
        for surface in 0..100 {
            buf.place_anchor(three_row_anchor(surface)).unwrap();
            fill(&mut buf, 50);
        }
        // The first sweep checks every reservation's rows...
        assert!(buf.sweep_anchors().is_empty());
        buf.anchor_visits.set(0);
        // ...after which only the first rows are looked at.
        assert!(buf.sweep_anchors().is_empty());
        assert_eq!(buf.anchor_visits.get(), 100);
    }

    #[test]
    fn reflow_leaves_exact_hints_for_the_next_sweep() {
        let mut buf = Buffer::with_scrollback(Dimensions::new(80, 24), 10_000);
        for surface in 0..100 {
            buf.place_anchor(one_row_anchor(surface)).unwrap();
            buf.write_ascii_span(b"a card");
            buf.grid_mut().carriage_return();
            buf.newline();
            // Doubling on the narrower screen keeps them all in scrollback.
            for _ in 0..40 {
                buf.write_ascii_span(&[b'x'; 70]);
                buf.grid_mut().carriage_return();
                buf.newline();
            }
        }
        assert!(buf.sweep_anchors().is_empty());
        buf.resize(Dimensions::new(40, 24));
        buf.anchor_visits.set(0);
        assert!(buf.sweep_anchors().is_empty());
        // One hint check each, no searching.
        assert_eq!(buf.anchor_visits.get(), 100);
        buf.check_integrity().unwrap();
    }

    #[test]
    fn visible_row_with_scroll() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write to grid
        buf.grid_mut().write_char('X');

        // Add history
        for i in 0..10 {
            let mut row = Row::new(10);
            row[0].set_char((b'0' + i) as char);
            buf.push_to_history(vec![row]);
        }

        // Not scrolled - should see grid
        assert_eq!(buf.visible_row(0).unwrap().cell(0).unwrap().char(), 'X');

        // Scroll up 5 lines
        buf.scroll_up(5);

        // Should now see history row at index (10-5) = 5
        assert_eq!(buf.visible_row(0).unwrap().cell(0).unwrap().char(), '5');
    }

    #[test]
    fn scroll_grid_up_saves_history() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write on first row
        buf.grid_mut().write_char('A');

        // Scroll up
        buf.scroll_grid_up(1);

        // First row should be in history now
        assert_eq!(buf.history_len(), 1);
        assert_eq!(buf.row_at(0).unwrap().cell(0).unwrap().char(), 'A');

        // Grid first row should be empty
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), ' ');
    }

    #[test]
    fn resize_reflows_content_narrower() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Write a long line that will wrap when narrowed
        let text = "Hello World!!!!!";
        for c in text.chars() {
            buf.grid_mut().write_char(c);
        }

        // Verify initial state - should fit on one row
        assert_eq!(buf.grid().row(0).unwrap().text_trimmed(), text);

        // Narrow to 10 columns
        buf.resize(Dimensions::new(10, 5));

        // Content should now span two rows
        assert_eq!(buf.grid().row(0).unwrap().text_trimmed(), "Hello Worl");
        assert_eq!(buf.grid().row(1).unwrap().text_trimmed(), "d!!!!!");
        assert!(buf.grid().row(0).unwrap().is_wrapped());
    }

    #[test]
    fn resize_reflows_content_wider() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        // Write a line that wraps at 10 columns
        let text = "Hello World!!!!!";
        for c in text.chars() {
            buf.grid_mut().write_char(c);
        }

        // Should be wrapped across two rows
        assert!(buf.grid().row(0).unwrap().is_wrapped());

        // Widen to 20 columns
        buf.resize(Dimensions::new(20, 5));

        // Content should now fit on one row
        assert_eq!(buf.grid().row(0).unwrap().text_trimmed(), text);
        assert!(!buf.grid().row(0).unwrap().is_wrapped());
    }

    #[test]
    fn reflow_wide_char_at_boundary_wraps_to_next_row() {
        // 4-column grid: "A" + wide '中' (cols 1-2) + "B" (col 3)
        let mut buf = Buffer::new(Dimensions::new(4, 3));
        buf.write_char('A');
        buf.write_char('中');
        buf.write_char('B');

        // After placing 'A' at col 0, '中' needs 2 cols but only 1 remains.
        buf.resize(Dimensions::new(2, 3));

        let grid = buf.grid();
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'A');
        assert!(!grid.cell(1, 0).unwrap().is_wide());
        assert!(grid.cell(0, 1).unwrap().is_wide());
        assert!(grid.cell(1, 1).unwrap().is_wide_spacer());
        assert_eq!(grid.cell(0, 2).unwrap().char(), 'B');
    }

    #[test]
    fn reflow_wide_char_to_one_column_terminates() {
        let mut buf = Buffer::new(Dimensions::new(10, 3));
        buf.write_char('漢');
        buf.write_char('字');
        buf.write_char('x');

        buf.resize(Dimensions::new(1, 3));

        assert_eq!(buf.dimensions().cols, 1);
        // Wide chars became single-cell placeholders; the ASCII char survived.
        let text: String = (0..buf.total_lines())
            .filter_map(|i| buf.row_at(i))
            .map(|row| row.text())
            .collect();
        assert!(text.contains('x'));
    }

    #[test]
    fn reflow_to_zero_columns_terminates() {
        let mut buf = Buffer::new(Dimensions::new(10, 3));
        buf.write_char('a');
        buf.write_char('b');

        buf.resize(Dimensions::new(0, 3));

        assert_eq!(buf.dimensions().cols, 0);
        assert_eq!(buf.grid().rows().count(), 3);
    }

    #[test]
    fn resize_height_shrink_moves_to_history() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Directly set cells in each row using a single borrow
        {
            let grid = buf.grid_mut();
            grid.row_mut(0)
                .unwrap()
                .update(0, |cell| cell.set_char('A'));
            grid.row_mut(1)
                .unwrap()
                .update(0, |cell| cell.set_char('B'));
            grid.row_mut(2)
                .unwrap()
                .update(0, |cell| cell.set_char('C'));
            grid.row_mut(3)
                .unwrap()
                .update(0, |cell| cell.set_char('D'));
            grid.row_mut(4)
                .unwrap()
                .update(0, |cell| cell.set_char('E'));
        }

        // Verify initial state - cell() takes (col, row)
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'A');
        assert_eq!(buf.grid().cell(0, 4).unwrap().char(), 'E');

        // Shrink height from 5 to 3 (width stays same)
        buf.resize(Dimensions::new(20, 3));

        // Top 2 rows (A, B) should move to history
        assert_eq!(buf.history_len(), 2);
        assert_eq!(buf.row_at(0).unwrap().cell(0).unwrap().char(), 'A');
        assert_eq!(buf.row_at(1).unwrap().cell(0).unwrap().char(), 'B');

        // Grid should have C, D, E - cell() takes (col, row)
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'C');
        assert_eq!(buf.grid().cell(0, 1).unwrap().char(), 'D');
        assert_eq!(buf.grid().cell(0, 2).unwrap().char(), 'E');
    }

    fn write_lines(buf: &mut Buffer, lines: &[&str]) {
        for (i, line) in lines.iter().enumerate() {
            buf.write_ascii_span(line.as_bytes());
            if i + 1 < lines.len() {
                buf.grid_mut().carriage_return();
                buf.newline();
            }
        }
    }

    fn screen(buf: &Buffer) -> Vec<String> {
        (0..buf.dimensions().rows)
            .map(|row| buf.grid().row(row).unwrap().text_trimmed())
            .collect()
    }

    #[test]
    fn shrinking_drops_blank_rows_below_the_cursor_first() {
        // Two lines on a five-row screen, then the tab bar appears and the
        // screen loses two rows: nothing should scroll away.
        let mut buf = Buffer::new(Dimensions::new(10, 5));
        write_lines(&mut buf, &["one", "two"]);
        buf.resize(Dimensions::new(10, 3));
        assert_eq!(buf.history_len(), 0);
        assert_eq!(screen(&buf), ["one", "two", ""]);
        assert_eq!(buf.grid().cursor().position.row, 1);
    }

    #[test]
    fn shrinking_with_the_cursor_at_the_bottom_moves_top_rows_to_history() {
        let mut buf = Buffer::new(Dimensions::new(10, 4));
        write_lines(&mut buf, &["a", "b", "c", "d"]);
        buf.resize(Dimensions::new(10, 2));
        assert_eq!(buf.history_len(), 2);
        assert_eq!(screen(&buf), ["c", "d"]);
    }

    #[test]
    fn shrinking_keeps_content_below_the_cursor() {
        // Text below the cursor (a full-screen app moved it up) is not
        // dropped; the screen scrolls instead.
        let mut buf = Buffer::new(Dimensions::new(10, 4));
        write_lines(&mut buf, &["a", "b", "c", "d"]);
        buf.grid_mut().set_cursor_position(0, 1);
        buf.resize(Dimensions::new(10, 2));
        assert_eq!(buf.history_len(), 2);
        assert_eq!(screen(&buf), ["c", "d"]);
    }

    #[test]
    fn resize_height_grow_pulls_from_history() {
        let mut buf = Buffer::new(Dimensions::new(20, 3));

        // Add history
        let mut row_a = Row::new(20);
        row_a[0].set_char('A');
        let mut row_b = Row::new(20);
        row_b[0].set_char('B');
        buf.push_to_history(vec![row_a, row_b]);

        // Directly set cells in grid
        {
            let grid = buf.grid_mut();
            grid.row_mut(0)
                .unwrap()
                .update(0, |cell| cell.set_char('C'));
            grid.row_mut(1)
                .unwrap()
                .update(0, |cell| cell.set_char('D'));
        }

        // Verify state: 2 in history (A, B), grid has C, D, empty - cell() takes (col, row)
        assert_eq!(buf.history_len(), 2);
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'C'); // col 0, row 0

        // Grow height from 3 to 5 (width stays same)
        buf.resize(Dimensions::new(20, 5));

        // History should be pulled into grid
        // History rows A, B should now be at top of grid - cell() takes (col, row)
        assert_eq!(buf.history_len(), 0);
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), 'A'); // col 0, row 0
        assert_eq!(buf.grid().cell(0, 1).unwrap().char(), 'B'); // col 0, row 1
        assert_eq!(buf.grid().cell(0, 2).unwrap().char(), 'C'); // col 0, row 2
        assert_eq!(buf.grid().cell(0, 3).unwrap().char(), 'D'); // col 0, row 3
    }

    #[test]
    fn write_char_saves_to_history_on_scroll() {
        let mut buf = Buffer::new(Dimensions::new(10, 5));

        for i in 0..5u16 {
            buf.grid_mut()
                .row_mut(i)
                .unwrap()
                .update(0, |cell| cell.set_char((b'0' + i as u8) as char));
        }

        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), '0');
        assert_eq!(buf.grid().cell(0, 4).unwrap().char(), '4');
        assert_eq!(buf.history_len(), 0);

        // Fill last row completely, then write one more char to trigger scroll
        // set_cursor_position is 1-based, so row 5 = 0-based row 4
        buf.grid_mut().set_cursor_position(5, 1);
        for _ in 0..10 {
            buf.write_char('X');
        }
        // Now cursor is at row 4, col 10 (past end) - next char triggers scroll
        buf.write_char('Y');

        assert_eq!(buf.history_len(), 1);
        assert_eq!(buf.row_at(0).unwrap().cell(0).unwrap().char(), '0');
        assert_eq!(buf.grid().cell(0, 0).unwrap().char(), '1');
    }

    #[test]
    fn write_char_fills_screen_and_scrolls() {
        // Simulate filling a small screen and scrolling multiple times
        let mut buf = Buffer::new(Dimensions::new(5, 3)); // 5 cols, 3 rows

        // Write enough to fill screen and scroll several times
        // Each row holds 5 chars, we have 3 rows = 15 chars before scroll
        // Then each additional 5 chars scrolls once
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 26 chars

        for c in text.chars() {
            buf.write_char(c);
        }

        // 26 chars / 5 cols = 5.2 rows needed
        // 3 visible rows means 2-3 rows should be in history
        // Let's verify history has content
        assert!(
            buf.history_len() >= 2,
            "Expected at least 2 history rows, got {}",
            buf.history_len()
        );

        // First history row should start with 'A'
        assert_eq!(buf.row_at(0).unwrap().cell(0).unwrap().char(), 'A');
    }

    #[test]
    fn reflow_preserves_cursor_on_narrow() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Write "Hello World" on row 0
        for c in "Hello World".chars() {
            buf.grid_mut().write_char(c);
        }
        // Cursor should be at row 0, col 11 (after 'd')
        assert_eq!(buf.grid().cursor().position.row, 0);
        assert_eq!(buf.grid().cursor().position.col, 11);

        // Narrow to 6 columns — "Hello " wraps, cursor should track to "World" part
        buf.resize(Dimensions::new(6, 5));

        // "Hello " on row 0 (wrapped), "World" on row 1
        // Cursor was at col 11 in the logical line, which maps to row 1 col 5
        assert_eq!(buf.grid().cursor().position.row, 1);
        assert_eq!(buf.grid().cursor().position.col, 5);
    }

    #[test]
    fn reflow_preserves_cursor_on_widen() {
        let mut buf = Buffer::new(Dimensions::new(6, 5));

        // Write text that wraps at 6 columns
        for c in "Hello World".chars() {
            buf.grid_mut().write_char(c);
        }
        // Cursor on row 1 (wrapped), col 5
        assert_eq!(buf.grid().cursor().position.row, 1);
        assert_eq!(buf.grid().cursor().position.col, 5);

        // Widen to 20 — everything fits on one row
        buf.resize(Dimensions::new(20, 5));

        // Cursor should be back on row 0, col 11
        assert_eq!(buf.grid().cursor().position.row, 0);
        assert_eq!(buf.grid().cursor().position.col, 11);
    }

    #[test]
    fn reflow_cursor_on_blank_trailing_row() {
        let mut buf = Buffer::new(Dimensions::new(20, 5));

        // Write text on row 0 only, leave cursor at bottom (row 4)
        for c in "Hello".chars() {
            buf.grid_mut().write_char(c);
        }
        // Move cursor to row 4 (blank trailing row)
        buf.grid_mut().set_cursor_position(5, 1); // 1-based: row 5, col 1

        assert_eq!(buf.grid().cursor().position.row, 4);

        // Narrow — reflow should not lose cursor
        buf.resize(Dimensions::new(10, 5));

        // Cursor should end up at a valid position (not row 0 col 0 due to lost tracking)
        let cursor = buf.grid().cursor().position;
        assert!(
            cursor.row < 5,
            "cursor row {} should be in bounds",
            cursor.row
        );
    }
}
