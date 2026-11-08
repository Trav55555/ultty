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

/// The extend class (see `cluster.rs`) as sorted, disjoint ranges: every
/// one also has Indic_Conjunct_Break=Extend.
pub(super) const EXTEND: &[(char, char)] = &[
    ('\u{300}', '\u{36F}'),
    ('\u{483}', '\u{489}'),
    ('\u{900}', '\u{902}'),
    ('\u{93A}', '\u{93A}'),
    ('\u{93C}', '\u{93C}'),
    ('\u{941}', '\u{948}'),
    ('\u{951}', '\u{957}'),
    ('\u{962}', '\u{963}'),
    ('\u{981}', '\u{981}'),
    ('\u{9BC}', '\u{9BC}'),
    ('\u{9BE}', '\u{9BE}'),
    ('\u{9C1}', '\u{9C4}'),
    ('\u{9D7}', '\u{9D7}'),
    ('\u{9E2}', '\u{9E3}'),
    ('\u{9FE}', '\u{9FE}'),
    ('\u{A01}', '\u{A02}'),
    ('\u{A3C}', '\u{A3C}'),
    ('\u{A41}', '\u{A42}'),
    ('\u{A47}', '\u{A48}'),
    ('\u{A4B}', '\u{A4D}'),
    ('\u{A51}', '\u{A51}'),
    ('\u{A70}', '\u{A71}'),
    ('\u{A75}', '\u{A75}'),
    ('\u{A81}', '\u{A82}'),
    ('\u{ABC}', '\u{ABC}'),
    ('\u{AC1}', '\u{AC5}'),
    ('\u{AC7}', '\u{AC8}'),
    ('\u{AE2}', '\u{AE3}'),
    ('\u{AFA}', '\u{AFF}'),
    ('\u{B01}', '\u{B01}'),
    ('\u{B3C}', '\u{B3C}'),
    ('\u{B3E}', '\u{B3F}'),
    ('\u{B41}', '\u{B44}'),
    ('\u{B55}', '\u{B57}'),
    ('\u{B62}', '\u{B63}'),
    ('\u{B82}', '\u{B82}'),
    ('\u{BBE}', '\u{BBE}'),
    ('\u{BC0}', '\u{BC0}'),
    ('\u{BCD}', '\u{BCD}'),
    ('\u{BD7}', '\u{BD7}'),
    ('\u{C00}', '\u{C00}'),
    ('\u{C04}', '\u{C04}'),
    ('\u{C3C}', '\u{C3C}'),
    ('\u{C3E}', '\u{C40}'),
    ('\u{C46}', '\u{C48}'),
    ('\u{C4A}', '\u{C4C}'),
    ('\u{C55}', '\u{C56}'),
    ('\u{C62}', '\u{C63}'),
    ('\u{C81}', '\u{C81}'),
    ('\u{CBC}', '\u{CBC}'),
    ('\u{CBF}', '\u{CC0}'),
    ('\u{CC2}', '\u{CC2}'),
    ('\u{CC6}', '\u{CC8}'),
    ('\u{CCA}', '\u{CCD}'),
    ('\u{CD5}', '\u{CD6}'),
    ('\u{CE2}', '\u{CE3}'),
    ('\u{D00}', '\u{D01}'),
    ('\u{D3B}', '\u{D3C}'),
    ('\u{D3E}', '\u{D3E}'),
    ('\u{D41}', '\u{D44}'),
    ('\u{D57}', '\u{D57}'),
    ('\u{D62}', '\u{D63}'),
    ('\u{D81}', '\u{D81}'),
    ('\u{DCA}', '\u{DCA}'),
    ('\u{DCF}', '\u{DCF}'),
    ('\u{DD2}', '\u{DD4}'),
    ('\u{DD6}', '\u{DD6}'),
    ('\u{DDF}', '\u{DDF}'),
    ('\u{1DC0}', '\u{1DFF}'),
    ('\u{20D0}', '\u{20F0}'),
    ('\u{FE00}', '\u{FE0F}'),
    ('\u{FE20}', '\u{FE2F}'),
    ('\u{1F3FB}', '\u{1F3FF}'),
    ('\u{E0020}', '\u{E007F}'),
    ('\u{E0100}', '\u{E01EF}'),
];

/// Whether `c` is in the extend class.
pub(super) fn is_extend(c: char) -> bool {
    in_ranges(EXTEND, c)
}

/// The Indic_Conjunct_Break=Linker codepoints (exactly those, as the tests
/// check): the viramas that join consonants into a conjunct (GB9c).
pub(super) const LINKER: &[(char, char)] = &[
    ('\u{94D}', '\u{94D}'),
    ('\u{9CD}', '\u{9CD}'),
    ('\u{ACD}', '\u{ACD}'),
    ('\u{B4D}', '\u{B4D}'),
    ('\u{C4D}', '\u{C4D}'),
    ('\u{D4D}', '\u{D4D}'),
    ('\u{1039}', '\u{1039}'),
    ('\u{17D2}', '\u{17D2}'),
    ('\u{1A60}', '\u{1A60}'),
    ('\u{1B44}', '\u{1B44}'),
    ('\u{1BAB}', '\u{1BAB}'),
    ('\u{A9C0}', '\u{A9C0}'),
    ('\u{AAF6}', '\u{AAF6}'),
    ('\u{10A3F}', '\u{10A3F}'),
    ('\u{11133}', '\u{11133}'),
    ('\u{113D0}', '\u{113D0}'),
    ('\u{1193E}', '\u{1193E}'),
    ('\u{11A47}', '\u{11A47}'),
    ('\u{11A99}', '\u{11A99}'),
    ('\u{11F42}', '\u{11F42}'),
];

/// Grapheme_Cluster_Break=SpacingMark codepoints of the Indic scripts, as
/// sorted, disjoint ranges: they join whatever is not a control (GB9a).
pub(super) const SPACING_MARK: &[(char, char)] = &[
    ('\u{903}', '\u{903}'),
    ('\u{93B}', '\u{93B}'),
    ('\u{93E}', '\u{940}'),
    ('\u{949}', '\u{94C}'),
    ('\u{94E}', '\u{94F}'),
    ('\u{982}', '\u{983}'),
    ('\u{9BF}', '\u{9C0}'),
    ('\u{9C7}', '\u{9C8}'),
    ('\u{9CB}', '\u{9CC}'),
    ('\u{A03}', '\u{A03}'),
    ('\u{A3E}', '\u{A40}'),
    ('\u{A83}', '\u{A83}'),
    ('\u{ABE}', '\u{AC0}'),
    ('\u{AC9}', '\u{AC9}'),
    ('\u{ACB}', '\u{ACC}'),
    ('\u{B02}', '\u{B03}'),
    ('\u{B40}', '\u{B40}'),
    ('\u{B47}', '\u{B48}'),
    ('\u{B4B}', '\u{B4C}'),
    ('\u{BBF}', '\u{BBF}'),
    ('\u{BC1}', '\u{BC2}'),
    ('\u{BC6}', '\u{BC8}'),
    ('\u{BCA}', '\u{BCC}'),
    ('\u{C01}', '\u{C03}'),
    ('\u{C41}', '\u{C44}'),
    ('\u{C82}', '\u{C83}'),
    ('\u{CBE}', '\u{CBE}'),
    ('\u{CC1}', '\u{CC1}'),
    ('\u{CC3}', '\u{CC4}'),
    ('\u{CF3}', '\u{CF3}'),
    ('\u{D02}', '\u{D03}'),
    ('\u{D3F}', '\u{D40}'),
    ('\u{D46}', '\u{D48}'),
    ('\u{D4A}', '\u{D4C}'),
    ('\u{D82}', '\u{D83}'),
    ('\u{DD0}', '\u{DD1}'),
    ('\u{DD8}', '\u{DDE}'),
    ('\u{DF2}', '\u{DF3}'),
];

/// The Indic_Conjunct_Break=Consonant codepoints (exactly those, as the
/// tests check), as sorted, disjoint ranges: they join a cluster that ends
/// in a consonant and a virama (GB9c).
pub(super) const CONSONANT: &[(char, char)] = &[
    ('\u{915}', '\u{939}'),
    ('\u{958}', '\u{95F}'),
    ('\u{978}', '\u{97F}'),
    ('\u{995}', '\u{9A8}'),
    ('\u{9AA}', '\u{9B0}'),
    ('\u{9B2}', '\u{9B2}'),
    ('\u{9B6}', '\u{9B9}'),
    ('\u{9DC}', '\u{9DD}'),
    ('\u{9DF}', '\u{9DF}'),
    ('\u{9F0}', '\u{9F1}'),
    ('\u{A95}', '\u{AA8}'),
    ('\u{AAA}', '\u{AB0}'),
    ('\u{AB2}', '\u{AB3}'),
    ('\u{AB5}', '\u{AB9}'),
    ('\u{AF9}', '\u{AF9}'),
    ('\u{B15}', '\u{B28}'),
    ('\u{B2A}', '\u{B30}'),
    ('\u{B32}', '\u{B33}'),
    ('\u{B35}', '\u{B39}'),
    ('\u{B5C}', '\u{B5D}'),
    ('\u{B5F}', '\u{B5F}'),
    ('\u{B71}', '\u{B71}'),
    ('\u{C15}', '\u{C28}'),
    ('\u{C2A}', '\u{C39}'),
    ('\u{C58}', '\u{C5A}'),
    ('\u{D15}', '\u{D3A}'),
    ('\u{1000}', '\u{102A}'),
    ('\u{103F}', '\u{103F}'),
    ('\u{1050}', '\u{1055}'),
    ('\u{105A}', '\u{105D}'),
    ('\u{1061}', '\u{1061}'),
    ('\u{1065}', '\u{1066}'),
    ('\u{106E}', '\u{1070}'),
    ('\u{1075}', '\u{1081}'),
    ('\u{108E}', '\u{108E}'),
    ('\u{1780}', '\u{17B3}'),
    ('\u{1A20}', '\u{1A54}'),
    ('\u{1B0B}', '\u{1B0C}'),
    ('\u{1B13}', '\u{1B33}'),
    ('\u{1B45}', '\u{1B4C}'),
    ('\u{1B83}', '\u{1BA0}'),
    ('\u{1BAE}', '\u{1BAF}'),
    ('\u{1BBB}', '\u{1BBD}'),
    ('\u{A989}', '\u{A98B}'),
    ('\u{A98F}', '\u{A9B2}'),
    ('\u{A9E0}', '\u{A9E4}'),
    ('\u{A9E7}', '\u{A9EF}'),
    ('\u{A9FA}', '\u{A9FE}'),
    ('\u{AA60}', '\u{AA6F}'),
    ('\u{AA71}', '\u{AA73}'),
    ('\u{AA7A}', '\u{AA7A}'),
    ('\u{AA7E}', '\u{AA7F}'),
    ('\u{AAE0}', '\u{AAEA}'),
    ('\u{ABC0}', '\u{ABDA}'),
    ('\u{10A00}', '\u{10A00}'),
    ('\u{10A10}', '\u{10A13}'),
    ('\u{10A15}', '\u{10A17}'),
    ('\u{10A19}', '\u{10A35}'),
    ('\u{11103}', '\u{11126}'),
    ('\u{11144}', '\u{11144}'),
    ('\u{11147}', '\u{11147}'),
    ('\u{11380}', '\u{11389}'),
    ('\u{1138B}', '\u{1138B}'),
    ('\u{1138E}', '\u{1138E}'),
    ('\u{11390}', '\u{113B5}'),
    ('\u{11900}', '\u{11906}'),
    ('\u{11909}', '\u{11909}'),
    ('\u{1190C}', '\u{11913}'),
    ('\u{11915}', '\u{11916}'),
    ('\u{11918}', '\u{1192F}'),
    ('\u{11A00}', '\u{11A00}'),
    ('\u{11A0B}', '\u{11A32}'),
    ('\u{11A50}', '\u{11A50}'),
    ('\u{11A5C}', '\u{11A83}'),
    ('\u{11F04}', '\u{11F10}'),
    ('\u{11F12}', '\u{11F33}'),
];

/// The Extended_Pictographic codepoints of the plain class, as sorted,
/// disjoint ranges: exactly those (the tests check every plain codepoint).
pub(super) const PICTOGRAPHIC: &[(char, char)] = &[
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

pub(super) const ZWJ: char = '\u{200D}';

pub(super) fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

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
    /// The plain class, but not [`PICTOGRAPHIC`].
    Plain = 1,
    /// The extend class or ZWJ: joins any cluster ending in a codepoint of
    /// these classes (GB9).
    Joiner = 2,
    /// [`LINKER`]: like `Joiner`, and makes a conjunct of the consonant
    /// before it and the one after it.
    Linker = 3,
    /// [`SPACING_MARK`]: joins like `Joiner` (GB9a).
    SpacingMark = 4,
    /// [`CONSONANT`]: plain, except that it joins a conjunct (GB9c).
    Consonant = 5,
    /// A regional indicator: they join in pairs (GB12, GB13).
    RegionalIndicator = 6,
    /// The plain class and [`PICTOGRAPHIC`]: plain, except that it joins
    /// a pictograph, extends and a ZWJ (GB11).
    Pictograph = 7,
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
        if in_ranges(PICTOGRAPHIC, c) {
            JoinClass::Pictograph
        } else {
            JoinClass::Plain
        }
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
    width | (class as u8) << CLASS_SHIFT
}
