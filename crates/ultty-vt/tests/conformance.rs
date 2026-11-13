//! Behavioural conformance cases: byte streams in, expected screens and
//! replies out, checked against xterm's behaviour.
//!
//! Each case feeds a fresh terminal and compares what a user (or the
//! application reading replies) would see, not internal state.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{
    CellFlags, Color, Dimensions, Terminal, TerminalHandler, ThemeColors, UnderlineStyle,
};

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

// ---------------------------------------------------------------------------
// LNM (mode 20) and DECSC via mode 1048
// ---------------------------------------------------------------------------

#[test]
fn lnm_makes_line_feeds_return_the_carriage() {
    let (s, c) = screen(5, 3, b"\x1b[20hab\ncd\x0bef");
    assert_eq!((s, c), (rows(&["ab", "cd", "ef"]), (2, 2)));
    let (s, _) = screen(5, 2, b"\x1b[20h\x1b[20lab\ncd");
    assert_eq!(s, rows(&["ab", "  cd"]));
}

#[test]
fn mode_1048_saves_and_restores_the_cursor() {
    assert_eq!(
        screen(5, 3, b"\x1b[2;3H\x1b[?1048h\x1b[H\x1b[?1048l").1,
        (1, 2)
    );
}

// ---------------------------------------------------------------------------
// Identity and status reports
// ---------------------------------------------------------------------------

fn version_number() -> u32 {
    let mut parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|p| p.parse::<u32>().unwrap());
    let (major, minor, patch) = (
        parts.next().unwrap(),
        parts.next().unwrap(),
        parts.next().unwrap(),
    );
    major * 10_000 + minor * 100 + patch
}

#[test]
fn device_attributes_tell_one_story() {
    let mut term = run(10, 3, b"");
    // VT220 class with sixel and ANSI colour, nothing it cannot do.
    assert_eq!(replies(&mut term, b"\x1b[c"), "\x1b[?62;4;22c");
    assert_eq!(replies(&mut term, b"\x1b[0c"), "\x1b[?62;4;22c");
    assert_eq!(
        replies(&mut term, b"\x1b[>c"),
        format!("\x1b[>1;{};0c", version_number())
    );
    assert_eq!(
        replies(&mut term, b"\x1b[>0q"),
        format!("\x1bP>|ultty {}\x1b\\", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn device_status_and_cursor_reports() {
    let mut term = run(10, 5, b"\x1b[3;7H");
    assert_eq!(replies(&mut term, b"\x1b[5n"), "\x1b[0n");
    assert_eq!(replies(&mut term, b"\x1b[6n"), "\x1b[3;7R");
    assert_eq!(replies(&mut term, b"\x1b[?6n"), "\x1b[?3;7R");
}

#[test]
fn window_size_reports() {
    let mut term = run(80, 24, b"");
    assert_eq!(replies(&mut term, b"\x1b[18t"), "\x1b[8;24;80t");
    // Pixel sizes are unknown until the embedder says how big a cell is.
    assert_eq!(replies(&mut term, b"\x1b[14t\x1b[16t"), "");
    term.set_cell_pixel_size(9, 18);
    assert_eq!(replies(&mut term, b"\x1b[16t"), "\x1b[6;18;9t");
    assert_eq!(replies(&mut term, b"\x1b[14t"), "\x1b[4;432;720t");
}

#[test]
fn title_stack_pushes_and_pops() {
    let mut term = run(10, 3, b"\x1b]2;shell\x07\x1b[22;0t\x1b]2;vim\x07");
    assert_eq!(term.title(), "vim");
    feed(&mut term, b"\x1b[23;0t");
    assert_eq!(term.title(), "shell");
    // Window-title-only forms work too; icon-only ones leave it alone.
    feed(&mut term, b"\x1b[22;2t\x1b]2;less\x07\x1b[23;1t");
    assert_eq!(term.title(), "less");
    feed(&mut term, b"\x1b[23;2t");
    assert_eq!(term.title(), "shell");
    // Popping an empty stack changes nothing.
    feed(&mut term, b"\x1b[23;0t");
    assert_eq!(term.title(), "shell");
}

#[test]
fn title_stack_depth_is_bounded() {
    let mut term = run(10, 3, b"");
    for i in 0..50 {
        feed(&mut term, format!("\x1b]2;t{i}\x07\x1b[22t").as_bytes());
    }
    feed(&mut term, b"\x1b]2;end\x07");
    let mut popped = 0;
    loop {
        let before = term.title().to_string();
        feed(&mut term, b"\x1b[23t");
        if term.title() == before {
            break;
        }
        popped += 1;
    }
    assert!(
        popped > 0 && popped < 50,
        "stack must be bounded, popped {popped}"
    );
    // The most recent pushes are the ones kept.
    assert_eq!(term.title(), format!("t{}", 50 - popped));
}

// ---------------------------------------------------------------------------
// DECRQM
// ---------------------------------------------------------------------------

/// The DECRQM status the terminal reports for private mode `mode`.
fn private_mode_status(term: &mut Terminal, mode: u16) -> String {
    let reply = replies(term, format!("\x1b[?{mode}$p").as_bytes());
    let prefix = format!("\x1b[?{mode};");
    reply
        .strip_prefix(&prefix)
        .and_then(|r| r.strip_suffix("$y"))
        .unwrap_or_else(|| panic!("bad DECRQM reply {reply:?}"))
        .to_string()
}

#[test]
fn decrqm_reports_every_private_mode_it_implements() {
    // Modes whose set and reset forms are plain toggles.
    for mode in [
        1u16, 5, 6, 7, 9, 12, 25, 40, 80, 1000, 1002, 1003, 1004, 1005, 1006, 1007, 1015, 2004,
        2026, 2027,
    ] {
        let mut term = run(10, 3, format!("\x1b[?{mode}h").as_bytes());
        assert_eq!(private_mode_status(&mut term, mode), "1", "mode {mode} set");
        feed(&mut term, format!("\x1b[?{mode}l").as_bytes());
        assert_eq!(
            private_mode_status(&mut term, mode),
            "2",
            "mode {mode} reset"
        );
    }
    for mode in [47u16, 1047, 1049] {
        let mut term = run(10, 3, format!("\x1b[?{mode}h").as_bytes());
        assert_eq!(private_mode_status(&mut term, mode), "1", "mode {mode} set");
        feed(&mut term, format!("\x1b[?{mode}l").as_bytes());
        assert_eq!(
            private_mode_status(&mut term, mode),
            "2",
            "mode {mode} reset"
        );
    }
    let mut term = run(10, 3, b"\x1b[?40h\x1b[?3h");
    assert_eq!(private_mode_status(&mut term, 3), "1");
    feed(&mut term, b"\x1b[?3l");
    assert_eq!(private_mode_status(&mut term, 3), "2");
}

#[test]
fn decrqm_reports_defaults_and_unknown_modes() {
    let mut term = run(10, 3, b"");
    assert_eq!(private_mode_status(&mut term, 7), "1", "autowrap on");
    assert_eq!(private_mode_status(&mut term, 25), "1", "cursor visible");
    assert_eq!(
        private_mode_status(&mut term, 1007),
        "1",
        "alternate scroll"
    );
    assert_eq!(private_mode_status(&mut term, 1049), "2");
    assert_eq!(private_mode_status(&mut term, 4242), "0");
}

#[test]
fn decrqm_for_ansi_modes() {
    let mut term = run(10, 3, b"\x1b[4h");
    assert_eq!(replies(&mut term, b"\x1b[4$p"), "\x1b[4;1$y");
    assert_eq!(replies(&mut term, b"\x1b[20$p"), "\x1b[20;2$y");
    feed(&mut term, b"\x1b[4l\x1b[20h");
    assert_eq!(
        replies(&mut term, b"\x1b[4$p\x1b[20$p"),
        "\x1b[4;2$y\x1b[20;1$y"
    );
    // Keyboard never locks; local echo never happens.
    assert_eq!(
        replies(&mut term, b"\x1b[2$p\x1b[12$p"),
        "\x1b[2;4$y\x1b[12;3$y"
    );
    assert_eq!(replies(&mut term, b"\x1b[99$p"), "\x1b[99;0$y");
}

// ---------------------------------------------------------------------------
// DECRQSS and XTGETTCAP
// ---------------------------------------------------------------------------

#[test]
fn decrqss_reports_sgr_margins_and_cursor_style() {
    let mut term = run(10, 6, b"");
    assert_eq!(replies(&mut term, b"\x1bP$qm\x1b\\"), "\x1bP1$r0m\x1b\\");
    feed(&mut term, b"\x1b[1;4;31;48;5;200m");
    assert_eq!(
        replies(&mut term, b"\x1bP$qm\x1b\\"),
        "\x1bP1$r0;1;4;31;48;5;200m\x1b\\"
    );
    feed(&mut term, b"\x1b[0;3;7;38;2;1;2;3;102m");
    assert_eq!(
        replies(&mut term, b"\x1bP$qm\x1b\\"),
        "\x1bP1$r0;3;7;38;2;1;2;3;102m\x1b\\"
    );
    feed(&mut term, b"\x1b[0;4:3;58:2::255:0:0m");
    assert_eq!(
        replies(&mut term, b"\x1bP$qm\x1b\\"),
        "\x1bP1$r0;4:3;58:2::255:0:0m\x1b\\"
    );
    feed(&mut term, b"\x1b[0;21;58;5;9m");
    assert_eq!(
        replies(&mut term, b"\x1bP$qm\x1b\\"),
        "\x1bP1$r0;21;58:5:9m\x1b\\"
    );
    assert_eq!(replies(&mut term, b"\x1bP$qr\x1b\\"), "\x1bP1$r1;6r\x1b\\");
    feed(&mut term, b"\x1b[2;5r");
    assert_eq!(replies(&mut term, b"\x1bP$qr\x1b\\"), "\x1bP1$r2;5r\x1b\\");
    feed(&mut term, b"\x1b[6 q");
    assert_eq!(replies(&mut term, b"\x1bP$q q\x1b\\"), "\x1bP1$r6 q\x1b\\");
    feed(&mut term, b"\x1b[3 q");
    assert_eq!(replies(&mut term, b"\x1bP$q q\x1b\\"), "\x1bP1$r3 q\x1b\\");
    assert_eq!(replies(&mut term, b"\x1bP$qzz\x1b\\"), "\x1bP0$r\x1b\\");
}

#[test]
fn decrqss_is_not_mistaken_for_sixel() {
    let term = run(10, 3, b"\x1bP$qm\x1b\\\x1bP+q544E\x1b\\");
    assert!(term.images().is_empty());
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02X}")).collect()
}

#[test]
fn xtgettcap_answers_a_small_truthful_set() {
    let mut term = run(10, 3, b"");
    let ask = |names: &[&str]| {
        let list: Vec<String> = names.iter().map(|n| hex(n)).collect();
        format!("\x1bP+q{}\x1b\\", list.join(";"))
    };
    assert_eq!(
        replies(&mut term, ask(&["TN"]).as_bytes()),
        format!("\x1bP1+r{}={}\x1b\\", hex("TN"), hex("xterm-256color"))
    );
    assert_eq!(
        replies(&mut term, ask(&["Co", "colors"]).as_bytes()),
        format!(
            "\x1bP1+r{}={}\x1b\\\x1bP1+r{}={}\x1b\\",
            hex("Co"),
            hex("256"),
            hex("colors"),
            hex("256")
        )
    );
    assert_eq!(
        replies(&mut term, ask(&["RGB"]).as_bytes()),
        format!("\x1bP1+r{}={}\x1b\\", hex("RGB"), hex("8/8/8"))
    );
    assert_eq!(
        replies(&mut term, ask(&["kbs"]).as_bytes()),
        format!("\x1bP0+r{}\x1b\\", hex("kbs"))
    );
    // Lower-case hex in the request is accepted.
    assert_eq!(
        replies(&mut term, b"\x1bP+q544e\x1b\\"),
        format!("\x1bP1+r{}={}\x1b\\", hex("TN"), hex("xterm-256color"))
    );
    // An unknown name in plain hex is echoed back as invalid.
    assert_eq!(
        replies(&mut term, b"\x1bP+q5858\x1b\\"),
        "\x1bP0+r5858\x1b\\"
    );
    // Anything else is never echoed: replies are typed into the shell, so
    // echoing the request would let a program inject a command.
    assert_eq!(replies(&mut term, b"\x1bP+qZZ\x1b\\"), "\x1bP0+r\x1b\\");
    assert_eq!(
        replies(&mut term, b"\x1bP+qfoo\rid\r\x1b\\"),
        "\x1bP0+r\x1b\\"
    );
}

#[test]
fn xtgettcap_reports_the_term_the_program_was_started_with() {
    let mut term = run(10, 3, b"");
    term.set_terminal_name("xterm-ultty");
    let ask_tn = format!("\x1bP+q{}\x1b\\", hex("TN"));
    assert_eq!(
        replies(&mut term, ask_tn.as_bytes()),
        format!("\x1bP1+r{}={}\x1b\\", hex("TN"), hex("xterm-ultty"))
    );
}

// ---------------------------------------------------------------------------
// Mouse mode 1005 (UTF-8 coordinates)
// ---------------------------------------------------------------------------

#[test]
fn utf8_mouse_encodes_large_coordinates_as_utf8() {
    use ultty_vt::grid::{MouseButton, MouseModifiers};
    let term = run(300, 300, b"\x1b[?1000h\x1b[?1005h");
    let none = MouseModifiers::default();
    // Small coordinates are single bytes, value + 32, exactly like X10.
    let small = term
        .encode_mouse_event(MouseButton::Left, 9, 4, true, false, none)
        .unwrap();
    assert_eq!(small, b"\x1b[M\x20\x2a\x25");
    // Column 200 (1-based 201) is U+00E9 + ... : 201 + 32 = 233 = U+00E9.
    let big = term
        .encode_mouse_event(MouseButton::Left, 200, 99, true, false, none)
        .unwrap();
    let mut expected = b"\x1b[M\x20".to_vec();
    expected.extend("\u{e9}".as_bytes());
    expected.extend(char::from_u32(100 + 32).unwrap().to_string().as_bytes());
    assert_eq!(big, expected);
    assert!(std::str::from_utf8(&big).is_ok());
    // Release is button 3 like X10.
    let release = term
        .encode_mouse_event(MouseButton::Left, 0, 0, false, false, none)
        .unwrap();
    assert_eq!(release, b"\x1b[M\x23\x21\x21");
}

// ---------------------------------------------------------------------------
// Sixel placement
// ---------------------------------------------------------------------------

#[test]
fn sixel_images_span_cells_of_the_reported_size() {
    // An 18 x 36 pixel image: six bands of 18 columns.
    let band = "!18~";
    let sixel = format!("\x1bPq#0;2;100;0;0#0{}\x1b\\", [band; 6].join("-"));
    let spans = |cell: Option<(u16, u16)>| {
        let mut term = run(20, 10, b"");
        if let Some((w, h)) = cell {
            term.set_cell_pixel_size(w, h);
        }
        feed(&mut term, sixel.as_bytes());
        let image = term.images().images().next().expect("image placed");
        assert_eq!((image.width, image.height), (18, 36));
        (image.cell_width, image.cell_height)
    };
    assert_eq!(spans(Some((9, 18))), (2, 2));
    assert_eq!(spans(None), (3, 3), "8x16 fallback");
}

/// A 4 x 6 pixel red sixel.
const SIXEL: &[u8] = b"\x1bPq#0;2;100;0;0#0~~~~\x1b\\";

fn image_ids(term: &Terminal) -> Vec<u32> {
    let mut ids: Vec<u32> = term.images().images().map(|image| image.id.0).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn each_screen_keeps_its_own_images() {
    // foot keeps `normal.sixel_images` and `alt.sixel_images` apart.
    let mut term = run(20, 10, SIXEL);
    let main_ids = image_ids(&term);
    assert_eq!(main_ids.len(), 1);

    feed(&mut term, b"\x1b[?1049h");
    assert!(term.images().is_empty(), "the alternate screen starts bare");
    feed(&mut term, SIXEL);
    let alt_ids = image_ids(&term);
    assert_eq!(alt_ids.len(), 1);
    assert_ne!(alt_ids, main_ids, "ids are unique across screens");

    feed(&mut term, b"\x1b[?1049l");
    assert_eq!(image_ids(&term), main_ids, "the main screen's image stays");

    feed(&mut term, b"\x1b[?1049h");
    assert!(
        term.images().is_empty(),
        "leaving cleared the alternate store"
    );
}

#[test]
fn leaving_the_alternate_screen_deletes_its_images_in_every_mode() {
    for (enter, leave) in [
        (&b"\x1b[?47h"[..], &b"\x1b[?47l"[..]),
        (b"\x1b[?1047h", b"\x1b[?1047l"),
        (b"\x1b[?1049h", b"\x1b[?1049l"),
    ] {
        let mut term = run(20, 10, enter);
        feed(&mut term, SIXEL);
        assert_eq!(term.images().images().count(), 1);
        feed(&mut term, leave);
        feed(&mut term, b"\x1b[?1047h");
        assert!(term.images().is_empty(), "after {enter:?}");
    }
}

#[test]
fn ed_2_deletes_the_images_on_screen() {
    let mut term = run(20, 10, SIXEL);
    feed(&mut term, SIXEL);
    assert_eq!(term.images().images().count(), 2);
    feed(&mut term, b"\x1b[2J");
    assert!(term.images().is_empty());
}

#[test]
fn ris_deletes_the_images_of_both_screens() {
    let mut term = run(20, 10, SIXEL);
    feed(&mut term, b"\x1b[?1049h");
    feed(&mut term, SIXEL);
    feed(&mut term, b"\x1bc");
    assert!(term.images().is_empty(), "main screen after RIS");
    feed(&mut term, b"\x1b[?47h");
    assert!(term.images().is_empty(), "alternate screen after RIS");
}

/// The screen rows (negative above the screen, in scrollback) of the active
/// screen's images, top first.
fn image_rows(term: &Terminal) -> Vec<isize> {
    let buffer = term.buffer();
    let mut rows: Vec<isize> = term
        .images()
        .images()
        .map(|image| buffer.screen_row_of_line(image.line))
        .collect();
    rows.sort_unstable();
    rows
}

/// A cursor-position escape to 1-based `row`, column 1.
fn goto(row: u16) -> Vec<u8> {
    format!("\x1b[{row};1H").into_bytes()
}

fn place_sixel_at(term: &mut Terminal, row: u16) {
    feed(term, &goto(row));
    feed(term, SIXEL);
}

#[test]
fn images_scroll_into_scrollback_with_their_text() {
    let mut term = run(20, 5, b"");
    place_sixel_at(&mut term, 2);
    assert_eq!(image_rows(&term), [1]);
    feed(&mut term, &goto(5));
    feed(&mut term, b"\n\n\n");
    assert_eq!(image_rows(&term), [-2], "three lines scrolled off");
    feed(&mut term, b"\n\n\n");
    assert_eq!(image_rows(&term), [-5]);
}

#[test]
fn images_below_a_top_region_move_with_their_text() {
    // Rows 1..=3 scroll (SU) and their lines go to scrollback; the image
    // below the region stays on its screen row, which is a new line.
    let mut term = run(20, 8, b"\x1b[1;3r");
    place_sixel_at(&mut term, 6);
    feed(&mut term, b"\x1b[2S");
    assert_eq!(image_rows(&term), [5]);
}

#[test]
fn images_in_a_lower_region_scroll_out_and_are_removed() {
    let mut term = run(20, 8, b"\x1b[3;6r");
    place_sixel_at(&mut term, 4);
    feed(&mut term, b"\x1b[1S");
    assert_eq!(image_rows(&term), [2], "up one row with its text");
    feed(&mut term, b"\x1b[1S");
    assert!(term.images().is_empty(), "pushed out of the region");

    let mut term = run(20, 8, b"\x1b[3;6r");
    place_sixel_at(&mut term, 4);
    feed(&mut term, b"\x1b[1T");
    assert_eq!(image_rows(&term), [4], "down one row");
    feed(&mut term, b"\x1b[2T");
    assert!(term.images().is_empty(), "pushed past the region's bottom");
}

#[test]
fn insert_and_delete_lines_move_the_images_below_the_cursor() {
    let mut term = run(20, 8, b"");
    place_sixel_at(&mut term, 4);
    feed(&mut term, &goto(2));
    feed(&mut term, b"\x1b[2L");
    assert_eq!(image_rows(&term), [5]);
    feed(&mut term, b"\x1b[1M");
    assert_eq!(image_rows(&term), [4]);
    // Deleting the image's own row deletes the image.
    feed(&mut term, &goto(5));
    feed(&mut term, b"\x1b[1M");
    assert!(term.images().is_empty());
}

#[test]
fn scrolling_the_alternate_screen_moves_its_images() {
    let mut term = run(20, 5, b"\x1b[?1049h");
    place_sixel_at(&mut term, 3);
    feed(&mut term, &goto(5));
    feed(&mut term, b"\n");
    assert_eq!(image_rows(&term), [1]);
    feed(&mut term, b"\n\n");
    assert!(term.images().is_empty(), "scrolled off, with no scrollback");
}

#[test]
fn ed_3_deletes_the_images_in_scrollback_only() {
    let mut term = run(20, 5, b"");
    place_sixel_at(&mut term, 1);
    feed(&mut term, &goto(5));
    feed(&mut term, b"\n\n");
    place_sixel_at(&mut term, 4);
    assert_eq!(image_rows(&term), [-2, 3]);
    feed(&mut term, b"\x1b[3J");
    assert_eq!(image_rows(&term), [3]);
}

#[test]
fn images_follow_their_row_through_reflow() {
    // Row 0 wraps to two rows at 5 columns, the blank row stays one, so the
    // image's row becomes row 3. It sits on the last row, which reflow
    // trims as blank unless it counts the image as content.
    let mut term = run(10, 8, b"aaaaaaaaaa\r\n");
    place_sixel_at(&mut term, 3);
    assert_eq!(image_rows(&term), [2]);
    term.resize(Dimensions::new(5, 8));
    assert_eq!(image_rows(&term), [3]);
    term.resize(Dimensions::new(10, 8));
    assert_eq!(image_rows(&term), [2]);
}

#[test]
fn images_keep_their_line_when_the_screen_gets_shorter() {
    let mut term = run(10, 8, b"");
    place_sixel_at(&mut term, 4);
    // The four blank rows below the cursor go first; the two top rows of
    // the rest move into scrollback, and the image with its row stays on
    // line 3.
    term.resize(Dimensions::new(10, 2));
    assert_eq!(image_rows(&term), [1]);
    assert_eq!(term.buffer().history_len(), 2);
}

/// A sixel of `bytes` data bytes: one 100-pixel red run redrawn over itself (`$`), so it decodes to a
/// 6-pixel-high image however long the string is.
fn long_sixel(bytes: usize) -> Vec<u8> {
    let mut data = b"\x1bPq#0;2;100;0;0#0".to_vec();
    while data.len() < bytes {
        data.extend_from_slice(b"!100~$");
    }
    data.extend_from_slice(b"\x1b\\");
    data
}

#[test]
fn sixel_past_the_dcs_cap_is_swallowed_not_printed() {
    // 64 KiB is the cap for ordinary DCS strings; 70 KB of sixel is within
    // the sixel cap and decodes as an image.
    let mut input = long_sixel(70_000);
    input.extend_from_slice(b"after");
    let term = run(20, 10, &input);
    let (screen, _) = snapshot(&term);
    assert_eq!(
        screen[0].trim_start_matches(' '),
        "after".to_string().as_str(),
        "{screen:?}"
    );
    assert_eq!(term.images().images().count(), 1);
}

#[test]
fn oversized_dcs_strings_are_swallowed_to_the_terminator() {
    // A non-sixel DCS stays at 64 KiB: the rest is dropped, never printed.
    let mut input = b"\x1bP1$r".to_vec();
    input.extend(std::iter::repeat_n(b'~', 70_000));
    input.extend_from_slice(b"\x1b\\after");
    let (screen, _) = screen(20, 10, &input);
    assert_eq!(screen[0], "after", "{screen:?}");

    // A sixel past the sixel cap (8 MiB) is dropped whole, with no image.
    let mut input = long_sixel(9 * 1024 * 1024);
    input.extend_from_slice(b"after");
    let term = run(20, 10, &input);
    let (screen, _) = snapshot(&term);
    assert_eq!(
        screen[0],
        "after",
        "{:?}",
        &screen[0][..screen[0].len().min(40)]
    );
    assert_eq!(term.images().images().count(), 0);
}

// ---------------------------------------------------------------------------
// Colour queries and the palette (OSC 4, 10, 11, 12, 104)
// ---------------------------------------------------------------------------

fn everforest() -> ThemeColors {
    let mut ansi = ThemeColors::default().ansi;
    ansi[1] = [0xe6, 0x7e, 0x80];
    ThemeColors {
        foreground: [0xd3, 0xc6, 0xaa],
        background: [0x2d, 0x35, 0x3b],
        cursor: [0xa7, 0xc0, 0x80],
        ansi,
    }
}

#[test]
fn dynamic_colour_queries_report_the_live_theme_with_the_same_terminator() {
    let mut term = run(10, 3, b"");
    term.set_theme_colors(everforest());
    assert_eq!(
        replies(&mut term, b"\x1b]10;?\x07"),
        "\x1b]10;rgb:d3d3/c6c6/aaaa\x07"
    );
    assert_eq!(
        replies(&mut term, b"\x1b]11;?\x1b\\"),
        "\x1b]11;rgb:2d2d/3535/3b3b\x1b\\"
    );
    assert_eq!(
        replies(&mut term, b"\x1b]12;?\x07"),
        "\x1b]12;rgb:a7a7/c0c0/8080\x07"
    );
    // A theme switch is reported at once.
    let mut light = everforest();
    light.background = [0xff, 0xff, 0xff];
    term.set_theme_colors(light);
    assert_eq!(
        replies(&mut term, b"\x1b]11;?\x07"),
        "\x1b]11;rgb:ffff/ffff/ffff\x07"
    );
}

#[test]
fn colour_query_and_da1_sentinel_arrive_in_order() {
    let mut term = run(10, 3, b"");
    term.set_theme_colors(everforest());
    assert_eq!(
        replies(&mut term, b"\x1b]11;?\x07\x1b[c"),
        "\x1b]11;rgb:2d2d/3535/3b3b\x07\x1b[?62;4;22c"
    );
}

#[test]
fn osc_4_queries_set_and_reset_palette_entries() {
    let mut term = run(10, 3, b"");
    term.set_theme_colors(everforest());
    assert_eq!(
        replies(&mut term, b"\x1b]4;1;?;196;?\x1b\\"),
        "\x1b]4;1;rgb:e6e6/7e7e/8080\x1b\\\x1b]4;196;rgb:ffff/0000/0000\x1b\\"
    );
    feed(&mut term, b"\x1b]4;1;rgb:12/34/56;196;#abcdef\x07");
    assert_eq!(term.palette().override_for(1), Some([0x12, 0x34, 0x56]));
    assert_eq!(
        replies(&mut term, b"\x1b]4;1;?;196;?\x07"),
        "\x1b]4;1;rgb:1212/3434/5656\x07\x1b]4;196;rgb:abab/cdcd/efef\x07"
    );
    // Theme changes do not undo application changes.
    term.set_theme_colors(ThemeColors::default());
    assert_eq!(term.palette().override_for(1), Some([0x12, 0x34, 0x56]));

    feed(&mut term, b"\x1b]104;1\x07");
    assert_eq!(term.palette().override_for(1), None);
    assert_eq!(term.palette().override_for(196), Some([0xab, 0xcd, 0xef]));
    feed(&mut term, b"\x1b]104\x07");
    assert!(!term.palette().has_overrides());
}

#[test]
fn ris_resets_the_palette() {
    let mut term = run(10, 3, b"\x1b]4;5;#010203\x07\x1bc");
    assert!(!term.palette().has_overrides());
    assert_eq!(
        replies(&mut term, b"\x1b]4;5;?\x07"),
        "\x1b]4;5;rgb:cdcd/0000/cdcd\x07"
    );
}

// ----------------------------------------------------------------------
// Control sequences never print their own bytes
// ----------------------------------------------------------------------

#[test]
fn colon_sub_parameters_are_not_printed() {
    let (text, cursor) = screen(20, 2, b"A\x1b[4:3mB\x1b[58:2::255:0:0mC\x1b[38:2:1:2:3mD");
    assert_eq!(text, rows(&["ABCD", ""]));
    assert_eq!(cursor, (0, 4));
}

#[test]
fn kitty_keyboard_set_form_is_not_printed() {
    let (text, _) = screen(20, 1, b"A\x1b[=1;1uB");
    assert_eq!(text, rows(&["AB"]));
}

#[test]
fn a_control_inside_a_sequence_runs_and_the_sequence_completes() {
    // LF executes mid-sequence, then CSI 1 K erases to the cursor.
    let (text, cursor) = screen(10, 2, b"abc\x1b[1\nK");
    assert_eq!(text, rows(&["abc", ""]));
    assert_eq!(cursor, (1, 3));
    // CR runs inside CSI 3 C.
    let (text, cursor) = screen(10, 1, b"abcdef\x1b[\r3CX");
    assert_eq!(text, rows(&["abcXef"]));
    assert_eq!(cursor, (0, 4));
}

#[test]
fn can_cancels_a_sequence_without_printing_it() {
    let (text, _) = screen(10, 1, b"a\x1b[3\x18b");
    assert_eq!(text, rows(&["ab"]));
}

#[test]
fn an_escape_ending_a_string_starts_the_next_sequence() {
    // OSC and DCS ended by ESC plus something other than `\`: the ESC begins
    // the next sequence, whose bytes must not be printed.
    let (text, _) = screen(20, 1, b"a\x1b]0;title\x1b[1mb");
    assert_eq!(text, rows(&["ab"]));
    let (text, _) = screen(20, 1, b"a\x1bP$qm\x1b[1mb");
    assert_eq!(text, rows(&["ab"]));
    let term = run(20, 1, b"\x1b]0;title\x1b[1mb");
    assert!(term
        .grid()
        .cell(0, 0)
        .expect("cell")
        .has_flag(CellFlags::BOLD));
}

#[test]
fn malformed_sequences_are_swallowed_whole() {
    let (text, _) = screen(10, 1, b"a\x1b[1?2hb\x1b[1 2qc\x1b[1\xc3\xa92md");
    assert_eq!(text, rows(&["abcd"]));
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

#[test]
fn kitty_keyboard_pops_restore_what_was_pushed() {
    let flags = |bytes: &[u8]| run(10, 1, bytes).modes().kitty_keyboard_flags;
    // Set without a push, then a pop: the shell must get legacy keys back.
    assert_eq!(flags(b"\x1b[=1;1u\x1b[<u"), 0);
    // A pop restores the flags from before the matching push.
    assert_eq!(flags(b"\x1b[=2u\x1b[>1u\x1b[<u"), 2);
    // CSI < n u pops n entries; popping past the bottom turns it off.
    assert_eq!(flags(b"\x1b[>1u\x1b[>2u\x1b[>4u\x1b[<2u"), 1);
    assert_eq!(flags(b"\x1b[>1u\x1b[>2u\x1b[<5u"), 0);
}

#[test]
fn kitty_keyboard_stack_is_bounded() {
    let mut bytes = Vec::new();
    for flags in 1..=20u8 {
        bytes.extend_from_slice(format!("\x1b[>{flags}u").as_bytes());
    }
    // Only the newest eight saved entries remain: popping them all lands on
    // the flags pushed twelfth, not on the original zero.
    bytes.extend_from_slice(b"\x1b[<8u");
    assert_eq!(run(10, 1, &bytes).modes().kitty_keyboard_flags, 12);
}

#[test]
fn main_and_alternate_screens_keep_their_own_kitty_keyboard_modes() {
    let mut term = run(10, 2, b"\x1b[>1u");
    feed(&mut term, b"\x1b[?1049h");
    assert_eq!(term.modes().kitty_keyboard_flags, 0);
    feed(&mut term, b"\x1b[>3u");
    feed(&mut term, b"\x1b[?1049l");
    assert_eq!(term.modes().kitty_keyboard_flags, 1);
    feed(&mut term, b"\x1b[?1049h");
    assert_eq!(term.modes().kitty_keyboard_flags, 3);
}

// ----------------------------------------------------------------------
// UTF-8 decoding
// ----------------------------------------------------------------------

#[test]
fn invalid_utf8_shows_replacement_characters() {
    let (text, cursor) = screen(20, 1, b"a\xF5\x80\x80\x80b\xED\xA0\x80c\xC0\xAFd\xE2\x82");
    assert_eq!(
        text,
        rows(&["a\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}b\u{FFFD}\u{FFFD}\u{FFFD}c\u{FFFD}\u{FFFD}d"])
    );
    // The truncated E2 82 at the end is still pending.
    assert_eq!(cursor, (0, 13));
}

// ---------------------------------------------------------------------------
// Inline images under erases and text, and sixel scrolling
// ---------------------------------------------------------------------------

/// A red sixel `cols` cells wide and `rows` cells high at the default 8x16
/// cell size. `rows` must be a multiple of 3: 48 pixels are eight bands.
fn sixel_cells(cols: u16, rows: u16) -> Vec<u8> {
    assert_eq!(rows % 3, 0);
    let band = format!("#0!{}~", u32::from(cols) * 8);
    let bands = vec![band; usize::from(rows / 3 * 8)];
    format!("\x1bPq#0;2;100;0;0{}\x1b\\", bands.join("-")).into_bytes()
}

/// Each image of the active screen as `(row, col, cells wide, cells high)`,
/// top first; the row is negative above the screen.
fn pieces(term: &Terminal) -> Vec<(isize, u16, u16, u16)> {
    let buffer = term.buffer();
    let mut pieces: Vec<_> = term
        .images()
        .images()
        .map(|image| {
            (
                buffer.screen_row_of_line(image.line),
                image.start_col,
                image.cell_width,
                image.cell_height,
            )
        })
        .collect();
    pieces.sort_unstable();
    pieces
}

/// A 20x10 terminal holding a 4 x 3 cell image at rows 2..=4, columns 4..=7.
/// The cursor is left on (row 4, col 4).
fn term_with_image() -> Terminal {
    let mut term = run(20, 10, b"");
    feed(&mut term, b"\x1b[3;5H");
    feed(&mut term, &sixel_cells(4, 3));
    assert_eq!(pieces(&term), [(2, 4, 4, 3)]);
    term
}

/// Move to 1-based `row`, `col`, then feed `bytes`.
fn at(term: &mut Terminal, row: u16, col: u16, bytes: &[u8]) {
    feed(term, format!("\x1b[{row};{col}H").as_bytes());
    feed(term, bytes);
}

#[test]
fn erase_in_line_removes_the_cells_it_covers_from_an_image() {
    // foot splits a sixel into the rectangles an erase leaves
    // (sixel_overwrite_by_row); ED and EL both go through it.
    let mut term = term_with_image();
    at(&mut term, 4, 1, b"\x1b[2K");
    assert_eq!(pieces(&term), [(2, 4, 4, 1), (4, 4, 4, 1)], "EL 2");

    let mut term = term_with_image();
    at(&mut term, 4, 7, b"\x1b[K");
    assert_eq!(
        pieces(&term),
        [(2, 4, 4, 1), (3, 4, 2, 1), (4, 4, 4, 1)],
        "EL 0 from col 7 erases the right two cells of the middle row"
    );

    let mut term = term_with_image();
    at(&mut term, 4, 6, b"\x1b[1K");
    assert_eq!(
        pieces(&term),
        [(2, 4, 4, 1), (3, 6, 2, 1), (4, 4, 4, 1)],
        "EL 1 through col 6 keeps the right two cells of the middle row"
    );
}

#[test]
fn erase_characters_remove_the_cells_they_cover_from_an_image() {
    let mut term = term_with_image();
    at(&mut term, 4, 6, b"\x1b[2X");
    assert_eq!(
        pieces(&term),
        [(2, 4, 4, 1), (3, 4, 1, 1), (3, 7, 1, 1), (4, 4, 4, 1)]
    );
}

#[test]
fn erase_in_display_removes_the_cells_it_covers_from_an_image() {
    // ED 0 from the middle row's col 6 (0-based 5): its left two cells stay.
    let mut term = term_with_image();
    at(&mut term, 4, 6, b"\x1b[J");
    assert_eq!(pieces(&term), [(2, 4, 4, 1), (3, 4, 1, 1)], "ED 0");

    // ED 1 through the middle row's col 6: the right two cells and the
    // bottom row stay.
    let mut term = term_with_image();
    at(&mut term, 4, 6, b"\x1b[1J");
    assert_eq!(pieces(&term), [(3, 6, 2, 1), (4, 4, 4, 1)], "ED 1");

    // An erase that misses the image leaves it whole.
    let mut term = term_with_image();
    at(&mut term, 4, 1, b"\x1b[1K\x1b[3;3H\x1b[1K");
    assert_eq!(pieces(&term), [(2, 4, 4, 3)]);
}

#[test]
fn a_cropped_piece_shows_the_same_part_of_the_picture() {
    // The picture is stretched over its cells, so the middle row of three
    // keeps a third of the pixels. Give each band its own colour to see
    // which rows survive.
    let colours = ["100;0;0", "0;100;0", "0;0;100"];
    let mut sixel = b"\x1bPq".to_vec();
    for (i, rgb) in colours.iter().enumerate() {
        sixel.extend_from_slice(format!("#{i};2;{rgb}").as_bytes());
    }
    for band in 0..24 {
        let colour = band / 8;
        sixel.extend_from_slice(format!("#{colour}!8~").as_bytes());
        sixel.push(if band < 23 { b'-' } else { b'\x1b' });
    }
    sixel.push(b'\\');
    let mut term = run(20, 10, b"");
    feed(&mut term, &sixel);
    let whole = term.images().images().next().expect("image").clone();
    assert_eq!((whole.cell_width, whole.cell_height), (1, 9));
    // Erase rows 0, 1, 2 and 6, 7, 8: the three middle rows stay.
    for row in [1, 2, 3, 7, 8, 9] {
        at(&mut term, row, 1, b"\x1b[2K");
    }
    let piece = term.images().images().next().expect("a piece stays");
    assert_eq!(term.images().len(), 1);
    assert_eq!((piece.cell_height, piece.height), (3, whole.height / 3));
    assert_ne!(piece.id, whole.id, "a piece is a new image");
    let first_pixel = &piece.data[..4];
    assert_eq!(first_pixel, &whole.data[whole.data.len() / 3..][..4]);
}

#[test]
fn printing_over_an_image_removes_the_cells_under_the_text() {
    // foot: term_print -> sixel_overwrite_at_cursor.
    let mut term = term_with_image();
    at(&mut term, 4, 6, b"X");
    assert_eq!(
        pieces(&term),
        [(2, 4, 4, 1), (3, 4, 1, 1), (3, 6, 2, 1), (4, 4, 4, 1)]
    );

    // A span of ASCII covers each of its cells.
    let mut term = term_with_image();
    at(&mut term, 4, 5, b"abc");
    assert_eq!(pieces(&term), [(2, 4, 4, 1), (3, 7, 1, 1), (4, 4, 4, 1)]);

    // A wide character covers two.
    let mut term = term_with_image();
    at(&mut term, 4, 6, "\u{3042}".as_bytes());
    assert_eq!(
        pieces(&term),
        [(2, 4, 4, 1), (3, 4, 1, 1), (3, 7, 1, 1), (4, 4, 4, 1)]
    );
}

#[test]
fn printing_that_wraps_removes_the_cells_on_the_next_row() {
    let mut term = run(10, 6, b"");
    feed(&mut term, b"\x1b[2;1H");
    feed(&mut term, &sixel_cells(10, 3));
    assert_eq!(pieces(&term), [(1, 0, 10, 3)]);
    // Fill the image's middle row (3) to its end and print one more
    // character: it wraps onto the image's bottom row, column 0.
    feed(&mut term, b"\x1b[3;1H0123456789!");
    assert_eq!(pieces(&term), [(1, 0, 10, 1), (3, 1, 9, 1)]);
}

#[test]
fn text_that_misses_an_image_leaves_it_whole() {
    let mut term = term_with_image();
    at(&mut term, 4, 1, b"abc");
    at(&mut term, 4, 9, "xyz\u{3042}".as_bytes());
    at(&mut term, 1, 1, b"\x1b[Kabc");
    assert_eq!(pieces(&term), [(2, 4, 4, 3)]);
}

#[test]
fn other_screen_clears_delete_the_images_on_screen() {
    let mut term = term_with_image();
    feed(&mut term, b"\x1b#8");
    assert!(term.images().is_empty(), "DECALN fills the screen");

    let mut term = term_with_image();
    feed(&mut term, b"\x1b[?40h\x1b[?3h");
    assert!(term.images().is_empty(), "DECCOLM clears the screen");
}

#[test]
fn a_tall_sixel_scrolls_the_screen_to_fit() {
    // xterm and foot scroll as for line feeds until the image fits; the
    // cursor ends on its last row. 6 rows on a 5-row screen: the top row of
    // the image goes into scrollback.
    let mut term = run(20, 5, b"\x1b[4;3H");
    feed(&mut term, &sixel_cells(2, 6));
    assert_eq!(pieces(&term), [(-1, 2, 2, 6)]);
    assert_eq!(term.grid().cursor().position.row, 4);
    assert_eq!(term.grid().cursor().position.col, 2);
    assert_eq!(term.buffer().history_len(), 4, "four rows scrolled off");

    // An image that fits below the cursor does not scroll.
    let mut term = run(20, 8, b"\x1b[2;1H");
    feed(&mut term, &sixel_cells(2, 6));
    assert_eq!(pieces(&term), [(1, 0, 2, 6)]);
    assert_eq!(term.buffer().history_len(), 0);
}

#[test]
fn a_tall_sixel_scrolls_the_alternate_screen_too() {
    // No scrollback there: the rows that leave are gone, and the image
    // moves up with the screen.
    let mut term = run(20, 5, b"\x1b[?1049h\x1b[4;1H");
    feed(&mut term, &sixel_cells(2, 3));
    assert_eq!(pieces(&term), [(2, 0, 2, 3)]);
    assert_eq!(term.grid().cursor().position.row, 4);

    // Taller than the screen: only the rows that exist are kept.
    let mut term = run(20, 5, b"\x1b[?1049h");
    feed(&mut term, &sixel_cells(2, 6));
    assert_eq!(pieces(&term), [(0, 0, 2, 5)]);
}

#[test]
fn sixel_scrolling_off_draws_at_the_top_left_without_moving_anything() {
    // DECSDM set: foot's `!term->sixel.scrolling`.
    let mut term = run(20, 5, b"\x1b[?80h");
    assert_eq!(private_mode_status(&mut term, 80), "1");
    at(&mut term, 4, 6, &sixel_cells(2, 3));
    assert_eq!(pieces(&term), [(0, 0, 2, 3)]);
    assert_eq!(term.grid().cursor().position.row, 3);
    assert_eq!(term.grid().cursor().position.col, 5);

    // Taller than the screen: cut to the screen's rows.
    feed(&mut term, &sixel_cells(2, 9));
    assert_eq!(
        pieces(&term).iter().map(|p| p.3).max(),
        Some(5),
        "no row past the screen"
    );
    assert_eq!(term.buffer().history_len(), 0, "nothing scrolled");

    // Reset puts scrolling back.
    feed(&mut term, b"\x1b[?80l");
    assert_eq!(private_mode_status(&mut term, 80), "2");
}

#[test]
fn reflow_puts_an_image_on_the_row_of_its_cell_after_wide_characters() {
    // Three wide characters (a cell and a spacer each), then ZZ with the
    // image on the second. At 3 columns the second wide character does not
    // fit after the first one's cells, so each takes a row of its own; the
    // third leaves room for the first Z and the second Z is on row 3.
    // Counting cells from the start of the line (seven before the image,
    // 7 / 3) says row 2.
    let mut term = run(10, 10, "\u{3042}\u{3042}\u{3042}ZZ".as_bytes());
    feed(&mut term, b"\x1b[1;8H");
    feed(&mut term, SIXEL);
    assert_eq!(pieces(&term)[0].0, 0);
    term.resize(Dimensions::new(3, 10));
    let (screen, _) = snapshot(&term);
    assert_eq!(&screen[..4], ["\u{3042}", "\u{3042}", "\u{3042} Z", "Z"]);
    assert_eq!(pieces(&term)[0].0, 3);
}
