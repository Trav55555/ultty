//! ANSI mode, end to end: each fixture is rendered into memory as on an
//! 80 x 24 terminal, the bytes are fed through Ultty's own VT parser and
//! grid (`ultty-vt`), and the screen and scrollback are compared with the
//! text the fixture holds.

use agent_console::ansi::AnsiScreen;
use agent_console::event::Event;
use ultty_vt::ansi::{Action, AnsiParser, Mode};
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

use crate::common::{
    assert_line_starts_in_order, assert_prose_counts, assert_segments_in_order, first_turns, load,
    prompts, prose_segments, run, tool_titles,
};

const COLS: u16 = 80;
const ROWS: u16 = 24;

/// What a terminal made of the console's output.
struct Rendered {
    bytes: Vec<u8>,
    /// Screen and scrollback as logical lines (soft wraps joined), the way
    /// a copy reads them.
    lines: Vec<String>,
    redraws: usize,
}

fn render(events: &[Event]) -> Rendered {
    let screen = run(AnsiScreen::fixed(Vec::new(), COLS, ROWS), events);
    let bytes = screen.into_inner();
    let mut terminal = Terminal::new(Dimensions::new(COLS, ROWS));
    terminal.set_scrollback_limit(1_000_000);
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(&mut terminal);
    let mut redraws = 0;
    parser.feed(&bytes, |action| {
        let synchronized = handler.terminal().modes().synchronized_update;
        match &action {
            Action::CursorUp(_)
            | Action::CursorPreviousLine(_)
            | Action::CursorPosition { .. }
            | Action::CursorVerticalAbsolute(_)
            | Action::EraseInDisplay(_)
            | Action::EraseInLine(_) => {
                assert!(synchronized, "{action:?} outside a synchronized update");
                redraws += 1;
            }
            Action::SetMode(
                Mode::AlternateScreen
                | Mode::AlternateScreenClear
                | Mode::AlternateScreenSaveCursor,
            ) => panic!("the console switched to the alternate screen"),
            _ => {}
        }
        handler.handle(action);
    });
    assert!(
        !terminal.modes().synchronized_update,
        "a synchronized update was left open"
    );
    assert!(!terminal.is_alternate_screen());
    let buffer = terminal.buffer();
    let text = buffer.text_of_rows(0, buffer.total_lines(), 0);
    Rendered {
        bytes,
        lines: text.split('\n').map(String::from).collect(),
        redraws,
    }
}

/// The output without escape sequences, as the lines it prints.
fn printed_lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let mut plain = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.next() == Some('[') {
                while chars.next().is_some_and(|c| !c.is_ascii_alphabetic()) {}
            }
        } else {
            plain.push(ch);
        }
    }
    plain.split(['\r', '\n']).map(String::from).collect()
}

fn check(events: &[Event]) -> Rendered {
    let rendered = render(events);
    let segments = prose_segments(events);
    assert!(!segments.is_empty());
    assert_segments_in_order(&rendered.lines, &segments);
    assert_prose_counts(&rendered.lines, &segments);
    assert_line_starts_in_order(&rendered.lines, &prompts(events));
    assert_line_starts_in_order(&rendered.lines, &tool_titles(events));
    for line in printed_lines(&rendered.bytes) {
        assert_eq!(line.trim_end(), line, "printed with trailing spaces");
    }
    for line in &rendered.lines {
        assert_eq!(line.trim_end(), line, "a row holds trailing spaces");
    }
    assert!(rendered.redraws > 0, "the live region was never redrawn");
    rendered
}

#[test]
fn test_short_session_reads_as_its_source_in_ansi_mode() {
    let events = load("short.jsonl");
    let rendered = check(&events);
    let lines = &rendered.lines;
    let code = lines.iter().position(|l| l == "```rust").unwrap();
    assert_eq!(lines[code + 1], "fn expand_tabs(line: &str) -> String {");
    assert_eq!(lines[code + 5], "        if ch == '\\t' {");
    assert_eq!(
        lines[code + 6],
        "            let pad = stop - out.len() % stop;"
    );
    assert_line_starts_in_order(
        lines,
        &[
            "[bash] cargo test -p pager parse:: (failed, exit 101, 10 lines, 3.9s)".into(),
            "test result: FAILED. 1 passed; 3 failed".into(),
            "[diff] src/parse.rs".into(),
            "+    let mut column = 0;".into(),
            "[approve] cargo test -p pager (Allow once)".into(),
            "> 1. Allow once".into(),
            "[bash] cargo test -p pager (ok, 2 lines, 2.1s)".into(),
            "example-model-1 | in 921 | out 424 | idle | 9.6s".into(),
        ],
    );
    assert!(
        !lines.iter().any(|l| l.contains("(running")),
        "a live view was left in the transcript"
    );
    assert_eq!(
        lines.iter().filter(|l| l.contains("| idle |")).count(),
        1,
        "the status line was left behind by a redraw"
    );
}

#[test]
fn test_long_tool_output_keeps_the_last_lines_and_the_prose_around_it() {
    let events = load("long-tool-output.jsonl");
    let rendered = check(&events);
    assert_line_starts_in_order(
        &rendered.lines,
        &["[bash] cargo test --workspace -- --report-time (ok, 20000 lines, 10.4s)".into()],
    );
}

#[test]
fn test_two_hours_first_turns_read_as_their_source() {
    let events = first_turns(load("two-hours.jsonl"), 12);
    check(&events);
}

#[test]
fn test_a_pipe_gets_the_same_transcript_without_escapes() {
    let events = load("short.jsonl");
    let screen = run(AnsiScreen::pipe(Vec::new()), &events);
    let bytes = screen.into_inner();
    assert!(!bytes.contains(&0x1b), "escape sequences in a pipe");
    let text = String::from_utf8(bytes).unwrap();
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let segments = prose_segments(&events);
    assert_segments_in_order(&lines, &segments);
    assert_prose_counts(&lines, &segments);
    assert_eq!(
        lines,
        render(&events).lines,
        "pipe and terminal transcripts differ"
    );
}
