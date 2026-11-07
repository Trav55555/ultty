//! Behavioural conformance cases: byte streams in, expected screens and
//! replies out, checked against xterm's behaviour.
//!
//! Each case feeds a fresh terminal and compares what a user (or the
//! application reading replies) would see, not internal state.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{CellFlags, Dimensions, Terminal, TerminalHandler};

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

#[test]
fn tab_stops_survive_a_height_only_resize() {
    let mut term = run(20, 3, b"\x1b[3g\x1b[1;6H\x1bH\r");
    term.resize(Dimensions::new(20, 6));
    feed(&mut term, b"\x1b[1;1H\tA");
    assert_eq!(snapshot(&term).0[0], "     A");
}

// ---------------------------------------------------------------------------
// DECCOLM (mode 3) and its gate, mode 40
// ---------------------------------------------------------------------------

#[test]
fn deccolm_is_ignored_unless_mode_40_allows_it() {
    // `tput reset` sends CSI ?3 l; without mode 40 it must neither clear the
    // screen nor ask for a resize.
    let mut term = run(10, 3, b"keep\x1b[?3l\x1b[?3h");
    assert_eq!(snapshot(&term), (rows(&["keep", "", ""]), (0, 4)));
    assert_eq!(term.take_column_resize_request(), None);
}

#[test]
fn deccolm_with_mode_40_resizes_clears_and_homes() {
    let mut term = run(10, 3, b"gone\x1b[?40h\x1b[?3h");
    assert_eq!(snapshot(&term), (rows(&["", "", ""]), (0, 0)));
    assert_eq!(term.take_column_resize_request(), Some(132));
    feed(&mut term, b"\x1b[?3l");
    assert_eq!(term.take_column_resize_request(), Some(80));
}

// ---------------------------------------------------------------------------
// ED / EL
// ---------------------------------------------------------------------------

#[test]
fn erase_in_display_variants() {
    let fill = b"abcde\r\nfghij\r\nklmno\x1b[2;3H";
    let (s, c) = screen(5, 3, &[&fill[..], b"\x1b[J"].concat());
    assert_eq!((s, c), (rows(&["abcde", "fg", ""]), (1, 2)));
    let (s, _) = screen(5, 3, &[&fill[..], b"\x1b[1J"].concat());
    assert_eq!(s, rows(&["", "   ij", "klmno"]));
    let (s, c) = screen(5, 3, &[&fill[..], b"\x1b[2J"].concat());
    assert_eq!((s, c), (rows(&["", "", ""]), (1, 2)));
}

#[test]
fn ed_3_clears_scrollback_but_not_the_screen() {
    let mut term = run(5, 2, b"1\r\n2\r\n3\r\n4");
    assert!(term.buffer().history_len() > 0);
    feed(&mut term, b"\x1b[3J");
    assert_eq!(term.buffer().history_len(), 0);
    assert_eq!(snapshot(&term), (rows(&["3", "4"]), (1, 1)));
}

#[test]
fn erase_in_line_variants() {
    let fill = b"abcde\x1b[1;3H";
    let erased = |op: &[u8]| screen(5, 1, &[&fill[..], op].concat()).0;
    assert_eq!(erased(b"\x1b[K"), rows(&["ab"]));
    assert_eq!(erased(b"\x1b[1K"), rows(&["   de"]));
    assert_eq!(erased(b"\x1b[2K"), rows(&[""]));
}

// ---------------------------------------------------------------------------
// DECSTBM, IL/DL at margins, DECOM
// ---------------------------------------------------------------------------

#[test]
fn invalid_decstbm_is_ignored() {
    // A valid region 2-4, then top == bottom and top > bottom requests.
    let (s, c) = screen(3, 5, b"\x1b[2;4r\x1b[3;2Hx\x1b[3;3r\x1b[4;2r");
    assert_eq!(c, (2, 2), "cursor must not home on an ignored DECSTBM");
    assert_eq!(s, rows(&["", "", " x", "", ""]));
    // The 2-4 region is still in force: LF at row 4 scrolls only rows 2-4.
    let (s, _) = screen(3, 5, b"1\r\n2\r\n3\r\n4\r\n5\x1b[2;4r\x1b[3;3r\x1b[4;1H\n");
    assert_eq!(s, rows(&["1", "3", "4", "", "5"]));
}

#[test]
fn decstbm_homes_the_cursor() {
    assert_eq!(screen(5, 5, b"\x1b[3;3H\x1b[2;4r").1, (0, 0));
}

#[test]
fn insert_and_delete_lines_stay_inside_the_margins() {
    let fill = b"1\r\n2\r\n3\r\n4\r\n5\x1b[2;4r";
    let (s, c) = screen(3, 5, &[&fill[..], b"\x1b[3;2H\x1b[L"].concat());
    assert_eq!(s, rows(&["1", "2", "", "3", "5"]));
    assert_eq!(c, (2, 0), "IL moves to the left margin");
    let (s, _) = screen(3, 5, &[&fill[..], b"\x1b[2;1H\x1b[2M"].concat());
    assert_eq!(s, rows(&["1", "4", "", "", "5"]));
    // Outside the margins IL/DL do nothing.
    let (s, _) = screen(
        3,
        5,
        &[&fill[..], b"\x1b[5;1H\x1b[L\x1b[1;1H\x1b[M"].concat(),
    );
    assert_eq!(s, rows(&["1", "2", "3", "4", "5"]));
}

#[test]
fn origin_mode_addresses_relative_to_the_region_and_clamps() {
    let (s, c) = screen(5, 6, b"\x1b[2;4r\x1b[?6h\x1b[1;1Ha\x1b[9;2Hb");
    assert_eq!(s, rows(&["", "a", "", " b", "", ""]));
    assert_eq!(c, (3, 2));
    // Setting DECOM homes to the region top; CPR reports relative rows.
    let mut term = run(5, 6, b"\x1b[2;4r\x1b[?6h");
    assert_eq!(snapshot(&term).1, (1, 0));
    assert_eq!(replies(&mut term, b"\x1b[2;3H\x1b[6n"), "\x1b[2;3R");
    // Resetting it homes to the screen origin.
    feed(&mut term, b"\x1b[?6l");
    assert_eq!(snapshot(&term).1, (0, 0));
}

// ---------------------------------------------------------------------------
// Scrollback and scroll regions
// ---------------------------------------------------------------------------

#[test]
fn main_screen_region_at_the_top_scrolls_into_history() {
    // apt-style: rows 1-4 scroll, row 5 is a status line.
    let mut term = run(5, 5, b"\x1b[1;4r");
    for i in 0..10 {
        feed(&mut term, format!("{i}\r\n").as_bytes());
    }
    assert_eq!(term.buffer().history_len(), 7);
    assert_eq!(snapshot(&term).0, rows(&["7", "8", "9", "", ""]));
    let mut view = term.buffer().clone();
    view.scroll_up(7);
    assert_eq!(view.visible_row(0).unwrap().text_trimmed(), "0");
}

#[test]
fn every_scroll_path_feeds_history_from_a_top_region() {
    for (name, bytes) in [
        ("wrap", &b"\x1b[1;2r\x1b[2;1Habcdefgh"[..]),
        ("ind", b"\x1b[1;2r\x1b[2;1H\x1bD"),
        ("nel", b"\x1b[1;2r\x1b[2;1H\x1bE"),
        ("su", b"\x1b[1;2r\x1b[S"),
        ("lf", b"\x1b[1;2r\x1b[2;1H\n"),
        ("wide wrap", "\x1b[1;2r\x1b[2;1Habc\u{4e2d}".as_bytes()),
    ] {
        let term = run(4, 3, &[&b"top\r\n"[..], bytes].concat());
        assert_eq!(term.buffer().history_len(), 1, "{name}");
    }
}

#[test]
fn scrolling_more_than_the_region_saves_only_region_rows() {
    let term = run(3, 4, b"a\r\nb\r\nc\r\nd\x1b[1;2r\x1b[9S");
    assert_eq!(term.buffer().history_len(), 2);
    assert_eq!(snapshot(&term).0, rows(&["", "", "c", "d"]));
}

#[test]
fn regions_not_at_the_top_and_the_alternate_screen_keep_no_history() {
    let mut term = run(5, 5, b"\x1b[2;4r\x1b[4;1H");
    feed(&mut term, b"a\r\nb\r\nc\r\n");
    assert_eq!(term.buffer().history_len(), 0);
    let term = run(5, 3, b"\x1b[?1049h1\r\n2\r\n3\r\n4\r\n\x1b[?1049l");
    assert_eq!(term.buffer().history_len(), 0);
}

// ---------------------------------------------------------------------------
// DECSTR, DECSC/DECRC
// ---------------------------------------------------------------------------

#[test]
fn decstr_keeps_the_cursor_and_resets_state() {
    let mut term = run(
        10,
        5,
        b"\x1b[2;4r\x1b[?6h\x1b[4h\x1b[?7l\x1b[?1h\x1b=\x1b[1m\x1b[?25l\x1b[3;3H\x1b7\x1b[2;4H",
    );
    feed(&mut term, b"\x1b[!p");
    let grid = term.grid();
    assert_eq!(snapshot(&term).1, (2, 3), "DECSTR must not move the cursor");
    assert_eq!(grid.scroll_region(), None);
    assert!(grid.cursor().visible);
    assert!(grid.auto_wrap());
    assert!(!grid.insert_mode());
    assert!(!grid.attrs().has_flag(CellFlags::BOLD));
    let modes = term.modes();
    assert!(!modes.origin_mode && !modes.insert_mode && modes.auto_wrap);
    assert!(!modes.application_cursor_keys && !modes.application_keypad);
    // The saved cursor is back to its default: DECRC homes.
    feed(&mut term, b"\x1b8");
    assert_eq!(snapshot(&term).1, (0, 0));
}

#[test]
fn decstr_resets_the_character_set() {
    // One parser across the stream: charsets live in the parser.
    let (s, _) = screen(5, 1, b"\x1b(0q\x1b[!pq");
    assert_ne!(s[0].chars().next(), Some('q'));
    assert_eq!(s[0].chars().nth(1), Some('q'));
}

#[test]
fn decsc_decrc_restore_position_attributes_and_origin_mode() {
    let mut term = run(10, 5, b"\x1b[2;4r\x1b[?6h\x1b[2;3H\x1b[1m\x1b7");
    feed(&mut term, b"\x1b[?6l\x1b[0m\x1b[5;5H\x1b8X");
    // Origin mode was on when saved: region row 2 is screen row 3.
    let cell = term.grid().cell(2, 2).unwrap();
    assert_eq!(cell.char(), 'X');
    assert!(cell.has_flag(CellFlags::BOLD));
    assert!(term.modes().origin_mode);
}

// ---------------------------------------------------------------------------
// Alternate screens 1047 / 1049
// ---------------------------------------------------------------------------

#[test]
fn mode_1049_saves_cursor_clears_and_restores() {
    let mut term = run(6, 3, b"main\x1b[2;2H\x1b[?1049h");
    assert_eq!(snapshot(&term), (rows(&["", "", ""]), (0, 0)));
    feed(&mut term, b"alt\x1b[3;3H\x1b[?1049l");
    assert_eq!(snapshot(&term), (rows(&["main", "", ""]), (1, 1)));
    feed(&mut term, b"\x1b[?1049h");
    assert_eq!(snapshot(&term).0, rows(&["", "", ""]));
}

#[test]
fn mode_1047_clears_on_entry_and_keeps_the_main_cursor() {
    let mut term = run(6, 3, b"main\x1b[?1047h");
    assert_eq!(snapshot(&term).0, rows(&["", "", ""]));
    feed(&mut term, b"alt\x1b[?1047l");
    assert_eq!(snapshot(&term), (rows(&["main", "", ""]), (0, 4)));
}

// ---------------------------------------------------------------------------
// Wide characters at the right margin
// ---------------------------------------------------------------------------

#[test]
fn wide_char_that_does_not_fit_without_autowrap_is_dropped() {
    let (s, c) = screen(4, 2, "\x1b[?7labc\u{4e2d}".as_bytes());
    assert_eq!(s, rows(&["abc", ""]));
    assert_eq!(c, (0, 3));
    let (s, _) = screen(4, 2, "abc\u{4e2d}".as_bytes());
    assert_eq!(s, rows(&["abc", "\u{4e2d}"]));
}
