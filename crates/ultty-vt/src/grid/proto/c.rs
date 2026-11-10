//! Variant C: one screen page with slot rotation, and append-only history
//! pages (ADR 0022 §2–§4).

use std::collections::VecDeque;

use super::{
    base, collect_table, collect_threshold, lookup, mark_live, remap_key, row_erase, style_key,
    unpack, used_len, Head, Storage, Style, StyleTable, BLANK, GRAPHEME, INLINE, STYLE_MASK,
};
use crate::grid::cell::Cell;
use crate::grid::row::{AnchorTag, RowMarks};

/// Rows per history page.
const PAGE_ROWS: usize = 256;

/// The screen: every row's cells in one allocation, shown through `slots`.
struct ScreenPage {
    cols: usize,
    cells: Vec<u64>,
    /// Screen row `r` shows slot `slots[r]`.
    slots: Vec<u16>,
    heads: Vec<Head>,
    styles: StyleTable,
    /// Stands in for the `GraphemeStore` (cell, start, len) index and its
    /// characters, so the page is the size it would be.
    _graphemes: (Vec<(u32, u32, u8)>, Vec<char>),
}

impl ScreenPage {
    fn slot_range(&self, row: u16) -> (usize, std::ops::Range<usize>) {
        let slot = usize::from(self.slots[usize::from(row)]);
        (slot, slot * self.cols..(slot + 1) * self.cols)
    }
}

/// The two forms a history row is stored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `len` bytes of ASCII in the page style `style`.
    Uniform,
    /// `len` packed cells (8 bytes each, little-endian) with page style ids.
    Cells,
}

#[derive(Debug, Clone, Copy)]
struct HistHead {
    offset: u32,
    len: u16,
    kind: Kind,
    width: u16,
    wrapped: bool,
    _marks: RowMarks,
    _anchor: AnchorTag,
    /// The style key of a uniform row, in the page's ids.
    style: u64,
}

/// Up to [`PAGE_ROWS`] history rows, appended in place.
///
/// The page's styles are a plain list (id 0 the default): the remap from
/// screen ids already makes each screen style appear once per page, so a
/// hash index would only cost a second lookup per new style. (A style can
/// appear twice after a screen collection; that wastes 20 bytes.)
struct HistoryPage {
    heads: Vec<HistHead>,
    bytes: Vec<u8>,
    styles: Vec<Style>,
}

impl HistoryPage {
    fn new() -> Self {
        Self {
            heads: Vec::with_capacity(PAGE_ROWS),
            bytes: Vec::new(),
            styles: vec![Style::default()],
        }
    }

    fn clear(&mut self) {
        self.heads.clear();
        self.bytes.clear();
        self.styles.truncate(1);
    }

    fn heap_bytes(&self) -> usize {
        self.heads.capacity() * std::mem::size_of::<HistHead>()
            + self.bytes.capacity()
            + self.styles.capacity() * std::mem::size_of::<Style>()
    }
}

/// Scrollback as a list of pages with per-row eviction.
struct History {
    pages: VecDeque<HistoryPage>,
    /// Rows of `pages[0]` already evicted.
    front: usize,
    free: Vec<HistoryPage>,
    len: usize,
    max_history: usize,
    evicted: usize,
    /// Screen style id to the newest page's id (`u32::MAX` unknown), valid
    /// for that page and for `remap_generation` of the screen table. One
    /// map for the page being appended to, rather than one per page.
    remap: Vec<u32>,
    remap_generation: u32,
}

const UNMAPPED: u32 = u32::MAX;

impl History {
    fn new(max_history: usize) -> Self {
        Self {
            pages: VecDeque::new(),
            front: 0,
            free: Vec::new(),
            len: 0,
            max_history,
            evicted: 0,
            remap: Vec::new(),
            remap_generation: 0,
        }
    }

    /// Drop the oldest row; a page whose rows are all gone is recycled.
    fn evict(&mut self) {
        self.front += 1;
        self.len -= 1;
        self.evicted += 1;
        if self
            .pages
            .front()
            .is_some_and(|page| self.front >= page.heads.len())
        {
            if let Some(mut page) = self.pages.pop_front() {
                page.clear();
                self.free.push(page);
            }
            self.front = 0;
            if self.pages.is_empty() {
                self.remap.clear();
            }
        }
    }

    /// Make sure the newest page has room, starting a new one when it is
    /// full, and that the remap belongs to it and to this generation.
    fn ensure_page(&mut self, generation: u32) {
        if self
            .pages
            .back()
            .is_none_or(|page| page.heads.len() >= PAGE_ROWS)
        {
            let page = self.free.pop().unwrap_or_else(HistoryPage::new);
            self.pages.push_back(page);
            self.remap.clear();
        }
        if self.remap_generation != generation {
            self.remap.clear();
            self.remap_generation = generation;
        }
    }

    fn push(&mut self, cells: &[u64], head: &Head, screen: &StyleTable, generation: u32) {
        if self.max_history == 0 {
            return;
        }
        if self.len >= self.max_history {
            self.evict();
        }
        self.ensure_page(generation);
        let remap = &mut self.remap;
        let page = self.pages.back_mut().expect("a page was just ensured");
        let offset = page.bytes.len() as u32;
        let used = &cells[..usize::from(head.used)];
        let (kind, len, style) = if head.uniform {
            let start = page.bytes.len();
            page.bytes.extend(used.iter().map(|&cell| cell as u8));
            if head.style == 0 {
                let len = page.bytes[start..]
                    .iter()
                    .rposition(|&b| b != b' ')
                    .map_or(0, |last| last + 1);
                page.bytes.truncate(start + len);
            }
            let len = page.bytes.len() - start;
            let style = remap_key_to_page(remap, &mut page.styles, screen, head.style);
            (Kind::Uniform, len, style)
        } else {
            let cells = &used[..used_len(used)];
            let start = page.bytes.len();
            page.bytes.resize(start + cells.len() * 8, 0);
            let (out, _) = page.bytes[start..].as_chunks_mut::<8>();
            let (mut last_key, mut last_page_key) = (0, 0);
            for (out, &cell) in out.iter_mut().zip(cells) {
                assert!(cell & GRAPHEME == 0, "the prototype stores no graphemes");
                let key = style_key(cell);
                if key != last_key {
                    last_key = key;
                    last_page_key = remap_key_to_page(remap, &mut page.styles, screen, key);
                }
                *out = ((cell & !STYLE_MASK) | base(last_page_key)).to_le_bytes();
            }
            (Kind::Cells, cells.len(), 0)
        };
        page.heads.push(HistHead {
            offset,
            len: len as u16,
            kind,
            width: cells.len() as u16,
            wrapped: head.wrapped,
            _marks: head.marks,
            _anchor: head.anchor,
            style,
        });
        self.len += 1;
    }

    fn get(&self, index: usize, out: &mut Vec<Cell>) -> bool {
        let at = self.front + index;
        let page = &self.pages[at / PAGE_ROWS];
        let head = page.heads[at % PAGE_ROWS];
        let start = head.offset as usize;
        out.clear();
        match head.kind {
            Kind::Uniform => {
                let style = lookup(&page.styles, head.style);
                let bytes = &page.bytes[start..start + usize::from(head.len)];
                out.extend(
                    bytes
                        .iter()
                        .map(|&b| style.cell(char::from(b), Default::default())),
                );
            }
            Kind::Cells => {
                let bytes = &page.bytes[start..start + usize::from(head.len) * 8];
                let (cells, _) = bytes.as_chunks::<8>();
                out.extend(
                    cells
                        .iter()
                        .map(|&cell| unpack(u64::from_le_bytes(cell), &page.styles)),
                );
            }
        }
        out.resize(usize::from(head.width), Cell::default());
        head.wrapped
    }

    fn heap_bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<HistoryPage>()
            + self
                .pages
                .iter()
                .chain(&self.free)
                .map(HistoryPage::heap_bytes)
                .sum::<usize>()
            + self.free.capacity() * std::mem::size_of::<HistoryPage>()
            + self.remap.capacity() * 4
    }
}

/// The newest page's key for screen style key `key`: inline keys and the
/// default are the same on every page; a table id is added to the page's
/// list (`styles`) on a miss.
#[inline(always)]
fn remap_key_to_page(
    remap: &mut Vec<u32>,
    styles: &mut Vec<Style>,
    screen: &StyleTable,
    key: u64,
) -> u64 {
    if key == 0 || key & INLINE != 0 {
        return key;
    }
    match remap.get(key as usize) {
        Some(&mapped) if mapped != UNMAPPED => u64::from(mapped),
        _ => add_to_page(remap, styles, screen, key),
    }
}

/// `remap_key_to_page`'s miss: append the screen style to the page.
#[inline(never)]
fn add_to_page(
    remap: &mut Vec<u32>,
    styles: &mut Vec<Style>,
    screen: &StyleTable,
    key: u64,
) -> u64 {
    let index = key as usize;
    let mapped = styles.len() as u32;
    styles.push(screen.style(key));
    if remap.len() <= index {
        remap.resize(index + 1, UNMAPPED);
    }
    remap[index] = mapped;
    u64::from(mapped)
}

/// Variant C's storage.
pub struct StoreC {
    screen: ScreenPage,
    generation: u32,
    collect_at: usize,
    history: History,
}

impl StoreC {
    fn collect(&mut self) {
        let page = &mut self.screen;
        let mut live = vec![false; page.styles.len()];
        for (slot, head) in page.heads.iter().enumerate() {
            mark_live(&mut live, head.style);
            let start = slot * page.cols;
            for &cell in &page.cells[start..start + usize::from(head.used)] {
                mark_live(&mut live, style_key(cell));
            }
        }
        let map = collect_table(&mut page.styles, &live);
        for (slot, head) in page.heads.iter_mut().enumerate() {
            head.style = remap_key(&map, head.style);
            let start = slot * page.cols;
            for cell in &mut page.cells[start..start + usize::from(head.used)] {
                *cell = (*cell & !STYLE_MASK) | base(remap_key(&map, style_key(*cell)));
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.collect_at = collect_threshold(page.styles.len(), page.cells.len());
    }
}

impl Storage for StoreC {
    fn new(cols: u16, rows: u16, max_history: usize) -> Self {
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        Self {
            screen: ScreenPage {
                cols,
                cells: vec![BLANK; cols * rows],
                slots: (0..rows as u16).collect(),
                heads: vec![Head::BLANK; rows],
                styles: StyleTable::new(),
                _graphemes: (Vec::new(), Vec::new()),
            },
            generation: 0,
            collect_at: collect_threshold(0, cols * rows),
            history: History::new(max_history),
        }
    }

    fn generation(&self) -> u32 {
        self.generation
    }

    fn intern(&mut self, style: Style) -> u32 {
        // Collect before adding, so the new id survives.
        if self.screen.styles.len() >= self.collect_at {
            self.collect();
        }
        self.screen.styles.insert(style)
    }

    #[inline]
    fn row_mut(&mut self, row: u16) -> (&mut [u64], &mut Head) {
        let (slot, range) = self.screen.slot_range(row);
        (&mut self.screen.cells[range], &mut self.screen.heads[slot])
    }

    #[inline]
    fn row(&self, row: u16) -> (&[u64], &Head) {
        let (slot, range) = self.screen.slot_range(row);
        (&self.screen.cells[range], &self.screen.heads[slot])
    }

    fn screen_styles(&self) -> &StyleTable {
        &self.screen.styles
    }

    fn save_top_row(&mut self) {
        let (slot, range) = self.screen.slot_range(0);
        self.history.push(
            &self.screen.cells[range],
            &self.screen.heads[slot],
            &self.screen.styles,
            self.generation,
        );
    }

    fn scroll_up(&mut self, erase_key: u64) {
        self.screen.slots.rotate_left(1);
        let bottom = self.screen.slots.len() as u16 - 1;
        let (slot, range) = self.screen.slot_range(bottom);
        row_erase(
            &mut self.screen.cells[range],
            &mut self.screen.heads[slot],
            erase_key,
        );
    }

    fn history_len(&self) -> usize {
        self.history.len
    }

    fn history_row(&self, index: usize, out: &mut Vec<Cell>) -> bool {
        self.history.get(index, out)
    }

    fn heap_bytes(&self) -> usize {
        let screen = &self.screen;
        screen.cells.capacity() * 8
            + screen.slots.capacity() * 2
            + screen.heads.capacity() * std::mem::size_of::<Head>()
            + screen.styles.heap_bytes()
            + self.history.heap_bytes()
    }
}
