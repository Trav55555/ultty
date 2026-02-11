//! Style types for visual presentation.

/// Complete style specification.
#[derive(Debug, Clone, PartialEq)]
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
    /// Transition for animated property changes.
    /// When set, style changes will animate over the specified duration.
    pub transition: Option<Transition>,
}

/// Transition configuration for animated style changes.
///
/// When a region's style is updated and the style has a transition,
/// the terminal will smoothly interpolate animatable properties
/// (opacity, colors) from old values to new values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    /// Animation duration in milliseconds.
    pub duration_ms: u32,
    /// Easing function.
    pub easing: Easing,
    /// Which properties to animate (bitmask of `TRANSITION_*` constants).
    pub properties: u32,
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            duration_ms: 200,
            easing: Easing::EaseInOut,
            properties: TRANSITION_ALL,
        }
    }
}

/// Easing function for transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Easing {
    /// Constant speed.
    Linear = 0,
    /// Start slow, accelerate.
    EaseIn = 1,
    /// Start fast, decelerate.
    EaseOut = 2,
    /// Start slow, accelerate, then decelerate.
    #[default]
    EaseInOut = 3,
}

impl Easing {
    /// Apply easing function to a linear progress value (0.0 - 1.0).
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

/// Transition property: animate opacity changes.
pub const TRANSITION_OPACITY: u32 = 0x01;
/// Transition property: animate background color changes.
pub const TRANSITION_BG: u32 = 0x02;
/// Transition property: animate foreground color changes.
pub const TRANSITION_FG: u32 = 0x04;
/// Transition property: animate border color changes.
pub const TRANSITION_BORDER_COLOR: u32 = 0x08;
/// Transition property: animate all supported properties.
pub const TRANSITION_ALL: u32 = 0x0F;

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: None,
            bg: None,
            border: BorderStyle::NONE,
            border_color: None,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            opacity: 1.0,
            transition: None,
        }
    }
}

impl Style {
    /// Create a new style with defaults.
    pub fn new() -> Self {
        Self::default()
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
    /// Standard black (color 0).
    Black = 0,
    /// Standard red (color 1).
    Red = 1,
    /// Standard green (color 2).
    Green = 2,
    /// Standard yellow (color 3).
    Yellow = 3,
    /// Standard blue (color 4).
    Blue = 4,
    /// Standard magenta (color 5).
    Magenta = 5,
    /// Standard cyan (color 6).
    Cyan = 6,
    /// Standard white (color 7).
    White = 7,
    /// Bright black / dark gray (color 8).
    BrightBlack = 8,
    /// Bright red (color 9).
    BrightRed = 9,
    /// Bright green (color 10).
    BrightGreen = 10,
    /// Bright yellow (color 11).
    BrightYellow = 11,
    /// Bright blue (color 12).
    BrightBlue = 12,
    /// Bright magenta (color 13).
    BrightMagenta = 13,
    /// Bright cyan (color 14).
    BrightCyan = 14,
    /// Bright white (color 15).
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
    /// No border.
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
    /// Top edge has a border.
    pub top: bool,
    /// Right edge has a border.
    pub right: bool,
    /// Bottom edge has a border.
    pub bottom: bool,
    /// Left edge has a border.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_linear() {
        assert_eq!(Easing::Linear.apply(0.0), 0.0);
        assert_eq!(Easing::Linear.apply(0.5), 0.5);
        assert_eq!(Easing::Linear.apply(1.0), 1.0);
    }

    #[test]
    fn easing_ease_in() {
        assert_eq!(Easing::EaseIn.apply(0.0), 0.0);
        assert_eq!(Easing::EaseIn.apply(1.0), 1.0);
        // EaseIn should be slower at the start
        assert!(Easing::EaseIn.apply(0.5) < 0.5);
    }

    #[test]
    fn easing_ease_out() {
        assert_eq!(Easing::EaseOut.apply(0.0), 0.0);
        assert_eq!(Easing::EaseOut.apply(1.0), 1.0);
        // EaseOut should be faster at the start
        assert!(Easing::EaseOut.apply(0.5) > 0.5);
    }

    #[test]
    fn easing_ease_in_out() {
        assert_eq!(Easing::EaseInOut.apply(0.0), 0.0);
        assert_eq!(Easing::EaseInOut.apply(1.0), 1.0);
        assert_eq!(Easing::EaseInOut.apply(0.5), 0.5);
        // First half is slow (ease in)
        assert!(Easing::EaseInOut.apply(0.25) < 0.25);
        // Second half is fast then slow (ease out)
        assert!(Easing::EaseInOut.apply(0.75) > 0.75);
    }

    #[test]
    fn easing_clamps_input() {
        assert_eq!(Easing::Linear.apply(-0.5), 0.0);
        assert_eq!(Easing::Linear.apply(1.5), 1.0);
    }

    #[test]
    fn transition_default() {
        let t = Transition::default();
        assert_eq!(t.duration_ms, 200);
        assert_eq!(t.easing, Easing::EaseInOut);
        assert_eq!(t.properties, TRANSITION_ALL);
    }

    #[test]
    fn style_with_transition() {
        let style = Style {
            opacity: 0.5,
            transition: Some(Transition {
                duration_ms: 300,
                easing: Easing::EaseOut,
                properties: TRANSITION_OPACITY,
            }),
            ..Default::default()
        };
        assert!(style.transition.is_some());
        let t = style.transition.unwrap();
        assert_eq!(t.duration_ms, 300);
        assert_eq!(t.easing, Easing::EaseOut);
        assert_eq!(t.properties, TRANSITION_OPACITY);
    }
}
