//! Cheap answers to "does this codepoint join the previous grapheme
//! cluster?" for the text terminals mostly print.
//!
//! The general answer runs the UAX #29 segmenter over the stored cluster
//! plus the new codepoint, which costs a few hundred instructions per
//! printed character. For most text the answer follows from two small
//! classes of codepoint:
//!
//! - *plain*: Grapheme_Cluster_Break is Other (including
//!   Extended_Pictographic), LV or LVT, and the codepoint is not an Indic
//!   conjunct consonant.
//! - *extend*: Grapheme_Cluster_Break is Extend: combining diacritics,
//!   variation selectors, emoji modifiers, tags.
//!
//! Nothing joins a plain codepoint to a cluster ending in plain or extend
//! (only a ZWJ before it could), and extend codepoints and ZWJ always
//! join a cluster made of a plain codepoint followed by more of them.
//!
//! Emoji sequences need two more rules, so for clusters made only of plain,
//! extend, ZWJ and regional indicator codepoints [`joins`] also runs the
//! few UAX #29 rules that can apply to them:
//!
//! - GB9: extend and ZWJ join whatever comes before them.
//! - GB11: an Extended_Pictographic codepoint joins after a ZWJ that
//!   follows another one and any extends (`👨‍👩`); the plain codepoints
//!   that are pictographic are listed in [`PICTOGRAPHIC`].
//! - GB12/13: regional indicators join in pairs (`🇯🇵`).
//!
//! Every other rule needs a codepoint outside these classes (controls,
//! Prepend, SpacingMark for GB9a, Hangul jamo, Indic conjuncts for GB9c),
//! so meeting one leaves the answer to the segmenter.
//!
//! The ranges are a conservative subset; the tests check every codepoint in
//! them against the segmenter, so a Unicode update that moves one fails
//! loudly instead of changing behaviour.

pub(super) use super::char_class::JoinClass;
use super::char_class::{in_ranges, is_extend, is_plain, props_bits, CLASS_SHIFT, ZWJ};
#[cfg(test)]
use super::char_class::{is_joiner, PLAIN};

// The table `build.rs` generates from `props_bits`: TABLE_LIMIT, BLOCK_SHIFT,
// BLOCK_INDEX and BLOCKS.
include!(concat!(env!("OUT_DIR"), "/char_props.rs"));

/// What printing a codepoint needs to know about it: the columns it takes
/// when it starts a cluster and its [`JoinClass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CharProps(u8);

impl CharProps {
    /// `c`'s properties: a table lookup for all but the rarest codepoints.
    #[inline]
    pub(super) fn of(c: char) -> Self {
        let cp = c as u32;
        if cp < TABLE_LIMIT {
            let block = BLOCK_INDEX[(cp >> BLOCK_SHIFT) as usize];
            let offset = cp & ((1 << BLOCK_SHIFT) - 1);
            Self(BLOCKS[usize::from(block)][offset as usize])
        } else {
            Self(props_bits(c))
        }
    }

    /// Columns taken when the codepoint starts a cluster: 0 to 3, as
    /// `unicode-width` has it (control characters take none).
    #[inline]
    pub(super) fn width(self) -> u16 {
        u16::from(self.0 & ((1 << CLASS_SHIFT) - 1))
    }

    #[inline]
    pub(super) fn class(self) -> JoinClass {
        match self.0 >> CLASS_SHIFT {
            1 => JoinClass::Plain,
            2 => JoinClass::Joiner,
            _ => JoinClass::Other,
        }
    }
}

/// The Extended_Pictographic codepoints of the plain class, as sorted,
/// disjoint ranges: exactly those (the tests check every plain codepoint).
const PICTOGRAPHIC: &[(char, char)] = &[
    ('\u{A9}', '\u{A9}'),
    ('\u{AE}', '\u{AE}'),
    ('\u{203C}', '\u{203C}'),
    ('\u{2049}', '\u{2049}'),
    ('\u{2122}', '\u{2122}'),
    ('\u{2139}', '\u{2139}'),
    ('\u{2194}', '\u{2199}'),
    ('\u{21A9}', '\u{21AA}'),
    ('\u{231A}', '\u{231B}'),
    ('\u{2328}', '\u{2328}'),
    ('\u{23CF}', '\u{23CF}'),
    ('\u{23E9}', '\u{23F3}'),
    ('\u{23F8}', '\u{23FA}'),
    ('\u{24C2}', '\u{24C2}'),
    ('\u{25AA}', '\u{25AB}'),
    ('\u{25B6}', '\u{25B6}'),
    ('\u{25C0}', '\u{25C0}'),
    ('\u{25FB}', '\u{25FE}'),
    ('\u{2600}', '\u{2604}'),
    ('\u{260E}', '\u{260E}'),
    ('\u{2611}', '\u{2611}'),
    ('\u{2614}', '\u{2615}'),
    ('\u{2618}', '\u{2618}'),
    ('\u{261D}', '\u{261D}'),
    ('\u{2620}', '\u{2620}'),
    ('\u{2622}', '\u{2623}'),
    ('\u{2626}', '\u{2626}'),
    ('\u{262A}', '\u{262A}'),
    ('\u{262E}', '\u{262F}'),
    ('\u{2638}', '\u{263A}'),
    ('\u{2640}', '\u{2640}'),
    ('\u{2642}', '\u{2642}'),
    ('\u{2648}', '\u{2653}'),
    ('\u{265F}', '\u{2660}'),
    ('\u{2663}', '\u{2663}'),
    ('\u{2665}', '\u{2666}'),
    ('\u{2668}', '\u{2668}'),
    ('\u{267B}', '\u{267B}'),
    ('\u{267E}', '\u{267F}'),
    ('\u{2692}', '\u{2697}'),
    ('\u{2699}', '\u{2699}'),
    ('\u{269B}', '\u{269C}'),
    ('\u{26A0}', '\u{26A1}'),
    ('\u{26A7}', '\u{26A7}'),
    ('\u{26AA}', '\u{26AB}'),
    ('\u{26B0}', '\u{26B1}'),
    ('\u{26BD}', '\u{26BE}'),
    ('\u{26C4}', '\u{26C5}'),
    ('\u{26C8}', '\u{26C8}'),
    ('\u{26CE}', '\u{26CF}'),
    ('\u{26D1}', '\u{26D1}'),
    ('\u{26D3}', '\u{26D4}'),
    ('\u{26E9}', '\u{26EA}'),
    ('\u{26F0}', '\u{26F5}'),
    ('\u{26F7}', '\u{26FA}'),
    ('\u{26FD}', '\u{26FD}'),
    ('\u{2702}', '\u{2702}'),
    ('\u{2705}', '\u{2705}'),
    ('\u{2708}', '\u{270D}'),
    ('\u{270F}', '\u{270F}'),
    ('\u{2712}', '\u{2712}'),
    ('\u{2714}', '\u{2714}'),
    ('\u{2716}', '\u{2716}'),
    ('\u{271D}', '\u{271D}'),
    ('\u{2721}', '\u{2721}'),
    ('\u{2728}', '\u{2728}'),
    ('\u{2733}', '\u{2734}'),
    ('\u{2744}', '\u{2744}'),
    ('\u{2747}', '\u{2747}'),
    ('\u{274C}', '\u{274C}'),
    ('\u{274E}', '\u{274E}'),
    ('\u{2753}', '\u{2755}'),
    ('\u{2757}', '\u{2757}'),
    ('\u{2763}', '\u{2764}'),
    ('\u{2795}', '\u{2797}'),
    ('\u{27A1}', '\u{27A1}'),
    ('\u{27B0}', '\u{27B0}'),
    ('\u{27BF}', '\u{27BF}'),
    ('\u{2934}', '\u{2935}'),
    ('\u{2B05}', '\u{2B07}'),
    ('\u{2B1B}', '\u{2B1C}'),
    ('\u{2B50}', '\u{2B50}'),
    ('\u{2B55}', '\u{2B55}'),
    ('\u{3030}', '\u{3030}'),
    ('\u{303D}', '\u{303D}'),
    ('\u{3297}', '\u{3297}'),
    ('\u{3299}', '\u{3299}'),
    ('\u{1F004}', '\u{1F004}'),
    ('\u{1F02C}', '\u{1F02F}'),
    ('\u{1F094}', '\u{1F09F}'),
    ('\u{1F0AF}', '\u{1F0B0}'),
    ('\u{1F0C0}', '\u{1F0C0}'),
    ('\u{1F0CF}', '\u{1F0D0}'),
    ('\u{1F0F6}', '\u{1F0FF}'),
    ('\u{1F170}', '\u{1F171}'),
    ('\u{1F17E}', '\u{1F17F}'),
    ('\u{1F18E}', '\u{1F18E}'),
    ('\u{1F191}', '\u{1F19A}'),
    ('\u{1F1AE}', '\u{1F1E5}'),
    ('\u{1F201}', '\u{1F20F}'),
    ('\u{1F21A}', '\u{1F21A}'),
    ('\u{1F22F}', '\u{1F22F}'),
    ('\u{1F232}', '\u{1F23A}'),
    ('\u{1F23C}', '\u{1F23F}'),
    ('\u{1F249}', '\u{1F25F}'),
    ('\u{1F266}', '\u{1F321}'),
    ('\u{1F324}', '\u{1F393}'),
    ('\u{1F396}', '\u{1F397}'),
    ('\u{1F399}', '\u{1F39B}'),
    ('\u{1F39E}', '\u{1F3F0}'),
    ('\u{1F3F3}', '\u{1F3F5}'),
    ('\u{1F3F7}', '\u{1F3FA}'),
    ('\u{1F400}', '\u{1F4FD}'),
    ('\u{1F4FF}', '\u{1F53D}'),
    ('\u{1F549}', '\u{1F54E}'),
    ('\u{1F550}', '\u{1F567}'),
    ('\u{1F56F}', '\u{1F570}'),
    ('\u{1F573}', '\u{1F57A}'),
    ('\u{1F587}', '\u{1F587}'),
    ('\u{1F58A}', '\u{1F58D}'),
    ('\u{1F590}', '\u{1F590}'),
    ('\u{1F595}', '\u{1F596}'),
    ('\u{1F5A4}', '\u{1F5A5}'),
    ('\u{1F5A8}', '\u{1F5A8}'),
    ('\u{1F5B1}', '\u{1F5B2}'),
    ('\u{1F5BC}', '\u{1F5BC}'),
    ('\u{1F5C2}', '\u{1F5C4}'),
    ('\u{1F5D1}', '\u{1F5D3}'),
    ('\u{1F5DC}', '\u{1F5DE}'),
    ('\u{1F5E1}', '\u{1F5E1}'),
    ('\u{1F5E3}', '\u{1F5E3}'),
    ('\u{1F5E8}', '\u{1F5E8}'),
    ('\u{1F5EF}', '\u{1F5EF}'),
    ('\u{1F5F3}', '\u{1F5F3}'),
    ('\u{1F5FA}', '\u{1F64F}'),
    ('\u{1F680}', '\u{1F6C5}'),
    ('\u{1F6CB}', '\u{1F6D2}'),
    ('\u{1F6D5}', '\u{1F6E5}'),
    ('\u{1F6E9}', '\u{1F6E9}'),
    ('\u{1F6EB}', '\u{1F6F0}'),
    ('\u{1F6F3}', '\u{1F6FF}'),
    ('\u{1F7DA}', '\u{1F7FF}'),
    ('\u{1F80C}', '\u{1F80F}'),
    ('\u{1F848}', '\u{1F84F}'),
    ('\u{1F85A}', '\u{1F85F}'),
    ('\u{1F888}', '\u{1F88F}'),
    ('\u{1F8AE}', '\u{1F8AF}'),
    ('\u{1F8BC}', '\u{1F8BF}'),
    ('\u{1F8C2}', '\u{1F8CF}'),
    ('\u{1F8D9}', '\u{1F8FF}'),
    ('\u{1F90C}', '\u{1F93A}'),
    ('\u{1F93C}', '\u{1F945}'),
    ('\u{1F947}', '\u{1F9FF}'),
    ('\u{1FA58}', '\u{1FA5F}'),
    ('\u{1FA6E}', '\u{1FAFF}'),
];

fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

/// The classes [`joins_by_rules`] knows the rules for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Plain { pictographic: bool },
    Extend,
    Zwj,
    RegionalIndicator,
}

impl Class {
    fn of(c: char) -> Option<Self> {
        if c == ZWJ {
            Some(Self::Zwj)
        } else if is_regional_indicator(c) {
            Some(Self::RegionalIndicator)
        } else if is_extend(c) {
            Some(Self::Extend)
        } else if is_plain(c) {
            Some(Self::Plain {
                pictographic: in_ranges(PICTOGRAPHIC, c),
            })
        } else {
            None
        }
    }
}

/// Whether `c` joins the cluster `base` + `extra`, when that follows from
/// the classes above; `None` when only the segmenter can tell.
pub(super) fn joins(base: char, extra: &[char], c: char) -> Option<bool> {
    let class = |c| CharProps::of(c).class();
    // The common cases first: plain text, and marks on a plain character.
    match class(c) {
        JoinClass::Plain => {
            let last = extra.last().copied().unwrap_or(base);
            if last != ZWJ && class(last) != JoinClass::Other {
                return Some(false);
            }
        }
        JoinClass::Joiner
            if class(base) == JoinClass::Plain
                && extra.iter().all(|&e| class(e) == JoinClass::Joiner) =>
        {
            return Some(true);
        }
        _ => {}
    }
    // Rule out the codepoints the rules do not cover before walking the
    // cluster.
    Class::of(c)?;
    joins_by_rules(
        std::iter::once(base)
            .chain(extra.iter().copied())
            .chain([c]),
    )
}

/// Whether `codepoints` form one grapheme cluster, by the rules in the
/// module docs; `None` if one of them is outside the classes those rules
/// cover before a break settles the answer.
fn joins_by_rules(codepoints: impl Iterator<Item = char>) -> Option<bool> {
    let mut first = true;
    // An Extended_Pictographic codepoint and extends end here (GB11).
    let mut pictograph = false;
    // ... followed by a ZWJ.
    let mut pictograph_zwj = false;
    // The regional indicators ending here are odd in number (GB12/13).
    let mut odd_indicators = false;
    for c in codepoints {
        let class = Class::of(c)?;
        if !first {
            let joined = match class {
                Class::Extend | Class::Zwj => true,
                Class::RegionalIndicator => odd_indicators,
                Class::Plain { pictographic } => pictographic && pictograph_zwj,
            };
            if !joined {
                return Some(false);
            }
        }
        pictograph_zwj = class == Class::Zwj && pictograph;
        pictograph = matches!(class, Class::Plain { pictographic: true })
            || (class == Class::Extend && pictograph);
        odd_indicators = class == Class::RegionalIndicator && !odd_indicators;
        first = false;
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

    fn clusters(s: &str) -> usize {
        s.graphemes(true).count()
    }

    fn all(class: fn(char) -> bool) -> impl Iterator<Item = char> {
        (0..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(move |&c| class(c))
    }

    /// What `joins` claims for a pair, checked by the segmenter.
    fn segmenter_joins(cluster: &str, c: char) -> bool {
        clusters(&format!("{cluster}{c}")) == 1
    }

    /// The generated table holds what [`props_bits`] computes for every
    /// codepoint, and that is `unicode-width`'s width and the classes
    /// above.
    #[test]
    fn char_props_match_unicode_width_and_the_classes_for_every_codepoint() {
        use unicode_width::UnicodeWidthChar;
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            let props = CharProps::of(c);
            assert_eq!(props, CharProps(props_bits(c)), "{c:?}");
            assert_eq!(props.width(), c.width().unwrap_or(0) as u16, "{c:?}");
            let class = if is_plain(c) {
                JoinClass::Plain
            } else if is_joiner(c) {
                JoinClass::Joiner
            } else {
                JoinClass::Other
            };
            assert_eq!(props.class(), class, "{c:?}");
        }
    }

    #[test]
    fn ranges_are_sorted_and_disjoint() {
        for ranges in [PLAIN, PICTOGRAPHIC] {
            for pair in ranges.windows(2) {
                assert!(pair[0].0 <= pair[0].1 && pair[0].1 < pair[1].0, "{pair:?}");
            }
        }
    }

    /// GB11 in both directions: a plain codepoint joins after `😀` ZWJ, and
    /// `😀` joins after it and a ZWJ, exactly when it is in PICTOGRAPHIC.
    #[test]
    fn pictographic_is_exactly_the_extended_pictographic_plain_codepoints() {
        for ranges in PICTOGRAPHIC {
            assert!(is_plain(ranges.0) && is_plain(ranges.1), "{ranges:?}");
        }
        for c in all(is_plain) {
            let pictographic = in_ranges(PICTOGRAPHIC, c);
            assert_eq!(
                segmenter_joins("\u{1F600}\u{200D}", c),
                pictographic,
                "{c:?} after a pictograph and ZWJ"
            );
            assert_eq!(
                segmenter_joins(&format!("{c}\u{200D}"), '\u{1F600}'),
                pictographic,
                "a pictograph after {c:?} and ZWJ"
            );
        }
    }

    #[test]
    fn regional_indicators_pair_up() {
        for a in all(is_regional_indicator) {
            for b in all(is_regional_indicator) {
                assert_eq!(joins(a, &[], b), Some(true), "{a:?} {b:?}");
                assert!(segmenter_joins(&a.to_string(), b));
                for c in all(is_regional_indicator) {
                    assert_eq!(joins(a, &[b], c), Some(false), "{a:?} {b:?} {c:?}");
                    assert!(!segmenter_joins(&format!("{a}{b}"), c));
                }
            }
        }
    }

    /// Every cluster of up to four codepoints drawn from samples of each
    /// class `joins` knows, and of the classes it leaves to the segmenter,
    /// with every codepoint after it: `joins` agrees with the segmenter
    /// whenever it answers, and answers whenever every codepoint is in a
    /// class it knows.
    #[test]
    fn joins_agrees_with_the_segmenter_on_every_short_sequence() {
        let known = [
            'a',
            '\u{AC00}',  // LV
            '\u{AC01}',  // LVT
            '\u{1F600}', // Extended_Pictographic
            '\u{2764}',  // Extended_Pictographic, text presentation
            '\u{A9}',    // Extended_Pictographic in Latin-1
            '\u{301}',   // Extend
            '\u{1F3FD}', // Extend: emoji modifier
            '\u{FE0F}',  // Extend: VS16
            '\u{E0067}', // Extend: tag
            ZWJ,
            '\u{1F1EF}', // Regional indicators
            '\u{1F1F5}',
        ];
        let unknown = [
            '\u{903}',  // SpacingMark (GB9a)
            '\u{915}',  // InCB consonant (GB9c)
            '\u{94D}',  // InCB linker (GB9c)
            '\u{1100}', // L
            '\u{1161}', // V
            '\u{11A8}', // T
            '\u{600}',  // Prepend (GB9b)
            '\u{AD}',   // Control
            '\u{200C}', // ZWNJ: Extend outside the extend class
        ];
        let alphabet: Vec<char> = known.iter().chain(&unknown).copied().collect();
        let mut sequences: Vec<Vec<char>> = alphabet.iter().map(|&c| vec![c]).collect();
        let mut start = 0;
        for _ in 1..4 {
            let end = sequences.len();
            for index in start..end {
                for &c in &alphabet {
                    let mut longer = sequences[index].clone();
                    longer.push(c);
                    sequences.push(longer);
                }
            }
            start = end;
        }
        for cluster in &sequences {
            let (&base, extra) = cluster.split_first().unwrap();
            let text: String = cluster.iter().collect();
            for &c in &alphabet {
                let answer = joins(base, extra, c);
                if let Some(answer) = answer {
                    assert_eq!(answer, segmenter_joins(&text, c), "{text:?} + {c:?}");
                }
                if cluster.iter().chain([&c]).all(|c| known.contains(c)) {
                    assert!(answer.is_some(), "{text:?} + {c:?} left to the segmenter");
                }
            }
        }
    }

    #[test]
    fn nothing_joins_a_plain_codepoint_to_plain_text_or_a_diacritic() {
        // One representative cluster of each break class a plain or
        // diacritic codepoint can end a cluster with, and the conjunct
        // context that would join a following consonant.
        let before = [
            "a",
            "\u{1F600}",
            "\u{AC00}",
            "\u{AC01}",
            "a\u{301}",
            "\u{1F600}\u{1F3FD}",
            "\u{1F1E6}",
            "\u{915}\u{94D}",
            "\u{915}\u{94D}\u{301}",
        ];
        for c in all(is_plain) {
            for cluster in before {
                assert!(
                    !segmenter_joins(cluster, c),
                    "{c:?} joins {cluster:?}, so it is not plain"
                );
            }
        }
    }

    #[test]
    fn a_plain_codepoint_joins_nothing_after_it_but_extends() {
        // Rules out Prepend, L and ZWJ, the classes that join what follows,
        // and Control, which a diacritic does not join.
        for c in all(is_plain) {
            for after in ['a', '\u{AC00}', '\u{1F600}'] {
                assert!(
                    !segmenter_joins(&c.to_string(), after),
                    "{after:?} joins {c:?}, so it is not plain"
                );
            }
            assert!(
                segmenter_joins(&c.to_string(), '\u{301}'),
                "a diacritic does not join {c:?}, so it is not plain"
            );
        }
    }

    #[test]
    fn extends_join_and_are_joined_by_nothing_plain() {
        for m in all(is_extend) {
            for cluster in ["a", "\u{1F600}", "a\u{301}", "a\u{200D}"] {
                assert!(
                    segmenter_joins(cluster, m),
                    "{m:?} does not join {cluster:?}"
                );
            }
            // Rules out ZWJ, which joins a following pictograph, and every
            // class that joins what follows it.
            for after in ['b', '\u{1F600}', '\u{AC00}', '\u{AC01}'] {
                let cluster = format!("\u{1F600}{m}");
                assert!(
                    !segmenter_joins(&cluster, after),
                    "{after:?} joins {cluster:?}"
                );
            }
            assert!(segmenter_joins(&format!("a{m}"), '\u{301}'), "after {m:?}");
            assert!(segmenter_joins(&format!("a{m}"), ZWJ), "ZWJ after {m:?}");
        }
        for cluster in ["a", "a\u{301}", "a\u{200D}"] {
            assert!(
                segmenter_joins(cluster, ZWJ),
                "ZWJ does not join {cluster:?}"
            );
        }
    }

    #[test]
    fn joins_agrees_with_the_segmenter_on_samples() {
        let cases: &[(char, &[char], char)] = &[
            ('a', &[], 'b'),
            ('a', &[], '\u{301}'),
            ('a', &['\u{301}'], '\u{302}'),
            ('a', &['\u{301}'], '\u{4E2D}'),
            ('\u{4E2D}', &[], '\u{65E5}'),
            ('\u{1F600}', &[], '\u{1F680}'),
            ('\u{AC00}', &[], '\u{AC01}'),
            ('e', &['\u{301}', '\u{303}'], 'x'),
            ('\u{1F468}', &[], '\u{200D}'),
            ('\u{1F44D}', &[], '\u{1F3FD}'),
            ('\u{1F44D}', &['\u{1F3FD}'], '\u{1F680}'),
            ('\u{2764}', &[], '\u{FE0F}'),
            ('\u{1F3F3}', &['\u{FE0F}'], '\u{200D}'),
            ('\u{1F468}', &['\u{200D}'], '\u{1F469}'),
            ('\u{1F3F3}', &['\u{FE0F}', '\u{200D}'], '\u{1F308}'),
            ('\u{1F9D1}', &['\u{1F3FD}', '\u{200D}'], '\u{1F4BB}'),
            ('a', &['\u{200D}'], '\u{1F469}'),
            ('\u{1F468}', &['\u{200D}', '\u{301}'], '\u{1F469}'),
            ('\u{1F1EF}', &[], '\u{1F1F5}'),
            ('\u{1F1EF}', &['\u{1F1F5}'], '\u{1F1FA}'),
            ('\u{1F1EF}', &['\u{1F1F5}'], '\u{FE0F}'),
            ('\u{1F1EF}', &[], 'a'),
            ('a', &[], '\u{1F1EF}'),
        ];
        for &(base, extra, c) in cases {
            let mut cluster: String = extra.iter().collect();
            cluster.insert(0, base);
            assert_eq!(
                joins(base, extra, c),
                Some(segmenter_joins(&cluster, c)),
                "{cluster:?} + {c:?}"
            );
        }
    }

    #[test]
    fn leaves_the_rest_to_the_segmenter() {
        // Hangul jamo, Indic conjuncts and spacing marks, and controls
        // need the full rules.
        let cases: &[(char, &[char], char)] = &[
            ('\u{1100}', &[], '\u{1161}'),
            ('\u{915}', &['\u{94D}'], '\u{915}'),
            ('a', &[], '\u{903}'),
            ('\u{1F468}', &['\u{200D}'], '\u{AD}'),
            ('\u{915}', &[], '\u{94D}'),
            ('\u{AD}', &[], '\u{301}'),
        ];
        for &(base, extra, c) in cases {
            assert_eq!(joins(base, extra, c), None, "{base:?} {extra:?} + {c:?}");
        }
    }
}
