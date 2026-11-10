//! Hostile-input invariant tests for the VT core.
//!
//! Random byte streams built from real VT fragments (cursor motion, scroll
//! regions, insert/delete, wide and combining characters, ZWJ sequences,
//! mode switches, resets) interleaved with resizes must never panic and must
//! leave the terminal structurally consistent after every step. The same
//! checker backs the `vt_terminal` libFuzzer target in `fuzz/`.

use proptest::prelude::*;
use ultty_vt::ansi::{AnsiOutput, AnsiParser};
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

#[path = "support/streams.rs"]
mod streams;
use streams::{step, Step};

/// Feed `steps` to a fresh terminal, checking it after each. With `spans`,
/// bytes go through the ASCII-span path the terminal uses for PTY output
/// rather than one action per character.
fn run(cols: u16, rows: u16, steps: &[Step], spans: bool) -> Result<(), String> {
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    terminal.set_inline_anchors(true);
    let mut parser = AnsiParser::new();
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Bytes(bytes) => {
                let mut handler = TerminalHandler::new(&mut terminal);
                if spans {
                    parser.feed_with_ascii_spans(bytes, |output| match output {
                        AnsiOutput::Action(action) => handler.handle(action),
                        AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
                    });
                } else {
                    parser.feed(bytes, |action| handler.handle(action));
                }
            }
            Step::Resize(c, r) => terminal.resize(Dimensions::new(*c, *r)),
        }
        terminal
            .check_integrity()
            .map_err(|e| format!("after step {index} ({step:?}): {e}"))?;
        // As the embedder does after each read: anchors still live after
        // the sweep have a first row to draw at.
        terminal.take_anchor_events();
        let buffer = terminal.buffer();
        if let Some((tag, _)) = buffer
            .anchors()
            .find(|&(tag, _)| buffer.anchor_line(tag).is_none())
        {
            return Err(format!(
                "after step {index} ({step:?}): anchor {tag} is live with no first row"
            ));
        }
        terminal
            .check_integrity()
            .map_err(|e| format!("after sweeping, step {index} ({step:?}): {e}"))?;
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
        for spans in [false, true] {
            if let Err(violation) = run(cols, rows, &steps, spans) {
                prop_assert!(false, "spans {}: {}", spans, violation);
            }
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
