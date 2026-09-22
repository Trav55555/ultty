//! Property-based robustness tests for the full terminal pipeline.
//!
//! Arbitrary byte chunks (biased toward escape-sequence introducers, Sixel
//! DCS payloads, OSC 8 hyperlinks, wide and combining characters) are fed
//! through the real `AnsiParser` -> `TerminalHandler` -> `Terminal` path,
//! interleaved with random resizes. The invariant is simple: nothing panics
//! and the grid and cursor always stay within the current dimensions.

use proptest::prelude::*;
use ultty::grid::{decode_sixel, Dimensions, Terminal, TerminalHandler};
use ultty::protocol::ansi::AnsiParser;

const MAX_COLS: u16 = 200;
const MAX_ROWS: u16 = 60;

/// One chunk of PTY output, biased toward the interesting bytes.
fn chunk_strategy() -> impl Strategy<Value = Vec<u8>> {
    let token = prop_oneof![
        8 => any::<u8>().prop_map(|b| vec![b]),
        6 => prop::sample::select(vec![
            0x1bu8, b'[', b';', b'?', b'P', b'q', b'!', b'~', b'M', b'r', b'S', b'T', b'H',
            b'J', b'K', b'L', b'@', b'h', b'l', b'm', b'c', b'(', b'0', b'\\', 0x07, 0x0e,
            0x0f, b'\n', b'\r', b'\t', 0x08,
        ])
        .prop_map(|b| vec![b]),
        4 => "[0-9]{1,6}".prop_map(|s| s.into_bytes()),
        2 => Just(b"\x1b]8;;https://example.com/a\x07link\x1b]8;;\x07".to_vec()),
        2 => Just(b"\x1b]8;id=x;file:///tmp\x1b\\x\x1b]8;;\x1b\\".to_vec()),
        2 => Just(b"\x1bPq#0;2;100;0;0!10~$-!5~\x1b\\".to_vec()),
        1 => Just(b"\x1bPq!2147483648~!2147483648~!100~\x1b\\".to_vec()),
        2 => prop::sample::select(vec!["漢", "字", "😀", "한", "日本語"])
            .prop_map(|s| s.as_bytes().to_vec()),
        2 => prop::sample::select(vec!["\u{0301}", "\u{0308}", "e\u{0301}\u{0301}\u{0301}"])
            .prop_map(|s| s.as_bytes().to_vec()),
        1 => Just(b"\x1b[?1049h".to_vec()),
        1 => Just(b"\x1b[?1049l".to_vec()),
        1 => Just(b"\x1b[5;20r".to_vec()),
        1 => Just(b"\x1bc".to_vec()),
        1 => Just(b"\x1b[2J".to_vec()),
    ];
    prop::collection::vec(token, 0..24).prop_map(|tokens| tokens.concat())
}

fn resize_strategy() -> impl Strategy<Value = Option<Dimensions>> {
    prop_oneof![
        3 => Just(None),
        1 => (1u16..=MAX_COLS, 1u16..=MAX_ROWS).prop_map(|(c, r)| Some(Dimensions::new(c, r))),
        1 => prop::sample::select(vec![
            Dimensions::new(1, 1),
            Dimensions::new(1, 60),
            Dimensions::new(200, 1),
            Dimensions::new(2, 2),
        ])
        .prop_map(Some),
    ]
}

fn check_invariants(term: &Terminal, expected: Dimensions) -> Result<(), TestCaseError> {
    let dims = term.dimensions();
    prop_assert_eq!(dims, expected);

    let grid = term.grid();
    prop_assert_eq!(grid.dimensions(), expected);
    prop_assert_eq!(grid.rows().count(), usize::from(expected.rows));
    for row in grid.rows() {
        prop_assert_eq!(row.width(), expected.cols);
    }

    let cursor = grid.cursor().position;
    prop_assert!(
        cursor.col < expected.cols,
        "cursor col {} out of bounds for {} cols",
        cursor.col,
        expected.cols
    );
    prop_assert!(
        cursor.row < expected.rows,
        "cursor row {} out of bounds for {} rows",
        cursor.row,
        expected.rows
    );

    let region = grid.effective_scroll_region();
    prop_assert!(region.top <= region.bottom);
    prop_assert!(region.bottom < expected.rows);

    prop_assert!(term.buffer().scroll_offset() <= term.buffer().history_len());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn terminal_survives_arbitrary_output_and_resizes(
        initial in (1u16..=MAX_COLS, 1u16..=MAX_ROWS),
        steps in prop::collection::vec((chunk_strategy(), resize_strategy()), 1..16),
    ) {
        let mut dims = Dimensions::new(initial.0, initial.1);
        let mut term = Terminal::new(dims);
        let mut parser = AnsiParser::new();

        for (chunk, resize) in steps {
            {
                let mut handler = TerminalHandler::new(&mut term);
                parser.feed(&chunk, |action| handler.handle(action));
            }
            check_invariants(&term, dims)?;

            if let Some(new_dims) = resize {
                term.resize(new_dims);
                dims = new_dims;
                check_invariants(&term, dims)?;
            }
        }
    }

    #[test]
    fn terminal_survives_arbitrary_output_with_ascii_spans(
        chunk in prop::collection::vec(any::<u8>(), 0..512),
        dims in (1u16..=MAX_COLS, 1u16..=MAX_ROWS),
    ) {
        let dims = Dimensions::new(dims.0, dims.1);
        let mut term = Terminal::new(dims);
        let mut parser = AnsiParser::new();
        {
            let mut handler = TerminalHandler::new(&mut term);
            parser.feed_with_ascii_spans(&chunk, |output| match output {
                ultty::protocol::ansi::AnsiOutput::Action(action) => handler.handle(action),
                ultty::protocol::ansi::AnsiOutput::PrintAscii(span) => {
                    handler.handle_ascii_span(span)
                }
            });
        }
        check_invariants(&term, dims)?;
    }

    #[test]
    fn decode_sixel_never_panics(
        payload in prop::collection::vec(
            prop_oneof![
                6 => any::<u8>(),
                4 => prop::sample::select(vec![b'!', b'#', b'$', b'-', b'"', b';', b'~', b'?']),
                3 => b'0'..=b'9',
            ],
            0..256,
        ),
        background in 0u16..=2,
    ) {
        if let Some((pixels, width, height)) = decode_sixel(&payload, background) {
            prop_assert!(width > 0 && height > 0);
            prop_assert_eq!(pixels.len(), (width as usize) * (height as usize) * 4);
        }
    }
}
