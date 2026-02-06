//! Property-based tests for ANSI parser robustness.
//!
//! These tests use proptest to fuzz the parser with arbitrary input,
//! ensuring it never panics and handles edge cases correctly.

use proptest::prelude::*;
use ultty::protocol::ansi::{Action, AnsiParser, Attribute, Color, Mode, NamedColor};

fn collect_actions(input: &[u8]) -> Vec<Action> {
    let mut parser = AnsiParser::new();
    let mut actions = Vec::new();
    parser.feed(input, |action| actions.push(action));
    actions
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10000))]

    #[test]
    fn parser_never_panics_on_arbitrary_bytes(data in prop::collection::vec(any::<u8>(), 0..1024)) {
        let mut parser = AnsiParser::new();
        let mut actions = Vec::new();
        parser.feed(&data, |action| actions.push(action));
    }

    #[test]
    fn parser_never_panics_on_escape_like_sequences(
        prefix in prop::collection::vec(0x1bu8..=0x1bu8, 1..=1),
        suffix in prop::collection::vec(any::<u8>(), 0..100)
    ) {
        let mut data = prefix;
        data.extend(suffix);
        let mut parser = AnsiParser::new();
        parser.feed(&data, |_| {});
    }

    #[test]
    fn parser_never_panics_on_csi_sequences(
        params in prop::collection::vec(any::<u8>(), 0..50),
        final_byte in 0x40u8..=0x7Eu8
    ) {
        let mut data = vec![0x1b, b'['];
        data.extend(params);
        data.push(final_byte);
        let mut parser = AnsiParser::new();
        parser.feed(&data, |_| {});
    }

    #[test]
    fn parser_never_panics_on_osc_sequences(
        content in prop::collection::vec(any::<u8>(), 0..200),
        terminator in prop::sample::select(vec![0x07u8, 0x9c])
    ) {
        let mut data = vec![0x1b, b']'];
        data.extend(content.iter().filter(|&&b| b != 0x07 && b != 0x9c && b != 0x1b));
        data.push(terminator);
        let mut parser = AnsiParser::new();
        parser.feed(&data, |_| {});
    }

    #[test]
    fn parser_handles_utf8_sequences(
        valid_ascii in "[a-zA-Z0-9 ]{0,50}",
        emoji in prop::sample::select(vec!["😀", "🎉", "中文", "日本語", "한국어", ""])
    ) {
        let data = format!("{}{}", valid_ascii, emoji);
        let mut parser = AnsiParser::new();
        let mut actions = Vec::new();
        parser.feed(data.as_bytes(), |action| actions.push(action));

        let print_count = actions.iter().filter(|a| matches!(a, Action::Print(_))).count();
        prop_assert!(print_count > 0 || data.is_empty());
    }

    #[test]
    fn parser_handles_mixed_escape_and_text(
        text1 in "[a-z]{0,20}",
        text2 in "[a-z]{0,20}"
    ) {
        let data = format!("{}\x1b[1m{}\x1b[0m", text1, text2);
        let actions = collect_actions(data.as_bytes());

        let has_bold = actions.iter().any(|a| matches!(a, Action::SetAttribute(Attribute::Bold)));
        let has_reset = actions.iter().any(|a| matches!(a, Action::ResetAttributes));

        prop_assert!(has_bold, "Should have bold attribute");
        prop_assert!(has_reset, "Should have reset");
    }

    #[test]
    fn cursor_movement_params_are_clamped(row in 0u16..=9999u16, col in 0u16..=9999u16) {
        let data = format!("\x1b[{};{}H", row, col);
        let actions = collect_actions(data.as_bytes());

        prop_assert_eq!(actions.len(), 1);
        if let Action::CursorPosition { row: r, col: c } = &actions[0] {
            prop_assert!(*r <= 9999);
            prop_assert!(*c <= 9999);
        } else {
            prop_assert!(false, "Expected CursorPosition action");
        }
    }

    #[test]
    fn sgr_256_color_valid_range(color_idx in 0u8..=255u8) {
        let fg_data = format!("\x1b[38;5;{}m", color_idx);
        let fg_actions = collect_actions(fg_data.as_bytes());

        prop_assert_eq!(fg_actions.len(), 1);
        match &fg_actions[0] {
            Action::SetAttribute(Attribute::ForegroundColor(Color::Indexed(idx))) => {
                prop_assert_eq!(*idx, color_idx);
            }
            _ => prop_assert!(false, "Expected ForegroundColor with Indexed"),
        }

        let bg_data = format!("\x1b[48;5;{}m", color_idx);
        let bg_actions = collect_actions(bg_data.as_bytes());

        prop_assert_eq!(bg_actions.len(), 1);
        match &bg_actions[0] {
            Action::SetAttribute(Attribute::BackgroundColor(Color::Indexed(idx))) => {
                prop_assert_eq!(*idx, color_idx);
            }
            _ => prop_assert!(false, "Expected BackgroundColor with Indexed"),
        }
    }

    #[test]
    fn sgr_true_color_valid_range(r in 0u8..=255u8, g in 0u8..=255u8, b in 0u8..=255u8) {
        let fg_data = format!("\x1b[38;2;{};{};{}m", r, g, b);
        let fg_actions = collect_actions(fg_data.as_bytes());

        prop_assert_eq!(fg_actions.len(), 1);
        match &fg_actions[0] {
            Action::SetAttribute(Attribute::ForegroundColor(Color::Rgb { r: rr, g: gg, b: bb })) => {
                prop_assert_eq!(*rr, r);
                prop_assert_eq!(*gg, g);
                prop_assert_eq!(*bb, b);
            }
            _ => prop_assert!(false, "Expected ForegroundColor with Rgb"),
        }
    }

    #[test]
    fn scroll_region_params_preserved(top in 1u16..=100u16, bottom in 1u16..=100u16) {
        let data = format!("\x1b[{};{}r", top, bottom);
        let actions = collect_actions(data.as_bytes());

        prop_assert_eq!(actions.len(), 1);
        if let Action::SetScrollingRegion { top: t, bottom: b } = &actions[0] {
            prop_assert_eq!(*t, top);
            prop_assert_eq!(*b, bottom);
        } else {
            prop_assert!(false, "Expected SetScrollingRegion action");
        }
    }

    #[test]
    fn erase_modes_valid(mode in 0u8..=3u8) {
        let data = format!("\x1b[{}J", mode);
        let actions = collect_actions(data.as_bytes());

        prop_assert_eq!(actions.len(), 1);
        if let Action::EraseInDisplay(m) = &actions[0] {
            prop_assert_eq!(*m, mode);
        } else {
            prop_assert!(false, "Expected EraseInDisplay action");
        }
    }

    #[test]
    fn parser_state_resets_on_invalid_sequence(
        garbage in prop::collection::vec(0x80u8..=0xFFu8, 1..20)
    ) {
        let mut parser = AnsiParser::new();
        parser.feed(&garbage, |_| {});
        parser.feed(b"\x1b[1m", |_| {});
        let mut actions = Vec::new();
        parser.feed(b"test", |a| actions.push(a));
        let print_count = actions.iter().filter(|a| matches!(a, Action::Print(_))).count();
        prop_assert_eq!(print_count, 4, "Should print 4 characters after reset");
    }

    #[test]
    fn incremental_feeding_same_as_bulk(data in prop::collection::vec(any::<u8>(), 0..500)) {
        let bulk_actions = collect_actions(&data);

        let mut parser = AnsiParser::new();
        let mut incremental_actions = Vec::new();
        for &byte in &data {
            parser.feed(&[byte], |action| incremental_actions.push(action));
        }

        prop_assert_eq!(bulk_actions, incremental_actions,
            "Incremental and bulk feeding should produce same results");
    }
}

#[test]
fn known_sequences_parse_correctly() {
    let test_cases = vec![
        (
            b"\x1b[H".as_slice(),
            Action::CursorPosition { row: 1, col: 1 },
        ),
        (b"\x1b[2J", Action::EraseInDisplay(2)),
        (b"\x1b[K", Action::EraseInLine(0)),
        (b"\x1b[1m", Action::SetAttribute(Attribute::Bold)),
        (b"\x1b[0m", Action::ResetAttributes),
        (b"\x1b[?25h", Action::SetMode(Mode::CursorVisible)),
        (b"\x1b[?25l", Action::ResetMode(Mode::CursorVisible)),
        (
            b"\x1b[?1049h",
            Action::SetMode(Mode::AlternateScreenSaveCursor),
        ),
        (
            b"\x1b[?1049l",
            Action::ResetMode(Mode::AlternateScreenSaveCursor),
        ),
        (b"\x1b7", Action::SaveCursorPosition),
        (b"\x1b8", Action::RestoreCursorPosition),
    ];

    for (input, expected) in test_cases {
        let actions = collect_actions(input);
        assert!(
            actions.contains(&expected),
            "Input {:?} should produce {:?}, got {:?}",
            String::from_utf8_lossy(input),
            expected,
            actions
        );
    }
}

#[test]
fn named_colors_parse_correctly() {
    let test_cases = vec![
        (30, NamedColor::Black),
        (31, NamedColor::Red),
        (32, NamedColor::Green),
        (33, NamedColor::Yellow),
        (34, NamedColor::Blue),
        (35, NamedColor::Magenta),
        (36, NamedColor::Cyan),
        (37, NamedColor::White),
    ];

    for (code, expected_color) in test_cases {
        let data = format!("\x1b[{}m", code);
        let actions = collect_actions(data.as_bytes());
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::SetAttribute(Attribute::ForegroundColor(Color::Named(c))) => {
                assert_eq!(*c, expected_color);
            }
            other => panic!("Expected ForegroundColor, got {:?}", other),
        }
    }
}

#[test]
fn osc_title_parsing() {
    let data = b"\x1b]0;My Terminal Title\x07";
    let actions = collect_actions(data);

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::SetTitle(title) => {
            assert_eq!(title, "My Terminal Title");
        }
        other => panic!("Expected SetTitle, got {:?}", other),
    }
}

#[test]
fn control_codes_produce_actions() {
    let data = b"\x07\x08\x09\x0a\x0d";
    let actions = collect_actions(data);

    assert!(actions.contains(&Action::Bell));
    assert!(actions.contains(&Action::Backspace));
    assert!(actions.contains(&Action::Tab));
    assert!(actions.contains(&Action::Newline));
    assert!(actions.contains(&Action::CarriageReturn));
}

#[test]
fn invalid_utf8_does_not_crash() {
    let invalid_sequences = vec![
        vec![0x80],
        vec![0xC0, 0x80],
        vec![0xE0, 0x80, 0x80],
        vec![0xF0, 0x80, 0x80, 0x80],
        vec![0xFF],
        vec![0xFE],
        vec![0xC2],
        vec![0xE0, 0xA0],
        vec![0xF0, 0x90, 0x80],
    ];

    for seq in invalid_sequences {
        let mut parser = AnsiParser::new();
        parser.feed(&seq, |_| {});
    }
}

#[test]
fn very_long_osc_does_not_crash() {
    let mut data = vec![0x1b, b']', b'0', b';'];
    data.extend(std::iter::repeat_n(b'x', 100_000));
    data.push(0x07);

    let mut parser = AnsiParser::new();
    parser.feed(&data, |_| {});
}

#[test]
fn very_long_csi_params_does_not_crash() {
    let mut data = vec![0x1b, b'['];
    for i in 0..1000 {
        if i > 0 {
            data.push(b';');
        }
        data.extend(format!("{}", i % 100).as_bytes());
    }
    data.push(b'm');

    let mut parser = AnsiParser::new();
    parser.feed(&data, |_| {});
}

#[test]
fn deeply_nested_escapes_handled() {
    let mut data = Vec::new();
    data.extend(std::iter::repeat_n(0x1b, 100));
    data.push(b'[');
    data.push(b'1');
    data.push(b'm');

    let mut parser = AnsiParser::new();
    parser.feed(&data, |_| {});
}
