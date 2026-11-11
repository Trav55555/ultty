//! Pushed user overrides (ADR 0023 §9, spec §10.7).
//!
//! `ulttyctl override push` sends an override file's text to the native app
//! in a pane. The terminal relays it as `SetOverride`, the app parses it
//! as it would the file and answers `OverrideResult`. Gated by
//! `CAP_OVERRIDE_PUSH`.

use serde::{Deserialize, Serialize};

/// Most bytes of override source one push may carry.
///
/// Smaller than the 1 MiB override file limit because the push travels in
/// a control request, which the control socket caps at 1 MiB of JSON. JSON
/// escaping can grow text up to six times (a control character becomes
/// `\u00XX`), so 128 KiB of source always fits with room for the envelope,
/// and a push is refused for its size only by this limit, never by the
/// request cap.
pub const MAX_OVERRIDE_SOURCE_LEN: usize = 128 * 1024;

/// The language an override source is written in.
///
/// The discriminant is the wire value; the lowercase name is the JSON and
/// command-line spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum OverrideFormat {
    /// A `[widget.<id>]` / `[keys]` TOML file.
    Toml = 0,
    /// A Luau file returning `widget` and `key` hooks.
    Luau = 1,
}

impl OverrideFormat {
    /// Every format, in wire order.
    pub const ALL: [OverrideFormat; 2] = [OverrideFormat::Toml, OverrideFormat::Luau];

    /// The format with wire value `value`, if there is one.
    pub fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }

    /// The format an override file's name implies: Luau for `.luau`, TOML
    /// otherwise, as the file reader decides it.
    pub fn for_path(path: &std::path::Path) -> Self {
        if path.extension().is_some_and(|ext| ext == "luau") {
            OverrideFormat::Luau
        } else {
            OverrideFormat::Toml
        }
    }
}

/// An override source and its format, as pushed to an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedOverride {
    pub format: OverrideFormat,
    /// The file's text, at most [`MAX_OVERRIDE_SOURCE_LEN`] bytes.
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wire_values_are_the_discriminants() {
        for (index, format) in OverrideFormat::ALL.into_iter().enumerate() {
            assert_eq!(format as u8 as usize, index);
            assert_eq!(OverrideFormat::from_u8(format as u8), Some(format));
        }
        assert_eq!(OverrideFormat::from_u8(2), None);
        assert_eq!(OverrideFormat::from_u8(u8::MAX), None);
    }

    #[test]
    fn json_spelling_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&OverrideFormat::Luau).unwrap(),
            "\"luau\""
        );
        assert_eq!(
            serde_json::from_str::<OverrideFormat>("\"toml\"").unwrap(),
            OverrideFormat::Toml
        );
    }

    #[test]
    fn format_follows_the_file_extension() {
        assert_eq!(
            OverrideFormat::for_path(Path::new("app.luau")),
            OverrideFormat::Luau
        );
        assert_eq!(
            OverrideFormat::for_path(Path::new("app.toml")),
            OverrideFormat::Toml
        );
        assert_eq!(
            OverrideFormat::for_path(Path::new("overrides")),
            OverrideFormat::Toml
        );
    }
}
