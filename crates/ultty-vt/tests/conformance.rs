//! Behavioural conformance cases: byte streams in, expected screens and
//! replies out, checked against xterm's behaviour.
//!
//! Each case feeds a fresh terminal and compares what a user (or the
//! application reading replies) would see, not internal state.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

/// Feed `bytes` to a fresh `cols` x `rows` terminal and return each visible
/// row's text with trailing blanks trimmed, and the cursor as `(row, col)`,
/// both 0-based.
fn screen(cols: u16, rows: u16, bytes: &[u8]) -> (Vec<String>, (u16, u16)) {
    let term = run(cols, rows, bytes);
    snapshot(&term)
}

/// A fresh `cols` x `rows` terminal after `bytes`.
fn run(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let mut term = Terminal::new(Dimensions::new(cols, rows));
    feed(&mut term, bytes);
    term
}

fn feed(term: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(term);
    parser.feed(bytes, |action| handler.handle(action));
}

fn snapshot(term: &Terminal) -> (Vec<String>, (u16, u16)) {
    let grid = term.grid();
    let rows = (0..grid.dimensions().rows)
        .map(|r| {
            grid.row(r)
                .map(|row| row.text_trimmed())
                .unwrap_or_default()
        })
        .collect();
    let cursor = grid.cursor().position;
    (rows, (cursor.row, cursor.col))
}

/// Everything the terminal answered to `bytes`, concatenated.
fn replies(term: &mut Terminal, bytes: &[u8]) -> String {
    feed(term, bytes);
    let out: Vec<u8> = term.take_responses().concat();
    String::from_utf8(out).expect("replies are UTF-8")
}

fn rows(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Tab stops across resize
// ---------------------------------------------------------------------------

#[test]
fn widening_gives_default_tab_stops_in_new_columns() {
    for alternate in [false, true] {
        let mut term = run(80, 5, b"");
        if alternate {
            feed(&mut term, b"\x1b[?1049h");
        }
        term.resize(Dimensions::new(120, 5));
        feed(&mut term, b"\x1b[1;80H\tX\tY");
        let (screen, _) = snapshot(&term);
        let expected = format!("{}X{}Y", " ".repeat(80), " ".repeat(7));
        assert_eq!(screen[0], expected, "alternate={alternate}");
    }
}

#[test]
fn hidden_alternate_screen_gets_tab_stops_when_widened() {
    let mut term = run(80, 5, b"");
    term.resize(Dimensions::new(120, 5));
    feed(&mut term, b"\x1b[?1049h\x1b[1;80H\tX");
    assert_eq!(snapshot(&term).0[0], format!("{}X", " ".repeat(80)));
}

#[test]
fn custom_tab_stops_survive_widening_and_narrowing_drops_far_ones() {
    // Clear all stops, set one at column 4 (0-based 3).
    let mut term = run(20, 3, b"\x1b[3g\x1b[1;4H\x1bH\r");
    term.resize(Dimensions::new(40, 3));
    feed(&mut term, b"\tA\tB");
    // Old range keeps only the custom stop; new columns 24 and 32 get defaults.
    assert_eq!(snapshot(&term).0[0], format!("   A{}B", " ".repeat(24 - 4)));

    term.resize(Dimensions::new(10, 3));
    feed(&mut term, b"\x1b[2J\x1b[2;1H\tC\tD");
    // Stops beyond the new width are gone: after the stop at 3, a tab goes to
    // the last column.
    assert_eq!(snapshot(&term).0[1], format!("   C{}D", " ".repeat(5)));
}
