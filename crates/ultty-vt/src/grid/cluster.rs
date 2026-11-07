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
//! The ranges are a conservative subset; the tests check every codepoint in
//! them against the segmenter, so a Unicode update that moves one fails
//! loudly instead of changing behaviour.

/// The plain class (see the module docs) as sorted, disjoint ranges.
const PLAIN: &[(char, char)] = &[
    ('\u{20}', '\u{7E}'),
    ('\u{A0}', '\u{AC}'),
    ('\u{AE}', '\u{2FF}'),
    ('\u{370}', '\u{482}'),
    ('\u{48A}', '\u{52F}'),
    ('\u{5D0}', '\u{5EA}'),
    ('\u{620}', '\u{64A}'),
    ('\u{1E00}', '\u{1FFF}'),
    ('\u{2010}', '\u{2027}'),
    ('\u{2030}', '\u{205F}'),
    ('\u{2070}', '\u{20CF}'),
    ('\u{2100}', '\u{2BFF}'),
    ('\u{2E80}', '\u{2FDF}'),
    ('\u{3000}', '\u{3029}'),
    ('\u{3030}', '\u{303F}'),
    ('\u{3041}', '\u{3096}'),
    ('\u{309B}', '\u{30FF}'),
    ('\u{3105}', '\u{312F}'),
    ('\u{3131}', '\u{318E}'),
    ('\u{31F0}', '\u{4DBF}'),
    ('\u{4E00}', '\u{A48C}'),
    ('\u{AC00}', '\u{D7A3}'),
    ('\u{F900}', '\u{FAFF}'),
    ('\u{FF01}', '\u{FF9D}'),
    ('\u{FFE0}', '\u{FFEE}'),
    ('\u{1F000}', '\u{1F1E5}'),
    ('\u{1F200}', '\u{1F3FA}'),
    ('\u{1F400}', '\u{1FAFF}'),
    ('\u{20000}', '\u{3FFFD}'),
];

/// Whether `c` is in the plain class.
fn is_plain(c: char) -> bool {
    // The first range starting after `c`; `c` is plain if the one before
    // it reaches `c`.
    let next = PLAIN.partition_point(|&(lo, _)| lo <= c);
    next > 0 && c <= PLAIN[next - 1].1
}

/// Whether `c` is in the extend class (see the module docs).
fn is_extend(c: char) -> bool {
    matches!(c,
        '\u{300}'..='\u{36F}'
        | '\u{483}'..='\u{489}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20F0}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FE20}'..='\u{FE2F}'
        | '\u{1F3FB}'..='\u{1F3FF}'
        | '\u{E0020}'..='\u{E007F}'
        | '\u{E0100}'..='\u{E01EF}'
    )
}

const ZWJ: char = '\u{200D}';

/// Whether `c` always joins a cluster of a plain codepoint and more such.
fn is_joiner(c: char) -> bool {
    c == ZWJ || is_extend(c)
}

/// Whether `c` joins the cluster `base` + `extra`, when that follows from
/// the classes above; `None` when only the segmenter can tell.
pub(super) fn joins(base: char, extra: &[char], c: char) -> Option<bool> {
    if is_plain(c) {
        let last = extra.last().copied().unwrap_or(base);
        return (is_plain(last) || is_extend(last)).then_some(false);
    }
    if is_joiner(c) && is_plain(base) && extra.iter().copied().all(is_joiner) {
        return Some(true);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

    fn clusters(s: &str) -> usize {
        s.graphemes(true).count()
    }

    fn all(class: fn(char) -> bool) -> impl Iterator<Item = char> {
        (0..=0x10FFFF).filter_map(char::from_u32).filter(move |&c| class(c))
    }

    /// What `joins` claims for a pair, checked by the segmenter.
    fn segmenter_joins(cluster: &str, c: char) -> bool {
        clusters(&format!("{cluster}{c}")) == 1
    }

    #[test]
    fn plain_ranges_are_sorted_and_disjoint() {
        for pair in PLAIN.windows(2) {
            assert!(pair[0].0 <= pair[0].1 && pair[0].1 < pair[1].0, "{pair:?}");
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
                assert!(segmenter_joins(cluster, m), "{m:?} does not join {cluster:?}");
            }
            // Rules out ZWJ, which joins a following pictograph, and every
            // class that joins what follows it.
            for after in ['b', '\u{1F600}', '\u{AC00}', '\u{AC01}'] {
                let cluster = format!("\u{1F600}{m}");
                assert!(!segmenter_joins(&cluster, after), "{after:?} joins {cluster:?}");
            }
            assert!(segmenter_joins(&format!("a{m}"), '\u{301}'), "after {m:?}");
            assert!(segmenter_joins(&format!("a{m}"), ZWJ), "ZWJ after {m:?}");
        }
        for cluster in ["a", "a\u{301}", "a\u{200D}"] {
            assert!(segmenter_joins(cluster, ZWJ), "ZWJ does not join {cluster:?}");
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
        // Pictographs after ZWJ, flags, Hangul jamo and Indic marks all
        // need the full rules.
        let cases: &[(char, &[char], char)] = &[
            ('\u{1F468}', &['\u{200D}'], '\u{1F469}'),
            ('\u{1F1EF}', &[], '\u{1F1F5}'),
            ('\u{1100}', &[], '\u{1161}'),
            ('\u{915}', &[], '\u{94D}'),
            ('\u{AD}', &[], '\u{301}'),
        ];
        for &(base, extra, c) in cases {
            assert_eq!(joins(base, extra, c), None, "{base:?} {extra:?} + {c:?}");
        }
    }
}
