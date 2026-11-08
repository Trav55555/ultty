//! Codepoint classes for the grapheme cluster shortcuts in `cluster.rs`,
//! and the per-codepoint properties the write path looks up.
//!
//! This file is also compiled into the crate's build script, which
//! tabulates [`props_bits`] for the codepoints most text uses, so it
//! depends on nothing but `unicode-width`.

use unicode_width::UnicodeWidthChar;

/// The plain class (see `cluster.rs`) as sorted, disjoint ranges.
pub(super) const PLAIN: &[(char, char)] = &[
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

/// Whether `c` is in one of `ranges` (sorted and disjoint).
pub(super) fn in_ranges(ranges: &[(char, char)], c: char) -> bool {
    // The first range starting after `c`; `c` is in one if the one before
    // it reaches `c`.
    let next = ranges.partition_point(|&(lo, _)| lo <= c);
    next > 0 && c <= ranges[next - 1].1
}

/// Whether `c` is in the plain class.
pub(super) fn is_plain(c: char) -> bool {
    in_ranges(PLAIN, c)
}

/// Whether `c` is in the extend class (see `cluster.rs`).
pub(super) fn is_extend(c: char) -> bool {
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

pub(super) const ZWJ: char = '\u{200D}';

/// Whether `c` always joins a cluster of a plain codepoint and more such.
pub(super) fn is_joiner(c: char) -> bool {
    c == ZWJ || is_extend(c)
}

/// How a codepoint can take part in a grapheme cluster, as far as the
/// shortcuts need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JoinClass {
    /// Outside the classes below: only the segmenter can tell.
    Other = 0,
    /// The plain class.
    Plain = 1,
    /// The extend class or ZWJ: joins any cluster ending in a plain, extend
    /// or ZWJ codepoint (GB9).
    Joiner = 2,
}

/// Where the [`JoinClass`] sits in a properties byte; the bits below it
/// are the width.
pub(super) const CLASS_SHIFT: u8 = 2;

/// What printing `c` needs to know about it, in one byte: the columns it
/// takes when it starts a cluster (0 to 3; control characters, which
/// have no width, take none) and its [`JoinClass`] above [`CLASS_SHIFT`].
pub(super) fn props_bits(c: char) -> u8 {
    let width = c.width().unwrap_or(0) as u8;
    let class = if is_plain(c) {
        JoinClass::Plain
    } else if is_joiner(c) {
        JoinClass::Joiner
    } else {
        JoinClass::Other
    };
    width | (class as u8) << CLASS_SHIFT
}
