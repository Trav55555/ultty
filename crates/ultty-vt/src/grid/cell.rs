//! Terminal cell representation.

use std::fmt;

/// Flags for cell rendering attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const NONE: CellFlags = CellFlags(0);
    pub const BOLD: CellFlags = CellFlags(1 << 0);
    pub const DIM: CellFlags = CellFlags(1 << 1);
    pub const ITALIC: CellFlags = CellFlags(1 << 2);
    pub const UNDERLINE: CellFlags = CellFlags(1 << 3);
    pub const BLINK: CellFlags = CellFlags(1 << 4);
    pub const REVERSE: CellFlags = CellFlags(1 << 5);
    pub const HIDDEN: CellFlags = CellFlags(1 << 6);
    pub const STRIKETHROUGH: CellFlags = CellFlags(1 << 7);
    pub const WIDE_CHAR: CellFlags = CellFlags(1 << 8);
    pub const WIDE_CHAR_SPACER: CellFlags = CellFlags(1 << 9);
    pub const HYPERLINK: CellFlags = CellFlags(1 << 10);
    /// Cell has combining characters stored in the grid's combining map
    pub const HAS_COMBINING: CellFlags = CellFlags(1 << 11);
    pub const DOUBLE_UNDERLINE: CellFlags = CellFlags(1 << 12);
    pub const OVERLINE: CellFlags = CellFlags(1 << 13);

    /// Bits 14-15: the shape of a decorated underline (0 straight,
    /// 1 curly, 2 dotted, 3 dashed). Only meaningful with
    /// [`CellFlags::UNDERLINE`]; read it through [`Cell::underline_style`].
    const UNDERLINE_SHAPE_SHIFT: u16 = 14;
    const UNDERLINE_SHAPE_MASK: u16 = 0b11 << Self::UNDERLINE_SHAPE_SHIFT;

    #[inline]
    pub fn contains(self, other: CellFlags) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub fn set(&mut self, flag: CellFlags) {
        self.0 |= flag.0;
    }

    #[inline]
    pub fn unset(&mut self, flag: CellFlags) {
        self.0 &= !flag.0;
    }

    #[inline]
    pub fn toggle(&mut self, flag: CellFlags) {
        self.0 ^= flag.0;
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The raw bits, for the page-storage prototype's style hashing.
    #[cfg(feature = "page-proto")]
    pub(crate) fn bits(self) -> u16 {
        self.0
    }

    /// The flags set in both.
    #[inline]
    pub(crate) fn intersection(self, other: CellFlags) -> CellFlags {
        CellFlags(self.0 & other.0)
    }

    /// The flags set in either; `|` for constants.
    pub(crate) const fn union(self, other: CellFlags) -> CellFlags {
        CellFlags(self.0 | other.0)
    }
}

impl std::ops::BitOr for CellFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        CellFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for CellFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// How a cell's underline is drawn (SGR 4, 4:0-4:5, 21, 24).
///
/// Stored in the cell's flags: [`CellFlags::UNDERLINE`] marks any underline,
/// [`CellFlags::DOUBLE_UNDERLINE`] the double one, and two further bits the
/// curly, dotted and dashed shapes. A renderer that knows only `UNDERLINE`
/// therefore draws the newer shapes as a single line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Color representation for terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// Default terminal color
    #[default]
    Default,
    /// Named color from the 16-color palette (0-15)
    Named(u8),
    /// 256-color palette index
    Indexed(u8),
    /// True color RGB
    Rgb(u8, u8, u8),
}

impl Color {
    // Named color constants
    pub const BLACK: Color = Color::Named(0);
    pub const RED: Color = Color::Named(1);
    pub const GREEN: Color = Color::Named(2);
    pub const YELLOW: Color = Color::Named(3);
    pub const BLUE: Color = Color::Named(4);
    pub const MAGENTA: Color = Color::Named(5);
    pub const CYAN: Color = Color::Named(6);
    pub const WHITE: Color = Color::Named(7);

    pub const BRIGHT_BLACK: Color = Color::Named(8);
    pub const BRIGHT_RED: Color = Color::Named(9);
    pub const BRIGHT_GREEN: Color = Color::Named(10);
    pub const BRIGHT_YELLOW: Color = Color::Named(11);
    pub const BRIGHT_BLUE: Color = Color::Named(12);
    pub const BRIGHT_MAGENTA: Color = Color::Named(13);
    pub const BRIGHT_CYAN: Color = Color::Named(14);
    pub const BRIGHT_WHITE: Color = Color::Named(15);

    /// Create an RGB color.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgb(r, g, b)
    }

    /// Create a 256-color palette color.
    pub fn indexed(index: u8) -> Self {
        Color::Indexed(index)
    }
}

/// A single terminal cell.
///
/// Contains a character, foreground/background colors, and style flags.
/// Designed for efficient storage and copying.
/// A [`Color`] packed into one `u32`: a tag in the top byte (0 default,
/// 1 named, 2 indexed, 3 RGB) and the payload below it.
///
/// Cells store colors this way so comparing two cells, or a cell's style
/// against a run's, is integer comparison rather than an enum match per
/// color. Encoding scrollback compares every cell's style, and with the
/// enum that comparison was most of the cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct PackedColor(u32);

impl PackedColor {
    pub(crate) const DEFAULT: Self = Self(0);

    /// The packed value, for the page-storage prototype's style hashing.
    #[cfg(feature = "page-proto")]
    pub(crate) fn bits(self) -> u32 {
        self.0
    }

    pub(crate) const fn pack(color: Color) -> Self {
        Self(match color {
            Color::Default => 0,
            Color::Named(n) => (1 << 24) | n as u32,
            Color::Indexed(n) => (2 << 24) | n as u32,
            Color::Rgb(r, g, b) => (3 << 24) | (r as u32) << 16 | (g as u32) << 8 | b as u32,
        })
    }

    pub(crate) const fn unpack(self) -> Color {
        let v = self.0;
        match v >> 24 {
            1 => Color::Named(v as u8),
            2 => Color::Indexed(v as u8),
            3 => Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
            _ => Color::Default,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The character in this cell (could be multi-byte for emoji)
    c: char,
    /// Foreground color
    fg: PackedColor,
    /// Background color
    bg: PackedColor,
    /// Style flags
    flags: CellFlags,
    /// Hyperlink ID (0 = no hyperlink, non-zero indexes into terminal's hyperlink map)
    hyperlink_id: u32,
    /// Underline color (SGR 58); default means the foreground color.
    underline_color: PackedColor,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: PackedColor::DEFAULT,
            bg: PackedColor::DEFAULT,
            flags: CellFlags::NONE,
            hyperlink_id: 0,
            underline_color: PackedColor::DEFAULT,
        }
    }
}

impl Cell {
    /// Create a new cell with the given character.
    pub fn new(c: char) -> Self {
        Self {
            c,
            ..Default::default()
        }
    }

    /// Create a cell with all attributes specified.
    pub fn with_attrs(c: char, fg: Color, bg: Color, flags: CellFlags) -> Self {
        Self {
            c,
            fg: PackedColor::pack(fg),
            bg: PackedColor::pack(bg),
            flags,
            ..Self::default()
        }
    }

    /// A cell built from packed colors (see [`PackedColor`]).
    pub(crate) const fn from_packed(
        c: char,
        fg: PackedColor,
        bg: PackedColor,
        underline_color: PackedColor,
        flags: CellFlags,
        hyperlink_id: u32,
    ) -> Self {
        Self {
            c,
            fg,
            bg,
            flags,
            hyperlink_id,
            underline_color,
        }
    }

    /// The packed underline color.
    #[inline]
    pub(crate) fn packed_underline_color(&self) -> PackedColor {
        self.underline_color
    }

    /// The packed foreground color.
    #[inline]
    pub(crate) fn packed_fg(&self) -> PackedColor {
        self.fg
    }

    /// The packed background color.
    #[inline]
    pub(crate) fn packed_bg(&self) -> PackedColor {
        self.bg
    }

    /// Get the character.
    #[inline]
    pub fn char(&self) -> char {
        self.c
    }

    /// This cell with its character replaced by `c`.
    #[inline]
    pub(crate) fn with_char(self, c: char) -> Self {
        Self { c, ..self }
    }

    /// Set the character.
    #[inline]
    pub fn set_char(&mut self, c: char) {
        self.c = c;
    }

    /// Get the foreground color.
    #[inline]
    pub fn fg(&self) -> Color {
        self.fg.unpack()
    }

    /// Set the foreground color.
    #[inline]
    pub fn set_fg(&mut self, color: Color) {
        self.fg = PackedColor::pack(color);
    }

    /// Get the background color.
    #[inline]
    pub fn bg(&self) -> Color {
        self.bg.unpack()
    }

    /// Set the background color.
    #[inline]
    pub fn set_bg(&mut self, color: Color) {
        self.bg = PackedColor::pack(color);
    }

    /// The underline color (SGR 58); [`Color::Default`] means "use the
    /// foreground color".
    #[inline]
    pub fn underline_color(&self) -> Color {
        self.underline_color.unpack()
    }

    /// Set the underline color.
    #[inline]
    pub fn set_underline_color(&mut self, color: Color) {
        self.underline_color = PackedColor::pack(color);
    }

    /// How this cell's underline is drawn.
    pub fn underline_style(&self) -> UnderlineStyle {
        if !self.flags.contains(CellFlags::UNDERLINE) {
            return UnderlineStyle::None;
        }
        match (self.flags.0 & CellFlags::UNDERLINE_SHAPE_MASK) >> CellFlags::UNDERLINE_SHAPE_SHIFT {
            1 => UnderlineStyle::Curly,
            2 => UnderlineStyle::Dotted,
            3 => UnderlineStyle::Dashed,
            _ if self.flags.contains(CellFlags::DOUBLE_UNDERLINE) => UnderlineStyle::Double,
            _ => UnderlineStyle::Single,
        }
    }

    /// Set how this cell's underline is drawn; `None` removes it.
    pub fn set_underline_style(&mut self, style: UnderlineStyle) {
        self.flags.unset(CellFlags::UNDERLINE);
        self.flags.unset(CellFlags::DOUBLE_UNDERLINE);
        self.flags.0 &= !CellFlags::UNDERLINE_SHAPE_MASK;
        let shape = match style {
            UnderlineStyle::None => return,
            UnderlineStyle::Single => 0,
            UnderlineStyle::Double => {
                self.flags.set(CellFlags::DOUBLE_UNDERLINE);
                0
            }
            UnderlineStyle::Curly => 1,
            UnderlineStyle::Dotted => 2,
            UnderlineStyle::Dashed => 3,
        };
        self.flags.set(CellFlags::UNDERLINE);
        self.flags.0 |= shape << CellFlags::UNDERLINE_SHAPE_SHIFT;
    }

    /// Get the flags.
    #[inline]
    pub fn flags(&self) -> CellFlags {
        self.flags
    }

    /// Get mutable access to flags.
    #[inline]
    pub fn flags_mut(&mut self) -> &mut CellFlags {
        &mut self.flags
    }

    /// Set a flag.
    #[inline]
    pub fn set_flag(&mut self, flag: CellFlags) {
        self.flags.set(flag);
    }

    /// Unset a flag.
    #[inline]
    pub fn unset_flag(&mut self, flag: CellFlags) {
        self.flags.unset(flag);
    }

    /// Check if a flag is set.
    #[inline]
    pub fn has_flag(&self, flag: CellFlags) -> bool {
        self.flags.contains(flag)
    }

    /// Get the hyperlink ID (0 = no hyperlink).
    #[inline]
    pub fn hyperlink_id(&self) -> u32 {
        self.hyperlink_id
    }

    /// Set the hyperlink ID.
    #[inline]
    pub fn set_hyperlink_id(&mut self, id: u32) {
        self.hyperlink_id = id;
        if id != 0 {
            self.flags.set(CellFlags::HYPERLINK);
        } else {
            self.flags.unset(CellFlags::HYPERLINK);
        }
    }

    /// Check if this cell has a hyperlink.
    #[inline]
    pub fn has_hyperlink(&self) -> bool {
        self.hyperlink_id != 0
    }

    /// Clear the cell to default state.
    #[inline]
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Erase the cell per ECMA-48: reset to space, preserve background color.
    #[inline]
    pub fn erase(&mut self, bg: Color) {
        *self = Self::default();
        self.bg = PackedColor::pack(bg);
    }

    /// Copy attributes (colors, flags, hyperlink) from another cell, keeping the character.
    #[inline]
    pub fn copy_attrs_from(&mut self, other: &Cell) {
        *self = Self {
            c: self.c,
            ..*other
        };
    }

    /// Whether this cell equals [`Cell::default`]. Unlike `==`, it checks
    /// every field without branching, so a scan over many cells can be
    /// vectorised.
    #[inline]
    pub(crate) fn is_default(&self) -> bool {
        ((self.c as u32 ^ ' ' as u32)
            | self.fg.0
            | self.bg.0
            | u32::from(self.flags.0)
            | self.hyperlink_id
            | self.underline_color.0)
            == 0
    }

    /// Check if this is a blank cell.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.c == ' ' && self.bg == PackedColor::DEFAULT && self.flags.is_empty()
    }

    /// Check if this is a wide character.
    #[inline]
    pub fn is_wide(&self) -> bool {
        self.flags.contains(CellFlags::WIDE_CHAR)
    }

    /// Check if this is a spacer for a wide character.
    #[inline]
    pub fn is_wide_spacer(&self) -> bool {
        self.flags.contains(CellFlags::WIDE_CHAR_SPACER)
    }
}

impl fmt::Debug for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_blank() {
            write!(f, "Cell(' ')")
        } else {
            write!(
                f,
                "Cell({:?}, fg={:?}, bg={:?}, flags={:?})",
                self.c,
                self.fg.unpack(),
                self.bg.unpack(),
                self.flags
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_color_survives_packing() {
        let mut colors = vec![Color::Default];
        for n in 0..=255u8 {
            colors.push(Color::Named(n));
            colors.push(Color::Indexed(n));
            colors.push(Color::Rgb(n, 255 - n, n / 2));
        }
        colors.push(Color::Rgb(0, 0, 0));
        colors.push(Color::Rgb(255, 255, 255));
        for color in colors {
            assert_eq!(PackedColor::pack(color).unpack(), color);
            let mut cell = Cell::default();
            cell.set_fg(color);
            cell.set_bg(color);
            assert_eq!((cell.fg(), cell.bg()), (color, color));
        }
        // Distinct colors pack to distinct values, so packed equality is
        // color equality.
        assert_ne!(
            PackedColor::pack(Color::Named(1)),
            PackedColor::pack(Color::Indexed(1))
        );
        assert_ne!(
            PackedColor::pack(Color::Default),
            PackedColor::pack(Color::Rgb(0, 0, 0))
        );
    }

    #[test]
    fn default_cell_is_blank_space() {
        let cell = Cell::default();
        assert_eq!(cell.char(), ' ');
        assert_eq!(cell.fg(), Color::Default);
        assert_eq!(cell.bg(), Color::Default);
        assert!(cell.flags().is_empty());
        assert!(cell.is_blank());
        assert!(cell.is_default());
    }

    #[test]
    fn any_changed_field_makes_a_cell_not_default() {
        let changes: [fn(&mut Cell); 6] = [
            |c| c.set_char('x'),
            |c| c.set_fg(Color::Indexed(0)),
            |c| c.set_bg(Color::Indexed(0)),
            |c| c.set_flag(CellFlags::WIDE_CHAR_SPACER),
            |c| c.hyperlink_id = 3,
            |c| c.set_underline_color(Color::Indexed(0)),
        ];
        for change in changes {
            let mut cell = Cell::default();
            change(&mut cell);
            assert_ne!(cell, Cell::default());
            assert!(!cell.is_default(), "{cell:?}");
        }
    }

    #[test]
    fn cell_flags_work() {
        let mut cell = Cell::new('X');
        assert!(!cell.has_flag(CellFlags::BOLD));

        cell.set_flag(CellFlags::BOLD);
        assert!(cell.has_flag(CellFlags::BOLD));

        cell.set_flag(CellFlags::ITALIC);
        assert!(cell.has_flag(CellFlags::BOLD));
        assert!(cell.has_flag(CellFlags::ITALIC));

        cell.unset_flag(CellFlags::BOLD);
        assert!(!cell.has_flag(CellFlags::BOLD));
        assert!(cell.has_flag(CellFlags::ITALIC));
    }

    #[test]
    fn cell_colors() {
        let mut cell = Cell::new('A');
        cell.set_fg(Color::RED);
        cell.set_bg(Color::rgb(100, 150, 200));

        assert_eq!(cell.fg(), Color::RED);
        assert_eq!(cell.bg(), Color::Rgb(100, 150, 200));
    }

    #[test]
    fn copy_attrs() {
        let template = Cell::with_attrs('T', Color::GREEN, Color::BLUE, CellFlags::BOLD);
        let mut cell = Cell::new('X');
        cell.copy_attrs_from(&template);

        assert_eq!(cell.char(), 'X'); // Character unchanged
        assert_eq!(cell.fg(), Color::GREEN);
        assert_eq!(cell.bg(), Color::BLUE);
        assert!(cell.has_flag(CellFlags::BOLD));
    }

    #[test]
    fn every_underline_style_round_trips_through_the_flags() {
        use UnderlineStyle::*;
        for style in [None, Single, Double, Curly, Dotted, Dashed] {
            let mut cell = Cell::new('u');
            cell.set_flag(CellFlags::BOLD);
            cell.set_underline_style(Dashed);
            cell.set_underline_style(style);
            assert_eq!(cell.underline_style(), style);
            assert!(cell.has_flag(CellFlags::BOLD));
            assert_eq!(cell.has_flag(CellFlags::UNDERLINE), style != None);
        }
        let mut cell = Cell::new('u');
        cell.set_underline_style(Curly);
        cell.set_underline_style(None);
        assert!(cell.flags().is_empty(), "no shape bits left behind");
    }

    #[test]
    fn copy_attrs_carries_the_underline() {
        let mut template = Cell::new('T');
        template.set_underline_style(UnderlineStyle::Curly);
        template.set_underline_color(Color::rgb(1, 2, 3));
        let mut cell = Cell::new('X');
        cell.copy_attrs_from(&template);
        assert_eq!(cell.char(), 'X');
        assert_eq!(cell.underline_style(), UnderlineStyle::Curly);
        assert_eq!(cell.underline_color(), Color::rgb(1, 2, 3));
    }

    #[test]
    fn a_cell_is_six_words() {
        // Screen rows hold one Cell per column; scrollback is compact (see
        // history.rs). Growing this needs a reason.
        assert_eq!(std::mem::size_of::<Cell>(), 24);
    }

    #[test]
    fn clear_cell() {
        let mut cell = Cell::with_attrs('X', Color::RED, Color::BLUE, CellFlags::BOLD);
        cell.clear();
        assert!(cell.is_blank());
    }
}
