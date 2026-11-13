//! Keeping a screen's inline images on their text lines.
//!
//! An image belongs to the line its top row sits on, numbered like anchors
//! are: `evicted + absolute index`. Scrolling the screen into scrollback
//! leaves that number alone, so the image scrolls with its text. The
//! operations that move rows on the screen without saving them, and reflow,
//! renumber or remove the images; this module holds those rules.

use super::{Buffer, Rewrap, ScrollRegion};
use crate::grid::TerminalImage;

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
