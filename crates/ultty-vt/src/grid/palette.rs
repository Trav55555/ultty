//! The colours the terminal reports to applications and the palette and
//! dynamic-colour changes they make (OSC 4 / 10 / 11 / 12 / 104 / 110–112).

use crate::ansi::DynamicColor;

/// An 8-bit-per-channel colour, `[r, g, b]`.
pub type Rgb = [u8; 3];

/// The colours the embedder draws with. Colour queries (OSC 4, 10, 11, 12)
/// report these, so applications such as neovim and delta can tell a light
/// theme from a dark one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThemeColors {
    /// Default foreground.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// Cursor colour.
    pub cursor: Rgb,
    /// The 16 ANSI colours. Indices 16-255 are the xterm colour cube and
    /// grey ramp.
    pub ansi: [Rgb; 16],
}

impl Default for ThemeColors {
    /// xterm's default colours, for a terminal no embedder has configured.
    fn default() -> Self {
        Self {
            foreground: [0xe5, 0xe5, 0xe5],
            background: [0x00, 0x00, 0x00],
            cursor: [0xe5, 0xe5, 0xe5],
            ansi: [
                [0x00, 0x00, 0x00],
                [0xcd, 0x00, 0x00],
                [0x00, 0xcd, 0x00],
                [0xcd, 0xcd, 0x00],
                [0x00, 0x00, 0xee],
                [0xcd, 0x00, 0xcd],
                [0x00, 0xcd, 0xcd],
                [0xe5, 0xe5, 0xe5],
                [0x7f, 0x7f, 0x7f],
                [0xff, 0x00, 0x00],
                [0x00, 0xff, 0x00],
                [0xff, 0xff, 0x00],
                [0x5c, 0x5c, 0xff],
                [0xff, 0x00, 0xff],
                [0x00, 0xff, 0xff],
                [0xff, 0xff, 0xff],
            ],
        }
    }
}

/// The 256-colour palette plus dynamic foreground, background, and cursor
/// overrides applications make with OSC 4 / 10 / 11 / 12.
#[derive(Debug, Clone, Default)]
pub struct Palette {
    theme: ThemeColors,
    /// Changed entries; `None` until the first change, so the common case
    /// costs nothing to store or look up.
    overrides: Option<Box<[Option<Rgb>; 256]>>,
    /// Dynamic foreground override set by OSC 10.
    foreground_override: Option<Rgb>,
    /// Dynamic background override set by OSC 11.
    background_override: Option<Rgb>,
    /// Dynamic cursor override set by OSC 12.
    cursor_override: Option<Rgb>,
}

impl Palette {
    /// The embedder's colours.
    pub fn theme(&self) -> &ThemeColors {
        &self.theme
    }

    /// Replace the embedder's colours. Application overrides stay.
    pub fn set_theme(&mut self, theme: ThemeColors) {
        self.theme = theme;
    }

    /// Effective default foreground colour (OSC 10 override or theme default).
    pub fn foreground(&self) -> Rgb {
        self.foreground_override.unwrap_or(self.theme.foreground)
    }

    /// Effective default background colour (OSC 11 override or theme default).
    pub fn background(&self) -> Rgb {
        self.background_override.unwrap_or(self.theme.background)
    }

    /// Effective cursor colour (OSC 12 override or theme default).
    pub fn cursor(&self) -> Rgb {
        self.cursor_override.unwrap_or(self.theme.cursor)
    }

    /// Effective dynamic colour for `color`.
    pub fn dynamic_color(&self, color: DynamicColor) -> Rgb {
        match color {
            DynamicColor::Foreground => self.foreground(),
            DynamicColor::Background => self.background(),
            DynamicColor::Cursor => self.cursor(),
        }
    }

    /// Application override for dynamic colour `color`, if set.
    pub fn dynamic_override(&self, color: DynamicColor) -> Option<Rgb> {
        match color {
            DynamicColor::Foreground => self.foreground_override,
            DynamicColor::Background => self.background_override,
            DynamicColor::Cursor => self.cursor_override,
        }
    }

    /// Change dynamic colour `color` (OSC 10 / 11 / 12).
    pub fn set_dynamic(&mut self, color: DynamicColor, rgb: Rgb) {
        match color {
            DynamicColor::Foreground => self.foreground_override = Some(rgb),
            DynamicColor::Background => self.background_override = Some(rgb),
            DynamicColor::Cursor => self.cursor_override = Some(rgb),
        }
    }

    /// Restore dynamic colour `color` to the theme default (OSC 110 / 111 / 112).
    pub fn reset_dynamic(&mut self, color: DynamicColor) {
        match color {
            DynamicColor::Foreground => self.foreground_override = None,
            DynamicColor::Background => self.background_override = None,
            DynamicColor::Cursor => self.cursor_override = None,
        }
    }

    /// Restore every dynamic colour (foreground, background, cursor) to the theme default.
    pub fn reset_all_dynamic(&mut self) {
        self.foreground_override = None;
        self.background_override = None;
        self.cursor_override = None;
    }

    /// The colour palette entry `index` shows now.
    pub fn color(&self, index: u8) -> Rgb {
        self.override_for(index)
            .unwrap_or_else(|| self.base_color(index))
    }

    /// The colour an application set for `index`, if it changed it.
    pub fn override_for(&self, index: u8) -> Option<Rgb> {
        self.overrides.as_ref()?[usize::from(index)]
    }

    /// Whether any palette entry or dynamic colour differs from the theme.
    pub fn has_overrides(&self) -> bool {
        self.overrides.is_some()
            || self.foreground_override.is_some()
            || self.background_override.is_some()
            || self.cursor_override.is_some()
    }

    /// Change entry `index` (OSC 4).
    pub fn set(&mut self, index: u8, color: Rgb) {
        self.overrides.get_or_insert_with(|| Box::new([None; 256]))[usize::from(index)] =
            Some(color);
    }

    /// Restore entry `index`, or every entry, to the theme (OSC 104).
    pub fn reset(&mut self, index: Option<u8>) {
        match index {
            None => self.overrides = None,
            Some(index) => {
                if let Some(overrides) = &mut self.overrides {
                    overrides[usize::from(index)] = None;
                    if overrides.iter().all(Option::is_none) {
                        self.overrides = None;
                    }
                }
            }
        }
    }

    fn base_color(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.theme.ansi[usize::from(index)],
            16..=231 => {
                let i = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                [level(i / 36), level((i / 6) % 6), level(i % 6)]
            }
            232..=255 => {
                let gray = (index - 232) * 10 + 8;
                [gray, gray, gray]
            }
        }
    }
}

/// Format a colour the way xterm answers colour queries:
/// `rgb:rrrr/gggg/bbbb`, each 8-bit channel doubled to 16 bits.
pub fn format_color_spec([r, g, b]: Rgb) -> String {
    format!("rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_palette_is_theme_then_cube_then_grey_ramp() {
        let palette = Palette::default();
        assert_eq!(palette.color(1), ThemeColors::default().ansi[1]);
        assert_eq!(palette.color(16), [0, 0, 0]);
        assert_eq!(palette.color(196), [255, 0, 0]);
        assert_eq!(palette.color(231), [255, 255, 255]);
        assert_eq!(palette.color(232), [8, 8, 8]);
        assert_eq!(palette.color(255), [238, 238, 238]);
    }

    #[test]
    fn overrides_are_dropped_once_all_entries_are_reset() {
        let mut palette = Palette::default();
        palette.set(3, [1, 2, 3]);
        palette.set(200, [4, 5, 6]);
        palette.reset(Some(3));
        assert!(palette.has_overrides());
        palette.reset(Some(200));
        assert!(!palette.has_overrides());
    }

    #[test]
    fn color_spec_doubles_each_channel() {
        assert_eq!(format_color_spec([0x2d, 0x35, 0x3b]), "rgb:2d2d/3535/3b3b");
    }
}
