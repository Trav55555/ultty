//! Behavioural conformance: bytes in, screen out.
//!
//! Each case feeds a byte stream to a fresh [`Terminal`] and checks what a
//! user would see (row text and cursor) or the attributes of a cell, the way
//! xterm and the VT500 series behave. Cases are grouped by area; add new
//! areas below the existing ones.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Color, Dimensions, Terminal, TerminalHandler, UnderlineStyle};

/// A `cols` x `rows` terminal after `bytes`.
fn run(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(&mut terminal);
    parser.feed(bytes, |action| handler.handle(action));
    terminal
}

/// The visible rows (trailing blanks trimmed) and the cursor as
/// `(col, row)`, both zero-based, after feeding `bytes`.
fn screen(cols: u16, rows: u16, bytes: &[u8]) -> (Vec<String>, (u16, u16)) {
    let terminal = run(cols, rows, bytes);
    let grid = terminal.grid();
    let text = grid.rows().map(|row| row.text_trimmed()).collect();
    let cursor = grid.cursor().position;
    (text, (cursor.col, cursor.row))
}

fn lines(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|row| row.to_string()).collect()
}

// ----------------------------------------------------------------------
// Control sequences never print their own bytes
// ----------------------------------------------------------------------

#[test]
fn colon_sub_parameters_are_not_printed() {
    let (text, cursor) = screen(20, 2, b"A\x1b[4:3mB\x1b[58:2::255:0:0mC\x1b[38:2:1:2:3mD");
    assert_eq!(text, lines(&["ABCD", ""]));
    assert_eq!(cursor, (4, 0));
}

#[test]
fn kitty_keyboard_set_form_is_not_printed() {
    let (text, _) = screen(20, 1, b"A\x1b[=1;1uB");
    assert_eq!(text, lines(&["AB"]));
}

#[test]
fn a_control_inside_a_sequence_runs_and_the_sequence_completes() {
    // LF executes mid-sequence, then CSI 1 K erases to the cursor.
    let (text, cursor) = screen(10, 2, b"abc\x1b[1\nK");
    assert_eq!(text, lines(&["abc", ""]));
    assert_eq!(cursor, (3, 1));
    // CR runs inside CSI 3 C.
    let (text, cursor) = screen(10, 1, b"abcdef\x1b[\r3CX");
    assert_eq!(text, lines(&["abcXef"]));
    assert_eq!(cursor, (4, 0));
}

#[test]
fn can_cancels_a_sequence_without_printing_it() {
    let (text, _) = screen(10, 1, b"a\x1b[3\x18b");
    assert_eq!(text, lines(&["ab"]));
}

#[test]
fn malformed_sequences_are_swallowed_whole() {
    let (text, _) = screen(10, 1, b"a\x1b[1?2hb\x1b[1 2qc\x1b[1\xc3\xa92md");
    assert_eq!(text, lines(&["abcd"]));
}

// ----------------------------------------------------------------------
// SGR: underline styles and underline color
// ----------------------------------------------------------------------

fn underline_of(bytes: &[u8]) -> (UnderlineStyle, Color) {
    let terminal = run(10, 1, bytes);
    let cell = terminal.grid().cell(0, 0).expect("cell");
    (cell.underline_style(), cell.underline_color())
}

#[test]
fn underline_styles() {
    use UnderlineStyle::*;
    let cases: [(&[u8], UnderlineStyle); 10] = [
        (b"\x1b[4mx", Single),
        (b"\x1b[21mx", Double),
        (b"\x1b[4:0mx", None),
        (b"\x1b[4:1mx", Single),
        (b"\x1b[4:2mx", Double),
        (b"\x1b[4:3mx", Curly),
        (b"\x1b[4:4mx", Dotted),
        (b"\x1b[4:5mx", Dashed),
        (b"\x1b[4:3m\x1b[24mx", None),
        // SGR 4 after a curly underline makes it single again.
        (b"\x1b[4:3m\x1b[4mx", Single),
    ];
    for (bytes, style) in cases {
        assert_eq!(underline_of(bytes).0, style, "{bytes:?}");
    }
}

#[test]
fn underline_color() {
    let cases: [(&[u8], Color); 6] = [
        (b"\x1b[58;5;9mx", Color::Indexed(9)),
        (b"\x1b[58;2;1;2;3mx", Color::Rgb(1, 2, 3)),
        (b"\x1b[58:5:9mx", Color::Indexed(9)),
        (b"\x1b[58:2::1:2:3mx", Color::Rgb(1, 2, 3)),
        (b"\x1b[58:2:1:2:3mx", Color::Rgb(1, 2, 3)),
        (b"\x1b[58;5;9m\x1b[59mx", Color::Default),
    ];
    for (bytes, color) in cases {
        assert_eq!(underline_of(bytes).1, color, "{bytes:?}");
    }
    // SGR 0 resets it.
    assert_eq!(
        underline_of(b"\x1b[4;58;5;9m\x1b[0mx"),
        (UnderlineStyle::None, Color::Default)
    );
}

#[test]
fn underline_color_does_not_eat_neighbouring_attributes() {
    let terminal = run(10, 1, b"\x1b[1;58;2;255;0;0;3mx");
    let cell = terminal.grid().cell(0, 0).expect("cell");
    assert!(cell.has_flag(ultty_vt::grid::CellFlags::BOLD));
    assert!(cell.has_flag(ultty_vt::grid::CellFlags::ITALIC));
    assert!(!cell.has_flag(ultty_vt::grid::CellFlags::DIM));
    assert_eq!(cell.underline_color(), Color::Rgb(255, 0, 0));
}

#[test]
fn colon_colors_set_foreground_and_background() {
    let terminal = run(10, 1, b"\x1b[38:2::10:20:30;48:5:200mx");
    let cell = terminal.grid().cell(0, 0).expect("cell");
    assert_eq!(cell.fg(), Color::Rgb(10, 20, 30));
    assert_eq!(cell.bg(), Color::Indexed(200));
}

// ----------------------------------------------------------------------
// Private markers select their own commands
// ----------------------------------------------------------------------

#[test]
fn xtmodkeys_does_not_change_attributes() {
    let terminal = run(10, 1, b"\x1b[>4;2mx");
    let cell = terminal.grid().cell(0, 0).expect("cell");
    assert!(cell.flags().is_empty(), "{:?}", cell.flags());
}

#[test]
fn kitty_keyboard_set_changes_the_current_flags() {
    let flags = |bytes: &[u8]| run(10, 1, bytes).modes().kitty_keyboard_flags;
    assert_eq!(flags(b"\x1b[=5u"), 5);
    assert_eq!(flags(b"\x1b[=5;1u"), 5);
    assert_eq!(flags(b"\x1b[=1u\x1b[=4;2u"), 5);
    assert_eq!(flags(b"\x1b[=7u\x1b[=2;3u"), 5);
    // On a pushed entry it replaces that entry, so a pop restores the one
    // below.
    assert_eq!(flags(b"\x1b[>1u\x1b[=3u"), 3);
    assert_eq!(flags(b"\x1b[>1u\x1b[>1u\x1b[=3u\x1b[<u"), 1);
    // Unknown modes change nothing.
    assert_eq!(flags(b"\x1b[=1u\x1b[=4;9u"), 1);
}

// ----------------------------------------------------------------------
// UTF-8 decoding
// ----------------------------------------------------------------------

#[test]
fn invalid_utf8_shows_replacement_characters() {
    let (text, cursor) = screen(20, 1, b"a\xF5\x80\x80\x80b\xED\xA0\x80c\xC0\xAFd\xE2\x82");
    assert_eq!(
        text,
        lines(&["a\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}b\u{FFFD}\u{FFFD}\u{FFFD}c\u{FFFD}\u{FFFD}d"])
    );
    // The truncated E2 82 at the end is still pending.
    assert_eq!(cursor, (13, 0));
}
