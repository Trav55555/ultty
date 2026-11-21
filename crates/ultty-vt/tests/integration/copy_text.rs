//! Copied text must read the way the user wrote it: wide characters once,
//! soft-wrapped rows joined into their logical line, no padding at line
//! ends. The clipboard (`Terminal::selected_text`) and the control plane's
//! text request (`Buffer::copy_text` over every row) both depend on it.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, SelectionMode, SelectionPoint, Terminal, TerminalHandler};

const ALPHABET_25: &str = "abcdefghijklmnopqrstuvwxy";

fn terminal_with(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(&mut terminal);
    parser.feed(bytes, |action| handler.handle(action));
    terminal
}

/// Every row of the active buffer, scrollback first, as the control plane
/// reads it.
fn all_text(terminal: &Terminal) -> String {
    let buffer = terminal.buffer();
    buffer.copy_text(0..=buffer.total_lines() - 1, 0, u16::MAX)
}

fn select(
    terminal: &mut Terminal,
    mode: SelectionMode,
    (col, row): (i32, i32),
    (end_col, end_row): (i32, i32),
) -> String {
    terminal.start_selection(SelectionPoint::new(col, row), mode);
    terminal.update_selection(SelectionPoint::new(end_col, end_row));
    terminal.selected_text().expect("selection is live")
}

#[test]
fn wide_characters_copy_once_without_spacers() {
    let mut terminal = terminal_with(10, 3, "日本語abc".as_bytes());
    assert_eq!(all_text(&terminal).lines().next(), Some("日本語abc"));
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (9, 0)),
        "日本語abc"
    );
}

#[test]
fn an_emoji_zwj_sequence_copies_intact() {
    let family = "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b";
    let mut terminal = terminal_with(10, 3, family.as_bytes());
    assert_eq!(all_text(&terminal).lines().next(), Some(family));
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (9, 0)),
        family
    );
}

#[test]
fn the_wrap_flag_marks_the_row_that_continues_onto_the_next() {
    let terminal = terminal_with(10, 4, ALPHABET_25.as_bytes());
    let grid = terminal.grid();
    let wrapped: Vec<bool> = (0..3).map(|r| grid.row(r).unwrap().is_wrapped()).collect();
    assert_eq!(wrapped, [true, true, false]);
}

#[test]
fn a_soft_wrapped_line_copies_as_one_line() {
    let mut terminal = terminal_with(10, 4, ALPHABET_25.as_bytes());
    assert_eq!(all_text(&terminal).lines().next(), Some(ALPHABET_25));
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (9, 2)),
        ALPHABET_25
    );
}

#[test]
fn a_space_at_the_wrap_point_is_kept() {
    let line = "abcdefghi jklmnopqr stuv";
    assert_eq!(line.as_bytes()[9], b' ');
    assert_eq!(line.as_bytes()[19], b' ');
    let mut terminal = terminal_with(10, 4, line.as_bytes());
    assert_eq!(all_text(&terminal).lines().next(), Some(line));
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (9, 2)),
        line
    );
}

#[test]
fn hard_line_ends_lose_their_padding() {
    let mut terminal = terminal_with(10, 3, b"ab\r\ncd");
    assert_eq!(all_text(&terminal), "ab\ncd\n");
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (9, 1)),
        "ab\ncd"
    );
}

#[test]
fn erased_cells_at_a_line_end_are_trimmed() {
    let mut terminal = terminal_with(10, 3, b"abcdefgh\x1b[4G\x1b[K\r\nxy");
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (9, 1)),
        "abc\nxy"
    );
}

#[test]
fn a_normal_selection_keeps_its_column_bounds_across_a_wrap() {
    let bytes = format!("{ALPHABET_25}\r\nnext");
    let mut terminal = terminal_with(10, 5, bytes.as_bytes());
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (3, 0), (4, 1)),
        "defghijklmno"
    );
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (7, 1), (2, 3)),
        "rstuvwxy\nnex"
    );
}

#[test]
fn a_line_selection_joins_wrapped_rows() {
    let bytes = format!("{ALPHABET_25}\r\nnext");
    let mut terminal = terminal_with(10, 5, bytes.as_bytes());
    assert_eq!(
        select(&mut terminal, SelectionMode::Line, (4, 0), (1, 3)),
        format!("{ALPHABET_25}\nnext")
    );
}

#[test]
fn a_block_selection_stays_rectangular_and_skips_spacers() {
    let bytes = format!("日本語abc\r\nxy\r\n{ALPHABET_25}");
    let mut terminal = terminal_with(10, 6, bytes.as_bytes());
    assert_eq!(
        select(&mut terminal, SelectionMode::Block, (0, 0), (3, 1)),
        "日本\nxy"
    );
    assert_eq!(
        select(&mut terminal, SelectionMode::Block, (0, 2), (1, 4)),
        "ab\nkl\nuv"
    );
}

#[test]
fn wrapped_rows_in_scrollback_still_join() {
    let bytes = format!("{ALPHABET_25}\r\none\r\ntwo\r\nthree");
    let terminal = terminal_with(10, 3, bytes.as_bytes());
    assert_eq!(terminal.buffer().history_len(), 3);
    assert_eq!(
        all_text(&terminal),
        format!("{ALPHABET_25}\none\ntwo\nthree")
    );
}

#[test]
fn reflowing_a_paragraph_does_not_change_its_copied_text() {
    let paragraph = "the quick brown fox jumps over the lazy dog";
    let bytes = format!("{paragraph}\r\n日本語の文字列もある\r\nend");
    let mut terminal = terminal_with(10, 12, bytes.as_bytes());
    let before = all_text(&terminal);
    assert!(before.starts_with(&format!("{paragraph}\n日本語の文字列もある\nend")));

    terminal.resize(Dimensions::new(17, 12));
    assert_eq!(all_text(&terminal).trim_end(), before.trim_end());
}

#[test]
fn the_blank_left_by_a_wide_character_that_wrapped_is_not_copied() {
    let mut terminal = terminal_with(9, 3, "日本語の文字".as_bytes());
    assert!(terminal.grid().row(0).unwrap().is_wrapped());
    assert_eq!(terminal.grid().row(0).unwrap().cell(8).unwrap().char(), ' ');
    assert_eq!(all_text(&terminal).lines().next(), Some("日本語の文字"));
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (8, 1)),
        "日本語の文字"
    );
}

#[test]
fn an_empty_last_row_adds_no_final_newline() {
    let mut terminal = terminal_with(10, 4, b"ab\r\ncd\r\n");
    assert_eq!(
        select(&mut terminal, SelectionMode::Line, (0, 0), (0, 2)),
        "ab\ncd"
    );
    assert_eq!(
        select(&mut terminal, SelectionMode::Normal, (0, 0), (-1, 2)),
        "ab\ncd"
    );
}
