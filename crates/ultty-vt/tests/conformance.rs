//! Behavioural conformance cases: byte streams in, expected screens and
//! replies out, checked against xterm's behaviour.
//!
//! Each case feeds a fresh terminal and compares what a user (or the
//! application reading replies) would see, not internal state.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{CellFlags, Dimensions, Terminal, TerminalHandler, ThemeColors};

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
        1u16, 5, 6, 7, 9, 12, 25, 40, 1000, 1002, 1003, 1004, 1005, 1006, 1007, 1015, 2004, 2026,
        2027,
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
    // Malformed hex is invalid, echoed back.
    assert_eq!(replies(&mut term, b"\x1bP+qZZ\x1b\\"), "\x1bP0+rZZ\x1b\\");
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
    let sixel = format!("\x1bPq#0;2;100;0;0#0{}\x1b\\", vec![band; 6].join("-"));
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
