//! Variant A: packed cells in `Vec<Row>`, a grid style table, and
//! `CompactRow`-like history encoded from packed cells.

use std::collections::VecDeque;

use super::{
    collect_table, collect_threshold, row_erase, style_id, used_len, Head, Storage, Style,
    StyleTable, BLANK, CP_MASK, GRAPHEME, SPACER, WIDE, WIDTH_MASK,
};
use crate::grid::cell::{Cell, CellFlags};
use crate::grid::row::{AnchorTag, RowMarks};

/// A screen row: its own allocation of packed cells, as `Row` has today.
struct PackedRow {
    cells: Vec<u64>,
    head: Head,
    /// Stands in for `Row::graphemes` so the row is the size it would be.
    _graphemes: Vec<(u16, Vec<char>)>,
}

impl PackedRow {
    fn new(cols: u16) -> Self {
        Self {
            cells: vec![BLANK; usize::from(cols)],
            head: Head::BLANK,
            _graphemes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Run {
    len: u16,
    style: Style,
}

/// Mark bit for a wide character followed by its spacer.
const WIDE_MARK: u16 = 1 << 15;
/// Mark bit for a cell whose width bits are stored as they are (orphans,
/// which the writers here never leave).
const RAW_MARK: u16 = 1 << 14;

/// `CompactRow` with the encoder reading packed cells: runs are found by
/// comparing style ids, and a style is looked up once per run.
#[derive(Default)]
struct CompactRow {
    width: u16,
    wrapped: bool,
    _row_marks: RowMarks,
    _anchor: AnchorTag,
    text: Vec<u8>,
    runs: Vec<Run>,
    marks: Vec<(u16, u16)>,
}

impl CompactRow {
    fn encode_from(&mut self, cells: &[u64], head: &Head, styles: &StyleTable) {
        self.width = cells.len() as u16;
        self.wrapped = head.wrapped;
        self._row_marks = head.marks;
        self._anchor = head.anchor;
        self.text.clear();
        self.runs.clear();
        self.marks.clear();

        let used = &cells[..usize::from(head.used)];
        if head.uniform {
            self.text.extend(used.iter().map(|&cell| cell as u8));
            let len = if head.style == 0 {
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
                    style: styles.get(head.style),
                });
            }
            return;
        }

        let cells = &used[..used_len(used)];
        self.text.reserve(cells.len());
        let mut col = 0;
        while col < cells.len() {
            let id = style_id(cells[col]);
            let start = col;
            while col < cells.len() && style_id(cells[col]) == id {
                let cell = cells[col];
                self.push_char(cell);
                if cell & (WIDTH_MASK | GRAPHEME) != 0 {
                    if cell & WIDTH_MASK == WIDE
                        && cell & GRAPHEME == 0
                        && cells
                            .get(col + 1)
                            .is_some_and(|&next| (next & !super::STYLE_MASK) == (BLANK | SPACER))
                        && cells.get(col + 1).map(|&next| style_id(next)) == Some(id)
                    {
                        self.push_char(cells[col + 1]);
                        self.marks.push((col as u16, WIDE_MARK));
                        col += 2;
                        continue;
                    }
                    assert!(cell & GRAPHEME == 0, "the prototype stores no graphemes");
                    self.marks
                        .push((col as u16, RAW_MARK | ((cell & WIDTH_MASK) >> 21) as u16));
                }
                col += 1;
            }
            self.runs.push(Run {
                len: (col - start) as u16,
                style: styles.get(id),
            });
        }
    }

    #[inline]
    fn push_char(&mut self, cell: u64) {
        let cp = (cell & CP_MASK) as u32;
        if cp < 0x80 {
            self.text.push(cp as u8);
        } else {
            let c = char::from_u32(cp).unwrap_or(' ');
            let mut utf8 = [0u8; 4];
            self.text
                .extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        }
    }

    fn decode(&self, out: &mut Vec<Cell>) {
        out.clear();
        let text = std::str::from_utf8(&self.text).unwrap_or_default();
        let mut chars = text.chars();
        let mut marks = self.marks.iter().peekable();
        for run in &self.runs {
            let mut left = run.len;
            while left > 0 {
                let col = out.len() as u16;
                let c = chars.next().unwrap_or(' ');
                let mark = match marks.peek() {
                    Some(&&(at, mark)) if at == col => {
                        marks.next();
                        mark
                    }
                    _ => 0,
                };
                if mark & WIDE_MARK != 0 {
                    out.push(run.style.cell(c, CellFlags::WIDE_CHAR));
                    let spacer = chars.next().unwrap_or(' ');
                    out.push(run.style.cell(spacer, CellFlags::WIDE_CHAR_SPACER));
                    left -= 2;
                    continue;
                }
                let structural = if mark & RAW_MARK != 0 {
                    match u64::from(mark & 3) << 21 {
                        WIDE => CellFlags::WIDE_CHAR,
                        SPACER => CellFlags::WIDE_CHAR_SPACER,
                        _ => CellFlags::WIDE_CHAR.union(CellFlags::WIDE_CHAR_SPACER),
                    }
                } else {
                    CellFlags::NONE
                };
                out.push(run.style.cell(c, structural));
                left -= 1;
            }
        }
        out.resize(usize::from(self.width), Cell::default());
    }

    fn heap_bytes(&self) -> usize {
        self.text.capacity()
            + self.runs.capacity() * std::mem::size_of::<Run>()
            + self.marks.capacity() * std::mem::size_of::<(u16, u16)>()
    }
}

/// Variant A's storage.
pub struct StoreA {
    rows: Vec<PackedRow>,
    styles: StyleTable,
    generation: u32,
    collect_at: usize,
    history: VecDeque<CompactRow>,
    max_history: usize,
    evicted: usize,
}

impl StoreA {
    fn collect(&mut self) {
        let mut live = vec![false; self.styles.len()];
        for row in &self.rows {
            live[row.head.style as usize] = true;
            for &cell in &row.cells[..usize::from(row.head.used)] {
                live[style_id(cell) as usize] = true;
            }
        }
        let map = collect_table(&mut self.styles, &live);
        for row in &mut self.rows {
            row.head.style = map[row.head.style as usize];
            for cell in &mut row.cells[..usize::from(row.head.used)] {
                let id = map[style_id(*cell) as usize];
                *cell = (*cell & !super::STYLE_MASK) | super::base(id);
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.collect_at = collect_threshold(self.styles.len());
    }
}

impl Storage for StoreA {
    fn new(cols: u16, rows: u16, max_history: usize) -> Self {
        Self {
            rows: (0..rows).map(|_| PackedRow::new(cols)).collect(),
            styles: StyleTable::new(),
            generation: 0,
            collect_at: collect_threshold(0),
            history: VecDeque::new(),
            max_history,
            evicted: 0,
        }
    }

    fn generation(&self) -> u32 {
        self.generation
    }

    fn intern(&mut self, style: Style) -> u32 {
        // Collecting when due, before the lookup, keeps it to one hash.
        if self.styles.len() >= self.collect_at {
            self.collect();
        }
        self.styles.insert(style)
    }

    fn row_mut(&mut self, row: u16) -> (&mut [u64], &mut Head) {
        let row = &mut self.rows[usize::from(row)];
        (&mut row.cells, &mut row.head)
    }

    fn row(&self, row: u16) -> (&[u64], &Head) {
        let row = &self.rows[usize::from(row)];
        (&row.cells, &row.head)
    }

    fn screen_styles(&self) -> &StyleTable {
        &self.styles
    }

    fn save_top_row(&mut self) {
        if self.max_history == 0 {
            return;
        }
        let mut compact = if self.history.len() >= self.max_history {
            self.evicted += 1;
            self.history.pop_front().unwrap_or_default()
        } else {
            CompactRow::default()
        };
        let top = &self.rows[0];
        compact.encode_from(&top.cells, &top.head, &self.styles);
        self.history.push_back(compact);
    }

    fn scroll_up(&mut self, erase_id: u32) {
        self.rows.rotate_left(1);
        if let Some(bottom) = self.rows.last_mut() {
            row_erase(&mut bottom.cells, &mut bottom.head, erase_id);
        }
    }

    fn history_len(&self) -> usize {
        self.history.len()
    }

    fn history_row(&self, index: usize, out: &mut Vec<Cell>) -> bool {
        let row = &self.history[index];
        row.decode(out);
        row.wrapped
    }

    fn heap_bytes(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.cells.capacity() * 8)
            .sum::<usize>()
            + self.rows.capacity() * std::mem::size_of::<PackedRow>()
            + self.styles.heap_bytes()
            + self.history.capacity() * std::mem::size_of::<CompactRow>()
            + self
                .history
                .iter()
                .map(CompactRow::heap_bytes)
                .sum::<usize>()
    }
}
