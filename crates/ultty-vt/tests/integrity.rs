//! Hostile-input invariant tests for the VT core.
//!
//! Random byte streams built from real VT fragments (cursor motion, scroll
//! regions, insert/delete, wide and combining characters, ZWJ sequences,
//! mode switches, resets) interleaved with resizes must never panic and must
//! leave the terminal structurally consistent after every step. The same
//! checker backs the `vt_terminal` libFuzzer target in `fuzz/`.

use proptest::prelude::*;
use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

#[derive(Debug, Clone)]
enum Step {
    Bytes(Vec<u8>),
    Resize(u16, u16),
}

fn csi() -> impl Strategy<Value = Vec<u8>> {
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

fn dec_mode() -> impl Strategy<Value = Vec<u8>> {
    let modes = prop::sample::select(vec![1u16, 3, 5, 6, 7, 25, 47, 1047, 1049, 2004, 2026, 2027]);
    (modes, any::<bool>())
        .prop_map(|(m, on)| format!("\x1b[?{m}{}", if on { 'h' } else { 'l' }).into_bytes())
}

fn text() -> impl Strategy<Value = Vec<u8>> {
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
    ]);
    prop::collection::vec(pieces, 1..8).prop_map(|v| v.concat().into_bytes())
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => text().prop_map(Step::Bytes),
        4 => csi().prop_map(Step::Bytes),
        2 => dec_mode().prop_map(Step::Bytes),
        1 => prop::collection::vec(any::<u8>(), 0..32).prop_map(Step::Bytes),
        1 => (1u16..=40, 1u16..=12).prop_map(|(c, r)| Step::Resize(c, r)),
    ]
}

fn run(cols: u16, rows: u16, steps: &[Step]) -> Result<(), String> {
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    let mut parser = AnsiParser::new();
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Bytes(bytes) => {
                let mut handler = TerminalHandler::new(&mut terminal);
                parser.feed(bytes, |action| handler.handle(action));
            }
            Step::Resize(c, r) => terminal.resize(Dimensions::new(*c, *r)),
        }
        terminal
            .check_integrity()
            .map_err(|e| format!("after step {index} ({step:?}): {e}"))?;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn hostile_streams_keep_terminal_consistent(
        cols in 1u16..=40,
        rows in 1u16..=12,
        steps in prop::collection::vec(step(), 1..48),
    ) {
        if let Err(violation) = run(cols, rows, &steps) {
            prop_assert!(false, "{}", violation);
        }
    }
}

/// A control sequence as a hostile or merely newer program might send it:
/// an optional private marker, parameters with `;` and `:` (some empty),
/// stray markers, intermediates, C0 controls and non-ASCII bytes anywhere
/// inside, then a final byte. ESC, CAN and SUB are left out: they end a
/// sequence early, after which the bytes that follow are ordinary text.
fn messy_csi() -> impl Strategy<Value = Vec<u8>> {
    let marker = prop::option::of(prop::sample::select(b"<=>?".to_vec()));
    let body_byte = prop_oneof![
        8 => prop::sample::select(b"0123456789;:".to_vec()),
        1 => prop::sample::select(b"<=>? !\"$'".to_vec()),
        1 => prop::sample::select(vec![
            0x00u8, 0x05, 0x07, 0x08, 0x09, 0x0A, 0x0D, 0x0E, 0x0F, 0x1F, 0x7F,
        ]),
        1 => prop::sample::select(vec![0x80u8, 0x9B, 0xC3, 0xA9, 0xFF]),
    ];
    let body = prop::collection::vec(body_byte, 0..24);
    let fin = 0x40u8..=0x7E;
    (marker, body, fin).prop_map(|(marker, body, fin)| {
        let mut out = b"\x1b[".to_vec();
        out.extend(marker);
        out.extend(body);
        out.push(fin);
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// No byte of a control sequence is ever printed.
    #[test]
    fn control_sequences_never_print_their_bytes(seq in messy_csi()) {
        let mut bytes = b"A".to_vec();
        bytes.extend(&seq);
        bytes.push(b'B');

        let mut printed = String::new();
        AnsiParser::new().feed(&bytes, |action| {
            if let ultty_vt::ansi::Action::Print(c) = action {
                printed.push(c);
            }
        });
        prop_assert_eq!(&printed, "AB", "sequence {:?}", seq);

        // The same holds on screen: nothing but A and B in any cell.
        let mut terminal = Terminal::new(Dimensions::new(40, 4));
        let mut handler = TerminalHandler::new(&mut terminal);
        AnsiParser::new().feed(&bytes, |action| handler.handle(action));
        for row in terminal.grid().rows() {
            for cell in row.cells() {
                prop_assert!(
                    matches!(cell.char(), ' ' | 'A' | 'B'),
                    "{:?} on screen after {:?}", cell.char(), seq
                );
            }
        }
    }
}
