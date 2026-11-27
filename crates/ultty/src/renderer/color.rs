//! Color types and terminal color scheme.

use std::borrow::Cow;

use crate::config::ColorConfig;

/// RGBA color for rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a new opaque RGB color (alpha = 1.0).
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Create a color from 8-bit RGB components (0-255).
    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// Convert to an `[r, g, b, a]` array for GPU upload.
    pub fn to_array(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// WCAG 2.x relative luminance of the colour's RGB (alpha ignored).
    pub fn relative_luminance(&self) -> f32 {
        fn channel(c: f32) -> f32 {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// WCAG 2.x contrast ratio between two colours, from 1.0 (identical)
    /// to 21.0 (black on white).
    pub fn contrast_ratio(&self, other: &Rgba) -> f32 {
        let (a, b) = (self.relative_luminance(), other.relative_luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// This colour drawn at its own alpha over an opaque `under`.
    pub fn over(&self, under: &Rgba) -> Rgba {
        let mix = |top: f32, bottom: f32| top * self.a + bottom * (1.0 - self.a);
        Rgba::rgb(
            mix(self.r, under.r),
            mix(self.g, under.g),
            mix(self.b, under.b),
        )
    }

    /// Convert from sRGB to linear color space.
    ///
    /// Colors from hex values (like #1D2021) are in sRGB space.
    /// When rendering to an sRGB framebuffer, wgpu expects the clear color
    /// to be in linear space - it will apply gamma encoding automatically.
    /// Without this conversion, colors appear washed out (dark grays look light gray).
    pub fn to_linear(&self) -> Self {
        fn srgb_to_linear(c: f32) -> f32 {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        Self {
            r: srgb_to_linear(self.r),
            g: srgb_to_linear(self.g),
            b: srgb_to_linear(self.b),
            a: self.a, // Alpha is not gamma-encoded
        }
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Self::rgb(0.0, 0.0, 0.0)
    }
}

/// Contrast an unthemed cell's own text colour needs against the selection to
/// stay as it is: WCAG AA for body text.
pub const SELECTION_KEEP_CONTRAST: f32 = 4.5;

/// Contrast the theme's foreground or background needs against the
/// selection to stand in for an unreadable cell colour; below it black or
/// white is used. WCAG AA for large text and UI.
pub const SELECTION_FALLBACK_CONTRAST: f32 = 3.0;

/// Terminal color scheme.
///
/// Maps terminal colors to RGB values for rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorScheme {
    /// 16 named colors (0-15)
    pub named: [Rgba; 16],
    /// Default foreground
    pub foreground: Rgba,
    /// Default background
    pub background: Rgba,
    /// Cursor color
    pub cursor: Rgba,
    /// Selection background
    pub selection: Rgba,
    /// Text colour inside a selection. `None` picks one per cell, see
    /// [`ColorScheme::selection_text`].
    pub selection_foreground: Option<Rgba>,
    /// Palette entries an application changed with OSC 4, by index. Only
    /// set on the per-terminal copy [`ColorScheme::for_palette`] makes.
    overrides: Option<Box<[Option<Rgba>; 256]>>,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::dark()
    }
}

impl ColorScheme {
    /// Selection background used for terminal-grid selections.
    ///
    /// The theme controls the hue. Alpha is raised to at least 0.55 so a
    /// faint selection stays visible; an opaque selection stays opaque,
    /// which is what a theme's `selection_foreground` was chosen against.
    pub fn selection_background(&self) -> Rgba {
        Rgba::new(
            self.selection.r,
            self.selection.g,
            self.selection.b,
            self.selection.a.clamp(0.55, 1.0),
        )
    }

    /// The opaque colour a selected cell shows: the selection background
    /// over the screen background.
    pub fn selection_fill(&self) -> Rgba {
        self.selection_background().over(&self.background)
    }

    /// Text colour for a selected cell whose own text colour is `cell_fg`.
    ///
    /// A `selection_foreground` from the theme or config always wins, as
    /// Ghostty's `selection-foreground` does. Without one, `cell_fg` is kept
    /// while it reads on the selection (at least
    /// [`SELECTION_KEEP_CONTRAST`]:1), so syntax colours survive a
    /// selection. Otherwise the theme's foreground or background, whichever
    /// contrasts more, replaces it, or black or white when neither reaches
    /// [`SELECTION_FALLBACK_CONTRAST`]. Ghostty, Alacritty and kitty swap the
    /// cell's foreground and background instead; that is unreadable when the
    /// selection colour is close to the text colour, which is what the
    /// bundled themes ran into.
    pub fn selection_text(&self, cell_fg: Rgba) -> Rgba {
        if let Some(fg) = self.selection_foreground {
            return fg;
        }
        let fill = self.selection_fill();
        if cell_fg.contrast_ratio(&fill) >= SELECTION_KEEP_CONTRAST {
            return cell_fg;
        }
        let (best, ratio) = [self.foreground, self.background]
            .into_iter()
            .map(|c| (c, c.contrast_ratio(&fill)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("two candidates");
        if ratio >= SELECTION_FALLBACK_CONTRAST {
            return best;
        }
        // Neither theme colour reads on this selection (TokyoNight Day's
        // blue text on a blue selection): use black or white.
        let (black, white) = (Rgba::rgb(0.0, 0.0, 0.0), Rgba::rgb(1.0, 1.0, 1.0));
        if black.contrast_ratio(&fill) >= white.contrast_ratio(&fill) {
            black
        } else {
            white
        }
    }

    /// Dark color scheme (similar to many terminal defaults).
    pub fn dark() -> Self {
        Self {
            named: [
                // Normal colors
                Rgba::from_u8(0x00, 0x00, 0x00), // Black
                Rgba::from_u8(0xCD, 0x00, 0x00), // Red
                Rgba::from_u8(0x00, 0xCD, 0x00), // Green
                Rgba::from_u8(0xCD, 0xCD, 0x00), // Yellow
                Rgba::from_u8(0x00, 0x00, 0xEE), // Blue
                Rgba::from_u8(0xCD, 0x00, 0xCD), // Magenta
                Rgba::from_u8(0x00, 0xCD, 0xCD), // Cyan
                Rgba::from_u8(0xF5, 0xF5, 0xF5), // White
                // Bright colors
                Rgba::from_u8(0x7F, 0x7F, 0x7F), // Bright Black
                Rgba::from_u8(0xFF, 0x00, 0x00), // Bright Red
                Rgba::from_u8(0x00, 0xFF, 0x00), // Bright Green
                Rgba::from_u8(0xFF, 0xFF, 0x00), // Bright Yellow
                Rgba::from_u8(0x5C, 0x5C, 0xFF), // Bright Blue
                Rgba::from_u8(0xFF, 0x00, 0xFF), // Bright Magenta
                Rgba::from_u8(0x00, 0xFF, 0xFF), // Bright Cyan
                Rgba::from_u8(0xFF, 0xFF, 0xFF), // Bright White
            ],
            foreground: Rgba::from_u8(0xF5, 0xF5, 0xF5),
            background: Rgba::from_u8(0x1E, 0x1E, 0x1E),
            cursor: Rgba::from_u8(0xFF, 0xFF, 0xFF),
            selection: Rgba::new(0.3, 0.4, 0.6, 0.5),
            selection_foreground: None,
            overrides: None,
        }
    }

    /// Create a color scheme from configuration.
    pub fn from_config(config: &ColorConfig) -> Self {
        let palette = config.palette.to_array();
        let named: [Rgba; 16] = std::array::from_fn(|i| {
            let c = palette[i];
            Rgba::from_u8(c.r, c.g, c.b)
        });

        let (sr, sg, sb, sa) = config.selection.to_f32();

        Self {
            named,
            foreground: Rgba::from_u8(
                config.foreground.r,
                config.foreground.g,
                config.foreground.b,
            ),
            background: Rgba::from_u8(
                config.background.r,
                config.background.g,
                config.background.b,
            ),
            cursor: Rgba::from_u8(config.cursor.r, config.cursor.g, config.cursor.b),
            selection: Rgba::new(sr, sg, sb, sa),
            selection_foreground: config
                .selection_foreground
                .map(|c| Rgba::from_u8(c.r, c.g, c.b)),
            overrides: None,
        }
    }

    /// This scheme as a terminal with `palette` shows it: entries the
    /// application changed with OSC 4 and dynamic foreground/background/cursor
    /// overrides from OSC 10/11/12 replace the theme's. Borrowed as is
    /// when nothing was changed, which is the usual case.
    pub fn for_palette(&self, palette: &crate::grid::Palette) -> Cow<'_, ColorScheme> {
        if !palette.has_overrides() {
            return Cow::Borrowed(self);
        }
        let has_indexed = (0..=255).any(|i| palette.override_for(i).is_some());
        let overrides = has_indexed.then(|| {
            Box::new(std::array::from_fn(|i| {
                palette
                    .override_for(i as u8)
                    .map(|[r, g, b]| Rgba::from_u8(r, g, b))
            }))
        });
        let mut scheme = Self {
            overrides,
            ..self.clone()
        };
        if let Some([r, g, b]) = palette.dynamic_override(ultty_vt::ansi::DynamicColor::Foreground)
        {
            scheme.foreground = Rgba::from_u8(r, g, b);
        }
        if let Some([r, g, b]) = palette.dynamic_override(ultty_vt::ansi::DynamicColor::Background)
        {
            scheme.background = Rgba::from_u8(r, g, b);
        }
        if let Some([r, g, b]) = palette.dynamic_override(ultty_vt::ansi::DynamicColor::Cursor) {
            scheme.cursor = Rgba::from_u8(r, g, b);
        }
        Cow::Owned(scheme)
    }

    /// The scheme a screen is drawn with. While the screen is in reverse
    /// video (DECSCNM, mode 5) the default foreground and background trade
    /// places, which is what makes `tput flash` and vim's visual bell visible.
    pub fn for_screen(&self, reverse_video: bool) -> Cow<'_, ColorScheme> {
        if !reverse_video {
            return Cow::Borrowed(self);
        }
        let mut reversed = self.clone();
        std::mem::swap(&mut reversed.foreground, &mut reversed.background);
        Cow::Owned(reversed)
    }

    /// The scheme a terminal is drawn with: its OSC 4 palette changes, then
    /// reverse video.
    pub fn for_terminal(
        &self,
        palette: &crate::grid::Palette,
        reverse_video: bool,
    ) -> Cow<'_, ColorScheme> {
        match self.for_palette(palette) {
            Cow::Borrowed(scheme) => scheme.for_screen(reverse_video),
            Cow::Owned(scheme) => Cow::Owned(scheme.for_screen(reverse_video).into_owned()),
        }
    }

    /// Resolve a terminal color to RGBA.
    pub fn resolve(&self, color: crate::grid::Color, is_fg: bool) -> Rgba {
        match color {
            crate::grid::Color::Default => {
                if is_fg {
                    self.foreground
                } else {
                    self.background
                }
            }
            crate::grid::Color::Named(idx) => self.indexed_color(idx % 16),
            crate::grid::Color::Indexed(idx) => self.indexed_color(idx),
            crate::grid::Color::Rgb(r, g, b) => Rgba::from_u8(r, g, b),
        }
    }

    /// Get a color from the 256-color palette.
    pub fn indexed_color(&self, idx: u8) -> Rgba {
        if let Some(color) = self
            .overrides
            .as_ref()
            .and_then(|overrides| overrides[usize::from(idx)])
        {
            return color;
        }
        match idx {
            // Standard 16 colors
            0..=15 => self.named[idx as usize],
            // 216-color cube (6x6x6)
            16..=231 => {
                let idx = idx - 16;
                let r = (idx / 36) % 6;
                let g = (idx / 6) % 6;
                let b = idx % 6;
                Rgba::from_u8(
                    if r == 0 { 0 } else { r * 40 + 55 },
                    if g == 0 { 0 } else { g * 40 + 55 },
                    if b == 0 { 0 } else { b * 40 + 55 },
                )
            }
            // Grayscale (24 shades)
            232..=255 => {
                let gray = (idx - 232) * 10 + 8;
                Rgba::from_u8(gray, gray, gray)
            }
        }
    }

    /// Resolve a native protocol color to an RGBA value.
    pub fn resolve_protocol_color(&self, color: &ultty_protocol::Color) -> Rgba {
        match color {
            ultty_protocol::Color::Named(named) => {
                use ultty_protocol::NamedColor;
                match named {
                    NamedColor::Black => self.named[0],
                    NamedColor::Red => self.named[1],
                    NamedColor::Green => self.named[2],
                    NamedColor::Yellow => self.named[3],
                    NamedColor::Blue => self.named[4],
                    NamedColor::Magenta => self.named[5],
                    NamedColor::Cyan => self.named[6],
                    NamedColor::White => self.named[7],
                    NamedColor::BrightBlack => self.named[8],
                    NamedColor::BrightRed => self.named[9],
                    NamedColor::BrightGreen => self.named[10],
                    NamedColor::BrightYellow => self.named[11],
                    NamedColor::BrightBlue => self.named[12],
                    NamedColor::BrightMagenta => self.named[13],
                    NamedColor::BrightCyan => self.named[14],
                    NamedColor::BrightWhite => self.named[15],
                    NamedColor::Foreground => self.foreground,
                    NamedColor::Background => self.background,
                }
            }
            ultty_protocol::Color::Rgb(r, g, b) => Rgba::from_u8(*r, *g, *b),
            ultty_protocol::Color::Indexed(idx) => self.indexed_color(*idx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Color;

    const EPS: f32 = 0.001;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn reverse_video_swaps_only_the_default_colors() {
        let scheme = ColorScheme::dark();
        assert_eq!(*scheme.for_screen(false), scheme);

        let reversed = scheme.for_screen(true);
        assert_eq!(reversed.resolve(Color::Default, true), scheme.background);
        assert_eq!(reversed.resolve(Color::Default, false), scheme.foreground);
        assert_eq!(
            reversed.resolve(Color::RED, true),
            scheme.resolve(Color::RED, true)
        );
        assert_eq!(reversed.cursor, scheme.cursor);
    }

    #[test]
    fn rgba_from_u8() {
        let c = Rgba::from_u8(255, 128, 0);
        assert!(approx(c.r, 1.0));
        assert!(approx(c.g, 128.0 / 255.0));
        assert!(approx(c.b, 0.0));
        assert!(approx(c.a, 1.0));
    }

    #[test]
    fn rgba_default_is_black() {
        let c = Rgba::default();
        assert!(approx(c.r, 0.0));
        assert!(approx(c.g, 0.0));
        assert!(approx(c.b, 0.0));
        assert!(approx(c.a, 1.0));
    }

    #[test]
    fn rgba_to_array() {
        let c = Rgba::new(0.1, 0.2, 0.3, 0.4);
        let arr = c.to_array();
        assert!(approx(arr[0], 0.1));
        assert!(approx(arr[1], 0.2));
        assert!(approx(arr[2], 0.3));
        assert!(approx(arr[3], 0.4));
    }

    #[test]
    fn selection_background_raises_faint_alpha() {
        let mut scheme = ColorScheme::dark();
        scheme.selection = Rgba::new(0.2, 0.3, 0.4, 0.1);

        assert_eq!(
            scheme.selection_background(),
            Rgba::new(0.2, 0.3, 0.4, 0.55)
        );
    }

    #[test]
    fn selection_background_keeps_an_opaque_alpha() {
        let mut scheme = ColorScheme::dark();
        scheme.selection = Rgba::new(0.2, 0.3, 0.4, 1.0);

        assert_eq!(scheme.selection_background(), Rgba::new(0.2, 0.3, 0.4, 1.0));
    }

    #[test]
    fn contrast_ratio_matches_the_wcag_extremes() {
        let black = Rgba::rgb(0.0, 0.0, 0.0);
        let white = Rgba::rgb(1.0, 1.0, 1.0);
        assert!((black.contrast_ratio(&white) - 21.0).abs() < 0.01);
        assert!((white.contrast_ratio(&black) - 21.0).abs() < 0.01);
        assert!((white.contrast_ratio(&white) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn selection_text_uses_the_configured_foreground() {
        let mut scheme = ColorScheme::dark();
        scheme.selection_foreground = Some(Rgba::from_u8(1, 2, 3));
        assert_eq!(
            scheme.selection_text(scheme.foreground),
            Rgba::from_u8(1, 2, 3)
        );
    }

    #[test]
    fn selection_text_keeps_a_readable_cell_colour() {
        let mut scheme = ColorScheme::dark();
        scheme.selection = Rgba::rgb(0.0, 0.0, 0.0);
        let cell = Rgba::rgb(1.0, 0.8, 0.2);
        assert_eq!(scheme.selection_text(cell), cell);
    }

    #[test]
    fn selection_text_replaces_a_colour_that_vanishes() {
        // Nord: light text on a light selection.
        let mut scheme = ColorScheme::dark();
        scheme.background = Rgba::from_u8(0x2e, 0x34, 0x40);
        scheme.foreground = Rgba::from_u8(0xd8, 0xde, 0xe9);
        scheme.selection = Rgba::from_u8(0xec, 0xef, 0xf4);
        let text = scheme.selection_text(scheme.foreground);
        assert_eq!(text, scheme.background);
        assert!(text.contrast_ratio(&scheme.selection_fill()) >= 4.5);
    }

    #[test]
    fn selection_background_preserves_midrange_alpha_and_hue() {
        let mut scheme = ColorScheme::dark();
        scheme.selection = Rgba::new(0.2, 0.3, 0.4, 0.6);

        assert_eq!(scheme.selection_background(), Rgba::new(0.2, 0.3, 0.4, 0.6));
    }

    #[test]
    fn srgb_to_linear_black() {
        let c = Rgba::rgb(0.0, 0.0, 0.0).to_linear();
        assert!(approx(c.r, 0.0));
        assert!(approx(c.g, 0.0));
        assert!(approx(c.b, 0.0));
    }

    #[test]
    fn srgb_to_linear_white() {
        let c = Rgba::rgb(1.0, 1.0, 1.0).to_linear();
        assert!(approx(c.r, 1.0));
        assert!(approx(c.g, 1.0));
        assert!(approx(c.b, 1.0));
    }

    #[test]
    fn srgb_to_linear_mid_gray() {
        let c = Rgba::rgb(0.5, 0.5, 0.5).to_linear();
        assert!(approx(c.r, 0.214));
        assert!(approx(c.g, 0.214));
        assert!(approx(c.b, 0.214));
    }

    #[test]
    fn srgb_to_linear_preserves_alpha() {
        let c = Rgba::new(0.5, 0.5, 0.5, 0.75).to_linear();
        assert!(approx(c.a, 0.75));
    }

    #[test]
    fn srgb_to_linear_low_value() {
        let v = 0.03;
        let c = Rgba::rgb(v, v, v).to_linear();
        let expected = v / 12.92;
        assert!(approx(c.r, expected));
        assert!(approx(c.g, expected));
        assert!(approx(c.b, expected));
    }

    #[test]
    fn dark_scheme_defaults() {
        let scheme = ColorScheme::dark();
        assert_eq!(scheme.foreground, Rgba::from_u8(0xF5, 0xF5, 0xF5));
        assert_eq!(scheme.background, Rgba::from_u8(0x1E, 0x1E, 0x1E));
    }

    #[test]
    fn resolve_default_fg() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve(Color::Default, true);
        assert_eq!(c, scheme.foreground);
    }

    #[test]
    fn resolve_default_bg() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve(Color::Default, false);
        assert_eq!(c, scheme.background);
    }

    #[test]
    fn resolve_named_color() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve(Color::Named(1), true);
        assert_eq!(c, scheme.named[1]);
    }

    #[test]
    fn a_terminal_scheme_combines_its_palette_and_reverse_video() {
        let scheme = ColorScheme::dark();
        let mut palette = crate::grid::Palette::default();
        assert!(matches!(
            scheme.for_terminal(&palette, false),
            Cow::Borrowed(_)
        ));

        palette.set(1, [10, 20, 30]);
        let both = scheme.for_terminal(&palette, true);
        assert_eq!(
            both.resolve(Color::Indexed(1), true),
            Rgba::from_u8(10, 20, 30)
        );
        assert_eq!(both.resolve(Color::Default, false), scheme.foreground);
        // The row cache keys on the scheme, so a palette change must make
        // the scheme compare unequal.
        assert_ne!(*scheme.for_terminal(&palette, false), scheme);
    }

    #[test]
    fn test_dynamic_color_overrides_recolour_fg_bg_cursor_and_reverse_video() {
        use ultty_vt::ansi::DynamicColor;

        let scheme = ColorScheme::dark();
        let mut palette = crate::grid::Palette::default();
        palette.set_dynamic(DynamicColor::Foreground, [0x11, 0x22, 0x33]);
        palette.set_dynamic(DynamicColor::Background, [0x44, 0x55, 0x66]);
        palette.set_dynamic(DynamicColor::Cursor, [0x77, 0x88, 0x99]);

        let normal = scheme.for_terminal(&palette, false);
        assert_eq!(normal.foreground, Rgba::from_u8(0x11, 0x22, 0x33));
        assert_eq!(normal.background, Rgba::from_u8(0x44, 0x55, 0x66));
        assert_eq!(normal.cursor, Rgba::from_u8(0x77, 0x88, 0x99));
        assert_ne!(*normal, scheme);

        let reversed = scheme.for_terminal(&palette, true);
        assert_eq!(reversed.foreground, Rgba::from_u8(0x44, 0x55, 0x66));
        assert_eq!(reversed.background, Rgba::from_u8(0x11, 0x22, 0x33));
        assert_eq!(reversed.cursor, Rgba::from_u8(0x77, 0x88, 0x99));

        palette.reset_dynamic(DynamicColor::Foreground);
        palette.reset_dynamic(DynamicColor::Background);
        palette.reset_dynamic(DynamicColor::Cursor);
        assert!(matches!(
            scheme.for_terminal(&palette, false),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn palette_changes_recolour_indexed_and_named_cells_only() {
        let scheme = ColorScheme::dark();
        let mut palette = crate::grid::Palette::default();
        assert!(matches!(scheme.for_palette(&palette), Cow::Borrowed(_)));

        palette.set(1, [10, 20, 30]);
        palette.set(200, [40, 50, 60]);
        let changed = scheme.for_palette(&palette);
        assert_eq!(
            changed.resolve(Color::Named(1), true),
            Rgba::from_u8(10, 20, 30)
        );
        assert_eq!(
            changed.resolve(Color::Indexed(1), true),
            Rgba::from_u8(10, 20, 30)
        );
        assert_eq!(
            changed.resolve(Color::Indexed(200), false),
            Rgba::from_u8(40, 50, 60)
        );
        assert_eq!(changed.resolve(Color::Named(2), true), scheme.named[2]);
        assert_eq!(changed.resolve(Color::Default, false), scheme.background);
        assert_eq!(
            changed.resolve(Color::Rgb(1, 2, 3), true),
            Rgba::from_u8(1, 2, 3)
        );
    }

    #[test]
    fn resolve_rgb_color() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve(Color::Rgb(255, 0, 0), true);
        assert_eq!(c, Rgba::from_u8(255, 0, 0));
    }

    #[test]
    fn indexed_color_standard() {
        let scheme = ColorScheme::dark();
        for i in 0..16u8 {
            assert_eq!(scheme.indexed_color(i), scheme.named[i as usize]);
        }
    }

    #[test]
    fn indexed_color_cube() {
        let scheme = ColorScheme::dark();
        let black = scheme.indexed_color(16);
        assert_eq!(black, Rgba::from_u8(0, 0, 0));
        let white = scheme.indexed_color(231);
        assert_eq!(white, Rgba::from_u8(255, 255, 255));
    }

    #[test]
    fn indexed_color_grayscale() {
        let scheme = ColorScheme::dark();
        let darkest = scheme.indexed_color(232);
        assert_eq!(darkest, Rgba::from_u8(8, 8, 8));
        let lightest = scheme.indexed_color(255);
        assert_eq!(lightest, Rgba::from_u8(238, 238, 238));
    }

    #[test]
    fn resolve_protocol_named() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve_protocol_color(&ultty_protocol::Color::Named(
            ultty_protocol::NamedColor::Red,
        ));
        assert_eq!(c, scheme.named[1]);
    }

    #[test]
    fn resolve_protocol_rgb() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve_protocol_color(&ultty_protocol::Color::Rgb(10, 20, 30));
        assert_eq!(c, Rgba::from_u8(10, 20, 30));
    }

    #[test]
    fn resolve_protocol_indexed() {
        let scheme = ColorScheme::dark();
        let c = scheme.resolve_protocol_color(&ultty_protocol::Color::Indexed(200));
        assert_eq!(c, scheme.indexed_color(200));
    }
}
