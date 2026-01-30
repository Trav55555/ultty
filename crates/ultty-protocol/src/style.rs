//! Style types for visual presentation.

/// Complete style specification.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Style {
    /// Foreground color.
    pub fg: Option<Color>,
    /// Background color.
    pub bg: Option<Color>,
    /// Border style.
    pub border: BorderStyle,
    /// Border color.
    pub border_color: Option<Color>,
    /// Bold text.
    pub bold: bool,
    /// Italic text.
    pub italic: bool,
    /// Underlined text.
    pub underline: bool,
    /// Dimmed text.
    pub dim: bool,
    /// Opacity (0.0 - 1.0).
    pub opacity: f32,
}

impl Style {
    /// Create a new style with defaults.
    pub fn new() -> Self {
        Self {
            opacity: 1.0,
            ..Default::default()
        }
    }

    /// Set foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Set bold.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Set border.
    pub fn border(mut self, border: BorderStyle) -> Self {
        self.border = border;
        self
    }
}

/// Color specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Named ANSI color (adapts to terminal theme).
    Named(NamedColor),
    /// True color RGB.
    Rgb(u8, u8, u8),
    /// 256-color palette index.
    Indexed(u8),
}

impl Color {
    /// Create RGB color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgb(r, g, b)
    }

    /// Black.
    pub const BLACK: Color = Color::Named(NamedColor::Black);
    /// Red.
    pub const RED: Color = Color::Named(NamedColor::Red);
    /// Green.
    pub const GREEN: Color = Color::Named(NamedColor::Green);
    /// Yellow.
    pub const YELLOW: Color = Color::Named(NamedColor::Yellow);
    /// Blue.
    pub const BLUE: Color = Color::Named(NamedColor::Blue);
    /// Magenta.
    pub const MAGENTA: Color = Color::Named(NamedColor::Magenta);
    /// Cyan.
    pub const CYAN: Color = Color::Named(NamedColor::Cyan);
    /// White.
    pub const WHITE: Color = Color::Named(NamedColor::White);
}

/// Named ANSI colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NamedColor {
    Black = 0,
    Red = 1,
    Green = 2,
    Yellow = 3,
    Blue = 4,
    Magenta = 5,
    Cyan = 6,
    White = 7,
    BrightBlack = 8,
    BrightRed = 9,
    BrightGreen = 10,
    BrightYellow = 11,
    BrightBlue = 12,
    BrightMagenta = 13,
    BrightCyan = 14,
    BrightWhite = 15,
    /// Terminal's default foreground.
    Foreground = 16,
    /// Terminal's default background.
    Background = 17,
}

/// Border style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BorderStyle {
    /// Border type.
    pub style: BorderType,
    /// Which edges have borders.
    pub edges: BorderEdges,
    /// Corner radius (0-2 cells).
    pub radius: u8,
}

impl BorderStyle {
    /// No border.
    pub const NONE: BorderStyle = BorderStyle {
        style: BorderType::None,
        edges: BorderEdges::NONE,
        radius: 0,
    };

    /// Thin border on all sides.
    pub const fn thin() -> Self {
        Self {
            style: BorderType::Thin,
            edges: BorderEdges::ALL,
            radius: 0,
        }
    }

    /// Rounded border on all sides.
    pub const fn rounded() -> Self {
        Self {
            style: BorderType::Rounded,
            edges: BorderEdges::ALL,
            radius: 1,
        }
    }

    /// Double border on all sides.
    pub const fn double() -> Self {
        Self {
            style: BorderType::Double,
            edges: BorderEdges::ALL,
            radius: 0,
        }
    }
}

/// Border line type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BorderType {
    #[default]
    None = 0,
    /// Thin lines: ─ │ ┌ ┐ └ ┘
    Thin = 1,
    /// Thick lines: ━ ┃ ┏ ┓ ┗ ┛
    Thick = 2,
    /// Double lines: ═ ║ ╔ ╗ ╚ ╝
    Double = 3,
    /// Rounded corners: ─ │ ╭ ╮ ╰ ╯
    Rounded = 4,
    /// Dashed: ╌ ╎
    Dashed = 5,
    /// ASCII: - | + +
    Ascii = 6,
}

/// Which edges have borders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BorderEdges {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl BorderEdges {
    /// No borders.
    pub const NONE: BorderEdges = BorderEdges {
        top: false,
        right: false,
        bottom: false,
        left: false,
    };

    /// All borders.
    pub const ALL: BorderEdges = BorderEdges {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };

    /// Horizontal borders only (top and bottom).
    pub const HORIZONTAL: BorderEdges = BorderEdges {
        top: true,
        right: false,
        bottom: true,
        left: false,
    };

    /// Vertical borders only (left and right).
    pub const VERTICAL: BorderEdges = BorderEdges {
        top: false,
        right: true,
        bottom: false,
        left: true,
    };
}
