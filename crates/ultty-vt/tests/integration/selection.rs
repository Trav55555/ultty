//! A selection must survive output that does not touch the selected rows.
//!
//! Apps like Claude Code keep repainting a spinner or status line while the
//! user selects text elsewhere on screen. Dropping the selection on every
//! repaint made mouse selection impossible in them.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, SelectionMode, SelectionPoint, Terminal, TerminalHandler};

fn feed(terminal: &mut Terminal, parser: &mut AnsiParser, bytes: &[u8]) {
    let mut handler = TerminalHandler::new(terminal);
    parser.feed(bytes, |action| handler.handle(action));
}

fn screen_with_transcript() -> (Terminal, AnsiParser) {
    let mut terminal = Terminal::new(Dimensions::new(20, 5));
    let mut parser = AnsiParser::new();
    feed(
        &mut terminal,
        &mut parser,
        b"first answer\r\nsecond answer\r\n\r\n> prompt\r\n",
    );
    (terminal, parser)
}

/// Erase and redraw row 4 (1-based row 5), the way a status line repaints.
const REPAINT_STATUS_LINE: &[u8] = b"\x1b7\x1b[5;1H\x1b[2K* Thinking 3s\x1b8";

#[test]
fn repainting_another_row_keeps_the_selection() {
    let (mut terminal, mut parser) = screen_with_transcript();
    terminal.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
    terminal.update_selection(SelectionPoint::new(5, 1));

    for _ in 0..10 {
        feed(&mut terminal, &mut parser, REPAINT_STATUS_LINE);
    }

    assert_eq!(
        terminal.selected_text().as_deref(),
        Some("first answer\nsecond")
    );
    // Dragging further still works after the repaints.
    terminal.update_selection(SelectionPoint::new(12, 1));
    assert_eq!(
        terminal.selected_text().as_deref(),
        Some("first answer\nsecond answer")
    );
}

#[test]
fn rewriting_a_selected_row_drops_the_selection() {
    let (mut terminal, mut parser) = screen_with_transcript();
    terminal.start_selection(SelectionPoint::new(0, 1), SelectionMode::Normal);
    terminal.update_selection(SelectionPoint::new(5, 1));

    feed(&mut terminal, &mut parser, b"\x1b[2;1H\x1b[2Kreplaced");

    assert!(terminal.selection().is_none());
}

#[test]
fn scrolling_output_drops_the_selection() {
    let (mut terminal, mut parser) = screen_with_transcript();
    terminal.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
    terminal.update_selection(SelectionPoint::new(4, 0));

    // A newline on the last row scrolls every row up, so row 0 now holds
    // different text.
    feed(&mut terminal, &mut parser, b"\x1b[5;1Hmore\r\nmore\r\n");

    assert!(terminal.selection().is_none());
}

#[test]
fn any_output_drops_the_selection_while_scrolled_back() {
    let (mut terminal, mut parser) = screen_with_transcript();
    feed(
        &mut terminal,
        &mut parser,
        b"\x1b[5;1Hmore\r\nmore\r\nmore\r\n",
    );
    terminal.buffer_mut().scroll_up(1);
    assert!(terminal.buffer().is_scrolled());
    terminal.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
    terminal.update_selection(SelectionPoint::new(4, 0));

    feed(&mut terminal, &mut parser, REPAINT_STATUS_LINE);

    assert!(terminal.selection().is_none());
}
