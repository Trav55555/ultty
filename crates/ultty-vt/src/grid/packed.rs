//! Packed cells and the grid's style table (ADR 0022 §1, migration slice 3).
//!
//! A screen row stores each cell in eight bytes, [`PackedCell`]: the
//! character, its structure (wide, spacer, grapheme extras) and a
//! [`StyleKey`] for everything else. A style with no truecolor, no
//! underline color and no hyperlink fits in the key itself; any other style
//! lives in the grid's [`StyleTable`] and the key is its id there.
//!
//! The public [`Cell`] stays the value readers see: packing is lossless for
//! every `Cell`, including the flag combinations only direct edits make, and
//! [`StyleTable::unpack`] gives the same `Cell` back.
//!
//! The table interns approximately (a small cache of recent styles, then a
//! new id) and is collected in place once enough new styles have been
//! added: the ids still on screen are kept and renumbered, and the rows are
//! rewritten. A collection changes ids, not content, so it neither dirties
//! rows nor bumps the content version. It bumps the table's generation,
//! which invalidates any id cached outside the rows (the grid's cursor
//! template). Collection runs only where nothing holds an id that is not yet
//! in a row: see [`super::Grid`]'s `collect_styles_if_due`.

use std::fmt;

use super::cell::{Cell, CellFlags, Color, PackedColor};
use super::row::{AnchorTag, GraphemeMap, Row, RowMarks};

/// Flags a packed cell keeps in its own bits rather than in its style.
const STRUCTURAL: CellFlags = CellFlags::WIDE_CHAR
    .union(CellFlags::WIDE_CHAR_SPACER)
    .union(CellFlags::HAS_COMBINING);

/// Everything about a cell except its character and structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Style {
    fg: PackedColor,
    bg: PackedColor,
    underline_color: PackedColor,
    flags: CellFlags,
    hyperlink_id: u32,
}

impl Style {
    /// `cell`'s style: its flags without the structural ones.
    #[inline]
    pub(super) fn of(cell: &Cell) -> Self {
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

    /// The style an erase leaves (ECMA-48): default but for background `bg`.
    #[inline]
    pub(super) fn erased(bg: Color) -> Self {
        Self {
            bg: PackedColor::pack(bg),
            ..Self::default()
        }
    }

    /// A cell in this style holding `c`, with the `structural` flags.
    #[inline]
    pub(super) fn cell(self, c: char, structural: CellFlags) -> Cell {
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

    /// The style's slot in [`StyleTable`]'s cache: a multiplicative hash of
    /// its words.
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

/// A cell's style: inline, or an id in the grid's [`StyleTable`].
///
/// With [`StyleKey::INLINE`] set, the key holds the whole style: foreground
/// and background as a 2-bit kind (default, named, indexed) and an 8-bit
/// value each, then the 16 flag bits. Without it, the key is a table id, 0
/// being the default style. A style that fits inline is always stored
/// inline, so equal keys mean equal styles; unequal keys may still name
/// equal styles (the table interns approximately).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) struct StyleKey(u64);

impl StyleKey {
    pub(super) const DEFAULT: StyleKey = StyleKey(0);
    /// 40 bits, the top 40 of a packed cell.
    const BITS: u32 = 40;
    const INLINE: u64 = 1 << 39;
    /// The bits an inline key uses below [`StyleKey::INLINE`].
    const INLINE_PAYLOAD: u64 = (1 << 36) - 1;

    #[inline]
    pub(super) fn is_default(self) -> bool {
        self.0 == 0
    }

    /// The table id this key names, if it is not inline.
    #[inline]
    fn table_id(self) -> Option<usize> {
        (self.0 & Self::INLINE == 0).then_some(self.0 as usize)
    }

    /// The key in a packed cell's position.
    #[inline]
    fn base(self) -> u64 {
        self.0 << PackedCell::KEY_SHIFT
    }

    /// `style`'s inline key, or `None` if it needs the table: a truecolor
    /// foreground or background, an underline color or a hyperlink.
    #[inline]
    fn inline(style: &Style) -> Option<StyleKey> {
        const RGB: u32 = 3;
        let (fg, bg) = (style.fg.bits(), style.bg.bits());
        // One branch rather than four: whether a style needs the table is a
        // coin flip on SGR-heavy output.
        if (fg >> 24 == RGB)
            | (bg >> 24 == RGB)
            | (style.underline_color != PackedColor::DEFAULT)
            | (style.hyperlink_id != 0)
        {
            return None;
        }
        let color = |bits: u32| u64::from((bits >> 24) << 8 | (bits & 0xFF));
        let key = color(fg) | color(bg) << 10 | u64::from(style.flags.bits()) << 20;
        Some(StyleKey(if key == 0 { 0 } else { key | Self::INLINE }))
    }

    /// The style an inline key holds.
    #[inline]
    fn inline_style(self) -> Style {
        let color = |c: u64| PackedColor::from_bits(((c >> 8 & 3) << 24 | c & 0xFF) as u32);
        Style {
            fg: color(self.0 & 0x3FF),
            bg: color(self.0 >> 10 & 0x3FF),
            underline_color: PackedColor::DEFAULT,
            flags: CellFlags::from_bits((self.0 >> 20) as u16),
            hyperlink_id: 0,
        }
    }
}

/// One cell in eight bytes (ADR 0022 §1).
///
/// ```text
/// bits  0..21  codepoint (a blank is 0x20, so a default cell is 0x20)
/// bit   21     wide character (CellFlags::WIDE_CHAR)
/// bit   22     wide spacer (CellFlags::WIDE_CHAR_SPACER)
/// bit   23     grapheme: the row holds extra codepoints for this cell
///              (CellFlags::HAS_COMBINING)
/// bits 24..64  StyleKey
/// ```
///
/// The two width bits are independent, as the flags are, so a cell with
/// both (which only a direct edit makes) still round-trips.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub(super) struct PackedCell(u64);

impl PackedCell {
    /// [`Cell::default`].
    pub(super) const BLANK: PackedCell = PackedCell(0x20);
    const CP_MASK: u64 = 0x1F_FFFF;
    const WIDE: u64 = 1 << 21;
    const SPACER: u64 = 1 << 22;
    const GRAPHEME: u64 = 1 << 23;
    const STRUCTURE: u64 = Self::WIDE | Self::SPACER | Self::GRAPHEME;
    const KEY_SHIFT: u32 = 24;
    /// Bits that must be clear for a cell to count toward a uniform row:
    /// anything above ASCII in the codepoint, and the structure.
    const NOT_UNIFORM: u64 = (Self::CP_MASK & !0x7F) | Self::STRUCTURE;

    /// `c`, narrow, in style `key`.
    #[inline]
    pub(super) fn new(c: char, key: StyleKey) -> Self {
        Self(key.base() | u64::from(c))
    }

    /// `c` as a wide character in style `key`.
    #[inline]
    pub(super) fn wide(c: char, key: StyleKey) -> Self {
        Self(key.base() | u64::from(c) | Self::WIDE)
    }

    /// The spacer after a wide character in style `key`.
    #[inline]
    pub(super) fn spacer(key: StyleKey) -> Self {
        Self(key.base() | 0x20 | Self::SPACER)
    }

    /// A blank in style `key`, as an erase leaves.
    #[inline]
    pub(super) fn blank(key: StyleKey) -> Self {
        Self(key.base() | 0x20)
    }

    #[inline]
    pub(super) fn char(self) -> char {
        // Only ever built from a `char`.
        char::from_u32((self.0 & Self::CP_MASK) as u32).unwrap_or(' ')
    }

    /// The codepoint, which is always a valid `char`'s.
    #[inline]
    pub(super) fn codepoint(self) -> u32 {
        (self.0 & Self::CP_MASK) as u32
    }

    /// The codepoint's low byte: the character itself for an ASCII cell.
    #[inline]
    pub(super) fn ascii_byte(self) -> u8 {
        self.0 as u8
    }

    #[inline]
    pub(super) fn key(self) -> StyleKey {
        StyleKey(self.0 >> Self::KEY_SHIFT)
    }

    #[inline]
    pub(super) fn is_wide(self) -> bool {
        self.0 & Self::WIDE != 0
    }

    #[inline]
    pub(super) fn is_spacer(self) -> bool {
        self.0 & Self::SPACER != 0
    }

    /// Wide or a spacer: part of a wide character.
    #[inline]
    pub(super) fn is_wide_part(self) -> bool {
        self.0 & (Self::WIDE | Self::SPACER) != 0
    }

    #[inline]
    pub(super) fn has_grapheme(self) -> bool {
        self.0 & Self::GRAPHEME != 0
    }

    /// Whether the cell has any structure bit: wide, spacer or grapheme.
    #[inline]
    pub(super) fn has_structure(self) -> bool {
        self.0 & Self::STRUCTURE != 0
    }

    /// The structure bits as the flags they stand for.
    #[inline]
    pub(super) fn structural_flags(self) -> CellFlags {
        let mut flags = CellFlags::NONE;
        if self.is_wide() {
            flags.set(CellFlags::WIDE_CHAR);
        }
        if self.is_spacer() {
            flags.set(CellFlags::WIDE_CHAR_SPACER);
        }
        if self.has_grapheme() {
            flags.set(CellFlags::HAS_COMBINING);
        }
        flags
    }

    /// This cell with the structure bits `flags` stands for added.
    #[inline]
    pub(super) fn with_structure(self, flags: CellFlags) -> Self {
        let mut bits = self.0;
        if flags.contains(CellFlags::WIDE_CHAR) {
            bits |= Self::WIDE;
        }
        if flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            bits |= Self::SPACER;
        }
        if flags.contains(CellFlags::HAS_COMBINING) {
            bits |= Self::GRAPHEME;
        }
        Self(bits)
    }

    /// This cell made wide.
    #[inline]
    pub(super) fn widened(self) -> Self {
        Self(self.0 | Self::WIDE)
    }

    /// This cell holding `c` instead, everything else kept.
    #[inline]
    pub(super) fn with_char(self, c: char) -> Self {
        Self(self.0 & !Self::CP_MASK | u64::from(c))
    }

    #[inline]
    fn without_grapheme(self) -> Self {
        Self(self.0 & !Self::GRAPHEME)
    }
}

impl fmt::Debug for PackedCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.char())?;
        if self.has_structure() {
            write!(f, "+{:?}", self.structural_flags())?;
        }
        if !self.key().is_default() {
            write!(f, "@{:#x}", self.key().0)?;
        }
        Ok(())
    }
}

/// Slots in [`StyleTable`]'s cache of recent styles.
const RECENT: usize = 256;

/// Styles by id for the keys that are not inline; id 0 is the default.
///
/// Interning is approximate: a direct-mapped cache of recent styles finds a
/// style that is still hot, and anything else is appended under a new id
/// without looking further. An id always names one style, but a style may
/// have several ids; that only splits a uniform row or a history run where
/// it need not. Collection drops the ids no longer on screen.
#[derive(Clone)]
pub(super) struct StyleTable {
    styles: Vec<Style>,
    /// Slot to the id last interned there; 0 means empty (the default style
    /// never reaches the cache).
    recent: Box<[u32; RECENT]>,
    /// Bumped by every collection or reset, which renumber ids.
    generation: u64,
    /// The table length at which the next collection is due.
    collect_at: usize,
}

impl fmt::Debug for StyleTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyleTable")
            .field("len", &self.styles.len())
            .field("generation", &self.generation)
            .field("collect_at", &self.collect_at)
            .finish()
    }
}

/// How many new styles the table takes between collections, for a screen
/// of `cells` cells: twice the screen, and at least 4096, so a collection,
/// which scans every cell, costs a few instructions per new style.
fn collect_interval(cells: usize) -> usize {
    #[cfg(test)]
    if let Some(every) = test_support::forced_interval() {
        return every;
    }
    4096.max(2 * cells)
}

impl StyleTable {
    /// An empty table for a screen of `cells` cells.
    pub(super) fn new(cells: usize) -> Self {
        Self {
            styles: vec![Style::default()],
            recent: Box::new([0; RECENT]),
            generation: 0,
            collect_at: 1 + collect_interval(cells),
        }
    }

    /// Changes whenever ids are renumbered.
    #[inline]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.styles.len()
    }

    /// `style`'s key: inline if it fits, otherwise an id, new unless the
    /// cache still holds it. Never collects.
    #[inline]
    pub(super) fn key(&mut self, style: &Style) -> StyleKey {
        match StyleKey::inline(style) {
            Some(key) => key,
            None => self.intern(style),
        }
    }

    #[inline(never)]
    fn intern(&mut self, style: &Style) -> StyleKey {
        let slot = style.slot();
        let cached = self.recent[slot];
        if cached != 0 && self.styles[cached as usize] == *style {
            return StyleKey(u64::from(cached));
        }
        let id = self.styles.len() as u32;
        self.styles.push(*style);
        self.recent[slot] = id;
        StyleKey(u64::from(id))
    }

    /// The style `key` names.
    #[inline]
    pub(super) fn style(&self, key: StyleKey) -> Style {
        match key.table_id() {
            Some(id) => self.styles[id],
            None => key.inline_style(),
        }
    }

    /// `cell` packed, interning its style if it needs the table.
    #[inline]
    pub(super) fn pack(&mut self, cell: &Cell) -> PackedCell {
        let key = self.key(&Style::of(cell));
        PackedCell::new(cell.char(), key).with_structure(cell.flags())
    }

    /// The [`Cell`] `cell` stands for.
    #[inline]
    pub(super) fn unpack(&self, cell: PackedCell) -> Cell {
        self.style(cell.key())
            .cell(cell.char(), cell.structural_flags())
    }

    /// Whether enough styles were added since the last collection.
    #[inline]
    pub(super) fn is_due(&self) -> bool {
        self.styles.len() >= self.collect_at
    }

    /// Keep only the ids `rows` use, renumbered densely, and rewrite the
    /// rows to match. The caller must hold no other id of this table.
    pub(super) fn collect(&mut self, rows: &mut [PackedRow]) {
        let mut map = vec![0u32; self.styles.len()];
        let mut mark = |key: StyleKey| {
            if let Some(id) = key.table_id() {
                map[id] = 1;
            }
        };
        let mut cells = 0;
        for row in rows.iter() {
            cells += row.cells.len();
            if row.is_uniform() {
                mark(row.uniform_key);
            }
            for cell in row.used_cells() {
                mark(cell.key());
            }
        }
        // Renumber the survivors in place; `map` becomes old id -> new id,
        // 0 for a dropped one. Id 0, the default style, stays 0.
        map[0] = 0;
        let mut next = 1;
        for (id, new) in map.iter_mut().enumerate().skip(1) {
            if *new != 0 {
                self.styles[next] = self.styles[id];
                *new = next as u32;
                next += 1;
            }
        }
        self.styles.truncate(next);
        for slot in self.recent.iter_mut() {
            *slot = map[*slot as usize];
        }
        let remap = |key: StyleKey| match key.table_id() {
            Some(id) => StyleKey(u64::from(map[id])),
            None => key,
        };
        for row in rows.iter_mut() {
            row.uniform_key = if row.is_uniform() {
                remap(row.uniform_key)
            } else {
                StyleKey::DEFAULT
            };
            let used = usize::from(row.used);
            for cell in &mut row.cells[..used] {
                if cell.key().table_id().is_some_and(|id| id != 0) {
                    *cell = cell.with_key(remap(cell.key()));
                }
            }
        }
        self.generation += 1;
        self.collect_at = next + collect_interval(cells);
    }

    /// Forget every style, for when every row is about to be replaced.
    pub(super) fn reset(&mut self, cells: usize) {
        self.styles.truncate(1);
        self.recent.fill(0);
        self.generation += 1;
        self.collect_at = 1 + collect_interval(cells);
    }

    /// Whether `key` is one this table can hand out: an inline key in its
    /// canonical form, or the id of a style that does not fit inline.
    pub(super) fn check_key(&self, key: StyleKey) -> Result<(), String> {
        match key.table_id() {
            Some(0) => Ok(()),
            Some(id) if id >= self.styles.len() => Err(format!(
                "style id {id} past the table's {} styles",
                self.styles.len()
            )),
            Some(id) if StyleKey::inline(&self.styles[id]).is_some() => {
                Err(format!("style id {id} names a style that fits inline"))
            }
            Some(_) => Ok(()),
            None => {
                let style = key.inline_style();
                if key.0 >> StyleKey::BITS != 0
                    || key.0 & !StyleKey::INLINE & !StyleKey::INLINE_PAYLOAD != 0
                    || StyleKey::inline(&style) != Some(key)
                {
                    Err(format!("inline style key {:#x} is not canonical", key.0))
                } else {
                    Ok(())
                }
            }
        }
    }
}

impl PackedCell {
    #[inline]
    fn with_key(self, key: StyleKey) -> Self {
        Self(self.0 & !(!0 << Self::KEY_SHIFT) | key.base())
    }
}

/// Test-only control over collection, so tests can force it often.
#[cfg(test)]
pub(super) mod test_support {
    use std::cell::Cell;

    thread_local! {
        static INTERVAL: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(in crate::grid) fn forced_interval() -> Option<usize> {
        INTERVAL.with(Cell::get)
    }

    /// While the guard lives, style tables created on this thread collect
    /// after every `every` new styles.
    pub(in crate::grid) fn collect_every(every: usize) -> Guard {
        INTERVAL.with(|interval| interval.set(Some(every.max(1))));
        Guard
    }

    pub(in crate::grid) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            INTERVAL.with(|interval| interval.set(None));
        }
    }
}

/// A screen row of packed cells, in the grid that owns its style table.
///
/// It keeps what [`Row`] keeps, with `uniform` in place of `plain`: every
/// cell before `used` is ASCII with no structure bits, in style
/// `uniform_key`. Like `used`, it may understate (anything writing a cell
/// directly clears it) but never overstates, so history can copy such a
/// row's text and style it once.
#[derive(Debug, Clone)]
pub(super) struct PackedRow {
    /// A boxed slice rather than a `Vec`: a row's width only changes on a
    /// resize, and the 8 bytes saved are moved on every scroll.
    cells: Box<[PackedCell]>,
    /// Extra grapheme codepoints by column; an entry counts only while its
    /// cell has the grapheme bit (see [`Row`]'s field of the same name).
    graphemes: GraphemeMap,
    /// The style of a uniform row; meaningless otherwise.
    uniform_key: StyleKey,
    /// Every cell at or after this column is [`PackedCell::BLANK`]. May
    /// overstate, never understates.
    used: u16,
    flags: u8,
    marks: RowMarks,
    anchor: AnchorTag,
}

impl PackedRow {
    const WRAPPED: u8 = 1 << 0;
    const UNIFORM: u8 = 1 << 1;

    pub(super) fn new(width: u16) -> Self {
        Self {
            cells: vec![PackedCell::BLANK; usize::from(width)].into_boxed_slice(),
            graphemes: GraphemeMap::default(),
            uniform_key: StyleKey::DEFAULT,
            used: 0,
            flags: Self::UNIFORM,
            marks: RowMarks::default(),
            anchor: AnchorTag::NONE,
        }
    }

    #[inline]
    fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    #[inline]
    pub(super) fn width(&self) -> u16 {
        self.cells.len() as u16
    }

    #[inline]
    pub(super) fn cells(&self) -> &[PackedCell] {
        &self.cells
    }

    #[inline]
    pub(super) fn get(&self, col: u16) -> Option<PackedCell> {
        self.cells.get(usize::from(col)).copied()
    }

    /// The cells up to the used mark: every cell after them is blank.
    #[inline]
    pub(super) fn used_cells(&self) -> &[PackedCell] {
        &self.cells[..usize::from(self.used)]
    }

    /// Whether every used cell is ASCII with no structure in
    /// [`PackedRow::uniform_key`]. `false` does not mean some cell is not.
    #[inline]
    pub(super) fn is_uniform(&self) -> bool {
        self.flags & Self::UNIFORM != 0
    }

    #[inline]
    pub(super) fn uniform_key(&self) -> StyleKey {
        self.uniform_key
    }

    #[inline]
    pub(super) fn is_wrapped(&self) -> bool {
        self.flags & Self::WRAPPED != 0
    }

    #[inline]
    pub(super) fn set_wrapped(&mut self, wrapped: bool) {
        self.set_flag(Self::WRAPPED, wrapped);
    }

    #[inline]
    pub(super) fn marks(&self) -> RowMarks {
        self.marks
    }

    #[inline]
    pub(super) fn set_marks(&mut self, marks: RowMarks) {
        self.marks = marks;
    }

    #[inline]
    pub(super) fn anchor(&self) -> AnchorTag {
        self.anchor
    }

    #[inline]
    pub(super) fn set_anchor(&mut self, anchor: AnchorTag) {
        self.anchor = anchor;
    }

    /// Record that cells before `end` may no longer be blank.
    #[inline]
    fn mark_used(&mut self, end: usize) {
        let end = end.min(self.cells.len()) as u16;
        self.used = self.used.max(end);
    }

    /// Set the cell at `col`, if there is one, as a direct edit: the row
    /// is no longer known to be uniform.
    #[inline]
    pub(super) fn set(&mut self, col: u16, cell: PackedCell) {
        if let Some(slot) = self.cells.get_mut(usize::from(col)) {
            *slot = cell;
            self.flags &= !Self::UNIFORM;
            self.mark_used(usize::from(col) + 1);
        }
    }

    /// Write printable ASCII `bytes` from `col` in style `key`. The caller
    /// has resolved any wide character straddling the span's edges (see
    /// [`PackedRow::clear_ascii_edges`]); the span must fit on the row.
    #[inline]
    pub(super) fn write_ascii(&mut self, col: u16, bytes: &[u8], key: StyleKey) {
        let start = usize::from(col);
        let end = start + bytes.len();
        self.drop_grapheme_entries(col, end as u16);
        let used = usize::from(self.used);
        if start == 0 && end >= used {
            // Every used cell is about to be in `key`.
            self.flags |= Self::UNIFORM;
            self.uniform_key = key;
        } else if self.uniform_key != key || (start > used && !key.is_default()) {
            // Cells in another style before or after the span, or blanks
            // between the used mark and the span.
            self.flags &= !Self::UNIFORM;
        }
        let base = key.base();
        for (cell, &byte) in self.cells[start..end].iter_mut().zip(bytes) {
            *cell = PackedCell(base | u64::from(byte));
        }
        self.used = self.used.max(end as u16);
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
        self.flags &= !Self::UNIFORM;
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

    /// The full grapheme at `col`: base character plus any extra codepoints.
    pub(super) fn grapheme(&self, col: u16) -> Option<String> {
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
        &self,
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

    /// Append a codepoint to the grapheme cluster at `col`. At most `cap`
    /// extra codepoints are kept per cell. Returns whether `c` was kept.
    pub(super) fn push_grapheme_char(&mut self, col: u16, c: char, cap: usize) -> bool {
        let Some(cell) = self.cells.get_mut(usize::from(col)) else {
            return false;
        };
        self.flags &= !Self::UNIFORM;
        let extra = self.graphemes.entry(col);
        if !cell.has_grapheme() {
            // Whatever is stored for this column belonged to earlier content.
            extra.clear();
            cell.0 |= PackedCell::GRAPHEME;
            self.used = self.used.max(col + 1);
        }
        let kept = extra.len() < cap;
        if kept {
            extra.push(c);
        }
        kept
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
        if start < end && self.graphemes.any_in(start, end) {
            self.graphemes.remove_range(start, end);
        }
    }

    /// Erase the row per ECMA-48 with blanks in style `key` (the erase
    /// style, background only): no grapheme data, marks, anchor or wrap.
    #[inline]
    pub(super) fn erase(&mut self, key: StyleKey) {
        if key.is_default() {
            // Cells past the used mark are already blank.
            self.cells[..usize::from(self.used)].fill(PackedCell::BLANK);
            self.used = 0;
        } else {
            self.cells.fill(PackedCell::blank(key));
            self.used = self.width();
        }
        self.uniform_key = key;
        self.graphemes.clear();
        self.marks = RowMarks::default();
        self.anchor = AnchorTag::NONE;
        // Uniform and no longer wrapped, in one store: every scroll erases
        // a row.
        self.flags = Self::UNIFORM;
    }

    /// Erase cells `start..end` per ECMA-48: blanks in style `key`, with no
    /// grapheme data.
    pub(super) fn erase_range(&mut self, start: u16, end: u16, key: StyleKey) {
        let end = end.min(self.width());
        if start >= end {
            return;
        }
        self.drop_grapheme_entries(start, end);
        if key.is_default() {
            // Cells past the used mark are already blank.
            let fill_end = end.min(self.used);
            if start < fill_end {
                self.cells[usize::from(start)..usize::from(fill_end)].fill(PackedCell::BLANK);
            }
            if end >= self.used {
                self.used = self.used.min(start);
            } else if !self.uniform_key.is_default() {
                // Default blanks inside a row of another style.
                self.flags &= !Self::UNIFORM;
            }
        } else {
            if start == 0 && end >= self.used {
                // Every used cell is about to be a blank in `key`; the
                // grapheme entries in range are gone, and no cell past
                // `end` is used, so no wide or grapheme state remains.
                self.flags |= Self::UNIFORM;
                self.uniform_key = key;
            } else if self.uniform_key != key || start > self.used {
                self.flags &= !Self::UNIFORM;
            }
            self.cells[usize::from(start)..usize::from(end)].fill(PackedCell::blank(key));
            self.mark_used(usize::from(end));
        }
    }

    /// Fill the row with `c` in the default style, as DECALN does.
    pub(super) fn fill_with_char(&mut self, c: char) {
        self.erase(StyleKey::DEFAULT);
        self.cells.fill(PackedCell::new(c, StyleKey::DEFAULT));
        self.used = self.width();
        self.flags &= !Self::UNIFORM;
    }

    /// Resize the row, dropping or adding blank cells at the end.
    pub(super) fn resize(&mut self, width: u16) {
        let mut cells = std::mem::take(&mut self.cells).into_vec();
        cells.resize(usize::from(width), PackedCell::BLANK);
        self.cells = cells.into_boxed_slice();
        self.used = self.used.min(width);
        self.graphemes.truncate(width);
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
        self.graphemes.shift(at as u16, count as i32, self.width());
        self.used = self.width();
        self.flags &= !Self::UNIFORM;
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
        self.used = self.width();
        self.flags &= !Self::UNIFORM;
        self.graphemes
            .shift(src_start as u16, -(count as i32), self.width());

        // A count running past the end of the row vacates only cells from
        // `at` onward.
        self.cells[len.saturating_sub(count).max(at)..len].fill(PackedCell::blank(key));
    }

    /// Make this row hold `row`'s cells, graphemes, wrap, marks and anchor,
    /// interning its styles in `styles` (which never collects).
    pub(super) fn assign_row(&mut self, row: &Row, styles: &mut StyleTable) {
        if self.cells.len() == usize::from(row.width()) {
            for (packed, cell) in self.cells.iter_mut().zip(row.cells()) {
                *packed = styles.pack(cell);
            }
        } else {
            self.cells = row.cells().map(|cell| styles.pack(cell)).collect();
        }
        self.graphemes.clear();
        for col in 0..row.width() {
            if self.cells[usize::from(col)].has_grapheme() {
                if let Some(extra) = row.grapheme_extra(col) {
                    self.graphemes.insert(col, extra.to_vec());
                }
            }
        }
        self.used = self
            .cells
            .iter()
            .rposition(|&cell| cell != PackedCell::BLANK)
            .map_or(0, |last| last as u16 + 1);
        let used = self.used_cells();
        let key = used.first().map_or(StyleKey::DEFAULT, |cell| cell.key());
        let uniform = used
            .iter()
            .all(|cell| cell.0 & PackedCell::NOT_UNIFORM == 0 && cell.key() == key);
        self.uniform_key = key;
        self.flags = if uniform { Self::UNIFORM } else { 0 };
        self.set_wrapped(row.is_wrapped());
        self.marks = row.marks();
        self.anchor = row.anchor();
    }

    /// Check the row's invariants: those of [`Row::check_integrity`], plus
    /// a valid style key in every cell, a uniform row that is uniform, and
    /// grapheme data behind every grapheme bit.
    pub(super) fn check_integrity(
        &self,
        max_extra: usize,
        styles: &StyleTable,
    ) -> Result<(), String> {
        let cells = &self.cells;
        let used = usize::from(self.used).min(cells.len());
        if usize::from(self.used) > cells.len() {
            return Err(format!(
                "used mark {} is past the row's {} cells",
                self.used,
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
                .check_key(self.uniform_key)
                .map_err(|e| format!("uniform style: {e}"))?;
            if let Some(col) = cells[..used].iter().position(|cell| {
                cell.0 & PackedCell::NOT_UNIFORM != 0 || cell.key() != self.uniform_key
            }) {
                return Err(format!(
                    "row is marked uniform but cell at col {col} ({:?}) is not",
                    cells[col]
                ));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::cell::UnderlineStyle;
    use proptest::prelude::*;

    fn color() -> impl Strategy<Value = Color> {
        prop_oneof![
            Just(Color::Default),
            any::<u8>().prop_map(Color::Named),
            any::<u8>().prop_map(Color::Indexed),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::rgb(r, g, b)),
        ]
    }

    /// Any cell at all: every flag bit, any character, any colors.
    fn any_cell() -> impl Strategy<Value = Cell> {
        (
            any::<char>(),
            color(),
            color(),
            color(),
            any::<u16>(),
            prop_oneof![Just(0u32), Just(1), any::<u32>()],
        )
            .prop_map(|(c, fg, bg, underline, flags, link)| {
                let mut cell = Cell::with_attrs(c, fg, bg, CellFlags::from_bits(flags));
                cell.set_underline_color(underline);
                // Set the id without touching the HYPERLINK flag, which the
                // flag bits above already chose.
                let flags = cell.flags();
                cell.set_hyperlink_id(link);
                *cell.flags_mut() = flags;
                cell
            })
    }

    proptest! {
        /// Packing is lossless for every cell, whether its style goes
        /// inline or into the table.
        #[test]
        fn every_cell_round_trips(cells in prop::collection::vec(any_cell(), 1..40)) {
            let mut table = StyleTable::new(0);
            let packed: Vec<_> = cells.iter().map(|cell| table.pack(cell)).collect();
            for (cell, &packed) in cells.iter().zip(&packed) {
                prop_assert_eq!(table.unpack(packed), *cell);
                prop_assert!(table.check_key(packed.key()).is_ok());
            }
        }
    }

    #[test]
    fn styles_that_fit_go_inline_and_the_rest_to_the_table() {
        let mut table = StyleTable::new(0);
        let mut bold = Cell::new('x');
        bold.set_flag(CellFlags::BOLD);
        bold.set_fg(Color::Indexed(200));
        bold.set_underline_style(UnderlineStyle::Curly);
        assert!(table.pack(&bold).key().table_id().is_none());
        assert_eq!(table.len(), 1);

        let mut rgb = Cell::new('x');
        rgb.set_bg(Color::rgb(1, 2, 3));
        let key = table.pack(&rgb).key();
        assert_eq!(key.table_id(), Some(1));
        // A hot style is found again rather than added.
        assert_eq!(table.pack(&rgb).key(), key);
        assert_eq!(table.len(), 2);

        assert_eq!(table.pack(&Cell::default()), PackedCell::BLANK);
    }

    #[test]
    fn inline_keys_round_trip_and_are_distinct() {
        let colors = [PackedColor::DEFAULT]
            .into_iter()
            .chain((0..=255).map(|n| PackedColor::pack(Color::Named(n))))
            .chain((0..=255).map(|n| PackedColor::pack(Color::Indexed(n))));
        let mut keys = std::collections::HashSet::new();
        for (i, fg) in colors.enumerate() {
            let style = Style {
                fg,
                bg: PackedColor::pack(Color::Indexed((i * 7 % 256) as u8)),
                flags: CellFlags::from_bits((i as u16).wrapping_mul(0x1F3) & !STRUCTURAL.bits()),
                ..Style::default()
            };
            let key = StyleKey::inline(&style).expect("no truecolor, so it fits inline");
            assert_eq!(key.inline_style(), style);
            assert!(keys.insert(key), "two styles share key {key:?}");
        }
        assert_eq!(StyleKey::inline(&Style::default()), Some(StyleKey::DEFAULT));
    }

    /// Rows of table styles, some on screen and some gone.
    fn truecolor_rows(table: &mut StyleTable, rows: usize, width: u16, blue: u8) -> Vec<PackedRow> {
        (0..rows)
            .map(|r| {
                let mut row = PackedRow::new(width);
                for col in 0..width {
                    let mut cell = Cell::new('a');
                    cell.set_fg(Color::rgb(r as u8, col as u8, blue));
                    row.set(col, table.pack(&cell));
                }
                row
            })
            .collect()
    }

    #[test]
    fn collection_keeps_what_is_on_screen_and_drops_the_rest() {
        let mut table = StyleTable::new(0);
        let mut rows = truecolor_rows(&mut table, 4, 6, 1);
        let before: Vec<Vec<Cell>> = rows
            .iter()
            .map(|row| row.cells().iter().map(|&c| table.unpack(c)).collect())
            .collect();
        // Styles no row uses any more.
        truecolor_rows(&mut table, 3, 6, 2);
        assert_eq!(table.len(), 1 + 7 * 6);
        let generation = table.generation();

        table.collect(&mut rows);

        assert_eq!(table.len(), 1 + 4 * 6);
        assert_ne!(table.generation(), generation);
        for (row, cells) in rows.iter().zip(&before) {
            row.check_integrity(16, &table).unwrap();
            let after: Vec<Cell> = row.cells().iter().map(|&c| table.unpack(c)).collect();
            assert_eq!(&after, cells);
        }
    }

    #[test]
    fn collection_renumbers_the_uniform_style() {
        let mut table = StyleTable::new(0);
        let mut gone = Cell::new('x');
        gone.set_fg(Color::rgb(9, 9, 9));
        table.pack(&gone);
        let mut kept = Cell::new('x');
        kept.set_bg(Color::rgb(1, 1, 1));
        let key = table.pack(&kept).key();
        let mut rows = vec![PackedRow::new(4), PackedRow::new(4)];
        rows[0].write_ascii(0, b"abc", key);
        assert!(rows[0].is_uniform());

        table.collect(&mut rows);

        assert!(rows[0].is_uniform());
        assert_eq!(rows[0].uniform_key().table_id(), Some(1));
        rows[0].check_integrity(16, &table).unwrap();
        assert_eq!(
            rows[1].uniform_key(),
            StyleKey::DEFAULT,
            "the default style stays id 0"
        );
        assert_eq!(
            table.unpack(rows[0].cells()[0]),
            kept.with_char('a'),
            "the cells and the uniform style moved together"
        );
    }

    #[test]
    fn a_row_is_uniform_until_something_else_lands() {
        let key = StyleKey::inline(&Style {
            fg: PackedColor::pack(Color::Named(2)),
            ..Style::default()
        })
        .unwrap();
        let mut row = PackedRow::new(10);
        row.write_ascii(0, b"abc", key);
        row.write_ascii(3, b"de", key);
        assert!(row.is_uniform());
        row.erase_range(4, 10, StyleKey::DEFAULT);
        assert!(row.is_uniform(), "erasing the tail keeps it");
        row.write_ascii(6, b"x", key);
        assert!(
            !row.is_uniform(),
            "blanks before the span are another style"
        );

        row.write_ascii(0, b"0123456789", StyleKey::DEFAULT);
        assert!(
            row.is_uniform(),
            "rewriting every used cell makes it uniform"
        );
        row.write_ascii(2, b"q", key);
        assert!(!row.is_uniform());

        row.erase(key);
        assert!(row.is_uniform());
        assert_eq!(row.uniform_key(), key);
        assert!(row.write_char_in_place(0, 'z', 1, key));
        assert!(!row.is_uniform(), "any other write gives up on it");
    }

    #[test]
    fn insert_at_a_spacer_clears_both_halves() {
        let mut row = PackedRow::new(6);
        row.write_ascii(0, b"ab", StyleKey::DEFAULT);
        assert!(row.write_char_in_place(2, '中', 2, StyleKey::DEFAULT));
        row.insert_cells(3, 1, StyleKey::DEFAULT);
        let table = StyleTable::new(0);
        row.check_integrity(16, &table).unwrap();
        assert_eq!(row.get(2), Some(PackedCell::BLANK));
    }

    #[test]
    fn insert_that_pushes_off_a_spacer_clears_its_wide_char() {
        let mut row = PackedRow::new(4);
        row.write_ascii(0, b"ab", StyleKey::DEFAULT);
        assert!(row.write_char_in_place(2, '中', 2, StyleKey::DEFAULT));
        row.insert_cells(0, 1, StyleKey::DEFAULT);
        row.check_integrity(16, &StyleTable::new(0)).unwrap();
        assert_eq!(row.get(3), Some(PackedCell::BLANK));
    }

    #[test]
    fn delete_from_a_spacer_or_into_a_wide_char_clears_the_orphan() {
        let table = StyleTable::new(0);
        let mut row = PackedRow::new(6);
        assert!(row.write_char_in_place(0, '中', 2, StyleKey::DEFAULT));
        assert!(row.write_char_in_place(2, '文', 2, StyleKey::DEFAULT));
        row.delete_cells(1, 2, StyleKey::DEFAULT);
        row.check_integrity(16, &table).unwrap();
        assert!(row.cells().iter().all(|cell| !cell.is_wide_part()));
    }

    #[test]
    fn integrity_catches_a_bad_key_and_a_bare_grapheme_bit() {
        let table = StyleTable::new(0);
        let mut row = PackedRow::new(4);
        row.set(0, PackedCell::new('a', StyleKey(5)));
        assert!(
            row.check_integrity(16, &table).is_err(),
            "id past the table"
        );

        let mut row = PackedRow::new(4);
        row.set(
            0,
            PackedCell::new('a', StyleKey::DEFAULT).with_structure(CellFlags::HAS_COMBINING),
        );
        assert!(
            row.check_integrity(16, &table).is_err(),
            "bit with no extras"
        );
        row.cells[0] = PackedCell::new('a', StyleKey::DEFAULT);
        row.push_grapheme_char(0, '\u{301}', 16);
        row.check_integrity(16, &table).unwrap();

        let mut row = PackedRow::new(4);
        row.write_ascii(0, b"ab", StyleKey::DEFAULT);
        row.cells[1] = PackedCell::new('é', StyleKey::DEFAULT);
        assert!(
            row.check_integrity(16, &table).is_err(),
            "uniform but not ASCII"
        );
    }

    #[test]
    fn grapheme_str_is_the_cluster_then_the_extra_char() {
        let mut row = PackedRow::new(4);
        row.set(1, PackedCell::new('e', StyleKey::DEFAULT));
        row.push_grapheme_char(1, '\u{301}', 16);
        let got = row.with_grapheme_str(1, Some('\u{302}'), str::to_string);
        assert_eq!(got.as_deref(), Some("e\u{301}\u{302}"));
        assert_eq!(
            row.with_grapheme_str(0, None, str::to_string).as_deref(),
            Some(" ")
        );
        assert_eq!(row.with_grapheme_str(4, None, str::to_string), None);

        // Longer than the stack buffer.
        let long = ['\u{1F600}'; 30];
        for c in long {
            row.push_grapheme_char(2, c, long.len());
        }
        let expected: String = std::iter::once(' ').chain(long).chain(['x']).collect();
        let got = row.with_grapheme_str(2, Some('x'), str::to_string);
        assert_eq!(got.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn erasing_a_blank_row_from_column_zero_leaves_it_uniform() {
        let mut table = StyleTable::new(0);
        let key = table.key(&Style::erased(Color::Indexed(4)));
        assert!(!key.is_default());
        for end in [3, 6] {
            let mut row = PackedRow::new(6);
            row.erase_range(0, end, key);
            assert!(row.is_uniform(), "erase to {end}");
            assert_eq!(row.uniform_key(), key);
            row.check_integrity(16, &table).unwrap();
        }
    }

    #[test]
    fn erasing_part_of_a_styled_row_is_not_uniform() {
        let mut table = StyleTable::new(0);
        let key = table.key(&Style::erased(Color::Indexed(4)));
        let mut row = PackedRow::new(6);
        row.set(4, PackedCell::new('x', StyleKey::DEFAULT));
        row.erase_range(0, 2, key);
        row.check_integrity(16, &table).unwrap();
        assert!(!row.is_uniform());
    }

    #[test]
    fn packed_rows_stay_small() {
        // Every scroll moves the region's rows by value, so its cost grows
        // with a row's size.
        assert_eq!(std::mem::size_of::<PackedRow>(), 40);
        assert_eq!(std::mem::size_of::<PackedCell>(), 8);
    }
}
