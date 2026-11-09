//! The checks behind the compatibility scorecard (`docs/compat-scorecard.md`).
//!
//! Each `scorecard_<slug>` test proves one row: what a program that uses the
//! feature sends, and what it gets back or sees. `scripts/compat-scorecard.sh`
//! runs every `scorecard_` test in the workspace and writes the page from the
//! results, so a row reads "yes" only while its test passes.

use ultty_vt::ansi::{Action, AnsiParser};
use ultty_vt::grid::{Color, Dimensions, Terminal, TerminalHandler, ThemeColors, UnderlineStyle};

fn run(bytes: &[u8]) -> Terminal {
    let mut term = Terminal::new(Dimensions::new(20, 5));
    feed(&mut term, bytes);
    term
}

fn feed(term: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(term);
    parser.feed(bytes, |action| handler.handle(action));
}

/// Everything the terminal answered to `bytes`, as text.
fn replies(term: &mut Terminal, bytes: &[u8]) -> String {
    feed(term, bytes);
    String::from_utf8(term.take_responses().concat()).expect("replies are UTF-8")
}

/// What the parser makes of `bytes`, before any terminal state.
fn actions(bytes: &[u8]) -> Vec<Action> {
    let mut parser = AnsiParser::new();
    let mut out = Vec::new();
    parser.feed(bytes, |action| out.push(action));
    out
}

/// DECRQM for a private mode, after `setup`: 1 set, 2 reset, 0 unknown.
fn private_mode(setup: &[u8], mode: u16) -> String {
    let mut term = run(setup);
    replies(&mut term, format!("\x1b[?{mode}$p").as_bytes())
}

#[test]
fn scorecard_synchronized_output() {
    assert_eq!(private_mode(b"", 2026), "\x1b[?2026;2$y");
    assert_eq!(private_mode(b"\x1b[?2026h", 2026), "\x1b[?2026;1$y");
}

#[test]
fn scorecard_kitty_keyboard_query() {
    let mut term = run(b"");
    // An app checks support by asking for the flags, then pushes its own.
    assert_eq!(replies(&mut term, b"\x1b[?u"), "\x1b[?0u");
    assert_eq!(replies(&mut term, b"\x1b[>11u\x1b[?u"), "\x1b[?11u");
    assert_eq!(replies(&mut term, b"\x1b[<u\x1b[?u"), "\x1b[?0u");
}

#[test]
fn scorecard_bracketed_paste() {
    assert_eq!(private_mode(b"\x1b[?2004h", 2004), "\x1b[?2004;1$y");
}

#[test]
fn scorecard_focus_reporting() {
    assert_eq!(private_mode(b"\x1b[?1004h", 1004), "\x1b[?1004;1$y");
}

#[test]
fn scorecard_alternate_screen() {
    let mut term = run(b"shell\x1b[?1049hfull screen");
    assert!(term.is_alternate_screen());
    feed(&mut term, b"\x1b[?1049l");
    assert!(!term.is_alternate_screen());
    let row = term.grid().row(0).unwrap().text_trimmed();
    assert_eq!(row, "shell");
}

#[test]
fn scorecard_sgr_mouse() {
    assert_eq!(private_mode(b"\x1b[?1006h", 1006), "\x1b[?1006;1$y");
}

#[test]
fn scorecard_grapheme_clusters() {
    assert_eq!(private_mode(b"\x1b[?2027h", 2027), "\x1b[?2027;1$y");
    // A family emoji joined with ZWJ is one cluster, two cells wide.
    let mut term = run(b"\x1b[?2027h");
    feed(&mut term, "👨\u{200d}👩\u{200d}👧x".as_bytes());
    assert_eq!(term.grid().cursor().position.col, 3);
}

#[test]
fn scorecard_true_colour_and_underline_styles() {
    let term = run(b"\x1b[38;2;10;20;30;4:3;58;2;1;2;3mx");
    let cell = term.grid().cell(0, 0).unwrap();
    assert_eq!(cell.fg(), Color::Rgb(10, 20, 30));
    assert_eq!(cell.underline_style(), UnderlineStyle::Curly);
    assert_eq!(cell.underline_color(), Color::Rgb(1, 2, 3));
}

#[test]
fn scorecard_hyperlinks() {
    let term = run(b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ plain");
    assert_eq!(term.hyperlink_at(0, 0), Some("https://example.com"));
    assert_eq!(term.hyperlink_at(0, 6), None);
}

#[test]
fn scorecard_clipboard_write() {
    assert_eq!(
        actions(b"\x1b]52;c;aGk=\x07"),
        vec![Action::SetClipboard {
            clipboard: 'c',
            data: "hi".into(),
        }]
    );
    // Reads would hand the clipboard to whatever is running; refused.
    assert_eq!(actions(b"\x1b]52;c;?\x07"), vec![]);
}

#[test]
fn scorecard_prompt_marks() {
    let term = run(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07a b\r\n\x1b]133;D;0\x07");
    let last = term.last_command(None).expect("command recorded");
    assert_eq!(last.command, "ls");
    assert_eq!(last.output, "a b");
    assert_eq!(last.exit_code, Some(0));
}

#[test]
fn scorecard_working_directory() {
    let term = run(b"\x1b]7;file://host/home/me\x07");
    assert_eq!(term.working_directory(), Some("/home/me"));
}

#[test]
fn scorecard_notifications() {
    let notify = |title: &str, body: &str| Action::Notification {
        title: title.into(),
        body: body.into(),
    };
    assert_eq!(
        actions(b"\x1b]9;build done\x07"),
        vec![notify("", "build done")]
    );
    assert_eq!(
        actions(b"\x1b]777;notify;Claude;needs input\x07"),
        vec![notify("Claude", "needs input")]
    );
}

#[test]
fn scorecard_progress() {
    use ultty_vt::ansi::Progress;
    assert_eq!(
        actions(b"\x1b]9;4;1;40\x07"),
        vec![Action::Progress(Progress::Normal(40))]
    );
    assert_eq!(
        actions(b"\x1b]9;4;0\x07"),
        vec![Action::Progress(Progress::Clear)]
    );
}

#[test]
fn scorecard_colour_scheme_updates() {
    let with_background = |background| ThemeColors {
        background,
        ..ThemeColors::default()
    };
    let mut term = run(b"");
    term.set_theme_colors(with_background([0x20, 0x20, 0x20]));
    assert_eq!(replies(&mut term, b"\x1b[?996n"), "\x1b[?997;1n");
    // Unasked reports only once the program turns them on.
    term.set_theme_colors(with_background([0xf0, 0xf0, 0xf0]));
    assert_eq!(replies(&mut term, b""), "");
    assert_eq!(replies(&mut term, b"\x1b[?996n"), "\x1b[?997;2n");
    assert_eq!(
        replies(&mut term, b"\x1b[?2031h\x1b[?2031$p"),
        "\x1b[?2031;1$y"
    );
    term.set_theme_colors(with_background([0x10, 0x10, 0x30]));
    assert_eq!(replies(&mut term, b""), "\x1b[?997;1n");
    // Another dark theme is not a change of scheme.
    term.set_theme_colors(with_background([0x30, 0x10, 0x10]));
    assert_eq!(replies(&mut term, b""), "");
}

#[test]
fn scorecard_colour_queries() {
    let mut term = run(b"");
    let fg = replies(&mut term, b"\x1b]10;?\x07");
    let bg = replies(&mut term, b"\x1b]11;?\x1b\\");
    assert!(
        fg.starts_with("\x1b]10;rgb:") && fg.ends_with('\x07'),
        "{fg:?}"
    );
    assert!(
        bg.starts_with("\x1b]11;rgb:") && bg.ends_with("\x1b\\"),
        "{bg:?}"
    );
}

#[test]
fn scorecard_terminal_name() {
    let mut term = run(b"");
    let name = replies(&mut term, b"\x1b[>q");
    assert!(name.starts_with("\x1bP>|ultty "), "{name:?}");
}

#[test]
fn scorecard_sixel() {
    let term = run(b"\x1bPq#0;2;100;0;0#0~~~~\x1b\\");
    assert_eq!(term.images().images().count(), 1);
}
