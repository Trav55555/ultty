//! Stream strategies for the VT core's proptests: random byte streams built
//! from real VT fragments, interleaved with resizes.
//!
//! Shared by `tests/integrity.rs` and the differential oracle in
//! `src/grid/reference/oracle.rs`, which includes this file by path.

use proptest::prelude::*;

/// One step of a stream: bytes for the parser, or a resize.
#[derive(Debug, Clone)]
pub enum Step {
    Bytes(Vec<u8>),
    Resize(u16, u16),
}

pub fn csi() -> impl Strategy<Value = Vec<u8>> {
    let finals = prop::sample::select(b"@ABCDEFGHIJKLMPSTXZ`abcdefghlmnqrstu".to_vec());
    let params = prop::collection::vec(prop_oneof![0u16..=4, 0u16..=300, Just(65535u16)], 0..4);
    (any::<bool>(), params, finals).prop_map(|(private, params, fin)| {
        let mut out = b"\x1b[".to_vec();
        if private {
            out.push(b'?');
        }
        let joined: Vec<String> = params.iter().map(u16::to_string).collect();
        out.extend(joined.join(";").into_bytes());
        out.push(fin);
        out
    })
}

pub fn dec_mode() -> impl Strategy<Value = Vec<u8>> {
    let modes = prop::sample::select(vec![
        1u16, 3, 5, 6, 7, 25, 40, 47, 1005, 1047, 1048, 1049, 2004, 2026, 2027,
    ]);
    (modes, any::<bool>())
        .prop_map(|(m, on)| format!("\x1b[?{m}{}", if on { 'h' } else { 'l' }).into_bytes())
}

pub fn text() -> impl Strategy<Value = Vec<u8>> {
    let pieces = prop::sample::select(vec![
        "a",
        "hello world ",
        "\u{4e2d}",
        "\u{1f600}",
        "e\u{301}",
        "\u{301}",
        "\u{200d}",
        "\u{fe0f}",
        "\u{2764}\u{fe0f}",
        "\u{1f1fa}",
        "\u{1f1f8}",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        "\u{1f44d}\u{1f3fd}",
        "\u{915}\u{94d}\u{937}\u{93f}",
        "\u{1100}\u{1161}\u{11a8}",
        "\r",
        "\n",
        "\r\n",
        "\t",
        "\x08",
        "\x1b7",
        "\x1b8",
        "\x1bD",
        "\x1bE",
        "\x1bM",
        "\x1bc",
        "\x1b#8",
        "\x1b[4h",
        "\x1b[4l",
        "\x1b]8;;http://x\x1b\\",
        "\x1b]8;;\x1b\\",
        "\x1b]133;A\x07",
        "\x1b]0;title\x07",
        // Inline surface anchors (ADR 0020).
        "\x1b_u1;2;1\x1b\\",
        "\x1b_u1;3;3\x1b\\",
        "\x1b_u9;4;12\x1b\\",
    ]);
    prop::collection::vec(pieces, 1..8).prop_map(|v| v.concat().into_bytes())
}

pub fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => text().prop_map(Step::Bytes),
        4 => csi().prop_map(Step::Bytes),
        2 => dec_mode().prop_map(Step::Bytes),
        1 => prop::collection::vec(any::<u8>(), 0..32).prop_map(Step::Bytes),
        1 => (1u16..=40, 1u16..=12).prop_map(|(c, r)| Step::Resize(c, r)),
    ]
}
