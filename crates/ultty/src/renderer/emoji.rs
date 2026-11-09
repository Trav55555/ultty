//! Which grapheme clusters are drawn as color emoji, and from which font.
//!
//! cosmic-text's font fallback on Linux tries DejaVu Sans before Noto Color
//! Emoji, and DejaVu Sans has monochrome outlines for many emoji (U+1F600
//! among them), so a cluster shaped with the configured monospace family
//! picks those. Clusters with emoji presentation are instead shaped with a
//! color emoji family directly, as other terminals do.

use std::cmp::Ordering;

/// Variation selector 15: text presentation.
const TEXT_SELECTOR: char = '\u{FE0E}';
/// Variation selector 16: emoji presentation.
const EMOJI_SELECTOR: char = '\u{FE0F}';

/// Color emoji families to use when `font.emoji` is unset, in order of
/// preference.
const KNOWN_EMOJI_FAMILIES: &[&str] = &[
    "Noto Color Emoji",
    "Apple Color Emoji",
    "Segoe UI Emoji",
    "Twemoji",
    "Twitter Color Emoji",
    "JoyPixels",
    "OpenMoji Color",
];

/// Whether `cluster` (one grapheme cluster) is drawn as emoji rather than
/// text: its first character has the Emoji_Presentation property and is not
/// followed by VS15, or the cluster asks for emoji with VS16 (`❤️`, `1️⃣`,
/// `🏳️‍🌈`).
pub(super) fn emoji_presentation(cluster: &str) -> bool {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    match chars.next() {
        Some(TEXT_SELECTOR) => false,
        Some(EMOJI_SELECTOR) => true,
        _ => has_emoji_presentation(first) || cluster.contains(EMOJI_SELECTOR),
    }
}

/// The installed color emoji family to draw emoji with: `configured` when it
/// is installed, else the first installed family of [`KNOWN_EMOJI_FAMILIES`].
/// Names match case-insensitively; the result is spelled as installed,
/// because font matching compares names exactly.
pub(super) fn pick_emoji_family<'a>(configured: &str, installed: &[&'a str]) -> Option<&'a str> {
    let find = |wanted: &str| {
        installed
            .iter()
            .copied()
            .find(|name| name.eq_ignore_ascii_case(wanted))
    };
    (!configured.is_empty())
        .then(|| find(configured))
        .flatten()
        .or_else(|| KNOWN_EMOJI_FAMILIES.iter().find_map(|&known| find(known)))
}

fn has_emoji_presentation(ch: char) -> bool {
    EMOJI_PRESENTATION
        .binary_search_by(|&(start, end)| {
            if end < ch {
                Ordering::Less
            } else if start > ch {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// The Emoji_Presentation=Yes codepoints of Unicode 17.0 (`emoji-data.txt`),
/// as sorted, disjoint ranges with adjacent ranges merged.
const EMOJI_PRESENTATION: &[(char, char)] = &[
    ('\u{231A}', '\u{231B}'),
    ('\u{23E9}', '\u{23EC}'),
    ('\u{23F0}', '\u{23F0}'),
    ('\u{23F3}', '\u{23F3}'),
    ('\u{25FD}', '\u{25FE}'),
    ('\u{2614}', '\u{2615}'),
    ('\u{2648}', '\u{2653}'),
    ('\u{267F}', '\u{267F}'),
    ('\u{2693}', '\u{2693}'),
    ('\u{26A1}', '\u{26A1}'),
    ('\u{26AA}', '\u{26AB}'),
    ('\u{26BD}', '\u{26BE}'),
    ('\u{26C4}', '\u{26C5}'),
    ('\u{26CE}', '\u{26CE}'),
    ('\u{26D4}', '\u{26D4}'),
    ('\u{26EA}', '\u{26EA}'),
    ('\u{26F2}', '\u{26F3}'),
    ('\u{26F5}', '\u{26F5}'),
    ('\u{26FA}', '\u{26FA}'),
    ('\u{26FD}', '\u{26FD}'),
    ('\u{2705}', '\u{2705}'),
    ('\u{270A}', '\u{270B}'),
    ('\u{2728}', '\u{2728}'),
    ('\u{274C}', '\u{274C}'),
    ('\u{274E}', '\u{274E}'),
    ('\u{2753}', '\u{2755}'),
    ('\u{2757}', '\u{2757}'),
    ('\u{2795}', '\u{2797}'),
    ('\u{27B0}', '\u{27B0}'),
    ('\u{27BF}', '\u{27BF}'),
    ('\u{2B1B}', '\u{2B1C}'),
    ('\u{2B50}', '\u{2B50}'),
    ('\u{2B55}', '\u{2B55}'),
    ('\u{1F004}', '\u{1F004}'),
    ('\u{1F0CF}', '\u{1F0CF}'),
    ('\u{1F18E}', '\u{1F18E}'),
    ('\u{1F191}', '\u{1F19A}'),
    ('\u{1F1E6}', '\u{1F1FF}'),
    ('\u{1F201}', '\u{1F201}'),
    ('\u{1F21A}', '\u{1F21A}'),
    ('\u{1F22F}', '\u{1F22F}'),
    ('\u{1F232}', '\u{1F236}'),
    ('\u{1F238}', '\u{1F23A}'),
    ('\u{1F250}', '\u{1F251}'),
    ('\u{1F300}', '\u{1F320}'),
    ('\u{1F32D}', '\u{1F335}'),
    ('\u{1F337}', '\u{1F37C}'),
    ('\u{1F37E}', '\u{1F393}'),
    ('\u{1F3A0}', '\u{1F3CA}'),
    ('\u{1F3CF}', '\u{1F3D3}'),
    ('\u{1F3E0}', '\u{1F3F0}'),
    ('\u{1F3F4}', '\u{1F3F4}'),
    ('\u{1F3F8}', '\u{1F43E}'),
    ('\u{1F440}', '\u{1F440}'),
    ('\u{1F442}', '\u{1F4FC}'),
    ('\u{1F4FF}', '\u{1F53D}'),
    ('\u{1F54B}', '\u{1F54E}'),
    ('\u{1F550}', '\u{1F567}'),
    ('\u{1F57A}', '\u{1F57A}'),
    ('\u{1F595}', '\u{1F596}'),
    ('\u{1F5A4}', '\u{1F5A4}'),
    ('\u{1F5FB}', '\u{1F64F}'),
    ('\u{1F680}', '\u{1F6C5}'),
    ('\u{1F6CC}', '\u{1F6CC}'),
    ('\u{1F6D0}', '\u{1F6D2}'),
    ('\u{1F6D5}', '\u{1F6D8}'),
    ('\u{1F6DC}', '\u{1F6DF}'),
    ('\u{1F6EB}', '\u{1F6EC}'),
    ('\u{1F6F4}', '\u{1F6FC}'),
    ('\u{1F7E0}', '\u{1F7EB}'),
    ('\u{1F7F0}', '\u{1F7F0}'),
    ('\u{1F90C}', '\u{1F93A}'),
    ('\u{1F93C}', '\u{1F945}'),
    ('\u{1F947}', '\u{1F9FF}'),
    ('\u{1FA70}', '\u{1FA7C}'),
    ('\u{1FA80}', '\u{1FA8A}'),
    ('\u{1FA8E}', '\u{1FAC6}'),
    ('\u{1FAC8}', '\u{1FAC8}'),
    ('\u{1FACD}', '\u{1FADC}'),
    ('\u{1FADF}', '\u{1FAEA}'),
    ('\u{1FAEF}', '\u{1FAF8}'),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_presentation_follows_the_property_and_variation_selectors() {
        let emoji = [
            "\u{1F600}",                                   // grinning face
            "\u{1F389}",                                   // party popper
            "\u{2764}\u{FE0F}",                            // heart, VS16
            "\u{263A}\u{FE0F}",                            // smiling face, VS16
            "\u{1F44D}\u{1F3FD}",                          // thumbs up, skin tone
            "\u{1F1E8}\u{1F1E6}",                          // flag of Canada
            "1\u{FE0F}\u{20E3}",                           // keycap one
            "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",          // rainbow flag
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", // family
            "\u{231A}",                                    // watch: first range
            "\u{1FAF8}",                                   // last range
        ];
        for cluster in emoji {
            assert!(emoji_presentation(cluster), "{cluster:?} is emoji");
        }
        let text = [
            "",
            "a",
            "1",
            "\u{2764}",          // heart alone defaults to text
            "\u{263A}",          // smiling face alone
            "\u{2122}",          // trade mark: pictographic, text by default
            "\u{2764}\u{FE0E}",  // heart, VS15
            "\u{1F600}\u{FE0E}", // grinning face, VS15
            "e\u{301}",          // accented letter
            "\u{2319}",          // just below the first range
            "\u{1FAF9}",         // just above the last range
            "\u{4E00}",          // CJK ideograph: wide, but not emoji
        ];
        for cluster in text {
            assert!(!emoji_presentation(cluster), "{cluster:?} is text");
        }
    }

    #[test]
    fn emoji_presentation_table_is_sorted_and_disjoint() {
        for pair in EMOJI_PRESENTATION.windows(2) {
            let ((start, end), (next, _)) = (pair[0], pair[1]);
            assert!(start <= end && end as u32 + 1 < next as u32, "{pair:?}");
        }
    }

    #[test]
    fn pick_emoji_family_prefers_the_configured_installed_family() {
        let installed = ["DejaVu Sans", "Twemoji", "Noto Color Emoji"];
        assert_eq!(pick_emoji_family("", &installed), Some("Noto Color Emoji"));
        assert_eq!(pick_emoji_family("twemoji", &installed), Some("Twemoji"));
        assert_eq!(
            pick_emoji_family("Missing Emoji", &installed),
            Some("Noto Color Emoji"),
            "a configured family that is not installed falls back"
        );
        assert_eq!(
            pick_emoji_family("", &["DejaVu Sans", "twitter color emoji"]),
            Some("twitter color emoji"),
            "spelled as installed"
        );
        assert_eq!(pick_emoji_family("", &["DejaVu Sans"]), None);
    }
}
