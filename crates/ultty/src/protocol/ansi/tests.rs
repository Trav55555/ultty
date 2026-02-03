use super::*;

fn parse(input: &[u8]) -> Vec<Action> {
    let mut parser = AnsiParser::new();
    let mut actions = Vec::new();
    parser.feed(input, |a| actions.push(a));
    actions
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
    assert_eq!(parse(b"\x1b[<u"), vec![Action::KittyKeyboardPop]);

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
        vec![
            Action::SetTitle("Title".to_string()),
            Action::Print('X')
        ]
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
        vec![Action::Print('\u{00E9}'), Action::SetAttribute(Attribute::Bold)]
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
            Action::Print('x'), // x in ASCII mode = x
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
            Action::Print('x'), // x after SI with G0=ASCII
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
