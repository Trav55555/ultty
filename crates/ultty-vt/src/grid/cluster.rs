//! Cheap answers to "does this codepoint join the previous grapheme
//! cluster?" for the text terminals mostly print.
//!
//! The general answer runs the UAX #29 segmenter over the stored cluster
//! plus the new codepoint, which costs a few hundred instructions per
//! printed character. For most text the answer follows from a few classes
//! of codepoint (`char_class.rs`):
//!
//! - *plain*: Grapheme_Cluster_Break is Other (including
//!   Extended_Pictographic), LV or LVT, and the codepoint is not an Indic
//!   conjunct consonant.
//! - *extend*: Grapheme_Cluster_Break is Extend and Indic_Conjunct_Break
//!   is Extend: combining diacritics, Indic vowel signs and nuktas,
//!   variation selectors, emoji modifiers, tags.
//! - *linker*: the Indic viramas that make conjuncts
//!   (Indic_Conjunct_Break=Linker); Grapheme_Cluster_Break is Extend.
//! - *spacing mark*: Grapheme_Cluster_Break is SpacingMark (Indic vowel
//!   signs that take a column).
//! - *consonant*: Indic_Conjunct_Break is Consonant.
//!
//! Extend, ZWJ, linkers and spacing marks join any cluster whose last
//! codepoint is in these classes (GB9, GB9a). A plain codepoint joins none
//! of them (only a ZWJ before it could), and a consonant only a conjunct:
//! a consonant, then extends and linkers with at least one linker (GB9c).
//! The grid keeps track of which of these a printed cluster ends with as
//! a [`Tail`], so [`joins_tail`] answers from that and the new
//! codepoint's class, one table lookup.
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
//! Prepend, Hangul jamo), so meeting one leaves the answer to the
//! segmenter.
//!
//! The ranges are a conservative subset (the consonants and linkers are
//! exact); the tests check every codepoint in them against the segmenter,
//! so a Unicode update that moves one fails loudly instead of changing
//! behaviour.

pub(super) use super::char_class::JoinClass;
#[cfg(test)]
use super::char_class::{
    in_ranges, is_extend, is_joiner, is_plain, is_regional_indicator, CONSONANT, EXTEND, LINKER,
    PICTOGRAPHIC, PLAIN, SPACING_MARK,
};
use super::char_class::{props_bits, CLASS_SHIFT, ZWJ};

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
            3 => JoinClass::Linker,
            4 => JoinClass::SpacingMark,
            5 => JoinClass::Consonant,
            6 => JoinClass::RegionalIndicator,
            7 => JoinClass::Pictograph,
            _ => JoinClass::Other,
        }
    }
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
        match CharProps::of(c).class() {
            JoinClass::Joiner if c == ZWJ => Some(Self::Zwj),
            JoinClass::Joiner => Some(Self::Extend),
            JoinClass::RegionalIndicator => Some(Self::RegionalIndicator),
            JoinClass::Plain => Some(Self::Plain {
                pictographic: false,
            }),
            JoinClass::Pictograph => Some(Self::Plain { pictographic: true }),
            _ => None,
        }
    }
}

/// How a printed cluster ends, as far as [`joins_tail`] needs to know:
/// its last codepoint, and whether it ends in an emoji sequence waiting
/// for a pictograph, an Indic conjunct or half a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tail {
    /// A plain, extend, linker or spacing-mark codepoint, not in any of
    /// the sequences below: extend, ZWJ, linkers and spacing marks join,
    /// nothing else.
    Settled,
    /// A pictograph, then perhaps extends: as `Settled`.
    Pictograph,
    /// A pictograph, extends and a ZWJ: as `Settled`, and a pictograph
    /// joins too (GB11).
    PictographZwj,
    /// Another ZWJ: as `Settled`, except that a consonant may join (GB9c,
    /// if the ZWJ is in a conjunct).
    Zwj,
    /// A consonant, then perhaps extends: as `Settled`.
    Consonant,
    /// A consonant, then extends and linkers with at least one linker:
    /// as `Settled`, and a consonant joins too (GB9c).
    Conjunct,
    /// An unpaired regional indicator: as `Settled`, and a regional
    /// indicator joins too (GB12, GB13).
    RegionalIndicator,
    /// Anything else.
    Other,
}

impl Tail {
    /// The end of a cluster that starts with `c`.
    #[inline]
    pub(super) fn start(c: char) -> Self {
        match CharProps::of(c).class() {
            JoinClass::Joiner if c == ZWJ => Self::Zwj,
            JoinClass::Plain | JoinClass::Joiner | JoinClass::Linker | JoinClass::SpacingMark => {
                Self::Settled
            }
            JoinClass::Pictograph => Self::Pictograph,
            JoinClass::Consonant => Self::Consonant,
            JoinClass::RegionalIndicator => Self::RegionalIndicator,
            JoinClass::Other => Self::Other,
        }
    }

    /// The end of this cluster with `c` appended.
    pub(super) fn then(self, c: char) -> Self {
        use JoinClass as J;
        match (CharProps::of(c).class(), self) {
            (J::Joiner, Self::Pictograph) if c == ZWJ => Self::PictographZwj,
            (J::Joiner, Self::Other) if c == ZWJ => Self::Other,
            (J::Joiner, _) if c == ZWJ => Self::Zwj,
            (J::Plain | J::SpacingMark, _) => Self::Settled,
            (J::Pictograph, _) => Self::Pictograph,
            (J::Consonant, _) => Self::Consonant,
            (J::RegionalIndicator, Self::RegionalIndicator) => Self::Settled,
            // Extends and linkers (which are extends too) keep a
            // pictograph or a conjunct going; linkers start a conjunct
            // after a consonant (GB9c). Both end a flag.
            (J::Joiner | J::Linker, Self::Pictograph) => Self::Pictograph,
            (J::Joiner, Self::Consonant | Self::Conjunct) => self,
            (J::Linker, Self::Consonant | Self::Conjunct) => Self::Conjunct,
            (
                J::Joiner | J::Linker,
                Self::Settled | Self::PictographZwj | Self::RegionalIndicator,
            ) => Self::Settled,
            // After another ZWJ a conjunct may still be open.
            _ => Self::Other,
        }
    }
}

/// Whether a codepoint of `class` joins a cluster that ends as `tail`,
/// when that follows from the two alone; `None` when it takes the whole
/// cluster.
///
/// Extend, ZWJ, linkers and spacing marks join whatever is not a control
/// (GB9, GB9a). A plain codepoint joins none of these ends, a pictograph
/// only one waiting for it after a ZWJ (GB11), a consonant only a
/// conjunct (GB9c) and a regional indicator only another (GB12, GB13).
#[inline]
pub(super) fn joins_tail(tail: Tail, class: JoinClass) -> Option<bool> {
    use JoinClass as J;
    match (class, tail) {
        (_, Tail::Other) | (J::Other, _) | (J::Consonant, Tail::Zwj) => None,
        (J::Joiner | J::Linker | J::SpacingMark, _) | (J::Plain, _) => Some(class != J::Plain),
        (J::Pictograph, _) => Some(tail == Tail::PictographZwj),
        (J::Consonant, _) => Some(tail == Tail::Conjunct),
        (J::RegionalIndicator, _) => Some(tail == Tail::RegionalIndicator),
    }
}

/// Whether `c` joins the cluster `base` + `extra`, when that follows from
/// the classes above; `None` when only the segmenter can tell.
pub(super) fn joins(base: char, extra: &[char], c: char) -> Option<bool> {
    let class = |c| CharProps::of(c).class();
    // The common cases first: plain text, and marks on a plain character.
    match class(c) {
        JoinClass::Plain | JoinClass::Pictograph => {
            let last = extra.last().copied().unwrap_or(base);
            if last != ZWJ && class(last) != JoinClass::Other {
                return Some(false);
            }
        }
        JoinClass::Joiner
            if matches!(class(base), JoinClass::Plain | JoinClass::Pictograph)
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
            let class = if is_plain(c) && in_ranges(PICTOGRAPHIC, c) {
                JoinClass::Pictograph
            } else if is_plain(c) {
                JoinClass::Plain
            } else if is_joiner(c) {
                JoinClass::Joiner
            } else if in_ranges(LINKER, c) {
                JoinClass::Linker
            } else if in_ranges(SPACING_MARK, c) {
                JoinClass::SpacingMark
            } else if in_ranges(CONSONANT, c) {
                JoinClass::Consonant
            } else if is_regional_indicator(c) {
                JoinClass::RegionalIndicator
            } else {
                JoinClass::Other
            };
            assert_eq!(props.class(), class, "{c:?}");
        }
    }

    /// The end of the cluster `codepoints`, as the grid tracks it.
    fn tail_of(codepoints: &[char]) -> Tail {
        let (&first, rest) = codepoints.split_first().unwrap();
        rest.iter()
            .fold(Tail::start(first), |tail, &c| tail.then(c))
    }

    /// Samples of every class, and of what the classes leave out.
    const SAMPLES: &[char] = &[
        'b',
        '\u{4E2D}',
        '\u{AC00}',  // LV
        '\u{AC01}',  // LVT
        '\u{1F600}', // Extended_Pictographic
        '\u{A9}',
        '\u{301}',
        '\u{FE0F}',
        '\u{1F3FD}',
        '\u{E0067}',
        '\u{941}', // Devanagari vowel sign u: extend
        ZWJ,
        '\u{94D}',   // Devanagari virama: linker
        '\u{9CD}',   // Bengali virama: linker
        '\u{93F}',   // Devanagari vowel sign i: spacing mark
        '\u{915}',   // Devanagari ka: consonant
        '\u{995}',   // Bengali ka: consonant
        '\u{1F1EF}', // Regional indicators
        '\u{1100}',  // L
        '\u{600}',   // Prepend
        '\u{AD}',    // Control
        '\u{200C}',  // ZWNJ
        '\u{BCD}',   // Tamil virama: extend, not a linker
    ];

    /// Every codepoint of a class `joins_tail` knows, ending clusters of
    /// each kind it can end: `joins_tail` agrees with the segmenter on
    /// every sample after it whenever it answers.
    #[test]
    fn joins_tail_agrees_with_the_segmenter_for_every_last_codepoint() {
        for t in (0..=0x10FFFF).filter_map(char::from_u32) {
            let befores: &[&str] = match CharProps::of(t).class() {
                JoinClass::Other => continue,
                JoinClass::Plain => &[""],
                JoinClass::Pictograph => &["", "\u{1F600}\u{200D}"],
                JoinClass::RegionalIndicator => &["", "\u{1F1EF}"],
                JoinClass::Consonant => &["", "\u{915}\u{94D}"],
                JoinClass::Joiner | JoinClass::Linker | JoinClass::SpacingMark => &[
                    "a",
                    "\u{1F600}",
                    "\u{915}",
                    "\u{915}\u{94D}",
                    "a\u{301}",
                    "\u{1F1EF}",
                    "\u{1F600}\u{200D}",
                ],
            };
            for before in befores {
                let cluster: Vec<char> = before.chars().chain([t]).collect();
                let text: String = cluster.iter().collect();
                if clusters(&text) != 1 {
                    continue;
                }
                let tail = tail_of(&cluster);
                for &c in SAMPLES {
                    if let Some(joins) = joins_tail(tail, CharProps::of(c).class()) {
                        assert_eq!(joins, segmenter_joins(&text, c), "{text:?} + {c:?}");
                    }
                }
            }
        }
    }

    /// Every cluster of up to four samples: `joins_tail` agrees with the
    /// segmenter on every sample after it whenever it answers, and knows
    /// how clusters of plain codepoints, consonants, extends, linkers and
    /// spacing marks end.
    #[test]
    fn joins_tail_agrees_with_the_segmenter_on_every_short_cluster() {
        let mut sequences: Vec<Vec<char>> = SAMPLES.iter().map(|&c| vec![c]).collect();
        let mut start = 0;
        for _ in 1..4 {
            let end = sequences.len();
            for index in start..end {
                for &c in SAMPLES {
                    let mut longer = sequences[index].clone();
                    longer.push(c);
                    if clusters(&longer.iter().collect::<String>()) == 1 {
                        sequences.push(longer);
                    }
                }
            }
            start = end;
        }
        for cluster in &sequences {
            let text: String = cluster.iter().collect();
            let tail = tail_of(cluster);
            for &c in SAMPLES {
                if let Some(joins) = joins_tail(tail, CharProps::of(c).class()) {
                    assert_eq!(joins, segmenter_joins(&text, c), "{text:?} + {c:?}");
                }
            }
            let known = cluster
                .iter()
                .all(|&c| c != ZWJ && CharProps::of(c).class() != JoinClass::Other);
            if known {
                assert_ne!(tail, Tail::Other, "{text:?}");
            }
        }
    }

    /// The consonants and linkers are exactly the codepoints that behave
    /// as such: a consonant joins a consonant and virama but not plain
    /// text, and a linker after a consonant makes the next one join it.
    #[test]
    fn consonants_and_linkers_are_exactly_those_of_the_segmenter() {
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            let consonant = !segmenter_joins("a", c) && segmenter_joins("\u{915}\u{94D}", c);
            assert_eq!(in_ranges(CONSONANT, c), consonant, "{c:?}");
            let linker =
                segmenter_joins("a", c) && segmenter_joins(&format!("\u{915}{c}"), '\u{915}');
            assert_eq!(in_ranges(LINKER, c), linker, "{c:?}");
        }
    }

    /// Spacing marks join plain text and consonants, are not extends (they
    /// end an emoji sequence and a conjunct) and join nothing plain after
    /// them.
    #[test]
    fn spacing_marks_are_spacing_marks() {
        for ranges in SPACING_MARK {
            for m in (ranges.0..=ranges.1).filter(|&m| char::from_u32(m as u32).is_some()) {
                for cluster in ["a", "\u{915}", "\u{1F600}", "a\u{301}"] {
                    assert!(segmenter_joins(cluster, m), "{m:?} after {cluster:?}");
                }
                assert!(!segmenter_joins(
                    &format!("\u{1F600}{m}\u{200D}"),
                    '\u{1F600}'
                ));
                assert!(!segmenter_joins(&format!("\u{915}\u{94D}{m}"), '\u{915}'));
                for after in ['b', '\u{915}', '\u{1F600}'] {
                    assert!(
                        !segmenter_joins(&format!("a{m}"), after),
                        "{after:?} after {m:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn ranges_are_sorted_and_disjoint() {
        for ranges in [PLAIN, PICTOGRAPHIC, EXTEND, LINKER, SPACING_MARK, CONSONANT] {
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
            // Indic_Conjunct_Break=Extend: it keeps a conjunct going.
            assert!(
                segmenter_joins(&format!("\u{915}\u{94D}{m}"), '\u{915}'),
                "{m:?} ends a conjunct"
            );
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
