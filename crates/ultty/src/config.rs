//! Configuration system for Ultty terminal emulator.
//!
//! Configuration is loaded from `~/.config/ultty/config.toml` if it exists,
//! otherwise sensible defaults are used.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Main configuration structure.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Color configuration
    pub colors: ColorConfig,
    /// Font configuration
    pub font: FontConfig,
    /// Keybinding configuration
    pub keybindings: KeybindingConfig,
    /// Behavior configuration
    pub behavior: BehaviorConfig,
    /// Launch profiles
    pub profiles: ProfilesConfig,
    /// Tmux integration configuration
    pub tmux: TmuxConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            colors: ColorConfig::default(),
            font: FontConfig::default(),
            keybindings: KeybindingConfig::default(),
            behavior: BehaviorConfig::default(),
            profiles: ProfilesConfig::default(),
            tmux: TmuxConfig::default(),
        }
    }
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
        toml::from_str(&contents).map_err(ConfigError::Parse)
    }

    /// Get the default config file path.
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("ultty").join("config.toml"))
    }

    /// Get the config directory path.
    pub fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("ultty"))
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
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            foreground: RgbColor::new(0xE5, 0xE5, 0xE5),
            background: RgbColor::new(0x1E, 0x1E, 0x1E),
            cursor: RgbColor::new(0xFF, 0xFF, 0xFF),
            selection: RgbaColor::new(0x4D, 0x4D, 0x4D, 0x80),
            palette: PaletteConfig::default(),
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
            white: RgbColor::new(0xE5, 0xE5, 0xE5),
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
}

/// Font configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font family name
    pub family: String,
    /// Font size in points
    pub size: f32,
    /// Line height multiplier
    pub line_height: f32,
    /// Enable ligatures
    pub ligatures: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: String::new(), // Empty means system monospace
            size: 14.0,
            line_height: 1.2,
            ligatures: true,
        }
    }
}

impl FontConfig {
    /// Check if using default system font.
    pub fn is_system_font(&self) -> bool {
        self.family.is_empty()
    }
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
    /// Reload configuration
    pub reload_config: String,
    /// Show keyboard shortcuts overlay
    pub show_shortcuts: String,
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
            reload_config: "ctrl+shift+r".to_string(),
            show_shortcuts: "f1".to_string(),
            custom: HashMap::new(),
        }
    }
}

impl KeybindingConfig {
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
    /// Scrollback buffer size (lines)
    pub scrollback_lines: usize,
    /// Enable URL detection
    pub url_detection: bool,
    /// Copy on selection (like xterm)
    pub copy_on_select: bool,
    /// Enable visual bell (border flash)
    pub bell_visual: bool,
    /// Enable audio bell (system sound) - off by default
    pub bell_sound: bool,
    /// Close window when shell exits
    pub close_on_exit: bool,
    /// Window opacity (0.0-1.0, 1.0 = fully opaque)
    pub opacity: f32,
    /// Restore previous session on startup
    pub restore_session: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            url_detection: true,
            copy_on_select: true,
            bell_visual: true,
            bell_sound: false, // Sound off by default
            close_on_exit: true,
            opacity: 1.0,
            restore_session: false, // Disabled by default
        }
    }
}

/// Tmux integration configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TmuxConfig {
    /// Enable tmux passthrough for escape sequences when running inside tmux.
    /// This allows OSC 52 clipboard, sixel graphics, etc. to reach the outer terminal.
    pub passthrough: bool,
    /// Automatically detect when running inside tmux and adjust behavior.
    pub auto_detect: bool,
    /// Session name for control mode (if launching tmux).
    pub session_name: Option<String>,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            passthrough: true,
            auto_detect: true,
            session_name: None,
        }
    }
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
pub struct LaunchProfile {
    /// Profile name (for display)
    pub name: String,
    /// Working directory for all tabs (if not overridden)
    pub working_dir: Option<String>,
    /// Tabs to create
    pub tabs: Vec<TabConfig>,
}

impl Default for LaunchProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            working_dir: None,
            tabs: Vec::new(),
        }
    }
}

/// Configuration for a single tab in a launch profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
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

impl Default for TabConfig {
    fn default() -> Self {
        Self {
            title: None,
            command: None,
            args: Vec::new(),
            working_dir: None,
            panes: Vec::new(),
        }
    }
}

/// Configuration for a pane in a tab.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
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

impl Default for PaneConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            working_dir: None,
            split: None,
        }
    }
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
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.behavior.scrollback_lines, 10_000);
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

[keybindings]
copy = "ctrl+c"
"##;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.colors.foreground.r, 0x00);
        assert_eq!(config.colors.foreground.g, 0xFF);
        assert_eq!(config.font.family, "Fira Code");
        assert_eq!(config.font.size, 16.0);
        assert_eq!(config.keybindings.copy, "ctrl+c");
    }

    #[test]
    fn tmux_config_defaults() {
        let config = TmuxConfig::default();
        assert!(config.passthrough);
        assert!(config.auto_detect);
        assert!(config.session_name.is_none());
    }

    #[test]
    fn parse_tmux_config() {
        let toml_str = r##"
[tmux]
passthrough = false
auto_detect = true
session_name = "dev"
"##;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.tmux.passthrough);
        assert!(config.tmux.auto_detect);
        assert_eq!(config.tmux.session_name, Some("dev".to_string()));
    }
}
