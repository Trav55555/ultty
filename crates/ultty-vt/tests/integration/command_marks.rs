//! The last command and its output, found through OSC 133 marks kept on
//! rows (ADR 0019). Each case replays what a shell with OSC 133
//! integration sends: A, the prompt, B, the typed command, Enter, C, the
//! output, then D with the exit status.

use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, LastCommand, PromptJump, Terminal, TerminalHandler};

fn feed(term: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(term);
    parser.feed(bytes, |action| handler.handle(action));
}

/// The prompt and typed `command`, Enter, and the start of its output.
fn start(term: &mut Terminal, command: &str) {
    let bytes = format!("\x1b]133;A\x07$ \x1b]133;B\x07{command}\r\n\x1b]133;C\x07");
    feed(term, bytes.as_bytes());
}

/// The command finishing with `status`.
fn finish(term: &mut Terminal, status: i32) {
    feed(term, format!("\x1b]133;D;{status}\x07").as_bytes());
}

/// `command` printing `output` (lines end in CRLF) and exiting `status`.
fn run(term: &mut Terminal, command: &str, output: &str, status: i32) {
    start(term, command);
    feed(term, output.as_bytes());
    finish(term, status);
}

/// The shell's next prompt, waiting for input.
fn prompt(term: &mut Terminal) {
    feed(term, b"\x1b]133;A\x07$ \x1b]133;B\x07");
}

fn last(term: &Terminal) -> LastCommand {
    term.last_command(None).expect("a command was recorded")
}

fn lines(range: std::ops::Range<usize>) -> (String, String) {
    let sent: String = range.clone().map(|n| format!("line {n}\r\n")).collect();
    let read: Vec<String> = range.map(|n| format!("line {n}")).collect();
    (sent, read.join("\n"))
}

#[test]
fn reports_the_command_its_output_and_exit_status() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    run(&mut term, "ls", "a.txt\r\nb.txt\r\n", 0);
    run(&mut term, "make test", "1 failed\r\n", 2);
    prompt(&mut term);

    let last = last(&term);
    assert_eq!(last.command, "make test");
    assert_eq!(last.output, "1 failed");
    assert_eq!(last.exit_code, Some(2));
    assert!(!last.running && last.output_available);
}

#[test]
fn nothing_is_recorded_before_the_first_command() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    // Shells send D before their first prompt.
    feed(&mut term, b"\x1b]133;D;0\x07");
    prompt(&mut term);
    assert_eq!(term.last_command(None), None);
}

#[test]
fn output_that_scrolled_into_scrollback_is_whole() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    let (sent, read) = lines(0..50);
    run(&mut term, "seq", &sent, 0);
    prompt(&mut term);
    assert_eq!(last(&term).output, read);
}

#[test]
fn output_pushed_out_of_scrollback_is_reported_unavailable() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    term.set_scrollback_limit(20);
    let (sent, _) = lines(0..10);
    run(&mut term, "first", &sent, 0);
    // The second command's start leaves the scrollback while it runs.
    let (long, _) = lines(0..100);
    run(&mut term, "second", &long, 0);
    prompt(&mut term);

    let last = last(&term);
    assert_eq!(last.command, "second");
    assert!(!last.output_available);
    assert_eq!(last.output, "");
}

#[test]
fn resizing_rewraps_the_output_without_losing_it() {
    let mut term = Terminal::new(Dimensions::new(40, 6));
    let long_line = "x".repeat(70);
    let (sent, read) = lines(0..20);
    run(&mut term, "build", &format!("{long_line}\r\n{sent}"), 1);
    prompt(&mut term);

    term.resize(Dimensions::new(17, 6));
    let narrow = last(&term);
    assert_eq!(narrow.output, format!("{long_line}\n{read}"));
    assert_eq!(narrow.exit_code, Some(1));

    term.resize(Dimensions::new(120, 6));
    assert_eq!(last(&term).output, narrow.output);
}

#[test]
fn a_running_command_reports_what_it_printed_so_far() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    start(&mut term, "tail -f log");
    feed(&mut term, b"first\r\nsecond\r\n");

    let last = last(&term);
    assert!(last.running);
    assert_eq!(last.output, "first\nsecond");
    assert_eq!(last.exit_code, None);
}

#[test]
fn a_command_with_no_output_has_empty_output() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    run(&mut term, "echo noise", "noise\r\n", 0);
    run(&mut term, "true", "", 0);
    prompt(&mut term);

    let last = last(&term);
    assert_eq!(last.command, "true");
    assert_eq!(last.output, "");
    assert!(last.output_available);
}

#[test]
fn a_cleared_screen_never_yields_another_commands_output() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    run(&mut term, "echo old", "old\r\n", 0);
    run(&mut term, "echo new", "new\r\n", 0);
    // clear: erase the screen and the scrollback, home the cursor.
    feed(&mut term, b"\x1b[H\x1b[2J\x1b[3J");
    prompt(&mut term);

    let last = last(&term);
    assert_eq!(last.command, "echo new");
    assert!(!last.output_available);
    assert_eq!(last.output, "");
}

#[test]
fn a_long_command_line_is_read_across_its_wrapped_rows() {
    let mut term = Terminal::new(Dimensions::new(20, 10));
    let command = "echo one two three four five six";
    run(&mut term, command, "one two three four five six\r\n", 0);
    prompt(&mut term);
    assert_eq!(last(&term).command, command);
}

#[test]
fn output_without_a_final_newline_is_kept_and_the_prompt_is_not() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    run(&mut term, "printf done", "done", 0);
    prompt(&mut term);
    assert_eq!(last(&term).output, "done");

    run(&mut term, "cat notes", "line one\r\nno newline at end", 0);
    prompt(&mut term);
    assert_eq!(last(&term).output, "line one\nno newline at end");

    // A later command with a newline does not inherit the old tail.
    run(&mut term, "echo ok", "ok\r\n", 0);
    prompt(&mut term);
    assert_eq!(last(&term).output, "ok");
}

#[test]
fn tail_lines_keeps_the_end_of_long_output() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    let (sent, _) = lines(0..30);
    run(&mut term, "seq", &sent, 0);
    prompt(&mut term);
    let tail = term.last_command(Some(3)).unwrap().output;
    assert_eq!(tail, "line 27\nline 28\nline 29");
}

// Jumping between prompts and selecting a command's output. The prompt a
// jump lands on is the row where its input starts (OSC 133;B).

/// Text of the row at the top of the view.
fn top_text(term: &Terminal) -> String {
    term.buffer().visible_row(0).unwrap().text_trimmed()
}

/// Three commands of ten output lines each, on a five-row screen, then a
/// fresh prompt: all three earlier prompts are in scrollback.
fn three_commands() -> Terminal {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    let (sent, _) = lines(0..10);
    for name in ["one", "two", "three"] {
        run(&mut term, name, &sent, 0);
    }
    prompt(&mut term);
    term
}

#[test]
fn without_any_marks_a_jump_goes_nowhere() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    let (sent, _) = lines(0..30);
    feed(&mut term, sent.as_bytes());
    term.buffer_mut().scroll_up(7);

    assert!(!term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(term.buffer().scroll_offset(), 7);
    // Forward with nothing to land on still returns to the live screen.
    assert!(term.jump_to_prompt(PromptJump::Next));
    assert_eq!(term.buffer().scroll_offset(), 0);
    assert!(!term.jump_to_prompt(PromptJump::Next));
}

#[test]
fn previous_walks_up_through_the_prompts_and_stops_at_the_first() {
    let mut term = three_commands();

    assert!(term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(top_text(&term), "$ three");
    assert!(term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(top_text(&term), "$ two");
    assert!(term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(top_text(&term), "$ one");

    // At the oldest prompt there is nowhere further to go.
    let offset = term.buffer().scroll_offset();
    assert!(!term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(term.buffer().scroll_offset(), offset);
}

#[test]
fn next_walks_back_down_and_ends_at_the_live_screen() {
    let mut term = three_commands();
    for _ in 0..3 {
        term.jump_to_prompt(PromptJump::Previous);
    }

    assert!(term.jump_to_prompt(PromptJump::Next));
    assert_eq!(top_text(&term), "$ two");
    assert!(term.jump_to_prompt(PromptJump::Next));
    assert_eq!(top_text(&term), "$ three");
    // The live prompt cannot be put at the top: the view goes to the bottom.
    assert!(term.jump_to_prompt(PromptJump::Next));
    assert_eq!(term.buffer().scroll_offset(), 0);
    assert!(!term.jump_to_prompt(PromptJump::Next));
}

#[test]
fn a_prompt_already_at_the_top_is_skipped_going_back() {
    let mut term = three_commands();
    term.jump_to_prompt(PromptJump::Previous);
    term.jump_to_prompt(PromptJump::Previous);
    assert_eq!(top_text(&term), "$ two");

    // Scrolling one line down leaves "$ two" just above the view.
    term.buffer_mut().scroll_down(1);
    assert!(term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(top_text(&term), "$ two");
}

#[test]
fn prompts_evicted_from_scrollback_are_not_jumped_to() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    term.set_scrollback_limit(12);
    let (sent, _) = lines(0..10);
    for name in ["one", "two", "three"] {
        run(&mut term, name, &sent, 0);
    }
    prompt(&mut term);

    // Only the newest prompts still have rows.
    let mut seen = Vec::new();
    while term.jump_to_prompt(PromptJump::Previous) {
        seen.push(top_text(&term));
    }
    assert!(!seen.is_empty());
    assert!(!seen.contains(&"$ one".to_string()), "{seen:?}");
    assert_eq!(seen.last().unwrap(), "$ three");
}

#[test]
fn when_every_marked_row_is_evicted_a_jump_goes_nowhere() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    term.set_scrollback_limit(10);
    run(&mut term, "one", "a\r\n", 0);
    let (flood, _) = lines(0..200);
    feed(&mut term, flood.as_bytes());

    assert!(!term.jump_to_prompt(PromptJump::Previous));
    assert_eq!(term.buffer().scroll_offset(), 0);
}

#[test]
fn the_alternate_screen_has_no_prompts_to_jump_to() {
    let mut term = three_commands();
    feed(&mut term, b"\x1b[?1049h");
    assert!(!term.jump_to_prompt(PromptJump::Previous));
    assert!(!term.select_last_output());
}

#[test]
fn selecting_the_last_output_picks_exactly_its_rows() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    run(&mut term, "ls", "a.txt\r\nb.txt\r\n", 0);
    run(&mut term, "make", "one\r\ntwo\r\nthree\r\n", 2);
    prompt(&mut term);

    assert!(term.select_last_output());
    assert_eq!(term.selected_text().as_deref(), Some("one\ntwo\nthree"));
}

#[test]
fn selecting_scrolls_to_output_that_has_left_the_view() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    run(&mut term, "seq", "1\r\n2\r\n3\r\n", 0);
    let (flood, _) = lines(0..20);
    feed(&mut term, flood.as_bytes());
    // Scroll away from the live screen.
    term.buffer_mut().scroll_to_top();

    assert!(term.select_last_output());
    assert_eq!(term.selected_text().as_deref(), Some("1\n2\n3"));
}

#[test]
fn output_taller_than_the_view_is_not_selected() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    let (sent, _) = lines(0..10);
    run(&mut term, "seq", &sent, 0);
    prompt(&mut term);

    assert!(!term.select_last_output());
    assert!(term.selected_text().is_none());
}

#[test]
fn nothing_to_select_without_a_command_or_after_eviction() {
    let mut term = Terminal::new(Dimensions::new(40, 5));
    prompt(&mut term);
    assert!(!term.select_last_output());

    term.set_scrollback_limit(10);
    run(&mut term, "one", "a\r\nb\r\n", 0);
    let (flood, _) = lines(0..100);
    feed(&mut term, flood.as_bytes());
    assert!(!term.select_last_output());
}

#[test]
fn a_running_command_is_selected_up_to_the_cursor() {
    let mut term = Terminal::new(Dimensions::new(40, 10));
    start(&mut term, "tail -f log");
    feed(&mut term, b"x\r\ny\r\n");

    assert!(term.select_last_output());
    assert_eq!(term.selected_text().as_deref(), Some("x\ny"));
}
