//! Throwaway measurement prototype for ADR 0022 (page storage).
//!
//! Built only with the `page-proto` feature, which nothing that ships
//! enables. It exists to answer the ADR's go/no-go question with numbers:
//! two stripped-down buffers, fed by the real parser, that handle only what
//! the measured corpora need (printable ASCII spans, `Print` written in
//! place, CR, LF, autowrap and SGR). Every other action is ignored.
//!
//! - [`a::StoreA`]: 8-byte packed cells in `Vec<Row>`, a grid style table,
//!   and `CompactRow`-like history encoded from packed cells (variant A).
//! - [`c::StoreC`]: one screen page with slot rotation and append-only
//!   history pages (variant C, ADR §2–§4).
//!
//! Both share the cursor and write logic in [`Core`], copied from
//! `Buffer` and `Grid` for the cases above, and the row operations below,
//! so the difference between them is only where rows and history live.
//! SGR goes through the existing attribute code (a scratch [`Terminal`]
//! and [`TerminalHandler`]) and the resulting template gets its style key
//! once per change, when the next character is printed: inline in the
//! cell when it has no truecolor, underline color or hyperlink, otherwise
//! from the style table (see [`INLINE`] and [`StyleTable`]). Grapheme
//! clusters are not supported: nothing in the measured corpora makes one,
//! and the correctness check would catch it.

pub mod a;
pub mod c;

use super::cell::{Cell, CellFlags, PackedColor};
use super::cluster::{self, CharProps, Tail};
use super::row::{AnchorTag, RowMarks};
use super::{Dimensions, Terminal, TerminalHandler, DEFAULT_SCROLLBACK};
use crate::ansi::{Action, AnsiOutput, AnsiParser};

/// Flags a packed cell keeps in its own bits rather than in its style.
const STRUCTURAL: CellFlags = CellFlags::WIDE_CHAR
    .union(CellFlags::WIDE_CHAR_SPACER)
    .union(CellFlags::HAS_COMBINING);

/// Everything about a cell except its character and structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    fg: PackedColor,
    bg: PackedColor,
    underline_color: PackedColor,
    flags: CellFlags,
    hyperlink_id: u32,
}

impl Style {
    fn of(cell: &Cell) -> Self {
        let mut flags = cell.flags();
        flags.unset(STRUCTURAL);
        Self {
            fg: cell.packed_fg(),
            bg: cell.packed_bg(),
            underline_color: cell.packed_underline_color(),
            flags,
            hyperlink_id: cell.hyperlink_id(),
        }
    }

    fn cell(self, c: char, structural: CellFlags) -> Cell {
        let mut flags = self.flags;
        flags.set(structural);
        Cell::from_packed(
            c,
            self.fg,
            self.bg,
            self.underline_color,
            flags,
            self.hyperlink_id,
        )
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The style's slot in [`StyleTable`]'s cache: a multiplicative hash
    /// of its words.
    #[inline]
    fn slot(&self) -> usize {
        let colors = u64::from(self.fg.bits()) | u64::from(self.bg.bits()) << 32;
        let rest = u64::from(self.underline_color.bits())
            ^ u64::from(self.flags.bits()) << 32
            ^ u64::from(self.hyperlink_id).rotate_left(16);
        let hash = (colors ^ rest.rotate_left(29)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (hash >> (64 - RECENT.trailing_zeros())) as usize
    }
}

/// Styles by id; id 0 is always the default style.
///
/// Interning is approximate: a small direct-mapped cache of recent styles
/// finds a style that is still hot, and anything else is appended under a
/// new id. An id always names one style, but a style may have several ids
/// (a uniform row or a history run may then split where it need not).
/// That trades a hash table lookup and insert per new style, which most
/// of an SGR-heavy stream is, for one cache probe. Collection drops the
/// duplicates that are no longer on screen.
pub struct StyleTable {
    styles: Vec<Style>,
    /// Slot to id of the style last interned there; 0 means empty (the
    /// default style never reaches the cache).
    recent: Box<[u32; RECENT]>,
}

/// Slots in [`StyleTable`]'s cache of recent styles.
const RECENT: usize = 256;

impl StyleTable {
    fn new() -> Self {
        Self {
            styles: vec![Style::default()],
            recent: Box::new([0; RECENT]),
        }
    }

    /// The style `key` names, inline or in this table.
    fn style(&self, key: u64) -> Style {
        lookup(&self.styles, key)
    }

    fn len(&self) -> usize {
        self.styles.len()
    }

    /// An id for `style`: the cached one if `style` is still in the
    /// cache, otherwise a new one.
    #[inline]
    fn insert(&mut self, style: Style) -> u32 {
        if style.is_default() {
            return 0;
        }
        let slot = style.slot();
        let cached = self.recent[slot];
        if cached != 0 && self.styles[cached as usize] == style {
            return cached;
        }
        let id = self.styles.len() as u32;
        self.styles.push(style);
        self.recent[slot] = id;
        id
    }

    fn heap_bytes(&self) -> usize {
        self.styles.capacity() * std::mem::size_of::<Style>() + RECENT * 4
    }
}

// Packed cells (ADR §1): codepoint in bits 0..21, width in 21..23, the
// grapheme bit at 23 and the style key in 24..64.
pub(crate) const BLANK: u64 = 0x20;
const CP_MASK: u64 = 0x1F_FFFF;
const WIDE: u64 = 1 << 21;
const SPACER: u64 = 2 << 21;
const WIDTH_MASK: u64 = 3 << 21;
const GRAPHEME: u64 = 1 << 23;
const STYLE_SHIFT: u32 = 24;
const STYLE_MASK: u64 = !0 << STYLE_SHIFT;

/// A style key (the cell's top 40 bits) with this bit set holds its style
/// inline: foreground and background as a 2-bit kind (default, named,
/// indexed) and an 8-bit value each, then the 16 flag bits. Without it,
/// the key is an id in the style table (0 the default style). A style that
/// fits inline is always stored inline, so the two never overlap.
const INLINE: u64 = 1 << 39;

#[inline]
fn style_key(cell: u64) -> u64 {
    cell >> STYLE_SHIFT
}

#[inline]
fn base(key: u64) -> u64 {
    key << STYLE_SHIFT
}

/// `style`'s inline key, or `None` if it needs the table: a truecolor
/// foreground or background, an underline color or a hyperlink. The
/// default style is key 0.
#[inline]
fn inline_key(style: &Style) -> Option<u64> {
    const RGB: u32 = 3;
    let (fg, bg) = (style.fg.bits(), style.bg.bits());
    if fg >> 24 == RGB
        || bg >> 24 == RGB
        || style.underline_color != PackedColor::DEFAULT
        || style.hyperlink_id != 0
    {
        return None;
    }
    let color = |bits: u32| u64::from((bits >> 24) << 8 | (bits & 0xFF));
    let key = color(fg) | color(bg) << 10 | u64::from(style.flags.bits()) << 20;
    Some(if key == 0 { 0 } else { key | INLINE })
}

/// The style an inline key holds.
fn inline_style(key: u64) -> Style {
    let color = |c: u64| PackedColor::from_bits(((c >> 8 & 3) << 24 | c & 0xFF) as u32);
    Style {
        fg: color(key & 0x3FF),
        bg: color(key >> 10 & 0x3FF),
        underline_color: PackedColor::DEFAULT,
        flags: CellFlags::from_bits((key >> 20) as u16),
        hyperlink_id: 0,
    }
}

/// The style `key` names, inline or in `styles`.
#[inline]
fn lookup(styles: &[Style], key: u64) -> Style {
    if key & INLINE != 0 {
        inline_style(key)
    } else {
        styles[key as usize]
    }
}

fn unpack(cell: u64, styles: &[Style]) -> Cell {
    let c = char::from_u32((cell & CP_MASK) as u32).unwrap_or(' ');
    let mut structural = match cell & WIDTH_MASK {
        WIDE => CellFlags::WIDE_CHAR,
        SPACER => CellFlags::WIDE_CHAR_SPACER,
        0 => CellFlags::NONE,
        _ => CellFlags::WIDE_CHAR.union(CellFlags::WIDE_CHAR_SPACER),
    };
    if cell & GRAPHEME != 0 {
        structural.set(CellFlags::HAS_COMBINING);
    }
    lookup(styles, style_key(cell)).cell(c, structural)
}

/// The number of cells up to and including the last one that is not
/// blank in the default style (as `history::used_len`, on packed cells).
fn used_len(cells: &[u64]) -> usize {
    const BLOCK: usize = 8;
    let mut end = cells.len();
    while end >= BLOCK
        && cells[end - BLOCK..end]
            .iter()
            .fold(0, |acc, &cell| acc | (cell ^ BLANK))
            == 0
    {
        end -= BLOCK;
    }
    cells[..end]
        .iter()
        .rposition(|&cell| cell != BLANK)
        .map_or(0, |last| last + 1)
}

/// Per-row state kept next to the cells (a `Row`'s flags, or a screen
/// page's row head).
#[derive(Debug, Clone, Copy)]
pub struct Head {
    /// Every cell at or after this column is [`BLANK`]. May overstate.
    used: u16,
    /// Every cell before `used` is ASCII, with no structural bits, in
    /// style `style` (a style key). May understate; generalises
    /// `Row::plain`.
    uniform: bool,
    style: u64,
    wrapped: bool,
    /// The row has grapheme extras somewhere. Never set here (see the
    /// module docs); the write paths test it as the real ones would.
    has_graphemes: bool,
    marks: RowMarks,
    anchor: AnchorTag,
}

impl Head {
    const BLANK: Head = Head {
        used: 0,
        uniform: true,
        style: 0,
        wrapped: false,
        has_graphemes: false,
        marks: RowMarks {
            input_start: 0,
            output_start: 0,
            command_end: 0,
        },
        anchor: AnchorTag::NONE,
    };
}

// Row operations shared by both variants.

/// Write ASCII `bytes` at `col` in style `key`. The span fits on the row
/// and the caller has cleared wide characters straddling its edges.
#[inline]
fn row_write_ascii(cells: &mut [u64], head: &mut Head, col: usize, bytes: &[u8], key: u64) {
    let end = col + bytes.len();
    let used = usize::from(head.used);
    // The real write drops extras here; testing the flag costs the same.
    assert!(!head.has_graphemes, "the prototype stores no graphemes");
    if col == 0 && end >= used {
        head.uniform = true;
        head.style = key;
    } else if head.style != key || (col > used && key != 0) {
        head.uniform = false;
    }
    let style = base(key);
    for (cell, &byte) in cells[col..end].iter_mut().zip(bytes) {
        *cell = style | u64::from(byte);
    }
    head.used = head.used.max(end as u16);
}

/// The wide-character edges of an ASCII span at `col..end`, as
/// `Grid::clear_wide_orphans_for_ascii_span` resolves them.
#[inline]
fn row_clear_ascii_edges(cells: &mut [u64], head: &mut Head, col: usize, end: usize) {
    if col > 0 && cells[col] & WIDTH_MASK == SPACER {
        cells[col - 1] = BLANK;
        head.uniform &= head.style == 0;
    }
    if end < cells.len() && cells[end] & WIDTH_MASK == SPACER {
        cells[end] = BLANK;
        head.uniform &= head.style == 0;
    }
}

/// `Row::write_char_in_place` on packed cells.
#[inline]
fn row_write_char_in_place(
    cells: &mut [u64],
    head: &mut Head,
    col: usize,
    c: char,
    width: usize,
    key: u64,
) -> bool {
    let end = col + width;
    let Some(span) = cells.get_mut(col..end) else {
        return false;
    };
    if span.iter().any(|&cell| cell & WIDTH_MASK != 0) {
        return false;
    }
    let style = base(key);
    if width > 1 {
        span[0] = style | c as u64 | WIDE;
        span[1] = style | BLANK | SPACER;
    } else {
        span[0] = style | c as u64;
    }
    // The real write drops extras here; testing the flag costs the same.
    assert!(!head.has_graphemes, "the prototype stores no graphemes");
    head.uniform = false;
    head.used = head.used.max(end as u16);
    true
}

/// The rest of `Grid::write_measured_char_slow` once the cursor is placed:
/// clear wide-character orphans, then write `c` and its spacer.
fn row_write_char_slow(
    cells: &mut [u64],
    head: &mut Head,
    col: usize,
    c: char,
    width: usize,
    key: u64,
) {
    let cols = cells.len();
    if cells[col] & WIDTH_MASK == WIDE && col + 1 < cols {
        cells[col + 1] = BLANK;
    }
    if cells[col] & WIDTH_MASK == SPACER && col > 0 {
        cells[col - 1] = BLANK;
    }
    if width > 1 && col + 2 < cols && cells[col + 1] & WIDTH_MASK == WIDE {
        cells[col + 2] = BLANK;
    }
    let style = base(key);
    cells[col] = style | c as u64 | if width > 1 { WIDE } else { 0 };
    let mut end = col + 1;
    if width > 1 && col + 1 < cols {
        cells[col + 1] = style | BLANK | SPACER;
        end += 1;
    }
    head.uniform = false;
    head.used = head.used.max(end as u16);
}

/// `Row::erase` with a blank in style `key` (0, or a background only).
#[inline]
fn row_erase(cells: &mut [u64], head: &mut Head, key: u64) {
    if key == 0 {
        cells[..usize::from(head.used)].fill(BLANK);
        *head = Head::BLANK;
    } else {
        cells.fill(base(key) | BLANK);
        *head = Head {
            used: cells.len() as u16,
            style: key,
            ..Head::BLANK
        };
    }
}

/// The screen rows and scrollback behind a [`Core`].
pub trait Storage {
    fn new(cols: u16, rows: u16, max_history: usize) -> Self;
    /// Changes whenever style ids are rewritten (collection), which
    /// invalidates every cached id.
    fn generation(&self) -> u32;
    /// The id of `style` (one that does not fit inline) in the screen's
    /// table, collecting first if the table is due.
    fn intern(&mut self, style: Style) -> u32;
    #[doc(hidden)]
    fn row_mut(&mut self, row: u16) -> (&mut [u64], &mut Head);
    #[doc(hidden)]
    fn row(&self, row: u16) -> (&[u64], &Head);
    #[doc(hidden)]
    fn screen_styles(&self) -> &StyleTable;
    /// Copy screen row 0 into history (evicting the oldest row when full).
    fn save_top_row(&mut self);
    /// Scroll the whole screen up one row and erase the new bottom row
    /// with style `erase_key`.
    fn scroll_up(&mut self, erase_key: u64);
    fn history_len(&self) -> usize;
    /// History row `index` (0 oldest) unpacked into `out`, padded to the
    /// width it was stored at; returns whether it was soft-wrapped.
    fn history_row(&self, index: usize, out: &mut Vec<Cell>) -> bool;
    /// Heap bytes held, for the record (the counting allocator is the
    /// measurement).
    fn heap_bytes(&self) -> usize;
}

/// What `Grid::record_print` keeps.
#[derive(Clone, Copy)]
struct LastPrint {
    base_row: u16,
    base_col: u16,
    cursor: (u16, u16),
    wrap_pending: bool,
    content_version: u64,
    tail: Tail,
}

/// Cursor, template and damage tracking over a [`Storage`]: `Buffer` and
/// `Grid` cut down to the actions the prototype handles, with the main
/// screen's full-height scroll region and autowrap on.
pub struct Core<S: Storage> {
    cols: u16,
    rows: u16,
    col: u16,
    row: u16,
    wrap_pending: bool,
    attrs: Cell,
    style: Style,
    /// `style`'s key, valid while `key_generation` matches the storage's.
    key: u64,
    key_generation: u32,
    dirty: Vec<bool>,
    row_versions: Vec<u64>,
    content_version: u64,
    last_char: char,
    last_print: Option<LastPrint>,
    store: S,
}

const NO_GENERATION: u32 = u32::MAX;

impl<S: Storage> Core<S> {
    fn new(cols: u16, rows: u16, max_history: usize) -> Self {
        Self {
            cols,
            rows,
            col: 0,
            row: 0,
            wrap_pending: false,
            attrs: Cell::default(),
            style: Style::default(),
            key: 0,
            key_generation: NO_GENERATION,
            dirty: vec![true; usize::from(rows)],
            row_versions: vec![0; usize::from(rows)],
            content_version: 0,
            last_char: ' ',
            last_print: None,
            store: S::new(cols, rows, max_history),
        }
    }

    fn set_attrs(&mut self, attrs: Cell) {
        self.attrs = attrs;
        self.style = Style::of(&attrs);
        self.key_generation = NO_GENERATION;
    }

    /// `style`'s key: inline if it fits, otherwise interned.
    #[inline]
    fn key(&mut self, style: Style) -> u64 {
        match inline_key(&style) {
            Some(key) => key,
            None => u64::from(self.store.intern(style)),
        }
    }

    #[inline]
    fn template_key(&mut self) -> u64 {
        if self.key_generation != self.store.generation() {
            self.key = self.key(self.style);
            self.key_generation = self.store.generation();
        }
        self.key
    }

    fn erase_key(&mut self) -> u64 {
        if self.attrs.packed_bg() == PackedColor::DEFAULT {
            0
        } else {
            self.key(Style {
                bg: self.attrs.packed_bg(),
                ..Style::default()
            })
        }
    }

    fn mark_dirty(&mut self, row: u16) {
        self.content_version = self.content_version.wrapping_add(1);
        self.row_versions[usize::from(row)] = self.content_version;
        self.dirty[usize::from(row)] = true;
    }

    fn record_print(&mut self, row: u16, col: u16, tail: Tail) {
        self.last_print = Some(LastPrint {
            base_row: row,
            base_col: col,
            cursor: (self.col, self.row),
            wrap_pending: self.wrap_pending,
            content_version: self.content_version,
            tail,
        });
    }

    /// Scroll up one line (the history copy is the caller's).
    fn scroll_up(&mut self) {
        let erase = self.erase_key();
        self.store.scroll_up(erase);
        self.content_version = self.content_version.wrapping_add(1);
        self.row_versions.fill(self.content_version);
        self.dirty.fill(true);
    }

    fn line_feed(&mut self) {
        if self.row == self.rows - 1 {
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }

    fn wrap_pending_cursor(&mut self) {
        self.store.row_mut(self.row).1.wrapped = true;
        self.col = 0;
        self.line_feed();
        self.wrap_pending = false;
    }

    fn advance_cursor(&mut self, width: u16) {
        self.col = self.col.saturating_add(width);
        if self.col >= self.cols {
            self.col = self.cols - 1;
            self.wrap_pending = true;
        }
    }

    /// `Buffer::write_ascii_span` and `Grid::write_ascii_span`.
    fn print_ascii(&mut self, bytes: &[u8]) {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            if self.wrap_pending && self.row == self.rows - 1 {
                self.store.save_top_row();
            }
            let chunk_len = if self.wrap_pending {
                1
            } else {
                usize::from(self.cols - self.col).min(remaining.len())
            };
            if self.wrap_pending {
                self.wrap_pending_cursor();
            }
            let (row, col) = (self.row, usize::from(self.col));
            let end = col + chunk_len;
            let key = self.template_key();
            let (cells, head) = self.store.row_mut(row);
            row_clear_ascii_edges(cells, head, col, end);
            row_write_ascii(cells, head, col, &remaining[..chunk_len], key);
            self.mark_dirty(row);
            self.last_char = remaining[chunk_len - 1] as char;
            remaining = &remaining[chunk_len..];
            if end >= usize::from(self.cols) {
                self.col = self.cols - 1;
                self.wrap_pending = true;
            } else {
                self.col = end as u16;
            }
            self.record_print(row, end as u16 - 1, Tail::Settled);
        }
    }

    /// `Buffer::write_char` for a character that starts a cluster.
    fn print(&mut self, c: char) {
        let props = CharProps::of(c);
        if c >= '\u{300}' {
            if let Some(last) = self.last_print {
                if cluster::joins_tail(last.tail, props.class()) == Some(true) {
                    // Extending a cluster: not supported (module docs).
                    return;
                }
            }
        }
        let width = props.width();
        if (1..=2).contains(&width) && !self.wrap_pending && self.col + width <= self.cols {
            let (row, col) = (self.row, self.col);
            let key = self.template_key();
            let (cells, head) = self.store.row_mut(row);
            if row_write_char_in_place(cells, head, usize::from(col), c, usize::from(width), key) {
                self.mark_dirty(row);
                self.last_char = c;
                self.advance_cursor(width);
                self.record_print(row, col, Tail::start(c));
                return;
            }
        }
        if width == 0 {
            // Zero-width characters attach to the previous cell: not
            // supported (module docs).
            return;
        }
        let wide_wraps = width > 1 && self.col + width > self.cols;
        if (self.wrap_pending || wide_wraps) && self.row == self.rows - 1 {
            self.store.save_top_row();
        }
        if self.wrap_pending {
            self.wrap_pending_cursor();
        }
        if width > 1 && self.col + width > self.cols {
            self.wrap_pending_cursor();
        }
        let (row, col) = (self.row, self.col);
        let key = self.template_key();
        self.mark_dirty(row);
        let (cells, head) = self.store.row_mut(row);
        row_write_char_slow(cells, head, usize::from(col), c, usize::from(width), key);
        self.last_char = c;
        self.advance_cursor(width);
        self.record_print(row, col, Tail::start(c));
    }

    fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.col = 0;
    }

    /// `Buffer::newline`.
    fn newline(&mut self) {
        self.wrap_pending = false;
        if self.row == self.rows - 1 {
            self.store.save_top_row();
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }
}

/// A prototype terminal: a [`Core`] over storage `S`, plus the scratch
/// terminal that applies SGR through the existing attribute code.
pub struct ProtoTerminal<S: Storage> {
    core: Core<S>,
    sgr: Terminal,
}

/// Variant A.
pub type TerminalA = ProtoTerminal<a::StoreA>;
/// Variant C.
pub type TerminalC = ProtoTerminal<c::StoreC>;

impl<S: Storage> ProtoTerminal<S> {
    /// A main screen with the default scrollback.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            core: Core::new(cols, rows, DEFAULT_SCROLLBACK),
            sgr: Terminal::new(Dimensions::new(1, 1)),
        }
    }

    /// Parse `data` and apply the actions the prototype handles.
    pub fn feed(&mut self, parser: &mut AnsiParser, data: &[u8]) {
        let core = &mut self.core;
        let sgr = &mut self.sgr;
        parser.feed_with_ascii_spans(data, |out| match out {
            AnsiOutput::PrintAscii(bytes) => core.print_ascii(bytes),
            AnsiOutput::Action(Action::Print(c)) => core.print(c),
            AnsiOutput::Action(Action::CarriageReturn) => core.carriage_return(),
            AnsiOutput::Action(Action::Newline) => core.newline(),
            AnsiOutput::Action(action @ (Action::SetAttribute(_) | Action::ResetAttributes)) => {
                TerminalHandler::new(sgr).handle(action);
                core.set_attrs(*sgr.grid().attrs());
            }
            AnsiOutput::Action(_) => {}
        });
    }

    /// Cursor column, row and pending wrap.
    pub fn cursor(&self) -> (u16, u16, bool) {
        (self.core.col, self.core.row, self.core.wrap_pending)
    }

    /// Screen row `row` unpacked into `out`; returns whether it is
    /// soft-wrapped. This is the copy a renderer's row cache makes.
    pub fn visible_row(&self, row: u16, out: &mut Vec<Cell>) -> bool {
        let (cells, head) = self.core.store.row(row);
        let styles = self.core.store.screen_styles();
        out.clear();
        out.extend(cells.iter().map(|&cell| unpack(cell, &styles.styles)));
        head.wrapped
    }

    pub fn history_len(&self) -> usize {
        self.core.store.history_len()
    }

    /// History row `index` (0 oldest); see [`Storage::history_row`].
    pub fn history_row(&self, index: usize, out: &mut Vec<Cell>) -> bool {
        self.core.store.history_row(index, out)
    }

    /// Heap bytes the storage holds, by its own accounting.
    pub fn heap_bytes(&self) -> usize {
        self.core.store.heap_bytes()
    }

    /// A digest of the bookkeeping the real code keeps (last character,
    /// cluster record, row versions), so the optimiser cannot drop it.
    pub fn bookkeeping_digest(&self) -> u64 {
        let core = &self.core;
        let last = core.last_print.map_or(0, |last| {
            last.content_version
                ^ u64::from(last.base_row) << 16
                ^ u64::from(last.base_col) << 32
                ^ u64::from(last.cursor.0 ^ last.cursor.1) << 48
                ^ u64::from(last.wrap_pending)
                ^ u64::from(last.tail == Tail::Settled) << 1
        });
        let versions = core.row_versions.iter().fold(0, |acc, &v| acc ^ v);
        let dirty = core.dirty.iter().filter(|&&d| d).count() as u64;
        last ^ versions ^ dirty ^ u64::from(core.last_char)
    }
}

/// Collect a screen style table in place: keep the ids `live` marks,
/// renumber them densely (0 stays 0), keep the cache pointing at the
/// survivors, and return the old-to-new map (0 for a dropped id).
fn collect_table(table: &mut StyleTable, live: &[bool]) -> Vec<u32> {
    let mut map = vec![0u32; table.len()];
    let mut next = 1;
    for id in 1..table.styles.len() {
        if live[id] {
            table.styles[next] = table.styles[id];
            map[id] = next as u32;
            next += 1;
        }
    }
    table.styles.truncate(next);
    for slot in table.recent.iter_mut() {
        *slot = map[*slot as usize];
    }
    map
}

/// Mark the table id `key` names, if it names one.
#[inline]
fn mark_live(live: &mut [bool], key: u64) {
    if key & INLINE == 0 {
        live[key as usize] = true;
    }
}

/// `key` after a collection that renumbered the table by `map`.
#[inline]
fn remap_key(map: &[u32], key: u64) -> u64 {
    if key & INLINE == 0 {
        u64::from(map[key as usize])
    } else {
        key
    }
}

/// The table size at which the next collection runs: once as many new
/// styles as twice the screen's cells (at least 4096) have been added
/// since the last one, so a collection, which scans every screen cell,
/// costs a few instructions per new style at any screen size.
fn collect_threshold(live: usize, cells: usize) -> usize {
    live + 4096.max(2 * cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Row, TerminalHandler};

    fn reference(cols: u16, rows: u16, input: &[u8]) -> Terminal {
        let mut terminal = Terminal::new(Dimensions::new(cols, rows));
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed_with_ascii_spans(input, |out| match out {
            AnsiOutput::Action(action) => handler.handle(action),
            AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
        });
        terminal
    }

    fn row_cells(row: &Row) -> Vec<Cell> {
        row.cells().copied().collect()
    }

    fn check<S: Storage>(cols: u16, rows: u16, input: &[u8]) {
        let expected = reference(cols, rows, input);
        let mut proto = ProtoTerminal::<S>::new(cols, rows);
        proto.feed(&mut AnsiParser::new(), input);
        let buffer = expected.buffer();
        let cursor = expected.grid().cursor();
        assert_eq!(
            proto.cursor(),
            (
                cursor.position.col,
                cursor.position.row,
                cursor.wrap_pending
            )
        );
        assert_eq!(proto.history_len(), buffer.history_len());
        let mut out = Vec::new();
        for i in 0..buffer.history_len() {
            let row = buffer.row_at(i).unwrap();
            assert_eq!(
                proto.history_row(i, &mut out),
                row.is_wrapped(),
                "history {i}"
            );
            assert_eq!(out, row_cells(&row), "history {i}");
        }
        for r in 0..rows {
            let row = buffer.visible_row(r).unwrap();
            assert_eq!(proto.visible_row(r, &mut out), row.is_wrapped(), "row {r}");
            assert_eq!(out, row_cells(&row), "row {r}");
        }
    }

    fn sample() -> Vec<u8> {
        let mut input = Vec::new();
        for i in 0..200u32 {
            match i % 5 {
                0 => input.extend_from_slice(b"plain line of ascii text"),
                1 => input.extend_from_slice(b"\x1b[32mgreen\x1b[0m and \x1b[48;5;17mbg\x1b[0m"),
                2 => input.extend_from_slice("日本語 wide 中文字符 text".as_bytes()),
                3 => input.extend_from_slice(&[b'w'; 50]),
                _ => input
                    .extend_from_slice("x漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字漢".as_bytes()),
            }
            input.extend_from_slice(b"\r\n");
        }
        input
    }

    #[test]
    fn variant_a_matches_the_reference() {
        check::<a::StoreA>(21, 5, &sample());
    }

    #[test]
    fn variant_c_matches_the_reference() {
        check::<c::StoreC>(21, 5, &sample());
    }

    #[test]
    fn inline_keys_round_trip() {
        let colors = [PackedColor::DEFAULT]
            .into_iter()
            .chain((0..16).map(|n| PackedColor::from_bits(1 << 24 | n)))
            .chain((0..=255).map(|n| PackedColor::from_bits(2 << 24 | n)));
        let mut keys = std::collections::HashSet::new();
        for (i, fg) in colors.enumerate() {
            let style = Style {
                fg,
                bg: PackedColor::from_bits(2 << 24 | ((i as u32 * 7) % 256)),
                flags: CellFlags::from_bits((i as u16).wrapping_mul(0x1F3) & !STRUCTURAL.bits()),
                ..Style::default()
            };
            let key = inline_key(&style).expect("no truecolor, so it fits inline");
            assert_eq!(inline_style(key), style);
            assert!(keys.insert(key), "two styles share key {key:#x}");
        }
        assert_eq!(inline_key(&Style::default()), Some(0));
        let truecolor = Style {
            fg: PackedColor::from_bits(3 << 24 | 0x12_3456),
            ..Style::default()
        };
        assert_eq!(inline_key(&truecolor), None);
    }

    #[test]
    fn inline_and_table_styles_survive_collection() {
        let mut input = Vec::new();
        for i in 0..30_000u32 {
            let sgr = match i % 4 {
                0 => format!("\x1b[38;5;{}m", i % 256),
                1 => format!("\x1b[48;2;{};{};{}m", i % 256, i / 256 % 256, 7),
                2 => format!("\x1b[1;4;3{}m", i % 8),
                _ => format!("\x1b[58;2;{};0;0m", i % 256),
            };
            input.extend_from_slice(sgr.as_bytes());
            input.push(b'a' + (i % 26) as u8);
            if i % 7 == 0 {
                input.extend_from_slice(b"\x1b[0m");
            }
            if i % 50 == 49 {
                input.extend_from_slice(b"\x1b[0m\r\n");
            }
        }
        check::<a::StoreA>(60, 10, &input);
        check::<c::StoreC>(60, 10, &input);
    }

    #[test]
    fn collection_keeps_content() {
        let mut input = Vec::new();
        for i in 0..20_000u32 {
            input.extend_from_slice(
                format!("\x1b[38;2;{};{};{}mx", i % 256, (i / 256) % 256, i % 7).as_bytes(),
            );
            if i % 50 == 49 {
                input.extend_from_slice(b"\x1b[0m\r\n");
            }
        }
        check::<a::StoreA>(60, 10, &input);
        check::<c::StoreC>(60, 10, &input);
    }
}
