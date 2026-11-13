//! Keeping a screen's inline images on their text lines.
//!
//! An image belongs to the line its top row sits on, numbered like anchors
//! are: `evicted + absolute index`. Scrolling the screen into scrollback
//! leaves that number alone, so the image scrolls with its text. The
//! operations that move rows on the screen without saving them, and reflow,
//! renumber or remove the images; this module holds those rules.

use std::ops::{Range, RangeInclusive};

use super::{Buffer, Rewrap, ScrollRegion};
use crate::grid::{ImageId, TerminalImage};

impl Buffer {
    /// The line number of screen row `row`.
    pub fn line_of_screen_row(&self, row: u16) -> usize {
        self.screen_top_line() + usize::from(row)
    }

    /// The line number of the topmost screen row.
    pub fn screen_top_line(&self) -> usize {
        self.evicted + self.history.len()
    }

    /// The screen row of `line`, negative above the screen (in scrollback).
    pub fn screen_row_of_line(&self, line: usize) -> isize {
        line as isize - self.screen_top_line() as isize
    }

    /// Cells `cols` of the screen rows `rows` are being erased or written
    /// over: the images under them lose those cells. As in foot
    /// (`sixel_overwrite_by_row`, `sixel_overwrite_by_rectangle`), an image
    /// that is only partly covered is split into the cell rectangles that
    /// remain, each cropped from the picture and given a fresh id, rather
    /// than deleted whole.
    pub fn overwrite_images(&mut self, rows: RangeInclusive<u16>, cols: Range<u16>) {
        if self.images.is_empty() || cols.is_empty() {
            return;
        }
        let top = self.screen_top_line();
        let first = top + usize::from(*rows.start());
        let last = top + usize::from(*rows.end());
        let under: Vec<ImageId> = self
            .images
            .images()
            .filter(|image| {
                image.line <= last
                    && image.line + usize::from(image.cell_height) > first
                    && image.start_col < cols.end
                    && u32::from(image.start_col) + u32::from(image.cell_width)
                        > u32::from(cols.start)
            })
            .map(|image| image.id)
            .collect();
        for id in under {
            let Some(image) = self.images.remove(id) else {
                continue;
            };
            let (width, height) = (image.cell_width, image.cell_height);
            let row_cell = |line: usize| line.saturating_sub(image.line).min(usize::from(height));
            // The covered cells of the image, from its own top-left cell.
            let covered_rows = row_cell(first) as u16..row_cell(last + 1) as u16;
            let covered_cols = cols.start.saturating_sub(image.start_col).min(width)
                ..cols.end.saturating_sub(image.start_col).min(width);
            let remaining = [
                (0..covered_rows.start, 0..width),
                (covered_rows.end..height, 0..width),
                (covered_rows.clone(), 0..covered_cols.start),
                (covered_rows.clone(), covered_cols.end..width),
            ];
            for (piece_rows, piece_cols) in remaining {
                if piece_rows.is_empty() || piece_cols.is_empty() {
                    continue;
                }
                if let Some(piece) = image.crop(self.image_ids.allocate(), piece_rows, piece_cols) {
                    self.images.add(piece);
                }
            }
        }
        self.prune_evicted_images();
    }

    /// `cells` columns were just written ending at the cursor (counting a
    /// pending wrap as past the last column): the images under them lose
    /// those cells, as foot's `sixel_overwrite_at_cursor` does.
    #[cold]
    pub(super) fn overwrite_images_under_print(&mut self, cells: Option<u16>) {
        let cursor = self.grid.cursor();
        let (row, end) = (
            cursor.position.row,
            cursor.position.col + u16::from(cursor.wrap_pending),
        );
        let Some(last) = end.checked_sub(1) else {
            return;
        };
        let cells = cells.unwrap_or_else(|| match self.grid.cell(last, row) {
            Some(cell) if cell.is_wide_spacer() => 2,
            _ => 1,
        });
        self.overwrite_images(row..=row, end.saturating_sub(cells)..end);
    }

    /// Remove the images whose last line has left scrollback.
    pub(super) fn prune_evicted_images(&mut self) {
        if self.images.is_empty() {
            return;
        }
        let evicted = self.evicted;
        self.images
            .retain_mut(|image| image.line + usize::from(image.cell_height.max(1)) > evicted);
    }

    /// The scroll region is about to scroll up by `n` rows (see
    /// [`Buffer::save_scrolled_rows`]). With the region at the top of the
    /// screen and scrollback on, the rows that leave go to scrollback, so
    /// images in the region keep their line, and images below it, whose
    /// screen rows do not move, are one `saved` lines further down.
    /// Otherwise the rows that leave are discarded, and the images in the
    /// region move up with their text, or are removed when pushed out.
    pub(super) fn shift_images_for_scroll_up(&mut self, region: ScrollRegion, n: usize) {
        if self.images.is_empty() {
            return;
        }
        let height = usize::from(region.bottom - region.top) + 1;
        let top = self.screen_top_line();
        let saved = if region.top == 0 && self.max_history > 0 {
            n.min(height)
        } else {
            0
        };
        let first = top + usize::from(region.top);
        let last = top + usize::from(region.bottom);
        self.images.retain_mut(|image| {
            if image.line < first {
                return true;
            }
            if image.line > last {
                image.line += saved;
                return true;
            }
            if saved > 0 {
                return true;
            }
            match image.line.checked_sub(n) {
                Some(line) if line >= first => {
                    image.line = line;
                    true
                }
                _ => false,
            }
        });
    }

    /// Rows `from..=to` (screen rows) are about to move down by `n`: the
    /// images on them move with them, and those pushed past `to` are
    /// removed.
    pub(super) fn shift_images_down(&mut self, from: u16, to: u16, n: u16) {
        if self.images.is_empty() {
            return;
        }
        let top = self.screen_top_line();
        let first = top + usize::from(from);
        let last = top + usize::from(to);
        self.images.retain_mut(|image| {
            if image.line < first || image.line > last {
                return true;
            }
            image.line += usize::from(n);
            image.line <= last
        });
    }

    /// Rows `from..=to` (screen rows) are about to move up by `n`, the top
    /// `n` of them deleted: the images on them move with them, and those on
    /// the deleted rows are removed.
    pub(super) fn shift_images_up(&mut self, from: u16, to: u16, n: u16) {
        if self.images.is_empty() {
            return;
        }
        let top = self.screen_top_line();
        let first = top + usize::from(from);
        let last = top + usize::from(to);
        self.images.retain_mut(|image| {
            if image.line < first || image.line > last {
                return true;
            }
            match image.line.checked_sub(usize::from(n)) {
                Some(line) if line >= first => {
                    image.line = line;
                    true
                }
                _ => false,
            }
        });
    }
}

/// The images of a screen while [`Buffer::reflow_all`] rewraps its rows:
/// they are followed to the row that holds their top-left corner.
pub(super) struct ImagesInReflow {
    /// Image tops already evicted while their lower pixels remain retained.
    partial: Vec<TerminalImage>,
    /// Sorted by line; the slot of an image is its index here.
    images: Vec<TerminalImage>,
    /// The first image not yet reached.
    next: usize,
    /// The buffer's `evicted` when the reflow began.
    evicted: usize,
}

impl ImagesInReflow {
    pub(super) fn new(buffer: &mut Buffer) -> Self {
        let evicted = buffer.evicted;
        let (mut partial, mut images): (Vec<_>, Vec<_>) = buffer
            .images
            .take_all()
            .into_iter()
            .partition(|image| image.line < evicted);
        partial.retain(|image| {
            image
                .line
                .checked_add(usize::from(image.cell_height))
                .is_some_and(|bottom| bottom > evicted)
        });
        images.sort_by_key(|image| image.line);
        Self {
            partial,
            images,
            next: 0,
            evicted,
        }
    }

    /// Whether an image's top row is old row `old_index`, so the row counts
    /// as content even if blank.
    pub(super) fn on_row(&self, old_index: usize) -> bool {
        let line = self.evicted + old_index;
        self.images
            .binary_search_by_key(&line, |image| image.line)
            .is_ok()
    }

    /// Old row `old_index` is about to join the logical line being gathered
    /// by `out`, `offset` cells in: follow the images on it.
    pub(super) fn visit_row(&mut self, old_index: usize, out: &mut Rewrap, offset: usize) {
        let line = self.evicted + old_index;
        while let Some(image) = self.images.get(self.next).filter(|i| i.line <= line) {
            if image.line == line {
                out.watch_image(self.next, offset + usize::from(image.start_col));
            }
            self.next += 1;
        }
    }

    /// Put the images back where the reflow laid their rows. `dropped` rows
    /// fell past the history limit.
    pub(super) fn restore(self, buffer: &mut Buffer, placed: &[(usize, usize)], dropped: usize) {
        let max_col = buffer.grid.dimensions().cols.saturating_sub(1);
        let mut moved: Vec<(usize, usize)> = placed
            .iter()
            .filter(|&&(_, made)| made >= dropped)
            .copied()
            .collect();
        moved.sort_unstable();
        let evicted = buffer.evicted;
        for mut image in self.partial {
            // Its lost top precedes row zero of the reflow. Newly dropped
            // rows advance the origin and crop it further, never move it down.
            if image
                .line
                .checked_add(usize::from(image.cell_height))
                .is_some_and(|bottom| bottom > evicted)
            {
                image.start_col = image.start_col.min(max_col);
                buffer.images.add(image);
            }
        }
        let mut images: Vec<Option<TerminalImage>> = self.images.into_iter().map(Some).collect();
        for (slot, made) in moved {
            let Some(mut image) = images.get_mut(slot).and_then(Option::take) else {
                continue;
            };
            image.line = evicted + made - dropped;
            image.start_col = image.start_col.min(max_col);
            buffer.images.add(image);
        }
    }
}
