//! Theme system for consistent widget styling.
//!
//! Theme state is thread-local and intended for the UI/render thread only.
//! Background tasks should not depend on `Theme::current()`.

use std::cell::RefCell;

use ultty_protocol::{Color, NamedColor};

/// Theme for consistent widget styling across the application.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Primary accent color.
    pub primary: Color,
    /// Background color.
    pub background: Color,
    /// Foreground (text) color.
    pub foreground: Color,
    /// Muted/secondary text color.
    pub muted: Color,
    /// Error/danger color.
    pub error: Color,
    /// Success color.
    pub success: Color,
    /// Warning color.
    pub warning: Color,

    /// Button-specific styling.
    pub button: ButtonTheme,
    /// Text input styling.
    pub input: InputTheme,
    /// Checkbox/toggle styling.
    pub control: ControlTheme,
    /// Selection/highlight styling.
    pub selection: SelectionTheme,
    /// Design tokens for consistent spacing and surfaces.
    pub tokens: DesignTokens,
}

/// Design tokens for layout spacing and surface hierarchy.
#[derive(Debug, Clone)]
pub struct DesignTokens {
    /// Spacing scale: extra-small (0.5 cells).
    pub space_xs: f32,
    /// Spacing scale: small (1 cell).
    pub space_sm: f32,
    /// Spacing scale: medium (2 cells).
    pub space_md: f32,
    /// Spacing scale: large (3 cells).
    pub space_lg: f32,
    /// Spacing scale: extra-large (4 cells).
    pub space_xl: f32,
    /// Base surface (lowest elevation).
    pub surface_0: Color,
    /// Elevated surface.
    pub surface_1: Color,
    /// Highest elevation surface.
    pub surface_2: Color,
    /// Subtle border color.
    pub border_subtle: Color,
    /// Strong border color.
    pub border_strong: Color,
    /// Title text color.
    pub text_title: Color,
    /// Body text color.
    pub text_body: Color,
    /// Muted text color.
    pub text_muted: Color,
    /// Accent text color.
    pub text_accent: Color,
}

#[derive(Debug, Clone)]
pub struct ButtonTheme {
    pub fg: Color,
    pub bg: Color,
    pub focused_fg: Color,
    pub focused_bg: Color,
}

#[derive(Debug, Clone)]
pub struct InputTheme {
    pub fg: Color,
    pub bg: Color,
    pub placeholder_fg: Color,
    pub cursor_fg: Color,
    pub focused_border: Color,
}

#[derive(Debug, Clone)]
pub struct ControlTheme {
    pub fg: Color,
    pub active_fg: Color,
    pub indicator: Color,
}

#[derive(Debug, Clone)]
pub struct SelectionTheme {
    pub fg: Color,
    pub bg: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            primary: Color::Rgb(97, 175, 239),     // Soft blue
            background: Color::Rgb(30, 30, 46),    // Dark bg
            foreground: Color::Rgb(205, 214, 244), // Light text
            muted: Color::Rgb(108, 112, 134),      // Gray
            error: Color::Rgb(243, 139, 168),      // Pink-red
            success: Color::Rgb(166, 227, 161),    // Green
            warning: Color::Rgb(249, 226, 175),    // Yellow
            button: ButtonTheme {
                fg: Color::Rgb(205, 214, 244),
                bg: Color::Rgb(69, 71, 90),
                focused_fg: Color::Rgb(30, 30, 46),
                focused_bg: Color::Rgb(97, 175, 239),
            },
            input: InputTheme {
                fg: Color::Rgb(205, 214, 244),
                bg: Color::Rgb(49, 50, 68),
                placeholder_fg: Color::Rgb(108, 112, 134),
                cursor_fg: Color::Rgb(245, 224, 220),
                focused_border: Color::Rgb(97, 175, 239),
            },
            control: ControlTheme {
                fg: Color::Rgb(205, 214, 244),
                active_fg: Color::Rgb(166, 227, 161),
                indicator: Color::Rgb(97, 175, 239),
            },
            selection: SelectionTheme {
                fg: Color::Rgb(30, 30, 46),
                bg: Color::Rgb(97, 175, 239),
            },
            tokens: DesignTokens {
                space_xs: 0.5,
                space_sm: 1.0,
                space_md: 2.0,
                space_lg: 3.0,
                space_xl: 4.0,
                surface_0: Color::Rgb(30, 30, 46),
                surface_1: Color::Rgb(49, 50, 68),
                surface_2: Color::Rgb(69, 71, 90),
                border_subtle: Color::Rgb(69, 71, 90),
                border_strong: Color::Rgb(108, 112, 134),
                text_title: Color::Rgb(205, 214, 244),
                text_body: Color::Rgb(186, 194, 222),
                text_muted: Color::Rgb(108, 112, 134),
                text_accent: Color::Rgb(97, 175, 239),
            },
        }
    }

    pub fn light() -> Self {
        Theme {
            primary: Color::Rgb(30, 102, 245),     // Blue
            background: Color::Rgb(239, 241, 245), // Light bg
            foreground: Color::Rgb(76, 79, 105),   // Dark text
            muted: Color::Rgb(140, 143, 161),      // Gray
            error: Color::Rgb(210, 15, 57),        // Red
            success: Color::Rgb(64, 160, 43),      // Green
            warning: Color::Rgb(223, 142, 29),     // Orange
            button: ButtonTheme {
                fg: Color::Rgb(76, 79, 105),
                bg: Color::Rgb(204, 208, 218),
                focused_fg: Color::Rgb(239, 241, 245),
                focused_bg: Color::Rgb(30, 102, 245),
            },
            input: InputTheme {
                fg: Color::Rgb(76, 79, 105),
                bg: Color::Rgb(230, 233, 239),
                placeholder_fg: Color::Rgb(140, 143, 161),
                cursor_fg: Color::Rgb(76, 79, 105),
                focused_border: Color::Rgb(30, 102, 245),
            },
            control: ControlTheme {
                fg: Color::Rgb(76, 79, 105),
                active_fg: Color::Rgb(64, 160, 43),
                indicator: Color::Rgb(30, 102, 245),
            },
            selection: SelectionTheme {
                fg: Color::Rgb(239, 241, 245),
                bg: Color::Rgb(30, 102, 245),
            },
            tokens: DesignTokens {
                space_xs: 0.5,
                space_sm: 1.0,
                space_md: 2.0,
                space_lg: 3.0,
                space_xl: 4.0,
                surface_0: Color::Rgb(239, 241, 245),
                surface_1: Color::Rgb(230, 233, 239),
                surface_2: Color::Rgb(204, 208, 218),
                border_subtle: Color::Rgb(204, 208, 218),
                border_strong: Color::Rgb(140, 143, 161),
                text_title: Color::Rgb(76, 79, 105),
                text_body: Color::Rgb(92, 95, 119),
                text_muted: Color::Rgb(140, 143, 161),
                text_accent: Color::Rgb(30, 102, 245),
            },
        }
    }

    pub fn high_contrast() -> Self {
        Theme {
            primary: Color::Rgb(255, 255, 0),
            background: Color::Rgb(0, 0, 0),
            foreground: Color::Rgb(255, 255, 255),
            muted: Color::Rgb(200, 200, 200),
            error: Color::Rgb(255, 64, 64),
            success: Color::Rgb(64, 255, 64),
            warning: Color::Rgb(255, 200, 0),
            button: ButtonTheme {
                fg: Color::Rgb(255, 255, 255),
                bg: Color::Rgb(0, 0, 0),
                focused_fg: Color::Rgb(0, 0, 0),
                focused_bg: Color::Rgb(255, 255, 0),
            },
            input: InputTheme {
                fg: Color::Rgb(255, 255, 255),
                bg: Color::Rgb(0, 0, 0),
                placeholder_fg: Color::Rgb(200, 200, 200),
                cursor_fg: Color::Rgb(255, 255, 0),
                focused_border: Color::Rgb(255, 255, 0),
            },
            control: ControlTheme {
                fg: Color::Rgb(255, 255, 255),
                active_fg: Color::Rgb(64, 255, 64),
                indicator: Color::Rgb(255, 255, 0),
            },
            selection: SelectionTheme {
                fg: Color::Rgb(0, 0, 0),
                bg: Color::Rgb(0, 255, 255),
            },
            tokens: DesignTokens {
                space_xs: 0.5,
                space_sm: 1.0,
                space_md: 2.0,
                space_lg: 3.0,
                space_xl: 4.0,
                surface_0: Color::Rgb(0, 0, 0),
                surface_1: Color::Rgb(32, 32, 32),
                surface_2: Color::Rgb(64, 64, 64),
                border_subtle: Color::Rgb(128, 128, 128),
                border_strong: Color::Rgb(255, 255, 255),
                text_title: Color::Rgb(255, 255, 255),
                text_body: Color::Rgb(255, 255, 255),
                text_muted: Color::Rgb(200, 200, 200),
                text_accent: Color::Rgb(255, 255, 0),
            },
        }
    }
}

/// The colour names `parse_color` accepts, as `ulttui-lua` scripts and
/// override files write them.
pub const COLOR_NAMES: &[(&str, NamedColor)] = &[
    ("black", NamedColor::Black),
    ("red", NamedColor::Red),
    ("green", NamedColor::Green),
    ("yellow", NamedColor::Yellow),
    ("blue", NamedColor::Blue),
    ("magenta", NamedColor::Magenta),
    ("cyan", NamedColor::Cyan),
    ("white", NamedColor::White),
    ("gray", NamedColor::BrightBlack),
    ("bright_black", NamedColor::BrightBlack),
    ("bright_red", NamedColor::BrightRed),
    ("bright_green", NamedColor::BrightGreen),
    ("bright_yellow", NamedColor::BrightYellow),
    ("bright_blue", NamedColor::BrightBlue),
    ("bright_magenta", NamedColor::BrightMagenta),
    ("bright_cyan", NamedColor::BrightCyan),
    ("bright_white", NamedColor::BrightWhite),
];

/// A colour written as a name from [`COLOR_NAMES`] or `#rrggbb`. (A palette
/// index is a number, not a string; callers that accept one check for it
/// first.)
pub fn parse_color(s: &str) -> Result<Color, String> {
    if let Some(hex) = s.strip_prefix('#') {
        let channel = |i: usize| {
            hex.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        };
        if hex.len() == 6 {
            if let (Some(r), Some(g), Some(b)) = (channel(0), channel(2), channel(4)) {
                return Ok(Color::Rgb(r, g, b));
            }
        }
        return Err(format!("'{s}' is not a #rrggbb colour"));
    }
    COLOR_NAMES
        .iter()
        .find(|(name, _)| *name == s)
        .map(|(_, c)| Color::Named(*c))
        .ok_or_else(|| {
            let names: Vec<_> = COLOR_NAMES.iter().map(|(n, _)| *n).collect();
            format!(
                "'{s}' is not a colour (use #rrggbb, 0-255, or one of: {})",
                names.join(", ")
            )
        })
}

thread_local! {
    static CURRENT_THEME: RefCell<Theme> = RefCell::new(Theme::dark());
}

impl Theme {
    /// Get the current theme.
    pub fn current() -> Theme {
        CURRENT_THEME.with(|t| t.borrow().clone())
    }

    /// Set the current theme (used by App::run internally).
    pub fn set_current(theme: Theme) {
        CURRENT_THEME.with(|t| *t.borrow_mut() = theme);
    }

    /// Replace the current theme and return the previous one.
    pub fn replace_current(theme: Theme) -> Theme {
        CURRENT_THEME.with(|t| {
            let mut borrowed = t.borrow_mut();
            std::mem::replace(&mut *borrowed, theme)
        })
    }
}

/// Convert a protocol Color to a ratatui Color for ANSI rendering.
pub fn to_ratatui_color(c: Color) -> ratatui::style::Color {
    use ratatui::style::Color as RColor;
    use ultty_protocol::NamedColor;

    match c {
        Color::Rgb(r, g, b) => RColor::Rgb(r, g, b),
        Color::Indexed(i) => RColor::Indexed(i),
        Color::Named(named) => match named {
            NamedColor::Black => RColor::Black,
            NamedColor::Red => RColor::Red,
            NamedColor::Green => RColor::Green,
            NamedColor::Yellow => RColor::Yellow,
            NamedColor::Blue => RColor::Blue,
            NamedColor::Magenta => RColor::Magenta,
            NamedColor::Cyan => RColor::Cyan,
            NamedColor::White => RColor::White,
            NamedColor::BrightBlack => RColor::DarkGray,
            NamedColor::BrightRed => RColor::LightRed,
            NamedColor::BrightGreen => RColor::LightGreen,
            NamedColor::BrightYellow => RColor::LightYellow,
            NamedColor::BrightBlue => RColor::LightBlue,
            NamedColor::BrightMagenta => RColor::LightMagenta,
            NamedColor::BrightCyan => RColor::LightCyan,
            NamedColor::BrightWhite => RColor::White,
            NamedColor::Foreground | NamedColor::Background => RColor::Reset,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::Theme;
    use ultty_protocol::Color;

    #[test]
    fn dark_theme_has_distinct_colors() {
        let theme = Theme::dark();
        assert_ne!(theme.primary, theme.background);
        assert_ne!(theme.background, theme.foreground);
        assert_ne!(theme.primary, theme.foreground);
    }

    #[test]
    fn light_theme_has_distinct_colors() {
        let theme = Theme::light();
        assert_ne!(theme.primary, theme.background);
        assert_ne!(theme.background, theme.foreground);
        assert_ne!(theme.primary, theme.foreground);
    }

    #[test]
    fn high_contrast_theme_has_distinct_colors() {
        let theme = Theme::high_contrast();
        assert_ne!(theme.primary, theme.background);
        assert_ne!(theme.background, theme.foreground);
        assert_ne!(theme.selection.bg, theme.selection.fg);
    }

    #[test]
    fn high_contrast_theme_uses_black_background() {
        let theme = Theme::high_contrast();
        assert_eq!(theme.background, Color::Rgb(0, 0, 0));
        assert_eq!(theme.foreground, Color::Rgb(255, 255, 255));
        assert_eq!(theme.button.focused_bg, theme.primary);
    }

    #[test]
    fn thread_local_set_and_get() {
        let theme = Theme::light();
        Theme::set_current(theme.clone());
        assert_eq!(Theme::current().primary, theme.primary);
    }

    #[test]
    fn replace_current_returns_old_theme() {
        let original = Theme::dark();
        Theme::set_current(original.clone());

        let new_theme = Theme::light();
        let old = Theme::replace_current(new_theme.clone());
        assert_eq!(old.primary, original.primary);
        assert_eq!(Theme::current().primary, new_theme.primary);

        Theme::set_current(old);
    }

    #[test]
    fn default_theme_is_dark() {
        let theme = Theme::current();
        let dark = Theme::dark();
        assert_eq!(theme.primary, dark.primary);
        assert_eq!(theme.background, dark.background);
        assert_eq!(theme.foreground, dark.foreground);
    }

    #[test]
    fn tokens_spacing_scale_is_ordered() {
        for theme in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            let t = &theme.tokens;
            assert!(t.space_xs < t.space_sm);
            assert!(t.space_sm < t.space_md);
            assert!(t.space_md < t.space_lg);
            assert!(t.space_lg < t.space_xl);
        }
    }

    #[test]
    fn tokens_surfaces_are_distinct() {
        for theme in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            let t = &theme.tokens;
            assert_ne!(t.surface_0, t.surface_1);
            assert_ne!(t.surface_1, t.surface_2);
            assert_ne!(t.surface_0, t.surface_2);
        }
    }
}
