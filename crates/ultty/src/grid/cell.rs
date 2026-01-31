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

/// Color representation for terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Default terminal color
    Default,
    /// Named color from the 16-color palette (0-15)
    Named(u8),
    /// 256-color palette index
    Indexed(u8),
    /// True color RGB
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The character in this cell (could be multi-byte for emoji)
    c: char,
    /// Foreground color
    fg: Color,
    /// Background color
    bg: Color,
    /// Style flags
    flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: CellFlags::NONE,
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
        Self { c, fg, bg, flags }
    }

    /// Get the character.
    #[inline]
    pub fn char(&self) -> char {
        self.c
    }

    /// Set the character.
    #[inline]
    pub fn set_char(&mut self, c: char) {
        self.c = c;
    }

    /// Get the foreground color.
    #[inline]
    pub fn fg(&self) -> Color {
        self.fg
    }

    /// Set the foreground color.
    #[inline]
    pub fn set_fg(&mut self, color: Color) {
        self.fg = color;
    }

    /// Get the background color.
    #[inline]
    pub fn bg(&self) -> Color {
        self.bg
    }

    /// Set the background color.
    #[inline]
    pub fn set_bg(&mut self, color: Color) {
        self.bg = color;
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

    /// Clear the cell to default state.
    #[inline]
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Copy attributes (colors, flags) from another cell, keeping the character.
    #[inline]
    pub fn copy_attrs_from(&mut self, other: &Cell) {
        self.fg = other.fg;
        self.bg = other.bg;
        self.flags = other.flags;
    }

    /// Check if this is a blank cell.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.c == ' ' && self.bg == Color::Default && self.flags.is_empty()
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
                self.c, self.fg, self.bg, self.flags
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cell_is_blank_space() {
        let cell = Cell::default();
        assert_eq!(cell.char(), ' ');
        assert_eq!(cell.fg(), Color::Default);
        assert_eq!(cell.bg(), Color::Default);
        assert!(cell.flags().is_empty());
        assert!(cell.is_blank());
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
    fn clear_cell() {
        let mut cell = Cell::with_attrs('X', Color::RED, Color::BLUE, CellFlags::BOLD);
        cell.clear();
        assert!(cell.is_blank());
    }
}
