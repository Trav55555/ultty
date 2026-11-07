use super::*;

fn parse(input: &[u8]) -> Vec<Action> {
    let mut parser = AnsiParser::new();
    let mut actions = Vec::new();
    parser.feed(input, |a| actions.push(a));
    actions
}

fn parse_spans<'a>(input: &'a [u8]) -> Vec<AnsiOutput<'a>> {
    let mut parser = AnsiParser::new();
    let mut output = Vec::new();
    parser.feed_with_ascii_spans(input, |item| output.push(item));
    output
}

#[test]
fn cursor_up() {
    assert_eq!(parse(b"\x1b[5A"), vec![Action::CursorUp(5)]);
    assert_eq!(parse(b"\x1b[A"), vec![Action::CursorUp(1)]);
}

#[test]
fn cursor_down() {
    assert_eq!(parse(b"\x1b[3B"), vec![Action::CursorDown(3)]);
}

#[test]
fn cursor_position() {
    assert_eq!(
        parse(b"\x1b[10;20H"),
        vec![Action::CursorPosition { row: 10, col: 20 }]
    );
    assert_eq!(
        parse(b"\x1b[H"),
        vec![Action::CursorPosition { row: 1, col: 1 }]
    );
}

#[test]
fn sgr_bold() {
    assert_eq!(
        parse(b"\x1b[1m"),
        vec![Action::SetAttribute(Attribute::Bold)]
    );
}

#[test]
fn sgr_reset() {
    assert_eq!(parse(b"\x1b[0m"), vec![Action::ResetAttributes]);
    assert_eq!(parse(b"\x1b[m"), vec![Action::ResetAttributes]);
}

#[test]
fn sgr_foreground_color() {
    assert_eq!(
        parse(b"\x1b[31m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Named(NamedColor::Red)
        ))]
    );
}

#[test]
fn sgr_256_color() {
    assert_eq!(
        parse(b"\x1b[38;5;200m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Indexed(200)
        ))]
    );
}

#[test]
fn sgr_true_color() {
    assert_eq!(
        parse(b"\x1b[38;2;100;150;200m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Rgb {
                r: 100,
                g: 150,
                b: 200
            }
        ))]
    );
}

#[test]
fn erase_display() {
    assert_eq!(parse(b"\x1b[2J"), vec![Action::EraseInDisplay(2)]);
    assert_eq!(parse(b"\x1b[J"), vec![Action::EraseInDisplay(0)]);
}

#[test]
fn scroll() {
    assert_eq!(parse(b"\x1b[5S"), vec![Action::ScrollUp(5)]);
    assert_eq!(parse(b"\x1b[3T"), vec![Action::ScrollDown(3)]);
}

#[test]
fn save_restore() {
    assert_eq!(parse(b"\x1b7"), vec![Action::SaveCursorPosition]);
    assert_eq!(parse(b"\x1b8"), vec![Action::RestoreCursorPosition]);
    assert_eq!(parse(b"\x1b[s"), vec![Action::SaveCursorPosition]);
    assert_eq!(parse(b"\x1b[u"), vec![Action::RestoreCursorPosition]);
}

#[test]
fn print_chars() {
    assert_eq!(parse(b"Hi"), vec![Action::Print('H'), Action::Print('i')]);
}

#[test]
fn ascii_span_feed_batches_ground_printables() {
    assert_eq!(
        parse_spans(b"Hello, World!"),
        vec![AnsiOutput::PrintAscii(b"Hello, World!")]
    );
}

#[test]
fn ascii_span_feed_splits_controls_and_escapes() {
    assert_eq!(
        parse_spans(b"ab\ncd\x1b[1mef"),
        vec![
            AnsiOutput::PrintAscii(b"ab"),
            AnsiOutput::Action(Action::Newline),
            AnsiOutput::PrintAscii(b"cd"),
            AnsiOutput::Action(Action::SetAttribute(Attribute::Bold)),
            AnsiOutput::PrintAscii(b"ef"),
        ]
    );
}

#[test]
fn ascii_span_feed_preserves_utf8_and_graphics_charset() {
    assert_eq!(
        parse_spans(b"caf\xC3\xA9"),
        vec![
            AnsiOutput::PrintAscii(b"caf"),
            AnsiOutput::Action(Action::Print('\u{00E9}')),
        ]
    );

    assert_eq!(
        parse_spans(b"\x1b(0x\x1b(Bx"),
        vec![
            AnsiOutput::Action(Action::Print('\u{2502}')),
            AnsiOutput::PrintAscii(b"x"),
        ]
    );
}

#[test]
fn control_codes() {
    assert_eq!(parse(b"\x07"), vec![Action::Bell]);
    assert_eq!(parse(b"\x08"), vec![Action::Backspace]);
    assert_eq!(parse(b"\x09"), vec![Action::Tab]);
    assert_eq!(parse(b"\x0A"), vec![Action::Newline]);
    assert_eq!(parse(b"\x0D"), vec![Action::CarriageReturn]);
}

#[test]
fn alternate_screen() {
    // Mode 1049 - save cursor and switch to alternate screen
    assert_eq!(
        parse(b"\x1b[?1049h"),
        vec![Action::SetMode(Mode::AlternateScreenSaveCursor)]
    );
    assert_eq!(
        parse(b"\x1b[?1049l"),
        vec![Action::ResetMode(Mode::AlternateScreenSaveCursor)]
    );

    // Mode 47 - simple alternate screen
    assert_eq!(
        parse(b"\x1b[?47h"),
        vec![Action::SetMode(Mode::AlternateScreen)]
    );
    assert_eq!(
        parse(b"\x1b[?47l"),
        vec![Action::ResetMode(Mode::AlternateScreen)]
    );

    // Mode 1047 - alternate screen with clear
    assert_eq!(
        parse(b"\x1b[?1047h"),
        vec![Action::SetMode(Mode::AlternateScreenClear)]
    );
    assert_eq!(
        parse(b"\x1b[?1047l"),
        vec![Action::ResetMode(Mode::AlternateScreenClear)]
    );
}

#[test]
fn cursor_visibility() {
    assert_eq!(
        parse(b"\x1b[?25h"),
        vec![Action::SetMode(Mode::CursorVisible)]
    );
    assert_eq!(
        parse(b"\x1b[?25l"),
        vec![Action::ResetMode(Mode::CursorVisible)]
    );
}

#[test]
fn osc_set_title() {
    // OSC 0 ; title BEL
    assert_eq!(
        parse(b"\x1b]0;My Terminal\x07"),
        vec![Action::SetTitle("My Terminal".to_string())]
    );
    // OSC 2 ; title BEL
    assert_eq!(
        parse(b"\x1b]2;Window Title\x07"),
        vec![Action::SetTitle("Window Title".to_string())]
    );
}

#[test]
fn osc_working_directory() {
    // OSC 7 ; file://host/path BEL
    assert_eq!(
        parse(b"\x1b]7;file://localhost/home/user\x07"),
        vec![Action::SetWorkingDirectory(
            "file://localhost/home/user".to_string()
        )]
    );
}

#[test]
fn osc_shell_integration() {
    // Prompt start
    assert_eq!(parse(b"\x1b]133;A\x07"), vec![Action::PromptStart]);

    // Command start
    assert_eq!(parse(b"\x1b]133;B\x07"), vec![Action::CommandStart]);

    // Command executed
    assert_eq!(parse(b"\x1b]133;C\x07"), vec![Action::CommandExecuted]);

    // Command finished with exit code
    assert_eq!(
        parse(b"\x1b]133;D;0\x07"),
        vec![Action::CommandFinished(Some(0))]
    );
    assert_eq!(
        parse(b"\x1b]133;D;1\x07"),
        vec![Action::CommandFinished(Some(1))]
    );
}

#[test]
fn osc_hyperlinks() {
    // Start hyperlink (OSC 8 ; params ; uri BEL)
    assert_eq!(
        parse(b"\x1b]8;;https://example.com\x07"),
        vec![Action::StartHyperlink("https://example.com".to_string())]
    );

    // Start hyperlink with id parameter
    assert_eq!(
        parse(b"\x1b]8;id=link1;https://github.com\x07"),
        vec![Action::StartHyperlink("https://github.com".to_string())]
    );

    // End hyperlink (empty URI)
    assert_eq!(parse(b"\x1b]8;;\x07"), vec![Action::EndHyperlink]);
}

#[test]
fn osc_notifications() {
    // OSC 9 - iTerm2 notification (body only)
    assert_eq!(
        parse(b"\x1b]9;Build complete!\x07"),
        vec![Action::Notification {
            title: String::new(),
            body: "Build complete!".to_string()
        }]
    );

    // OSC 777 - rxvt/urxvt notification (with title and body)
    assert_eq!(
        parse(b"\x1b]777;notify;Build;Build finished successfully\x07"),
        vec![Action::Notification {
            title: "Build".to_string(),
            body: "Build finished successfully".to_string()
        }]
    );

    // OSC 777 with just title
    assert_eq!(
        parse(b"\x1b]777;notify;Alert;\x07"),
        vec![Action::Notification {
            title: "Alert".to_string(),
            body: String::new()
        }]
    );
}

#[test]
fn kitty_keyboard_protocol() {
    // CSI > flags u - push keyboard mode
    assert_eq!(parse(b"\x1b[>1u"), vec![Action::KittyKeyboardPush(1)]);

    // Push with multiple flags (disambiguate + report events)
    assert_eq!(parse(b"\x1b[>3u"), vec![Action::KittyKeyboardPush(3)]);

    // CSI < u - pop keyboard mode
    assert_eq!(parse(b"\x1b[<u"), vec![Action::KittyKeyboardPop(1)]);

    // CSI ? u - query keyboard mode
    assert_eq!(parse(b"\x1b[?u"), vec![Action::KittyKeyboardQuery]);
}

#[test]
fn osc_66_character_width() {
    // OSC 66 - Kitty character width protocol
    // Format: OSC 66 ; w=N ; CHAR ST
    // Should print the character(s) after the width specification

    // Single block character with ST terminator
    assert_eq!(
        parse(b"\x1b]66;w=1;\xE2\x96\x80\x1b\\"),
        vec![Action::Print('\u{2580}')]
    );

    // Block character with BEL terminator
    assert_eq!(
        parse(b"\x1b]66;w=1;\xE2\x96\x88\x07"),
        vec![Action::Print('\u{2588}')]
    );

    // Multiple characters
    assert_eq!(
        parse(b"\x1b]66;w=1;\xE2\x96\x80\xE2\x96\x84\x1b\\"),
        vec![Action::Print('\u{2580}'), Action::Print('\u{2584}')]
    );

    // Screen width (s=N) format
    assert_eq!(
        parse(b"\x1b]66;s=2;\xE2\x96\x84\x1b\\"),
        vec![Action::Print('\u{2584}')]
    );
}

#[test]
fn osc_st_terminator() {
    // Test that ST (ESC \) properly terminates OSC and doesn't print backslash
    assert_eq!(
        parse(b"\x1b]0;My Title\x1b\\"),
        vec![Action::SetTitle("My Title".to_string())]
    );

    // Verify no extra backslash is printed
    assert_eq!(
        parse(b"\x1b]0;Title\x1b\\X"),
        vec![Action::SetTitle("Title".to_string()), Action::Print('X')]
    );
}

#[test]
fn apc_sequences() {
    // APC sequences (ESC _) should be consumed without printing anything
    // Used by Kitty graphics protocol

    // Simple APC with ST terminator
    assert_eq!(parse(b"\x1b_G\x1b\\"), vec![]);

    // APC followed by text - text should print
    assert_eq!(
        parse(b"\x1b_Gi=31337,s=1,v=1,a=q\x1b\\Hello"),
        vec![
            Action::Print('H'),
            Action::Print('e'),
            Action::Print('l'),
            Action::Print('l'),
            Action::Print('o'),
        ]
    );

    // Multiple APC sequences
    assert_eq!(
        parse(b"\x1b_G1\x1b\\\x1b_G2\x1b\\X"),
        vec![Action::Print('X')]
    );
}

#[test]
fn utf8_two_byte_chars() {
    // 2-byte UTF-8: e with acute (U+00E9) = 0xC3 0xA9
    assert_eq!(parse(b"\xC3\xA9"), vec![Action::Print('\u{00E9}')]);
    // a with circumflex (U+00E2) = 0xC3 0xA2
    assert_eq!(parse(b"\xC3\xA2"), vec![Action::Print('\u{00E2}')]);
    // Mixed ASCII and UTF-8
    assert_eq!(
        parse(b"caf\xC3\xA9"),
        vec![
            Action::Print('c'),
            Action::Print('a'),
            Action::Print('f'),
            Action::Print('\u{00E9}')
        ]
    );
}

#[test]
fn utf8_three_byte_chars() {
    // 3-byte UTF-8: CJK character (U+4E2D) = 0xE4 0xB8 0xAD
    assert_eq!(parse(b"\xE4\xB8\xAD"), vec![Action::Print('\u{4E2D}')]);
    // Euro sign (U+20AC) = 0xE2 0x82 0xAC
    assert_eq!(parse(b"\xE2\x82\xAC"), vec![Action::Print('\u{20AC}')]);
}

#[test]
fn utf8_four_byte_chars() {
    // 4-byte UTF-8: grinning face (U+1F600) = 0xF0 0x9F 0x98 0x80
    assert_eq!(parse(b"\xF0\x9F\x98\x80"), vec![Action::Print('\u{1F600}')]);
    // crab (U+1F980) = 0xF0 0x9F 0xA6 0x80
    assert_eq!(parse(b"\xF0\x9F\xA6\x80"), vec![Action::Print('\u{1F980}')]);
}

#[test]
fn utf8_invalid_continuation() {
    // Lead byte followed by non-continuation - should emit replacement char
    // then process the next byte normally
    assert_eq!(
        parse(b"\xC3X"),
        vec![Action::Print('\u{FFFD}'), Action::Print('X')]
    );
}

#[test]
fn utf8_orphan_continuation() {
    // Continuation byte without lead - emit replacement char
    assert_eq!(parse(b"\x80"), vec![Action::Print('\u{FFFD}')]);
    assert_eq!(parse(b"\xBF"), vec![Action::Print('\u{FFFD}')]);
}

#[test]
fn utf8_invalid_lead_bytes() {
    // Invalid lead bytes 0xF8-0xFF - emit replacement char
    assert_eq!(parse(b"\xF8"), vec![Action::Print('\u{FFFD}')]);
    assert_eq!(parse(b"\xFF"), vec![Action::Print('\u{FFFD}')]);
}

#[test]
fn utf8_mixed_with_escape_sequences() {
    // UTF-8 chars mixed with ANSI escapes
    assert_eq!(
        parse(b"\xC3\xA9\x1b[1m"),
        vec![
            Action::Print('\u{00E9}'),
            Action::SetAttribute(Attribute::Bold)
        ]
    );
}

#[test]
fn dec_special_graphics_line_drawing() {
    // ESC ( 0 switches to DEC Special Graphics (G0)
    // 'x' (0x78) should become vertical line
    // 'q' (0x71) should become horizontal line
    // 'l' (0x6C) should become upper left corner
    // ESC ( B switches back to ASCII
    assert_eq!(
        parse(b"\x1b(0lqx\x1b(B"),
        vec![
            Action::Print('\u{250C}'), // l = upper left corner
            Action::Print('\u{2500}'), // q = horizontal line
            Action::Print('\u{2502}'), // x = vertical line
        ]
    );
}

#[test]
fn dec_special_graphics_corners() {
    // All four corners
    assert_eq!(
        parse(b"\x1b(0lkjm\x1b(B"),
        vec![
            Action::Print('\u{250C}'), // l = upper left
            Action::Print('\u{2510}'), // k = upper right
            Action::Print('\u{2518}'), // j = lower right
            Action::Print('\u{2514}'), // m = lower left
        ]
    );
}

#[test]
fn dec_special_graphics_tees_and_cross() {
    // Tee characters and crossing lines
    assert_eq!(
        parse(b"\x1b(0tuvwn\x1b(B"),
        vec![
            Action::Print('\u{251C}'), // t = left tee
            Action::Print('\u{2524}'), // u = right tee
            Action::Print('\u{2534}'), // v = bottom tee
            Action::Print('\u{252C}'), // w = top tee
            Action::Print('\u{253C}'), // n = crossing lines
        ]
    );
}

#[test]
fn dec_special_graphics_switch_back() {
    // After ESC ( B, characters should be normal ASCII
    assert_eq!(
        parse(b"\x1b(0x\x1b(Bx"),
        vec![
            Action::Print('\u{2502}'), // x in graphics mode = vertical line
            Action::Print('x'),        // x in ASCII mode = x
        ]
    );
}

#[test]
fn dec_special_graphics_g1_charset() {
    // ESC ) 0 sets G1 to graphics, SO (0x0E) switches to G1
    // SI (0x0F) switches back to G0
    assert_eq!(
        parse(b"\x1b)0\x0Ex\x0Fx"),
        vec![
            Action::Print('\u{2502}'), // x after SO with G1=graphics
            Action::Print('x'),        // x after SI with G0=ASCII
        ]
    );
}

#[test]
fn dec_special_graphics_ascii_passthrough() {
    // Characters before 0x5F should pass through unchanged in graphics mode
    assert_eq!(
        parse(b"\x1b(0ABC\x1b(B"),
        vec![
            Action::Print('A'), // Passthrough
            Action::Print('B'), // Passthrough
            Action::Print('C'), // Passthrough
        ]
    );
}

#[test]
fn primary_device_attributes() {
    assert_eq!(parse(b"\x1b[c"), vec![Action::DeviceAttributes]);
    assert_eq!(parse(b"\x1b[0c"), vec![Action::DeviceAttributes]);
}

#[test]
fn secondary_device_attributes() {
    assert_eq!(parse(b"\x1b[>c"), vec![Action::SecondaryDeviceAttributes]);
    assert_eq!(parse(b"\x1b[>0c"), vec![Action::SecondaryDeviceAttributes]);
}

// ---------------------------------------------------------------
// CSI: cursor movement (remaining variants)
// ---------------------------------------------------------------

#[test]
fn cursor_forward() {
    assert_eq!(parse(b"\x1b[4C"), vec![Action::CursorForward(4)]);
    assert_eq!(parse(b"\x1b[C"), vec![Action::CursorForward(1)]);
}

#[test]
fn cursor_back() {
    assert_eq!(parse(b"\x1b[2D"), vec![Action::CursorBack(2)]);
    assert_eq!(parse(b"\x1b[D"), vec![Action::CursorBack(1)]);
}

#[test]
fn cursor_next_line() {
    assert_eq!(parse(b"\x1b[3E"), vec![Action::CursorNextLine(3)]);
    assert_eq!(parse(b"\x1b[E"), vec![Action::CursorNextLine(1)]);
}

#[test]
fn cursor_previous_line() {
    assert_eq!(parse(b"\x1b[2F"), vec![Action::CursorPreviousLine(2)]);
    assert_eq!(parse(b"\x1b[F"), vec![Action::CursorPreviousLine(1)]);
}

#[test]
fn cursor_horizontal_absolute() {
    assert_eq!(
        parse(b"\x1b[15G"),
        vec![Action::CursorHorizontalAbsolute(15)]
    );
    assert_eq!(parse(b"\x1b[G"), vec![Action::CursorHorizontalAbsolute(1)]);
}

#[test]
fn cursor_position_f_variant() {
    // CSI f is the same as CSI H
    assert_eq!(
        parse(b"\x1b[5;10f"),
        vec![Action::CursorPosition { row: 5, col: 10 }]
    );
}

// ---------------------------------------------------------------
// CSI: erase operations
// ---------------------------------------------------------------

#[test]
fn erase_in_line() {
    assert_eq!(parse(b"\x1b[K"), vec![Action::EraseInLine(0)]);
    assert_eq!(parse(b"\x1b[0K"), vec![Action::EraseInLine(0)]);
    assert_eq!(parse(b"\x1b[1K"), vec![Action::EraseInLine(1)]);
    assert_eq!(parse(b"\x1b[2K"), vec![Action::EraseInLine(2)]);
}

#[test]
fn erase_characters() {
    assert_eq!(parse(b"\x1b[5X"), vec![Action::EraseCharacters(5)]);
    assert_eq!(parse(b"\x1b[X"), vec![Action::EraseCharacters(1)]);
}

#[test]
fn erase_display_all_modes() {
    assert_eq!(parse(b"\x1b[0J"), vec![Action::EraseInDisplay(0)]);
    assert_eq!(parse(b"\x1b[1J"), vec![Action::EraseInDisplay(1)]);
    assert_eq!(parse(b"\x1b[3J"), vec![Action::EraseInDisplay(3)]);
}

// ---------------------------------------------------------------
// CSI: line/character insert/delete
// ---------------------------------------------------------------

#[test]
fn insert_lines() {
    assert_eq!(parse(b"\x1b[3L"), vec![Action::InsertLines(3)]);
    assert_eq!(parse(b"\x1b[L"), vec![Action::InsertLines(1)]);
}

#[test]
fn delete_lines() {
    assert_eq!(parse(b"\x1b[2M"), vec![Action::DeleteLines(2)]);
    assert_eq!(parse(b"\x1b[M"), vec![Action::DeleteLines(1)]);
}

#[test]
fn insert_characters() {
    assert_eq!(parse(b"\x1b[4@"), vec![Action::InsertCharacters(4)]);
    assert_eq!(parse(b"\x1b[@"), vec![Action::InsertCharacters(1)]);
}

#[test]
fn delete_characters() {
    assert_eq!(parse(b"\x1b[3P"), vec![Action::DeleteCharacters(3)]);
    assert_eq!(parse(b"\x1b[P"), vec![Action::DeleteCharacters(1)]);
}

// ---------------------------------------------------------------
// CSI: scrolling region
// ---------------------------------------------------------------

#[test]
fn set_scrolling_region() {
    assert_eq!(
        parse(b"\x1b[5;20r"),
        vec![Action::SetScrollingRegion { top: 5, bottom: 20 }]
    );
}

// ---------------------------------------------------------------
// CSI: cursor style
// ---------------------------------------------------------------

#[test]
fn set_cursor_style() {
    // Blinking block
    assert_eq!(parse(b"\x1b[1 q"), vec![Action::SetCursorStyle(1)]);
    // Steady block
    assert_eq!(parse(b"\x1b[2 q"), vec![Action::SetCursorStyle(2)]);
    // Blinking underline
    assert_eq!(parse(b"\x1b[3 q"), vec![Action::SetCursorStyle(3)]);
    // Steady bar
    assert_eq!(parse(b"\x1b[6 q"), vec![Action::SetCursorStyle(6)]);
}

// ---------------------------------------------------------------
// CSI: report cursor position
// ---------------------------------------------------------------

#[test]
fn report_cursor_position() {
    assert_eq!(parse(b"\x1b[6n"), vec![Action::ReportCursorPosition]);
}

// ---------------------------------------------------------------
// CSI: modes (additional)
// ---------------------------------------------------------------

#[test]
fn bracketed_paste_mode() {
    assert_eq!(
        parse(b"\x1b[?2004h"),
        vec![Action::SetMode(Mode::BracketedPaste)]
    );
    assert_eq!(
        parse(b"\x1b[?2004l"),
        vec![Action::ResetMode(Mode::BracketedPaste)]
    );
}

#[test]
fn focus_reporting_mode() {
    assert_eq!(
        parse(b"\x1b[?1004h"),
        vec![Action::SetMode(Mode::FocusReporting)]
    );
    assert_eq!(
        parse(b"\x1b[?1004l"),
        vec![Action::ResetMode(Mode::FocusReporting)]
    );
}

#[test]
fn application_cursor_keys_mode() {
    assert_eq!(
        parse(b"\x1b[?1h"),
        vec![Action::SetMode(Mode::ApplicationCursorKeys)]
    );
    assert_eq!(
        parse(b"\x1b[?1l"),
        vec![Action::ResetMode(Mode::ApplicationCursorKeys)]
    );
}

#[test]
fn mouse_tracking_modes() {
    assert_eq!(parse(b"\x1b[?9h"), vec![Action::SetMode(Mode::MouseX10)]);
    assert_eq!(
        parse(b"\x1b[?1000h"),
        vec![Action::SetMode(Mode::MouseNormal)]
    );
    assert_eq!(
        parse(b"\x1b[?1002h"),
        vec![Action::SetMode(Mode::MouseButtonEvent)]
    );
    assert_eq!(
        parse(b"\x1b[?1003h"),
        vec![Action::SetMode(Mode::MouseAnyEvent)]
    );
    assert_eq!(parse(b"\x1b[?1006h"), vec![Action::SetMode(Mode::MouseSgr)]);
}

// ---------------------------------------------------------------
// SGR: all attribute types
// ---------------------------------------------------------------

#[test]
fn sgr_dim() {
    assert_eq!(
        parse(b"\x1b[2m"),
        vec![Action::SetAttribute(Attribute::Dim)]
    );
}

#[test]
fn sgr_italic() {
    assert_eq!(
        parse(b"\x1b[3m"),
        vec![Action::SetAttribute(Attribute::Italic)]
    );
}

#[test]
fn sgr_underline() {
    assert_eq!(
        parse(b"\x1b[4m"),
        vec![Action::SetAttribute(Attribute::Underline)]
    );
}

#[test]
fn sgr_blink() {
    assert_eq!(
        parse(b"\x1b[5m"),
        vec![Action::SetAttribute(Attribute::Blink)]
    );
}

#[test]
fn sgr_reverse() {
    assert_eq!(
        parse(b"\x1b[7m"),
        vec![Action::SetAttribute(Attribute::Reverse)]
    );
}

#[test]
fn sgr_hidden() {
    assert_eq!(
        parse(b"\x1b[8m"),
        vec![Action::SetAttribute(Attribute::Hidden)]
    );
}

#[test]
fn sgr_strikethrough() {
    assert_eq!(
        parse(b"\x1b[9m"),
        vec![Action::SetAttribute(Attribute::Strikethrough)]
    );
}

#[test]
fn sgr_disable_attributes() {
    assert_eq!(
        parse(b"\x1b[22m"),
        vec![Action::SetAttribute(Attribute::NoBold)]
    );
    assert_eq!(
        parse(b"\x1b[23m"),
        vec![Action::SetAttribute(Attribute::NoItalic)]
    );
    assert_eq!(
        parse(b"\x1b[24m"),
        vec![Action::SetAttribute(Attribute::NoUnderline)]
    );
    assert_eq!(
        parse(b"\x1b[25m"),
        vec![Action::SetAttribute(Attribute::NoBlink)]
    );
    assert_eq!(
        parse(b"\x1b[27m"),
        vec![Action::SetAttribute(Attribute::NoReverse)]
    );
    assert_eq!(
        parse(b"\x1b[28m"),
        vec![Action::SetAttribute(Attribute::NoHidden)]
    );
    assert_eq!(
        parse(b"\x1b[29m"),
        vec![Action::SetAttribute(Attribute::NoStrikethrough)]
    );
}

// ---------------------------------------------------------------
// SGR: background colors
// ---------------------------------------------------------------

#[test]
fn sgr_background_color() {
    assert_eq!(
        parse(b"\x1b[41m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Named(NamedColor::Red)
        ))]
    );
    assert_eq!(
        parse(b"\x1b[44m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Named(NamedColor::Blue)
        ))]
    );
}

#[test]
fn sgr_background_256_color() {
    assert_eq!(
        parse(b"\x1b[48;5;100m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Indexed(100)
        ))]
    );
}

#[test]
fn sgr_background_true_color() {
    assert_eq!(
        parse(b"\x1b[48;2;10;20;30m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        ))]
    );
}

// ---------------------------------------------------------------
// SGR: default colors
// ---------------------------------------------------------------

#[test]
fn sgr_default_foreground() {
    assert_eq!(
        parse(b"\x1b[39m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Default
        ))]
    );
}

#[test]
fn sgr_default_background() {
    assert_eq!(
        parse(b"\x1b[49m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Default
        ))]
    );
}

// ---------------------------------------------------------------
// SGR: bright colors
// ---------------------------------------------------------------

#[test]
fn sgr_bright_foreground() {
    assert_eq!(
        parse(b"\x1b[90m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Named(NamedColor::BrightBlack)
        ))]
    );
    assert_eq!(
        parse(b"\x1b[91m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Named(NamedColor::BrightRed)
        ))]
    );
    assert_eq!(
        parse(b"\x1b[97m"),
        vec![Action::SetAttribute(Attribute::ForegroundColor(
            Color::Named(NamedColor::BrightWhite)
        ))]
    );
}

#[test]
fn sgr_bright_background() {
    assert_eq!(
        parse(b"\x1b[100m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Named(NamedColor::BrightBlack)
        ))]
    );
    assert_eq!(
        parse(b"\x1b[107m"),
        vec![Action::SetAttribute(Attribute::BackgroundColor(
            Color::Named(NamedColor::BrightWhite)
        ))]
    );
}

// ---------------------------------------------------------------
// SGR: multiple params in one sequence
// ---------------------------------------------------------------

#[test]
fn sgr_multiple_params() {
    assert_eq!(
        parse(b"\x1b[1;31m"),
        vec![
            Action::SetAttribute(Attribute::Bold),
            Action::SetAttribute(Attribute::ForegroundColor(Color::Named(NamedColor::Red))),
        ]
    );
}

#[test]
fn sgr_reset_with_bold_and_color() {
    assert_eq!(
        parse(b"\x1b[0;1;32m"),
        vec![
            Action::ResetAttributes,
            Action::SetAttribute(Attribute::Bold),
            Action::SetAttribute(Attribute::ForegroundColor(Color::Named(NamedColor::Green))),
        ]
    );
}

// ---------------------------------------------------------------
// SGR: all 8 named foreground colors
// ---------------------------------------------------------------

#[test]
fn sgr_all_named_foreground_colors() {
    let expected = [
        (30, NamedColor::Black),
        (31, NamedColor::Red),
        (32, NamedColor::Green),
        (33, NamedColor::Yellow),
        (34, NamedColor::Blue),
        (35, NamedColor::Magenta),
        (36, NamedColor::Cyan),
        (37, NamedColor::White),
    ];
    for (code, color) in expected {
        let input = format!("\x1b[{}m", code);
        assert_eq!(
            parse(input.as_bytes()),
            vec![Action::SetAttribute(Attribute::ForegroundColor(
                Color::Named(color)
            ))],
            "SGR {} should map to {:?}",
            code,
            color,
        );
    }
}

// ---------------------------------------------------------------
// OSC: clipboard (OSC 52)
// ---------------------------------------------------------------

#[test]
fn osc_clipboard() {
    // OSC 52 ; c ; base64data BEL
    assert_eq!(
        parse(b"\x1b]52;c;SGVsbG8=\x07"),
        vec![Action::SetClipboard {
            clipboard: 'c',
            data: "Hello".to_string(),
        }]
    );
}

// ---------------------------------------------------------------
// OSC: color queries
// ---------------------------------------------------------------

#[test]
fn osc_query_colors() {
    use super::{DynamicColor, StringTerminator};
    for (bytes, color) in [
        (b"\x1b]10;?\x07", DynamicColor::Foreground),
        (b"\x1b]11;?\x07", DynamicColor::Background),
        (b"\x1b]12;?\x07", DynamicColor::Cursor),
    ] {
        assert_eq!(
            parse(bytes),
            vec![Action::QueryDynamicColor {
                color,
                terminator: StringTerminator::Bel
            }]
        );
    }
}

// ---------------------------------------------------------------
// ESC sequences: index, reverse index, next line
// ---------------------------------------------------------------

#[test]
fn esc_reverse_index() {
    assert_eq!(parse(b"\x1bM"), vec![Action::ReverseIndex]);
}

#[test]
fn esc_index() {
    assert_eq!(parse(b"\x1bD"), vec![Action::Index]);
}

#[test]
fn esc_next_line() {
    assert_eq!(parse(b"\x1bE"), vec![Action::NextLine]);
}

// ---------------------------------------------------------------
// Tab operations
// ---------------------------------------------------------------

#[test]
fn tab_clear() {
    assert_eq!(parse(b"\x1b[0g"), vec![Action::ClearTabStop(0)]);
    assert_eq!(parse(b"\x1b[3g"), vec![Action::ClearTabStop(3)]);
}

// ---------------------------------------------------------------
// Synchronized update
// ---------------------------------------------------------------

#[test]
fn synchronized_update_mode() {
    assert_eq!(
        parse(b"\x1b[?2026h"),
        vec![Action::SetMode(Mode::SynchronizedUpdate)]
    );
    assert_eq!(
        parse(b"\x1b[?2026l"),
        vec![Action::ResetMode(Mode::SynchronizedUpdate)]
    );
}

// ---------------------------------------------------------------
// DECRQM: request private mode
// ---------------------------------------------------------------

#[test]
fn request_private_mode() {
    assert_eq!(
        parse(b"\x1b[?2026$p"),
        vec![Action::RequestPrivateMode(2026)]
    );
}

// ---------------------------------------------------------------
// DECSTBM default params
// ---------------------------------------------------------------

#[test]
fn decstbm_default_params() {
    // CSI r with no params sends 0, 0 as sentinels
    assert_eq!(
        parse(b"\x1b[r"),
        vec![Action::SetScrollingRegion { top: 0, bottom: 0 }]
    );
}

#[test]
fn decstbm_one_param() {
    // CSI 5 r — top=5, bottom=0 (default)
    assert_eq!(
        parse(b"\x1b[5r"),
        vec![Action::SetScrollingRegion { top: 5, bottom: 0 }]
    );
}

// ---------------------------------------------------------------
// Insert mode (IRM) — CSI 4 h / CSI 4 l
// ---------------------------------------------------------------

#[test]
fn insert_mode_set_reset() {
    assert_eq!(parse(b"\x1b[4h"), vec![Action::SetInsertMode(true)]);
    assert_eq!(parse(b"\x1b[4l"), vec![Action::SetInsertMode(false)]);
}

// ---------------------------------------------------------------
// SGR double underline, overline
// ---------------------------------------------------------------

#[test]
fn sgr_double_underline() {
    assert_eq!(
        parse(b"\x1b[21m"),
        vec![Action::SetAttribute(Attribute::DoubleUnderline)]
    );
}

#[test]
fn sgr_overline() {
    assert_eq!(
        parse(b"\x1b[53m"),
        vec![Action::SetAttribute(Attribute::Overline)]
    );
    assert_eq!(
        parse(b"\x1b[55m"),
        vec![Action::SetAttribute(Attribute::NoOverline)]
    );
}

// ---------------------------------------------------------------
// Full reset (RIS — ESC c)
// ---------------------------------------------------------------

#[test]
fn full_reset_esc_c() {
    assert_eq!(parse(b"\x1bc"), vec![Action::FullReset]);
}

#[test]
fn full_reset_restores_ascii_charset() {
    // ESC ( 0 designates DEC Special Graphics into G0: 'a' becomes a
    // checkerboard. RIS must undo that, unlike an ordinary sequence reset.
    let actions = parse(b"\x1b(0a\x1bca");
    assert_eq!(
        actions,
        vec![
            Action::Print('\u{2592}'),
            Action::FullReset,
            Action::Print('a')
        ]
    );
}

#[test]
fn full_reset_restores_shift_in_state() {
    // ESC ) 0 designates graphics into G1, SO (0x0E) shifts it in.
    let actions = parse(b"\x1b)0\x0ea\x1bca");
    assert_eq!(
        actions,
        vec![
            Action::Print('\u{2592}'),
            Action::FullReset,
            Action::Print('a')
        ]
    );
}

// ---------------------------------------------------------------
// Soft reset (DECSTR — CSI ! p)
// ---------------------------------------------------------------

#[test]
fn soft_reset_csi_bang_p() {
    assert_eq!(parse(b"\x1b[!p"), vec![Action::SoftReset]);
}

// ---------------------------------------------------------------
// Screen alignment pattern (DECALN — ESC # 8)
// ---------------------------------------------------------------

#[test]
fn decaln_esc_hash_8() {
    assert_eq!(parse(b"\x1b#8"), vec![Action::ScreenAlignmentPattern]);
}

// ---------------------------------------------------------------
// Keypad modes (DECKPAM/DECKPNM)
// ---------------------------------------------------------------

#[test]
fn application_keypad_mode() {
    assert_eq!(parse(b"\x1b="), vec![Action::ApplicationKeypadMode]);
}

#[test]
fn normal_keypad_mode() {
    assert_eq!(parse(b"\x1b>"), vec![Action::NormalKeypadMode]);
}

/// The characters `input` prints.
fn printed(input: &[u8]) -> String {
    parse(input)
        .into_iter()
        .map(|action| match action {
            Action::Print(c) => c,
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn utf8_ill_formed_sequences_become_one_replacement_per_maximal_subpart() {
    // An F5 lead and an encoded surrogate: each byte is its own subpart.
    assert_eq!(
        printed(b"a\xF5\x80\x80\x80b\xED\xA0\x80c"),
        "a\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}b\u{FFFD}\u{FFFD}\u{FFFD}c"
    );
    // Overlong forms: C0/C1 leads, and E0 / F0 followed by too small a byte.
    assert_eq!(printed(b"\xC0\xAF"), "\u{FFFD}\u{FFFD}");
    assert_eq!(printed(b"\xE0\x80\xAF"), "\u{FFFD}\u{FFFD}\u{FFFD}");
    assert_eq!(
        printed(b"\xF0\x8F\xBF\xBF"),
        "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"
    );
    // Above U+10FFFF.
    assert_eq!(
        printed(b"\xF4\x90\x80\x80"),
        "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"
    );
    // A truncated sequence is one replacement, then the next byte.
    assert_eq!(printed(b"\xE2\x82X"), "\u{FFFD}X");
    assert_eq!(printed(b"\xF0\x9F\x98X"), "\u{FFFD}X");
    // The Unicode Standard's own example (section 3.9, table 3-8).
    assert_eq!(
        printed(b"\x61\xF1\x80\x80\xE1\x80\xC2\x62\x80\x63\x80\xBF\x64"),
        "a\u{FFFD}\u{FFFD}\u{FFFD}b\u{FFFD}c\u{FFFD}\u{FFFD}d"
    );
}

#[test]
fn utf8_boundaries_of_the_well_formed_ranges_decode() {
    assert_eq!(printed(b"\xC2\x80\xDF\xBF"), "\u{80}\u{7FF}");
    assert_eq!(
        printed(b"\xE0\xA0\x80\xED\x9F\xBF\xEE\x80\x80"),
        "\u{800}\u{D7FF}\u{E000}"
    );
    assert_eq!(
        printed(b"\xF0\x90\x80\x80\xF4\x8F\xBF\xBF"),
        "\u{10000}\u{10FFFF}"
    );
}

#[test]
fn utf8_sequence_split_across_feeds_decodes() {
    let mut parser = AnsiParser::new();
    let mut actions = Vec::new();
    parser.feed(b"\xE2\x82", |a| actions.push(a));
    parser.feed(b"\xAC", |a| actions.push(a));
    assert_eq!(actions, vec![Action::Print('\u{20AC}')]);
}

/// Bytes biased towards UTF-8: whole characters of every length, broken
/// and overlong sequences, controls and escapes.
fn utf8_heavy_bytes() -> impl proptest::strategy::Strategy<Value = Vec<u8>> {
    use proptest::prelude::*;
    let piece = prop_oneof![
        4 => any::<char>().prop_map(|c| c.to_string().into_bytes()),
        2 => prop::sample::select(vec!["é", "中", "😀", "\u{301}", "€", "\u{10FFFF}"])
            .prop_map(|s| s.as_bytes().to_vec()),
        1 => prop::collection::vec(0x80u8..=0xFF, 1..4),
        1 => prop::sample::select(vec![
            &b"\xC0\xAF"[..], b"\xE0\x80\x80", b"\xED\xA0\x80", b"\xF4\x90\x80\x80",
            b"\xE2\x82", b"\xF0\x9F\x98", b"\x1b[1m", b"\x1b(0", b"\x1b(B", b"\r\n",
            b"\x1b]0;t\xC3\xA9\x07",
        ])
        .prop_map(<[u8]>::to_vec),
        1 => any::<u8>().prop_map(|b| vec![b]),
    ];
    prop::collection::vec(piece, 0..24).prop_map(|v| v.concat())
}

proptest::proptest! {
    /// Span feeding is an optimisation only: split anywhere, it yields
    /// exactly the characters and actions that byte-wise feeding does.
    #[test]
    fn ascii_span_feed_matches_feed(
        bytes in utf8_heavy_bytes(),
        cuts in proptest::collection::vec(proptest::prelude::any::<usize>(), 0..4),
    ) {
        let mut cuts: Vec<usize> = cuts.iter().map(|c| c % (bytes.len() + 1)).collect();
        cuts.sort_unstable();
        let mut chunks = Vec::new();
        let mut start = 0;
        for cut in cuts.into_iter().chain([bytes.len()]) {
            chunks.push(&bytes[start..cut]);
            start = cut;
        }

        let mut expected = Vec::new();
        let mut parser = AnsiParser::new();
        for chunk in &chunks {
            parser.feed(chunk, |a| expected.push(a));
        }

        let mut actual = Vec::new();
        let mut parser = AnsiParser::new();
        for chunk in &chunks {
            parser.feed_with_ascii_spans(chunk, |out| match out {
                AnsiOutput::Action(a) => actual.push(a),
                AnsiOutput::PrintAscii(span) => {
                    actual.extend(span.iter().map(|&b| Action::Print(b as char)));
                }
            });
        }
        proptest::prop_assert_eq!(actual, expected);
    }
}
