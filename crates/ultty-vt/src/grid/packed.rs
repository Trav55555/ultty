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
use super::screen::ScreenPage;

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

    /// The key with these raw bits, for tests that need a bad one.
    #[cfg(test)]
    pub(super) fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// The key in a packed cell's position.
    #[inline]
    pub(super) fn base(self) -> u64 {
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

    /// The ASCII character `byte` in the style whose
    /// [`StyleKey::base`] is `base`.
    #[inline]
    pub(super) fn ascii(base: u64, byte: u8) -> Self {
        Self(base | u64::from(byte))
    }

    /// Whether the cell can be part of a uniform row: ASCII with no
    /// structure bits.
    #[inline]
    pub(super) fn is_uniform_ascii(self) -> bool {
        self.0 & Self::NOT_UNIFORM == 0
    }

    #[inline]
    pub(super) fn with_grapheme(self) -> Self {
        Self(self.0 | Self::GRAPHEME)
    }

    #[inline]
    pub(super) fn without_grapheme(self) -> Self {
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

    /// Keep only the ids `page` uses, renumbered densely, and rewrite the
    /// page to match. The caller must hold no other id of this table.
    pub(super) fn collect(&mut self, page: &mut ScreenPage) {
        let mut map = vec![0u32; self.styles.len()];
        page.for_each_key(|key| {
            if let Some(id) = key.table_id() {
                map[id] = 1;
            }
        });
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
        page.remap_keys(|key| match key.table_id() {
            Some(id) => StyleKey(u64::from(map[id])),
            None => key,
        });
        self.generation += 1;
        self.collect_at = next + collect_interval(page.cell_count());
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
    /// This cell in style `key` instead.
    #[inline]
    pub(super) fn with_key(self, key: StyleKey) -> Self {
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

    /// A page of table styles, some on screen and some gone.
    fn truecolor_page(table: &mut StyleTable, rows: u16, width: u16, blue: u8) -> ScreenPage {
        let mut page = ScreenPage::new(width, rows);
        for r in 0..rows {
            let mut row = page.row_mut(r).unwrap();
            for col in 0..width {
                let mut cell = Cell::new('a');
                cell.set_fg(Color::rgb(r as u8, col as u8, blue));
                row.set(col, table.pack(&cell));
            }
        }
        page
    }

    fn unpacked(page: &ScreenPage, table: &StyleTable) -> Vec<Vec<Cell>> {
        page.iter()
            .map(|row| row.cells().iter().map(|&c| table.unpack(c)).collect())
            .collect()
    }

    #[test]
    fn collection_keeps_what_is_on_screen_and_drops_the_rest() {
        let mut table = StyleTable::new(0);
        let mut page = truecolor_page(&mut table, 4, 6, 1);
        let before = unpacked(&page, &table);
        // Styles no row uses any more.
        truecolor_page(&mut table, 3, 6, 2);
        assert_eq!(table.len(), 1 + 7 * 6);
        let generation = table.generation();

        table.collect(&mut page);

        assert_eq!(table.len(), 1 + 4 * 6);
        assert_ne!(table.generation(), generation);
        page.check_integrity(16, &table).unwrap();
        assert_eq!(unpacked(&page, &table), before);
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
        let mut page = ScreenPage::new(4, 2);
        page.row_mut(0).unwrap().write_ascii(0, b"abc", key);
        assert!(page.row(0).unwrap().is_uniform());

        table.collect(&mut page);

        let row = page.row(0).unwrap();
        assert!(row.is_uniform());
        assert_eq!(row.uniform_key().table_id(), Some(1));
        page.check_integrity(16, &table).unwrap();
        assert_eq!(
            page.row(1).unwrap().uniform_key(),
            StyleKey::DEFAULT,
            "the default style stays id 0"
        );
        assert_eq!(
            table.unpack(page.row(0).unwrap().cells()[0]),
            kept.with_char('a'),
            "the cells and the uniform style moved together"
        );
    }

    #[test]
    fn packed_cells_stay_small() {
        assert_eq!(std::mem::size_of::<PackedCell>(), 8);
    }
}
