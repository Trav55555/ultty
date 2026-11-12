//! Configuration system for Ultty terminal emulator.
//!
//! Configuration is loaded from `~/.config/ultty/config.toml` if it exists,
//! otherwise sensible defaults are used.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::warn;

/// Default patched monospace font used for terminal icons.
pub const DEFAULT_FONT_FAMILY: &str = "DroidSansMono Nerd Font";

/// Main configuration structure.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    /// Color configuration
    pub colors: ColorConfig,
    /// Font configuration
    pub font: FontConfig,
    /// Keybinding configuration
    pub keybindings: KeybindingConfig,
    /// Behavior configuration
    pub behavior: BehaviorConfig,
    /// What programs in the terminal are told about it
    pub terminal: TerminalConfig,
    /// Launch profiles
    pub profiles: ProfilesConfig,
}

impl Config {
    /// Load configuration from the default config file location.
    ///
    /// Returns default config if no config file exists.
    pub fn load() -> Self {
        Self::config_path()
            .and_then(|path| Self::load_from_path(&path).ok())
            .unwrap_or_default()
    }

    /// Load configuration from a specific path.
    pub fn load_from_path(path: &PathBuf) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let mut config: Self = toml::from_str(&contents).map_err(ConfigError::Parse)?;
        config.warn_on_duplicate_keybindings();
        config.font.clamp_size();
        config.font.clamp_line_height();
        config.behavior.clamp_scrollback();
        config.terminal.validate_term();
        Ok(config)
    }

    /// Get the default config file path.
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("ultty").join("config.toml"))
    }

    /// Get the config directory path.
    pub fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("ultty"))
    }

    fn warn_on_duplicate_keybindings(&self) {
        let duplicates = self.keybindings.duplicate_bindings();
        for (binding, actions) in duplicates {
            warn!(
                "Duplicate keybinding '{}' assigned to actions: {}",
                binding,
                actions.join(", ")
            );
        }
    }
}

/// Color configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    /// Default foreground color
    #[serde(deserialize_with = "deserialize_color")]
    pub foreground: RgbColor,
    /// Default background color
    #[serde(deserialize_with = "deserialize_color")]
    pub background: RgbColor,
    /// Cursor color
    #[serde(deserialize_with = "deserialize_color")]
    pub cursor: RgbColor,
    /// Selection background color
    #[serde(deserialize_with = "deserialize_color_with_alpha")]
    pub selection: RgbaColor,
    /// 16-color palette
    pub palette: PaletteConfig,
    /// Named color themes (can be cycled with keybinding)
    #[serde(default)]
    pub themes: HashMap<String, ThemeConfig>,
    /// Currently active theme name (if using themes)
    pub theme: Option<String>,
}

/// A named color theme.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Theme display name
    pub name: String,
    #[serde(deserialize_with = "deserialize_color")]
    pub foreground: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub background: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub cursor: RgbColor,
    #[serde(deserialize_with = "deserialize_color_with_alpha")]
    pub selection: RgbaColor,
    pub palette: Option<PaletteConfig>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            foreground: RgbColor::new(0xF5, 0xF5, 0xF5),
            background: RgbColor::new(0x1E, 0x1E, 0x1E),
            cursor: RgbColor::new(0xFF, 0xFF, 0xFF),
            selection: RgbaColor::new(0x4D, 0x4D, 0x4D, 0x80),
            palette: None,
        }
    }
}

impl ThemeConfig {
    /// Create a light theme.
    pub fn light() -> Self {
        Self {
            name: "Light".to_string(),
            foreground: RgbColor::new(0x00, 0x00, 0x00),
            background: RgbColor::new(0xFF, 0xFF, 0xFF),
            cursor: RgbColor::new(0x00, 0x00, 0x00),
            selection: RgbaColor::new(0xAD, 0xD8, 0xE6, 0x80),
            palette: Some(PaletteConfig::light()),
        }
    }

    /// Create a dark theme.
    pub fn dark() -> Self {
        Self {
            name: "Dark".to_string(),
            foreground: RgbColor::new(0xF5, 0xF5, 0xF5),
            background: RgbColor::new(0x1E, 0x1E, 0x1E),
            cursor: RgbColor::new(0xFF, 0xFF, 0xFF),
            selection: RgbaColor::new(0x4D, 0x4D, 0x4D, 0x80),
            palette: None,
        }
    }

    /// Create the Gruvbox Dark theme (hard contrast) - classic dark theme.
    pub fn gruvbox_dark() -> Self {
        Self {
            name: "Gruvbox Dark".to_string(),
            foreground: RgbColor::new(0xEB, 0xDB, 0xB2), // fg
            background: RgbColor::new(0x1D, 0x20, 0x21), // bg0_h (hard) - darkest
            cursor: RgbColor::new(0xFE, 0x80, 0x19),     // orange cursor
            selection: RgbaColor::new(0x45, 0x85, 0x88, 0x80), // blue selection
            palette: Some(PaletteConfig::gruvbox_dark()),
        }
    }

    /// Create the Tokyo Night theme - very dark, cool tones.
    pub fn tokyo_night() -> Self {
        Self {
            name: "Tokyo Night".to_string(),
            foreground: RgbColor::new(0xA9, 0xB1, 0xD6), // fg
            background: RgbColor::new(0x1A, 0x1B, 0x26), // bg - very dark
            cursor: RgbColor::new(0x7A, 0xA2, 0xF7),     // blue cursor
            selection: RgbaColor::new(0x28, 0x2D, 0x42, 0xC0), // dark blue selection
            palette: Some(PaletteConfig::tokyo_night()),
        }
    }

    /// Create the Catppuccin Mocha theme - modern dark theme.
    pub fn catppuccin_mocha() -> Self {
        Self {
            name: "Catppuccin Mocha".to_string(),
            foreground: RgbColor::new(0xCD, 0xD6, 0xF4), // text
            background: RgbColor::new(0x1E, 0x1E, 0x2E), // base
            cursor: RgbColor::new(0xF5, 0xE0, 0xDC),     // rosewater cursor
            selection: RgbaColor::new(0x45, 0x47, 0x5A, 0xC0), // surface1
            palette: Some(PaletteConfig::catppuccin_mocha()),
        }
    }

    /// Create the Everforest Dark theme - comfortable green-based dark theme.
    pub fn everforest_dark() -> Self {
        Self {
            name: "Everforest Dark".to_string(),
            foreground: RgbColor::new(0xF2, 0xE8, 0xD5), // fg (brightened)
            background: RgbColor::new(0x2D, 0x35, 0x3B), // bg0 (medium)
            cursor: RgbColor::new(0xA7, 0xC0, 0x80),     // green cursor
            selection: RgbaColor::new(0x3D, 0x48, 0x4D, 0xC0), // bg3
            palette: Some(PaletteConfig::everforest_dark()),
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        // Include built-in themes
        let mut themes = HashMap::new();
        themes.insert(
            "everforest-dark".to_string(),
            ThemeConfig::everforest_dark(),
        );
        themes.insert("tokyo-night".to_string(), ThemeConfig::tokyo_night());
        themes.insert("gruvbox-dark".to_string(), ThemeConfig::gruvbox_dark());
        themes.insert(
            "catppuccin-mocha".to_string(),
            ThemeConfig::catppuccin_mocha(),
        );
        themes.insert("dark".to_string(), ThemeConfig::dark());
        themes.insert("light".to_string(), ThemeConfig::light());

        let default_theme = ThemeConfig::everforest_dark();

        Self {
            foreground: default_theme.foreground,
            background: default_theme.background,
            cursor: default_theme.cursor,
            selection: default_theme.selection,
            palette: PaletteConfig::everforest_dark(),
            themes,
            theme: Some("everforest-dark".to_string()),
        }
    }
}

impl ColorConfig {
    /// Get the list of available theme names.
    pub fn theme_names(&self) -> Vec<String> {
        self.themes.keys().cloned().collect()
    }

    /// Get a theme by name.
    pub fn get_theme(&self, name: &str) -> Option<&ThemeConfig> {
        self.themes.get(name)
    }

    /// Apply a theme to this color config.
    pub fn apply_theme(&mut self, theme: &ThemeConfig) {
        self.foreground = theme.foreground;
        self.background = theme.background;
        self.cursor = theme.cursor;
        self.selection = theme.selection;
        if let Some(ref palette) = theme.palette {
            self.palette = palette.clone();
        }
    }

    /// The colours terminals report to applications that ask (OSC 4, 10,
    /// 11, 12): the ones they are drawn with.
    pub fn theme_colors(&self) -> crate::grid::ThemeColors {
        let rgb = |c: RgbColor| [c.r, c.g, c.b];
        crate::grid::ThemeColors {
            foreground: rgb(self.foreground),
            background: rgb(self.background),
            cursor: rgb(self.cursor),
            ansi: self.palette.to_array().map(rgb),
        }
    }
}

/// 16-color palette configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PaletteConfig {
    #[serde(deserialize_with = "deserialize_color")]
    pub black: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub red: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub green: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub yellow: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub blue: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub magenta: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub cyan: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub white: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_black: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_red: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_green: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_yellow: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_blue: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_magenta: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_cyan: RgbColor,
    #[serde(deserialize_with = "deserialize_color")]
    pub bright_white: RgbColor,
}

impl Default for PaletteConfig {
    fn default() -> Self {
        Self {
            // Normal colors (matching ColorScheme::dark())
            black: RgbColor::new(0x00, 0x00, 0x00),
            red: RgbColor::new(0xCD, 0x00, 0x00),
            green: RgbColor::new(0x00, 0xCD, 0x00),
            yellow: RgbColor::new(0xCD, 0xCD, 0x00),
            blue: RgbColor::new(0x00, 0x00, 0xEE),
            magenta: RgbColor::new(0xCD, 0x00, 0xCD),
            cyan: RgbColor::new(0x00, 0xCD, 0xCD),
            white: RgbColor::new(0xF5, 0xF5, 0xF5),
            // Bright colors
            bright_black: RgbColor::new(0x7F, 0x7F, 0x7F),
            bright_red: RgbColor::new(0xFF, 0x00, 0x00),
            bright_green: RgbColor::new(0x00, 0xFF, 0x00),
            bright_yellow: RgbColor::new(0xFF, 0xFF, 0x00),
            bright_blue: RgbColor::new(0x5C, 0x5C, 0xFF),
            bright_magenta: RgbColor::new(0xFF, 0x00, 0xFF),
            bright_cyan: RgbColor::new(0x00, 0xFF, 0xFF),
            bright_white: RgbColor::new(0xFF, 0xFF, 0xFF),
        }
    }
}

impl PaletteConfig {
    /// Get the 16 colors as an array.
    pub fn to_array(&self) -> [RgbColor; 16] {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ]
    }

    /// Create a light-themed palette.
    pub fn light() -> Self {
        Self {
            // Light theme uses darker colors for contrast on white background
            black: RgbColor::new(0x00, 0x00, 0x00),
            red: RgbColor::new(0xC9, 0x1B, 0x00),
            green: RgbColor::new(0x00, 0xC2, 0x00),
            yellow: RgbColor::new(0xC7, 0xC4, 0x00),
            blue: RgbColor::new(0x00, 0x00, 0xDB),
            magenta: RgbColor::new(0xC9, 0x30, 0xC7),
            cyan: RgbColor::new(0x00, 0xC5, 0xC7),
            white: RgbColor::new(0xC7, 0xC7, 0xC7),
            // Bright colors
            bright_black: RgbColor::new(0x67, 0x67, 0x67),
            bright_red: RgbColor::new(0xFF, 0x6D, 0x67),
            bright_green: RgbColor::new(0x5F, 0xF9, 0x67),
            bright_yellow: RgbColor::new(0xFE, 0xFB, 0x67),
            bright_blue: RgbColor::new(0x68, 0x71, 0xFF),
            bright_magenta: RgbColor::new(0xFF, 0x76, 0xFF),
            bright_cyan: RgbColor::new(0x5F, 0xFD, 0xFF),
            bright_white: RgbColor::new(0xFF, 0xFF, 0xFF),
        }
    }

    /// Create Gruvbox Dark palette (hard contrast).
    pub fn gruvbox_dark() -> Self {
        Self {
            // Gruvbox dark colors - classic warm dark theme
            black: RgbColor::new(0x28, 0x28, 0x28),   // bg
            red: RgbColor::new(0xCC, 0x24, 0x1D),     // red
            green: RgbColor::new(0x98, 0x97, 0x1A),   // green
            yellow: RgbColor::new(0xD7, 0x99, 0x21),  // yellow
            blue: RgbColor::new(0x45, 0x85, 0x88),    // blue
            magenta: RgbColor::new(0xB1, 0x62, 0x86), // purple
            cyan: RgbColor::new(0x68, 0x9D, 0x6A),    // aqua
            white: RgbColor::new(0xA8, 0x99, 0x84),   // fg4
            // Bright colors
            bright_black: RgbColor::new(0x92, 0x83, 0x74), // gray
            bright_red: RgbColor::new(0xFB, 0x49, 0x34),   // bright red
            bright_green: RgbColor::new(0xB8, 0xBB, 0x26), // bright green
            bright_yellow: RgbColor::new(0xFA, 0xBD, 0x2F), // bright yellow
            bright_blue: RgbColor::new(0x83, 0xA5, 0x98),  // bright blue
            bright_magenta: RgbColor::new(0xD3, 0x86, 0x9B), // bright purple
            bright_cyan: RgbColor::new(0x8E, 0xC0, 0x7C),  // bright aqua
            bright_white: RgbColor::new(0xEB, 0xDB, 0xB2), // fg
        }
    }

    /// Create Tokyo Night palette - very dark, cool tones.
    pub fn tokyo_night() -> Self {
        Self {
            // Tokyo Night colors - modern dark theme
            black: RgbColor::new(0x32, 0x34, 0x4A), // terminal black
            red: RgbColor::new(0xF7, 0x76, 0x8E),   // red
            green: RgbColor::new(0x9E, 0xCE, 0x6A), // green
            yellow: RgbColor::new(0xE0, 0xAF, 0x68), // yellow
            blue: RgbColor::new(0x7A, 0xA2, 0xF7),  // blue
            magenta: RgbColor::new(0xAD, 0x8E, 0xE6), // magenta
            cyan: RgbColor::new(0x44, 0x9D, 0xAB),  // cyan
            white: RgbColor::new(0x78, 0x7C, 0x99), // fg_dark
            // Bright colors
            bright_black: RgbColor::new(0x44, 0x4B, 0x6A), // comment
            bright_red: RgbColor::new(0xFF, 0x7A, 0x93),   // bright red
            bright_green: RgbColor::new(0xB9, 0xF2, 0x7C), // bright green
            bright_yellow: RgbColor::new(0xFF, 0x9E, 0x64), // orange
            bright_blue: RgbColor::new(0x7D, 0xA6, 0xFF),  // bright blue
            bright_magenta: RgbColor::new(0xBB, 0x9A, 0xF7), // bright magenta
            bright_cyan: RgbColor::new(0x0D, 0xB9, 0xD7),  // bright cyan
            bright_white: RgbColor::new(0xAC, 0xB0, 0xD0), // fg
        }
    }

    /// Create Catppuccin Mocha palette - modern dark theme.
    pub fn catppuccin_mocha() -> Self {
        Self {
            // Catppuccin Mocha colors
            black: RgbColor::new(0x45, 0x47, 0x5A), // surface1
            red: RgbColor::new(0xF3, 0x8B, 0xA8),   // red
            green: RgbColor::new(0xA6, 0xE3, 0xA1), // green
            yellow: RgbColor::new(0xF9, 0xE2, 0xAF), // yellow
            blue: RgbColor::new(0x89, 0xB4, 0xFA),  // blue
            magenta: RgbColor::new(0xF5, 0xC2, 0xE7), // pink
            cyan: RgbColor::new(0x94, 0xE2, 0xD5),  // teal
            white: RgbColor::new(0xBA, 0xC2, 0xDE), // subtext1
            // Bright colors
            bright_black: RgbColor::new(0x58, 0x5B, 0x70), // surface2
            bright_red: RgbColor::new(0xF3, 0x8B, 0xA8),   // red
            bright_green: RgbColor::new(0xA6, 0xE3, 0xA1), // green
            bright_yellow: RgbColor::new(0xF9, 0xE2, 0xAF), // yellow
            bright_blue: RgbColor::new(0x89, 0xB4, 0xFA),  // blue
            bright_magenta: RgbColor::new(0xF5, 0xC2, 0xE7), // pink
            bright_cyan: RgbColor::new(0x94, 0xE2, 0xD5),  // teal
            bright_white: RgbColor::new(0xA6, 0xAD, 0xC8), // subtext0
        }
    }

    /// Create Everforest Dark palette - comfortable green-based theme.
    pub fn everforest_dark() -> Self {
        Self {
            // Everforest Dark colors
            black: RgbColor::new(0x4B, 0x56, 0x5C),   // bg3
            red: RgbColor::new(0xE6, 0x7E, 0x80),     // red
            green: RgbColor::new(0xA7, 0xC0, 0x80),   // green
            yellow: RgbColor::new(0xDB, 0xBC, 0x7F),  // yellow
            blue: RgbColor::new(0x7F, 0xBB, 0xB3),    // blue
            magenta: RgbColor::new(0xD6, 0x99, 0xB6), // purple
            cyan: RgbColor::new(0x83, 0xC0, 0x92),    // aqua
            white: RgbColor::new(0xE8, 0xDC, 0xC4),   // fg (brightened)
            // Bright colors
            bright_black: RgbColor::new(0x9D, 0xA9, 0xA0), // grey1 (5.1:1 contrast)
            bright_red: RgbColor::new(0xE6, 0x7E, 0x80),   // red
            bright_green: RgbColor::new(0xA7, 0xC0, 0x80), // green
            bright_yellow: RgbColor::new(0xE6, 0x98, 0x75), // orange
            bright_blue: RgbColor::new(0x7F, 0xBB, 0xB3),  // blue
            bright_magenta: RgbColor::new(0xD6, 0x99, 0xB6), // purple
            bright_cyan: RgbColor::new(0x83, 0xC0, 0x92),  // aqua
            bright_white: RgbColor::new(0xF2, 0xE8, 0xD5), // fg (brightened)
        }
    }
}

/// Font configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font family name
    pub family: String,
    /// Font size in points
    pub size: f32,
    /// Line height as a multiple of the font size: the cell height is
    /// `size * line_height`, rounded up to whole pixels. Clamped to
    /// [`MIN_LINE_HEIGHT`]..=[`MAX_LINE_HEIGHT`].
    pub line_height: f32,
    /// Draw font ligatures (`=>`, `!=`, `===` in fonts such as Fira Code).
    ///
    /// When on, runs of printable ASCII cells that contain punctuation and
    /// share a face and foreground color are shaped together, so the font
    /// can join them. A run ends at the block cursor, so the cursor's cell
    /// is always drawn on its own. When off, each cell is shaped alone.
    /// Single cells and multi-codepoint graphemes always get full shaping
    /// (with font fallback) either way.
    pub ligatures: bool,
    /// Color emoji font family for emoji-presentation characters (`😀`,
    /// `❤️`). Empty (the default) picks the first installed of Noto Color
    /// Emoji, Apple Color Emoji, Segoe UI Emoji, Twemoji, Twitter Color
    /// Emoji, JoyPixels and OpenMoji Color.
    pub emoji: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            // A patched monospace default keeps terminal UI icons readable in
            // apps such as LazyVim. Users can override this in config.toml.
            family: DEFAULT_FONT_FAMILY.to_string(),
            size: 14.0,
            // 1.0 = no extra spacing between lines, required for block characters
            // to touch (e.g., OpenCode logo). Users can increase if they prefer
            // more line spacing for regular text.
            line_height: 1.0,
            ligatures: true,
            emoji: String::new(),
        }
    }
}

/// Smallest font size (points) the renderer accepts.
pub const MIN_FONT_SIZE: f32 = 4.0;

/// Largest font size (points) the renderer accepts.
pub const MAX_FONT_SIZE: f32 = 128.0;

impl FontConfig {
    /// Check if using the system monospace fallback.
    pub fn is_system_font(&self) -> bool {
        self.family.is_empty()
    }

    /// Clamp `size` into [`MIN_FONT_SIZE`]..=[`MAX_FONT_SIZE`], warning when
    /// the configured value was out of range or not a number.
    ///
    /// A zero, negative, NaN or huge size would give zero-width cells and
    /// nonsensical grid dimensions.
    pub fn clamp_size(&mut self) {
        let clamped = clamp_font_size(self.size);
        if clamped != self.size {
            warn!(
                "font.size {} is out of range; using {} (allowed {}..={})",
                self.size, clamped, MIN_FONT_SIZE, MAX_FONT_SIZE
            );
            self.size = clamped;
        }
    }
}

/// Smallest `font.line_height` accepted.
pub const MIN_LINE_HEIGHT: f32 = 0.5;

/// Largest `font.line_height` accepted.
pub const MAX_LINE_HEIGHT: f32 = 3.0;

impl FontConfig {
    /// Clamp `line_height` into [`MIN_LINE_HEIGHT`]..=[`MAX_LINE_HEIGHT`],
    /// warning when it was out of range or not a number. A zero or NaN
    /// multiplier would give one-pixel cells.
    pub fn clamp_line_height(&mut self) {
        let clamped = if self.line_height.is_nan() {
            FontConfig::default().line_height
        } else {
            self.line_height.clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT)
        };
        if clamped != self.line_height {
            warn!(
                "font.line_height {} is out of range; using {} (allowed {}..={})",
                self.line_height, clamped, MIN_LINE_HEIGHT, MAX_LINE_HEIGHT
            );
            self.line_height = clamped;
        }
    }
}

/// Clamp a font size into the supported range, mapping NaN to the default.
pub fn clamp_font_size(size: f32) -> f32 {
    if size.is_nan() {
        return FontConfig::default().size;
    }
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// Keybinding configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeybindingConfig {
    /// Copy selection to clipboard
    pub copy: String,
    /// Paste from clipboard
    pub paste: String,
    /// Create new tab
    pub new_tab: String,
    /// Close current tab
    pub close_tab: String,
    /// Next tab
    pub next_tab: String,
    /// Previous tab
    pub prev_tab: String,
    /// Split pane horizontally
    pub split_horizontal: String,
    /// Split pane vertically
    pub split_vertical: String,
    /// Focus pane to the left
    pub focus_pane_left: String,
    /// Focus pane to the right
    pub focus_pane_right: String,
    /// Focus pane above
    pub focus_pane_up: String,
    /// Focus pane below
    pub focus_pane_down: String,
    /// Close current pane
    pub close_pane: String,
    /// Enter vim/copy mode
    pub vim_mode: String,
    /// Search scrollback (find)
    pub find: String,
    /// Reload configuration
    pub reload_config: String,
    /// Show keyboard shortcuts overlay
    pub show_shortcuts: String,
    /// Show native protocol status overlay
    pub protocol_status: String,
    /// Cycle through color themes
    pub cycle_theme: String,
    /// Increase font size
    pub font_increase: String,
    /// Decrease font size
    pub font_decrease: String,
    /// Reset font size to default
    pub font_reset: String,
    /// Take a screenshot of the terminal
    pub screenshot: String,
    /// Dump terminal state to file for debugging
    pub debug_dump: String,
    /// Scroll back through history by a page (main screen only; on the
    /// alternate screen the key goes to the app)
    pub scroll_page_up: String,
    /// Scroll forward through history by a page
    pub scroll_page_down: String,
    /// Additional custom keybindings
    #[serde(flatten)]
    pub custom: HashMap<String, String>,
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        Self {
            copy: "ctrl+shift+c".to_string(),
            paste: "ctrl+shift+v".to_string(),
            new_tab: "ctrl+shift+t".to_string(),
            close_tab: "ctrl+shift+w".to_string(),
            next_tab: "ctrl+tab".to_string(),
            prev_tab: "ctrl+shift+tab".to_string(),
            split_horizontal: "ctrl+shift+d".to_string(),
            split_vertical: "ctrl+shift+e".to_string(),
            focus_pane_left: "ctrl+shift+left".to_string(),
            focus_pane_right: "ctrl+shift+right".to_string(),
            focus_pane_up: "ctrl+shift+up".to_string(),
            focus_pane_down: "ctrl+shift+down".to_string(),
            close_pane: "ctrl+shift+x".to_string(),
            vim_mode: "ctrl+shift+space".to_string(),
            find: "ctrl+shift+f".to_string(),
            reload_config: "ctrl+shift+r".to_string(),
            show_shortcuts: "ctrl+shift+?".to_string(),
            protocol_status: "ctrl+shift+i".to_string(),
            cycle_theme: "ctrl+shift+n".to_string(),
            font_increase: "ctrl+=".to_string(),
            font_decrease: "ctrl+-".to_string(),
            font_reset: "ctrl+0".to_string(),
            screenshot: "ctrl+shift+s".to_string(),
            debug_dump: "ctrl+shift+alt+d".to_string(),
            scroll_page_up: "shift+pageup".to_string(),
            scroll_page_down: "shift+pagedown".to_string(),
            custom: HashMap::new(),
        }
    }
}

impl KeybindingConfig {
    /// Return duplicate keybindings as (binding, actions) pairs.
    pub fn duplicate_bindings(&self) -> Vec<(String, Vec<String>)> {
        let mut by_binding: HashMap<String, Vec<String>> = HashMap::new();

        for (action, binding) in self.all_bindings() {
            let normalized = binding.trim().to_lowercase();
            if normalized.is_empty() {
                continue;
            }
            by_binding.entry(normalized).or_default().push(action);
        }

        let mut duplicates: Vec<(String, Vec<String>)> = by_binding
            .into_iter()
            .filter_map(|(binding, mut actions)| {
                if actions.len() > 1 {
                    actions.sort();
                    Some((binding, actions))
                } else {
                    None
                }
            })
            .collect();

        duplicates.sort_by(|a, b| a.0.cmp(&b.0));
        duplicates
    }

    fn all_bindings(&self) -> Vec<(String, String)> {
        let mut bindings = vec![
            ("copy".to_string(), self.copy.clone()),
            ("paste".to_string(), self.paste.clone()),
            ("new_tab".to_string(), self.new_tab.clone()),
            ("close_tab".to_string(), self.close_tab.clone()),
            ("next_tab".to_string(), self.next_tab.clone()),
            ("prev_tab".to_string(), self.prev_tab.clone()),
            (
                "split_horizontal".to_string(),
                self.split_horizontal.clone(),
            ),
            ("split_vertical".to_string(), self.split_vertical.clone()),
            ("focus_pane_left".to_string(), self.focus_pane_left.clone()),
            (
                "focus_pane_right".to_string(),
                self.focus_pane_right.clone(),
            ),
            ("focus_pane_up".to_string(), self.focus_pane_up.clone()),
            ("focus_pane_down".to_string(), self.focus_pane_down.clone()),
            ("close_pane".to_string(), self.close_pane.clone()),
            ("vim_mode".to_string(), self.vim_mode.clone()),
            ("find".to_string(), self.find.clone()),
            ("reload_config".to_string(), self.reload_config.clone()),
            ("show_shortcuts".to_string(), self.show_shortcuts.clone()),
            ("protocol_status".to_string(), self.protocol_status.clone()),
            ("cycle_theme".to_string(), self.cycle_theme.clone()),
            ("font_increase".to_string(), self.font_increase.clone()),
            ("font_decrease".to_string(), self.font_decrease.clone()),
            ("font_reset".to_string(), self.font_reset.clone()),
            ("screenshot".to_string(), self.screenshot.clone()),
            ("debug_dump".to_string(), self.debug_dump.clone()),
            ("scroll_page_up".to_string(), self.scroll_page_up.clone()),
            (
                "scroll_page_down".to_string(),
                self.scroll_page_down.clone(),
            ),
        ];

        let mut custom: Vec<(String, String)> = self
            .custom
            .iter()
            .map(|(action, binding)| (action.clone(), binding.clone()))
            .collect();
        custom.sort_by(|a, b| a.0.cmp(&b.0));
        bindings.extend(custom);
        bindings
    }

    /// Parse a keybinding string into modifiers and key.
    ///
    /// Format: `mod1+mod2+key` where modifiers are ctrl, shift, alt, super
    pub fn parse_keybinding(binding: &str) -> Option<ParsedKeybinding> {
        let lowercased = binding.to_lowercase();
        let parts: Vec<&str> = lowercased.split('+').collect();
        if parts.is_empty() {
            return None;
        }

        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut super_key = false;
        let mut key = None;

        for (i, part) in parts.iter().enumerate() {
            let part = part.trim();
            if i == parts.len() - 1 {
                // Last part is the key
                key = Some(part.to_string());
            } else {
                // Modifier
                match part {
                    "ctrl" | "control" => ctrl = true,
                    "shift" => shift = true,
                    "alt" => alt = true,
                    "super" | "cmd" | "meta" | "win" => super_key = true,
                    _ => return None,
                }
            }
        }

        key.map(|k| ParsedKeybinding {
            ctrl,
            shift,
            alt,
            super_key,
            key: k,
        })
    }
}

/// A parsed keybinding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKeybinding {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
    pub key: String,
}

/// Behavior configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    /// Lines of scrollback each pane keeps, at most
    /// [`MAX_SCROLLBACK_LINES`]. A config reload applies it to open panes
    /// too; lowering it drops their oldest lines.
    pub scrollback_lines: usize,
    /// Enable URL detection
    pub url_detection: bool,
    /// Copy on selection (like xterm)
    pub copy_on_select: bool,
    /// Enable visual bell (border flash)
    pub bell_visual: bool,
    /// Mark the window as wanting attention (the taskbar urgency hint) when
    /// a pane rings the bell while the window is unfocused. Ultty has no
    /// audible bell.
    pub bell_urgent: bool,
    /// Let the cursor blink when a program asks for a blinking cursor (the
    /// default shape blinks). When false the cursor is always steady. It
    /// blinks only in the focused window.
    pub cursor_blink: bool,
    /// Whether programs may set the clipboard with OSC 52.
    ///
    /// `"allow"` (the default) lets the focused pane write the clipboard;
    /// `"deny"` ignores every OSC 52 write. Any program whose output reaches
    /// the pane can write, including a `cat`ed file or a remote host over ssh.
    /// OSC 52 clipboard reads are always refused.
    pub clipboard_write: ClipboardWrite,
    /// Close a pane when its program exits, and the window when its last
    /// pane closes. When false, a pane whose program exited stays open with
    /// its last output until it is closed by hand.
    pub close_on_exit: bool,
    /// Window opacity (0.0-1.0, 1.0 = fully opaque)
    pub opacity: f32,
    /// Restore previous session on startup
    pub restore_session: bool,
    /// Open a new tab or split in the directory the focused pane is in, as
    /// its shell reports it (OSC 7), else the directory of the pane's
    /// foreground process. When false, or when neither is usable, a new pane
    /// starts in the directory Ultty was started in.
    pub inherit_working_directory: bool,
    /// Expose the window to assistive technology (screen readers) through
    /// AccessKit (the default). When false Ultty never connects to the
    /// accessibility bus. Read at start only.
    pub accessibility: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            url_detection: true,
            copy_on_select: true,
            bell_visual: true,
            bell_urgent: true,
            cursor_blink: true,
            clipboard_write: ClipboardWrite::Allow,
            close_on_exit: true,
            opacity: 1.0,
            restore_session: false, // Disabled by default
            inherit_working_directory: true,
            accessibility: true,
        }
    }
}

/// What programs in the terminal are told about it.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// `TERM` for shells and commands started in new tabs (splits share
    /// their tab's).
    ///
    /// `"xterm-256color"` (the default) is installed almost everywhere.
    /// `"xterm-ultty"` describes what Ultty implements more accurately, but
    /// needs its terminfo entry on every host it reaches, including hosts
    /// reached over ssh.
    pub term: String,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            term: crate::grid::DEFAULT_TERMINAL_NAME.to_string(),
        }
    }
}

impl TerminalConfig {
    /// Fall back to the default `term`, with a warning, unless it is a
    /// plausible terminfo name: 1 to 64 ASCII letters, digits, `.`, `_`,
    /// `+` or `-`. Anything else could not name an entry and would break
    /// every program that looks `TERM` up.
    pub fn validate_term(&mut self) {
        let valid = (1..=64).contains(&self.term.len())
            && self
                .term
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
        if !valid {
            let default = Self::default().term;
            warn!(
                "terminal.term {:?} is not a terminfo name; using {}",
                self.term, default
            );
            self.term = default;
        }
    }
}

/// Largest `behavior.scrollback_lines` accepted. History is stored
/// compactly, but a million long lines is still hundreds of megabytes per
/// pane.
pub const MAX_SCROLLBACK_LINES: usize = 1_000_000;

impl BehaviorConfig {
    /// Clamp `scrollback_lines` to [`MAX_SCROLLBACK_LINES`], warning when it
    /// was larger.
    pub fn clamp_scrollback(&mut self) {
        if self.scrollback_lines > MAX_SCROLLBACK_LINES {
            warn!(
                "behavior.scrollback_lines {} is too large; using {}",
                self.scrollback_lines, MAX_SCROLLBACK_LINES
            );
            self.scrollback_lines = MAX_SCROLLBACK_LINES;
        }
    }

    /// Put back the settings a reload cannot apply (`accessibility`) from
    /// `running`, so the config describes the app as it is. Returns whether
    /// the reloaded file asked for something different, which takes effect
    /// after a restart.
    pub fn keep_start_only(&mut self, running: &BehaviorConfig) -> bool {
        let changed = self.accessibility != running.accessibility;
        self.accessibility = running.accessibility;
        changed
    }
}

/// Policy for clipboard writes requested by programs (OSC 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardWrite {
    /// The focused pane may set the clipboard.
    #[default]
    Allow,
    /// OSC 52 writes are ignored.
    Deny,
}

/// Launch profiles configuration.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ProfilesConfig {
    /// Default profile to use when launching
    pub default: Option<String>,
    /// Named profiles
    #[serde(flatten)]
    pub profiles: HashMap<String, LaunchProfile>,
}

/// A launch profile defines a set of tools/tabs to open.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct LaunchProfile {
    /// Profile name (for display)
    pub name: String,
    /// Working directory for all tabs (if not overridden)
    pub working_dir: Option<String>,
    /// Tabs to create
    pub tabs: Vec<TabConfig>,
}

/// Configuration for a single tab in a launch profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct TabConfig {
    /// Tab title
    pub title: Option<String>,
    /// Command to run (defaults to shell)
    pub command: Option<String>,
    /// Arguments for the command
    pub args: Vec<String>,
    /// Working directory (overrides profile default)
    pub working_dir: Option<String>,
    /// Panes in this tab (for splits)
    pub panes: Vec<PaneConfig>,
}

/// Configuration for a pane in a tab.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct PaneConfig {
    /// Command to run
    pub command: Option<String>,
    /// Arguments
    pub args: Vec<String>,
    /// Working directory
    pub working_dir: Option<String>,
    /// Split direction ("horizontal" or "vertical")
    pub split: Option<String>,
}

/// RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse from hex string like "#RRGGBB" or "RRGGBB".
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }

        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;

        Some(Self { r, g, b })
    }

    /// Convert to f32 components (0.0-1.0).
    pub fn to_f32(&self) -> (f32, f32, f32) {
        (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        )
    }
}

/// RGBA color with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parse from hex string like "#RRGGBBAA" or "#RRGGBB".
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim_start_matches('#');
        match s.len() {
            6 => {
                let rgb = RgbColor::from_hex(s)?;
                Some(Self {
                    r: rgb.r,
                    g: rgb.g,
                    b: rgb.b,
                    a: 255,
                })
            }
            8 => {
                let r = u8::from_str_radix(&s[0..2], 16).ok()?;
                let g = u8::from_str_radix(&s[2..4], 16).ok()?;
                let b = u8::from_str_radix(&s[4..6], 16).ok()?;
                let a = u8::from_str_radix(&s[6..8], 16).ok()?;
                Some(Self { r, g, b, a })
            }
            _ => None,
        }
    }

    /// Convert to f32 components (0.0-1.0).
    pub fn to_f32(&self) -> (f32, f32, f32, f32) {
        (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        )
    }
}

/// Configuration error.
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "IO error: {}", e),
            ConfigError::Parse(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}

// Custom deserializer for RgbColor
fn deserialize_color<'de, D>(deserializer: D) -> Result<RgbColor, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    RgbColor::from_hex(&s).ok_or_else(|| serde::de::Error::custom("invalid color format"))
}

// Custom deserializer for RgbaColor
fn deserialize_color_with_alpha<'de, D>(deserializer: D) -> Result<RgbaColor, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    RgbaColor::from_hex(&s).ok_or_else(|| serde::de::Error::custom("invalid color format"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = Config::default();
        assert_eq!(config.font.family, DEFAULT_FONT_FAMILY);
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.font.emoji, "", "picked from the installed fonts");
        assert_eq!(config.behavior.scrollback_lines, 10_000);
    }

    #[test]
    fn terminal_term_defaults_to_xterm_256color() {
        assert_eq!(Config::default().terminal.term, "xterm-256color");
        let config: Config = toml::from_str("[behavior]\nopacity = 1.0\n").unwrap();
        assert_eq!(config.terminal.term, "xterm-256color");
    }

    #[test]
    fn terminal_term_is_read_from_the_terminal_section() {
        let config: Config = toml::from_str("[terminal]\nterm = \"xterm-ultty\"\n").unwrap();
        assert_eq!(config.terminal.term, "xterm-ultty");
    }

    #[test]
    fn invalid_terminal_term_falls_back_to_the_default() {
        for bad in ["", "xterm 256color", "a=b", "x\u{0}y", &"x".repeat(65)] {
            let mut terminal = TerminalConfig {
                term: bad.to_string(),
            };
            terminal.validate_term();
            assert_eq!(terminal.term, "xterm-256color", "for {bad:?}");
        }
        let mut terminal = TerminalConfig {
            term: "xterm-ultty".to_string(),
        };
        terminal.validate_term();
        assert_eq!(terminal.term, "xterm-ultty");
    }

    #[test]
    fn parse_rgb_color() {
        let color = RgbColor::from_hex("#FF8800").unwrap();
        assert_eq!(color.r, 0xFF);
        assert_eq!(color.g, 0x88);
        assert_eq!(color.b, 0x00);

        let color = RgbColor::from_hex("AABBCC").unwrap();
        assert_eq!(color.r, 0xAA);
        assert_eq!(color.g, 0xBB);
        assert_eq!(color.b, 0xCC);
    }

    #[test]
    fn parse_rgba_color() {
        let color = RgbaColor::from_hex("#FF880080").unwrap();
        assert_eq!(color.r, 0xFF);
        assert_eq!(color.g, 0x88);
        assert_eq!(color.b, 0x00);
        assert_eq!(color.a, 0x80);

        // 6-char falls back to full alpha
        let color = RgbaColor::from_hex("#FF8800").unwrap();
        assert_eq!(color.a, 255);
    }

    #[test]
    fn parse_keybinding() {
        let kb = KeybindingConfig::parse_keybinding("ctrl+shift+c").unwrap();
        assert!(kb.ctrl);
        assert!(kb.shift);
        assert!(!kb.alt);
        assert_eq!(kb.key, "c");

        let kb = KeybindingConfig::parse_keybinding("alt+enter").unwrap();
        assert!(kb.alt);
        assert!(!kb.ctrl);
        assert_eq!(kb.key, "enter");
    }

    #[test]
    fn palette_to_array() {
        let palette = PaletteConfig::default();
        let array = palette.to_array();
        assert_eq!(array.len(), 16);
        assert_eq!(array[0], palette.black);
        assert_eq!(array[15], palette.bright_white);
    }

    #[test]
    fn parse_toml_config() {
        let toml_str = r##"
[colors]
foreground = "#00FF00"
background = "#000000"

[font]
family = "Fira Code"
size = 16.0
emoji = "Twemoji"

[keybindings]
copy = "ctrl+c"
"##;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.colors.foreground.r, 0x00);
        assert_eq!(config.colors.foreground.g, 0xFF);
        assert_eq!(config.font.family, "Fira Code");
        assert_eq!(config.font.size, 16.0);
        assert_eq!(config.font.emoji, "Twemoji");
        assert_eq!(config.keybindings.copy, "ctrl+c");
    }

    #[test]
    fn font_size_is_clamped_to_sane_range() {
        assert_eq!(clamp_font_size(0.0), MIN_FONT_SIZE);
        assert_eq!(clamp_font_size(-3.0), MIN_FONT_SIZE);
        assert_eq!(clamp_font_size(1.0e9), MAX_FONT_SIZE);
        assert_eq!(clamp_font_size(f32::NAN), FontConfig::default().size);
        assert_eq!(clamp_font_size(14.0), 14.0);

        let mut font = FontConfig {
            size: 0.0,
            ..Default::default()
        };
        font.clamp_size();
        assert_eq!(font.size, MIN_FONT_SIZE);
    }

    #[test]
    fn loaded_config_clamps_font_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[font]\nsize = 0.0\n").unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.font.size, MIN_FONT_SIZE);
    }

    #[test]
    fn loaded_config_clamps_line_height_and_scrollback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[font]\nline_height = 0.0\n[behavior]\nscrollback_lines = 99999999999\n",
        )
        .unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.font.line_height, MIN_LINE_HEIGHT);
        assert_eq!(config.behavior.scrollback_lines, MAX_SCROLLBACK_LINES);

        let mut font = FontConfig {
            line_height: f32::NAN,
            ..Default::default()
        };
        font.clamp_line_height();
        assert_eq!(font.line_height, 1.0);
        font.line_height = 1.4;
        font.clamp_line_height();
        assert_eq!(font.line_height, 1.4);
    }

    #[test]
    fn loaded_config_rejects_an_invalid_terminal_term() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[terminal]\nterm = \"not a name\"\n").unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.terminal.term, "xterm-256color");
    }

    #[test]
    fn clipboard_write_defaults_to_allow_and_parses_deny() {
        assert_eq!(
            Config::default().behavior.clipboard_write,
            ClipboardWrite::Allow
        );

        let config: Config = toml::from_str("[behavior]\nclipboard_write = \"deny\"\n").unwrap();
        assert_eq!(config.behavior.clipboard_write, ClipboardWrite::Deny);

        assert!(toml::from_str::<Config>("[behavior]\nclipboard_write = \"maybe\"\n").is_err());
    }

    #[test]
    fn cursor_blink_defaults_on_and_can_be_turned_off() {
        assert!(Config::default().behavior.cursor_blink);

        let config: Config = toml::from_str("[behavior]\ncursor_blink = false\n").unwrap();
        assert!(!config.behavior.cursor_blink);
    }

    #[test]
    fn accessibility_is_on_unless_turned_off() {
        // On by default since the adapter's idle wakeups were traced to
        // blocking 1.7.0 and pinned away
        // (docs/perf/2026-11-08-accessibility-adapter.md).
        assert!(Config::default().behavior.accessibility);
        let config: Config = toml::from_str("[behavior]\ncursor_blink = false\n").unwrap();
        assert!(config.behavior.accessibility);

        let config: Config = toml::from_str("[behavior]\naccessibility = false\n").unwrap();
        assert!(!config.behavior.accessibility);
    }

    #[test]
    fn reload_keeps_the_running_accessibility_setting() {
        let running = BehaviorConfig {
            accessibility: false,
            ..BehaviorConfig::default()
        };
        let mut reloaded = BehaviorConfig {
            accessibility: true,
            cursor_blink: false,
            ..BehaviorConfig::default()
        };

        assert!(reloaded.keep_start_only(&running));
        assert!(!reloaded.accessibility);
        // Settings that do apply on reload are left alone.
        assert!(!reloaded.cursor_blink);

        assert!(!reloaded.keep_start_only(&running));
    }

    #[test]
    fn config_with_retired_tmux_table_still_loads() {
        let config: Config =
            toml::from_str("[tmux]\npassthrough = true\n[font]\nfamily = \"Fira Code\"\n").unwrap();
        assert_eq!(config.font.family, "Fira Code");
    }

    #[test]
    fn duplicate_bindings_detected() {
        let keybindings = KeybindingConfig {
            copy: "ctrl+shift+c".to_string(),
            paste: "ctrl+shift+c".to_string(),
            ..Default::default()
        };

        let duplicates = keybindings.duplicate_bindings();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].0, "ctrl+shift+c");
        assert!(duplicates[0].1.contains(&"copy".to_string()));
        assert!(duplicates[0].1.contains(&"paste".to_string()));
    }
}
