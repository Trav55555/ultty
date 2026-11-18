//! Colour themes: the ones bundled in the binary, the user's own, and the
//! registry that merges them.
//!
//! Bundled themes are the TOML files in `crates/ultty/themes/`, embedded at
//! build time (see `build.rs`) and written by `scripts/import-themes.py`.
//! A user theme is a file of the same format in `themes/` next to
//! `config.toml`; one with the same name replaces the bundled theme.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use tracing::warn;

use crate::config::{PaletteConfig, ThemeConfig};

include!(concat!(env!("OUT_DIR"), "/bundled_themes.rs"));

/// Palette keys a theme file must define.
const PALETTE_KEYS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

/// The key a theme is looked up by: lower case, with each run of other
/// characters as one dash, so `Catppuccin Mocha`, `catppuccin_mocha` and
/// `catppuccin-mocha` are the same theme. Keep in step with `slugify` in
/// scripts/import-themes.py.
pub fn normalize_name(name: &str) -> String {
    let mut key = String::with_capacity(name.len());
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            key.push(c);
        } else if !key.ends_with('-') {
            key.push('-');
        }
    }
    key.trim_matches('-').to_owned()
}

/// Parse a theme file. The 16 palette colours, foreground, background and
/// cursor must all be present; a theme that leaves one out would silently
/// borrow a default.
pub fn parse_theme(text: &str) -> Result<ThemeConfig, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    for key in ["foreground", "background", "cursor"] {
        if !table.contains_key(key) {
            return Err(format!("missing `{key}`"));
        }
    }
    let palette = table
        .get("palette")
        .and_then(toml::Value::as_table)
        .ok_or("missing [palette]")?;
    for key in PALETTE_KEYS {
        if !palette.contains_key(key) {
            return Err(format!("missing palette.{key}"));
        }
    }
    let theme: ThemeConfig = table
        .try_into()
        .map_err(|e: toml::de::Error| e.to_string())?;
    Ok(theme)
}

/// The themes bundled in the binary, keyed by file name, in name order.
pub fn bundled() -> &'static [(String, ThemeConfig)] {
    static PARSED: OnceLock<Vec<(String, ThemeConfig)>> = OnceLock::new();
    PARSED.get_or_init(|| {
        BUNDLED
            .iter()
            .filter_map(|(key, text)| match parse_theme(text) {
                Ok(theme) => Some(((*key).to_owned(), theme)),
                Err(error) => {
                    warn!("bundled theme {key} is invalid: {error}");
                    None
                }
            })
            .collect()
    })
}

/// Themes Ultty always had, written in code. Their palettes are tuned for
/// contrast, so bundled files do not replace them.
fn builtin() -> [(&'static str, ThemeConfig); 6] {
    [
        ("everforest-dark", ThemeConfig::everforest_dark()),
        ("tokyo-night", ThemeConfig::tokyo_night()),
        ("gruvbox-dark", ThemeConfig::gruvbox_dark()),
        ("catppuccin-mocha", ThemeConfig::catppuccin_mocha()),
        ("dark", ThemeConfig::dark()),
        ("light", ThemeConfig::light()),
    ]
}

/// Built-in and bundled themes.
pub fn default_registry() -> HashMap<String, ThemeConfig> {
    let mut themes: HashMap<String, ThemeConfig> = bundled()
        .iter()
        .map(|(key, theme)| (normalize_name(key), theme.clone()))
        .collect();
    themes.extend(builtin().map(|(key, theme)| (key.to_owned(), theme)));
    themes
}

/// The `*.toml` themes in `dir`, keyed by normalized file name. A missing
/// directory is no themes; an unreadable or invalid file is skipped with a
/// warning.
pub fn load_dir(dir: &Path) -> Vec<(String, ThemeConfig)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    let mut themes = Vec::new();
    for path in paths {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse_theme(&text));
        match loaded {
            Ok(theme) => themes.push((normalize_name(stem), theme)),
            Err(error) => warn!("ignoring theme {}: {error}", path.display()),
        }
    }
    themes
}

/// Build a palette from 16 colours in ANSI order.
pub(crate) fn palette_from_array(colors: [crate::config::RgbColor; 16]) -> PaletteConfig {
    let [black, red, green, yellow, blue, magenta, cyan, white, bright_black, bright_red, bright_green, bright_yellow, bright_blue, bright_magenta, bright_cyan, bright_white] =
        colors;
    PaletteConfig {
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    }
}

/// Palette keys in ANSI order, for overlaying explicit config values.
pub(crate) const fn palette_keys() -> [&'static str; 16] {
    PALETTE_KEYS
}

/// Print a theme list: names only, or with a colour swatch when `swatches`.
pub fn format_listing(themes: &[(String, ThemeConfig)], swatches: bool) -> String {
    let mut out = String::new();
    for (key, theme) in themes {
        out.push_str(key);
        if swatches {
            let bg = theme.background;
            let fg = theme.foreground;
            out.push_str(&format!(
                "  \x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m Abc \x1b[0m ",
                bg.r, bg.g, bg.b, fg.r, fg.g, fg.b
            ));
            let colors = theme
                .palette
                .as_ref()
                .map(PaletteConfig::to_array)
                .unwrap_or_else(|| PaletteConfig::default().to_array());
            for c in colors {
                out.push_str(&format!("\x1b[48;2;{};{};{}m \x1b[0m", c.r, c.g, c.b));
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColorConfig;
    use crate::renderer::ColorScheme;

    /// WCAG contrast of the selection fill against the screen background.
    /// 1.3:1 is the least that shows as a distinct band: Solarized, at 1.15,
    /// is barely there. It is far under WCAG's 3:1 for UI components on
    /// purpose, since a selection is a fill behind text and a 3:1 fill would
    /// override the theme's own look. Keep in step with
    /// `MIN_SELECTION_DISTINCT` in scripts/import-themes.py.
    const MIN_SELECTION_DISTINCT: f32 = 1.3;

    /// WCAG contrast of selected text on the selection fill. 3:1 is the AA
    /// floor for large text and UI, and what ANSI colours reach on most
    /// themes' own backgrounds. Keep in step with `MIN_SELECTION_TEXT` in
    /// scripts/import-themes.py.
    const MIN_SELECTION_TEXT: f32 = 3.0;

    #[test]
    fn bundled_themes_have_a_legible_selection() {
        let registry = default_registry();
        assert!(registry.len() >= 90, "only {} themes", registry.len());
        let mut failures = Vec::new();
        for (key, theme) in &registry {
            let mut colors = ColorConfig::default();
            colors.apply_theme(theme);
            let scheme = ColorScheme::from_config(&colors);
            let fill = scheme.selection_fill();

            let distinct = fill.contrast_ratio(&scheme.background);
            if distinct < MIN_SELECTION_DISTINCT {
                failures.push(format!("{key}: selection vs background {distinct:.2}"));
            }
            // The default text colour and every palette colour, as a shell
            // or a program can put any of them in a selection.
            let texts = std::iter::once(("foreground".to_owned(), scheme.foreground)).chain(
                scheme
                    .named
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (format!("ansi {i}"), *c)),
            );
            for (what, text) in texts {
                let shown = scheme.selection_text(text);
                let ratio = shown.contrast_ratio(&fill);
                if ratio < MIN_SELECTION_TEXT {
                    failures.push(format!("{key}: selected {what} text {ratio:.2}"));
                }
            }
        }
        failures.sort();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn a_theme_file_may_set_the_selection_foreground() {
        let with = format!("selection_foreground = \"#abcdef\"\n{SAMPLE}");
        let theme = parse_theme(&with).unwrap();
        assert_eq!(
            theme.selection_foreground,
            Some(crate::config::RgbColor::new(0xab, 0xcd, 0xef))
        );
        assert_eq!(parse_theme(SAMPLE).unwrap().selection_foreground, None);
        assert!(parse_theme(&format!("selection_foreground = \"nope\"\n{SAMPLE}")).is_err());
    }

    const SAMPLE: &str = r##"
name = "Mine"
foreground = "#111111"
background = "#222222"
cursor = "#333333"
[palette]
black = "#000001"
red = "#000002"
green = "#000003"
yellow = "#000004"
blue = "#000005"
magenta = "#000006"
cyan = "#000007"
white = "#000008"
bright_black = "#000009"
bright_red = "#00000a"
bright_green = "#00000b"
bright_yellow = "#00000c"
bright_blue = "#00000d"
bright_magenta = "#00000e"
bright_cyan = "#00000f"
bright_white = "#000010"
"##;

    #[test]
    fn names_ignore_case_spaces_and_underscores() {
        for name in [
            "Catppuccin Mocha",
            "catppuccin_mocha",
            "CATPPUCCIN-MOCHA",
            " catppuccin  mocha ",
        ] {
            assert_eq!(normalize_name(name), "catppuccin-mocha", "{name:?}");
        }
    }

    #[test]
    fn every_bundled_theme_is_complete() {
        assert!(BUNDLED.len() >= 50, "{} themes", BUNDLED.len());
        for (key, text) in BUNDLED {
            let theme = parse_theme(text).unwrap_or_else(|e| panic!("{key}: {e}"));
            let palette = theme.palette.unwrap_or_else(|| panic!("{key}: no palette"));
            assert_eq!(palette.to_array().len(), 16);
            assert!(!theme.name.is_empty(), "{key}: no name");
        }
        assert_eq!(
            bundled().len(),
            BUNDLED.len(),
            "a bundled theme failed to load"
        );
    }

    #[test]
    fn bundled_names_are_unique_and_normalized() {
        let mut seen = std::collections::HashSet::new();
        for (key, _) in BUNDLED {
            assert_eq!(
                normalize_name(key),
                *key,
                "file name is not a normalized name"
            );
            assert!(seen.insert(*key), "duplicate theme {key}");
        }
        let registry = default_registry();
        assert_eq!(
            registry.len(),
            BUNDLED.len() + 6 - overlap(),
            "a name is defined twice"
        );
    }

    /// Bundled themes that share a name with a built-in one.
    fn overlap() -> usize {
        builtin()
            .iter()
            .filter(|(key, _)| BUNDLED.iter().any(|(b, _)| b == key))
            .count()
    }

    #[test]
    fn bundled_themes_do_not_reuse_builtin_names() {
        assert_eq!(overlap(), 0);
    }

    #[test]
    fn wanted_themes_are_present() {
        let registry = default_registry();
        for key in [
            "catppuccin-latte",
            "dracula",
            "nord",
            "solarized-dark",
            "tokyo-night-storm",
            "one-half-dark",
            "rose-pine",
            "kanagawa-wave",
            "everforest-dark-hard",
            "github-dark",
            "github-light-default",
            "gruvbox-light",
        ] {
            assert!(registry.contains_key(key), "{key}");
        }
    }

    #[test]
    fn incomplete_themes_are_rejected() {
        assert!(parse_theme(&SAMPLE.replace("cursor = \"#333333\"\n", "")).is_err());
        assert!(parse_theme(&SAMPLE.replace("cyan = \"#000007\"\n", "")).is_err());
        assert!(parse_theme(SAMPLE).is_ok());
    }

    #[test]
    fn a_user_theme_overrides_a_bundled_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dracula.toml"), SAMPLE).unwrap();
        std::fs::write(dir.path().join("broken.toml"), "name = 1").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        let mut registry = default_registry();
        assert_ne!(registry["dracula"].name, "Mine");
        registry.extend(load_dir(dir.path()));

        assert_eq!(registry["dracula"].name, "Mine");
        assert!(!registry.contains_key("broken"));
        assert!(load_dir(&dir.path().join("absent")).is_empty());
    }

    #[test]
    fn listing_has_one_name_per_line() {
        let themes = vec![("mine".to_owned(), parse_theme(SAMPLE).unwrap())];
        assert_eq!(format_listing(&themes, false), "mine\n");
        assert!(format_listing(&themes, true).starts_with("mine  \x1b["));
    }
}
