//! Reference tests for the VT100/ANSI escape sequence parser.
//!
//! These tests feed raw escape sequences to the parser and verify
//! it produces the correct sequence of actions. Tests are data-driven
//! using JSON files in the `tests/ref/` directory.
//!
//! # Test Format
//!
//! Each test directory contains:
//! - `input.bin` - Raw bytes to feed to the parser
//! - `expected.json` - Expected actions as JSON array
//!
//! # Running Tests
//!
//! ```bash
//! cargo test -p ultimate-terminal --test ref
//! ```

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Actions the ANSI parser can emit.
///
/// This enum represents all terminal control operations that
/// escape sequences can trigger. The parser converts raw bytes
/// into a sequence of these actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params")]
pub enum Action {
    /// Print a character to the current cursor position
    Print(char),

    /// Execute a C0 control code (0x00-0x1F)
    Execute(u8),

    // === Cursor Movement ===
    /// Move cursor up N lines (CSI A)
    CursorUp(u16),

    /// Move cursor down N lines (CSI B)
    CursorDown(u16),

    /// Move cursor right N columns (CSI C)
    CursorForward(u16),

    /// Move cursor left N columns (CSI D)
    CursorBack(u16),

    /// Move cursor to next line, N lines down (CSI E)
    CursorNextLine(u16),

    /// Move cursor to previous line, N lines up (CSI F)
    CursorPreviousLine(u16),

    /// Move cursor to column N (CSI G)
    CursorHorizontalAbsolute(u16),

    /// Move cursor to row;column (CSI H or CSI f)
    CursorPosition { row: u16, col: u16 },

    // === Erase Operations ===
    /// Erase in display (CSI J)
    /// 0 = cursor to end, 1 = start to cursor, 2 = entire, 3 = entire + scrollback
    EraseInDisplay(u8),

    /// Erase in line (CSI K)
    /// 0 = cursor to end, 1 = start to cursor, 2 = entire line
    EraseInLine(u8),

    /// Erase N characters from cursor (CSI X)
    EraseCharacters(u16),

    // === Line Operations ===
    /// Insert N blank lines (CSI L)
    InsertLines(u16),

    /// Delete N lines (CSI M)
    DeleteLines(u16),

    /// Delete N characters (CSI P)
    DeleteCharacters(u16),

    /// Insert N blank characters (CSI @)
    InsertCharacters(u16),

    // === Scrolling ===
    /// Scroll up N lines (CSI S)
    ScrollUp(u16),

    /// Scroll down N lines (CSI T)
    ScrollDown(u16),

    /// Set scrolling region (CSI r)
    SetScrollingRegion { top: u16, bottom: u16 },

    // === Cursor Save/Restore ===
    /// Save cursor position (ESC 7 or CSI s)
    SaveCursorPosition,

    /// Restore cursor position (ESC 8 or CSI u)
    RestoreCursorPosition,

    // === Text Attributes (SGR) ===
    /// Set graphic rendition (CSI m)
    SetAttribute(Attribute),

    /// Reset all attributes to default (CSI 0 m)
    ResetAttributes,

    // === Modes ===
    /// Set mode (CSI h)
    SetMode(Mode),

    /// Reset mode (CSI l)
    ResetMode(Mode),

    // === Tabs ===
    /// Horizontal tab (0x09)
    Tab,

    /// Set tab stop at current column (ESC H)
    SetTabStop,

    /// Clear tab stop (CSI g)
    ClearTabStop(u8),

    // === Terminal Queries ===
    /// Report cursor position (CSI 6 n)
    ReportCursorPosition,

    /// Device attributes request (CSI c)
    DeviceAttributes,

    // === Bell ===
    /// Bell character (0x07)
    Bell,

    // === Newline/Carriage Return ===
    /// Newline (0x0A, 0x0B, 0x0C)
    Newline,

    /// Carriage return (0x0D)
    CarriageReturn,

    /// Backspace (0x08)
    Backspace,

    // === Screen Modes ===
    /// Switch to alternate screen buffer (CSI ?1049h)
    EnterAlternateScreen,

    /// Switch back to main screen buffer (CSI ?1049l)
    LeaveAlternateScreen,
}

/// Text attributes settable via SGR sequences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "attr", content = "value")]
pub enum Attribute {
    Bold,
    Dim,
    Italic,
    Underline,
    Blink,
    Reverse,
    Hidden,
    Strikethrough,

    // Foreground colors
    ForegroundColor(Color),

    // Background colors
    BackgroundColor(Color),

    // Disable specific attributes
    NoBold,
    NoItalic,
    NoUnderline,
    NoBlink,
    NoReverse,
    NoHidden,
    NoStrikethrough,
}

/// Color values for text attributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Color {
    /// Standard 8 colors (0-7)
    Named(NamedColor),
    /// 256-color palette index
    Indexed(u8),
    /// True color RGB
    Rgb { r: u8, g: u8, b: u8 },
    /// Default terminal color
    Default,
}

/// Named terminal colors (the standard 8/16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

/// Terminal modes that can be set/reset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Cursor visibility (CSI ?25h/l)
    CursorVisible,
    /// Application cursor keys (CSI ?1h/l)
    ApplicationCursorKeys,
    /// Bracketed paste (CSI ?2004h/l)
    BracketedPaste,
    /// Focus reporting (CSI ?1004h/l)
    FocusReporting,
    /// Mouse tracking modes
    MouseTracking,
    /// Origin mode (CSI ?6h/l)
    OriginMode,
    /// Auto-wrap mode (CSI ?7h/l)
    AutoWrap,
}

/// Expected test result.
#[derive(Debug, Deserialize)]
pub struct ExpectedResult {
    pub actions: Vec<Action>,
}

/// Macro to generate test functions for each reference test.
macro_rules! ref_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let test_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ref"));
                let test_path = test_dir.join(stringify!($name));
                ref_test(&test_path);
            }
        )*
    };
}

// Register all reference tests
ref_tests! {
    cursor_up,
    cursor_down,
    cursor_forward,
    cursor_back,
    cursor_position,
    erase_display,
    erase_line,
    sgr_bold,
    sgr_colors,
    sgr_reset,
    scroll_up,
    scroll_down,
    save_restore_cursor,
}

/// Run a reference test by loading input and expected output from files.
fn ref_test(dir: &Path) {
    // Load input bytes
    let input_path = dir.join("input.bin");
    let input = fs::read(&input_path)
        .unwrap_or_else(|e| panic!("Failed to read input file {}: {}", input_path.display(), e));

    // Load expected actions
    let expected_path = dir.join("expected.json");
    let expected_json = fs::read_to_string(&expected_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read expected file {}: {}",
            expected_path.display(),
            e
        )
    });
    let expected: ExpectedResult = serde_json::from_str(&expected_json).unwrap_or_else(|e| {
        panic!(
            "Failed to parse expected JSON {}: {}",
            expected_path.display(),
            e
        )
    });

    // Parse input and collect actions
    let actions = parse_and_collect(&input);

    // Compare
    if actions != expected.actions {
        eprintln!("=== Input bytes ===");
        eprintln!("{:?}", input);
        eprintln!("\n=== Expected actions ===");
        for (i, action) in expected.actions.iter().enumerate() {
            eprintln!("[{}] {:?}", i, action);
        }
        eprintln!("\n=== Actual actions ===");
        for (i, action) in actions.iter().enumerate() {
            eprintln!("[{}] {:?}", i, action);
        }
        panic!(
            "Reference test failed: {} actions expected, {} actual",
            expected.actions.len(),
            actions.len()
        );
    }
}

/// Parse bytes and collect resulting actions.
///
/// This is a stub implementation that needs to be replaced with
/// actual parser integration once the parser is implemented.
fn parse_and_collect(input: &[u8]) -> Vec<Action> {
    // TODO: Replace with actual parser when implemented
    //
    // For now, this is a minimal inline parser that demonstrates
    // the expected behavior. This will be replaced by AnsiParser.
    let mut actions = Vec::new();
    let mut i = 0;

    while i < input.len() {
        let byte = input[i];

        match byte {
            // ESC - start of escape sequence
            0x1B => {
                i += 1;
                if i >= input.len() {
                    break;
                }
                match input[i] {
                    // CSI - Control Sequence Introducer
                    b'[' => {
                        i += 1;
                        let (action, consumed) = parse_csi(&input[i..]);
                        if let Some(a) = action {
                            actions.push(a);
                        }
                        i += consumed;
                    }
                    // Save cursor (ESC 7)
                    b'7' => {
                        actions.push(Action::SaveCursorPosition);
                        i += 1;
                    }
                    // Restore cursor (ESC 8)
                    b'8' => {
                        actions.push(Action::RestoreCursorPosition);
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            // C0 control codes
            0x07 => {
                actions.push(Action::Bell);
                i += 1;
            }
            0x08 => {
                actions.push(Action::Backspace);
                i += 1;
            }
            0x09 => {
                actions.push(Action::Tab);
                i += 1;
            }
            0x0A | 0x0B | 0x0C => {
                actions.push(Action::Newline);
                i += 1;
            }
            0x0D => {
                actions.push(Action::CarriageReturn);
                i += 1;
            }
            // Printable ASCII
            0x20..=0x7E => {
                actions.push(Action::Print(byte as char));
                i += 1;
            }
            _ => i += 1,
        }
    }

    actions
}

/// Parse a CSI sequence, returning the action and number of bytes consumed.
fn parse_csi(input: &[u8]) -> (Option<Action>, usize) {
    let mut params: Vec<u16> = Vec::new();
    let mut current_param: Option<u16> = None;
    let mut i = 0;
    let mut is_private = false;

    // Check for private mode indicator
    if i < input.len() && input[i] == b'?' {
        is_private = true;
        i += 1;
    }

    // Parse parameters
    while i < input.len() {
        match input[i] {
            b'0'..=b'9' => {
                let digit = (input[i] - b'0') as u16;
                current_param = Some(current_param.unwrap_or(0) * 10 + digit);
                i += 1;
            }
            b';' => {
                params.push(current_param.unwrap_or(0));
                current_param = None;
                i += 1;
            }
            // Final byte - determines the command
            b'@'..=b'~' => {
                if let Some(p) = current_param {
                    params.push(p);
                }
                let cmd = input[i];
                i += 1;
                let action = match_csi_command(cmd, &params, is_private);
                return (action, i);
            }
            _ => {
                i += 1;
                return (None, i);
            }
        }
    }

    (None, i)
}

/// Match a CSI final byte to an action.
fn match_csi_command(cmd: u8, params: &[u16], is_private: bool) -> Option<Action> {
    let p1 = params.first().copied().unwrap_or(1);
    let p2 = params.get(1).copied().unwrap_or(1);

    match cmd {
        // Cursor movement
        b'A' => Some(Action::CursorUp(p1.max(1))),
        b'B' => Some(Action::CursorDown(p1.max(1))),
        b'C' => Some(Action::CursorForward(p1.max(1))),
        b'D' => Some(Action::CursorBack(p1.max(1))),
        b'E' => Some(Action::CursorNextLine(p1.max(1))),
        b'F' => Some(Action::CursorPreviousLine(p1.max(1))),
        b'G' => Some(Action::CursorHorizontalAbsolute(p1.max(1))),
        b'H' | b'f' => Some(Action::CursorPosition {
            row: p1.max(1),
            col: p2.max(1),
        }),

        // Erase operations (default to 0, not 1)
        b'J' => Some(Action::EraseInDisplay(
            params.first().copied().unwrap_or(0).min(3) as u8,
        )),
        b'K' => Some(Action::EraseInLine(
            params.first().copied().unwrap_or(0).min(2) as u8,
        )),
        b'X' => Some(Action::EraseCharacters(p1.max(1))),

        // Line operations
        b'L' => Some(Action::InsertLines(p1.max(1))),
        b'M' => Some(Action::DeleteLines(p1.max(1))),
        b'P' => Some(Action::DeleteCharacters(p1.max(1))),
        b'@' => Some(Action::InsertCharacters(p1.max(1))),

        // Scrolling
        b'S' => Some(Action::ScrollUp(p1.max(1))),
        b'T' => Some(Action::ScrollDown(p1.max(1))),
        b'r' => Some(Action::SetScrollingRegion {
            top: p1,
            bottom: p2,
        }),

        // Cursor save/restore
        b's' => Some(Action::SaveCursorPosition),
        b'u' => Some(Action::RestoreCursorPosition),

        // SGR (text attributes)
        b'm' => parse_sgr(params),

        // Mode set/reset
        b'h' if is_private => match p1 {
            25 => Some(Action::SetMode(Mode::CursorVisible)),
            1 => Some(Action::SetMode(Mode::ApplicationCursorKeys)),
            1049 => Some(Action::EnterAlternateScreen),
            2004 => Some(Action::SetMode(Mode::BracketedPaste)),
            _ => None,
        },
        b'l' if is_private => match p1 {
            25 => Some(Action::ResetMode(Mode::CursorVisible)),
            1 => Some(Action::ResetMode(Mode::ApplicationCursorKeys)),
            1049 => Some(Action::LeaveAlternateScreen),
            2004 => Some(Action::ResetMode(Mode::BracketedPaste)),
            _ => None,
        },

        _ => None,
    }
}

/// Parse SGR (Select Graphic Rendition) parameters.
fn parse_sgr(params: &[u16]) -> Option<Action> {
    if params.is_empty() || params == [0] {
        return Some(Action::ResetAttributes);
    }

    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        match p {
            0 => return Some(Action::ResetAttributes),
            1 => return Some(Action::SetAttribute(Attribute::Bold)),
            2 => return Some(Action::SetAttribute(Attribute::Dim)),
            3 => return Some(Action::SetAttribute(Attribute::Italic)),
            4 => return Some(Action::SetAttribute(Attribute::Underline)),
            5 => return Some(Action::SetAttribute(Attribute::Blink)),
            7 => return Some(Action::SetAttribute(Attribute::Reverse)),
            8 => return Some(Action::SetAttribute(Attribute::Hidden)),
            9 => return Some(Action::SetAttribute(Attribute::Strikethrough)),
            22 => return Some(Action::SetAttribute(Attribute::NoBold)),
            23 => return Some(Action::SetAttribute(Attribute::NoItalic)),
            24 => return Some(Action::SetAttribute(Attribute::NoUnderline)),
            25 => return Some(Action::SetAttribute(Attribute::NoBlink)),
            27 => return Some(Action::SetAttribute(Attribute::NoReverse)),
            28 => return Some(Action::SetAttribute(Attribute::NoHidden)),
            29 => return Some(Action::SetAttribute(Attribute::NoStrikethrough)),

            // Foreground colors
            30 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Black),
                )))
            }
            31 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Red),
                )))
            }
            32 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Green),
                )))
            }
            33 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Yellow),
                )))
            }
            34 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Blue),
                )))
            }
            35 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Magenta),
                )))
            }
            36 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::Cyan),
                )))
            }
            37 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(NamedColor::White),
                )))
            }
            38 => {
                // Extended foreground color
                if i + 2 < params.len() && params[i + 1] == 5 {
                    // 256-color: ESC[38;5;Nm
                    let idx = params[i + 2] as u8;
                    return Some(Action::SetAttribute(Attribute::ForegroundColor(
                        Color::Indexed(idx),
                    )));
                } else if i + 4 < params.len() && params[i + 1] == 2 {
                    // True color: ESC[38;2;R;G;Bm
                    let r = params[i + 2] as u8;
                    let g = params[i + 3] as u8;
                    let b = params[i + 4] as u8;
                    return Some(Action::SetAttribute(Attribute::ForegroundColor(
                        Color::Rgb { r, g, b },
                    )));
                }
            }
            39 => {
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Default,
                )))
            }

            // Background colors
            40 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Black),
                )))
            }
            41 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Red),
                )))
            }
            42 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Green),
                )))
            }
            43 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Yellow),
                )))
            }
            44 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Blue),
                )))
            }
            45 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Magenta),
                )))
            }
            46 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::Cyan),
                )))
            }
            47 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(NamedColor::White),
                )))
            }
            48 => {
                // Extended background color
                if i + 2 < params.len() && params[i + 1] == 5 {
                    let idx = params[i + 2] as u8;
                    return Some(Action::SetAttribute(Attribute::BackgroundColor(
                        Color::Indexed(idx),
                    )));
                } else if i + 4 < params.len() && params[i + 1] == 2 {
                    let r = params[i + 2] as u8;
                    let g = params[i + 3] as u8;
                    let b = params[i + 4] as u8;
                    return Some(Action::SetAttribute(Attribute::BackgroundColor(
                        Color::Rgb { r, g, b },
                    )));
                }
            }
            49 => {
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Default,
                )))
            }

            // Bright foreground colors
            90..=97 => {
                let color = match p {
                    90 => NamedColor::BrightBlack,
                    91 => NamedColor::BrightRed,
                    92 => NamedColor::BrightGreen,
                    93 => NamedColor::BrightYellow,
                    94 => NamedColor::BrightBlue,
                    95 => NamedColor::BrightMagenta,
                    96 => NamedColor::BrightCyan,
                    97 => NamedColor::BrightWhite,
                    _ => unreachable!(),
                };
                return Some(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Named(color),
                )));
            }

            // Bright background colors
            100..=107 => {
                let color = match p {
                    100 => NamedColor::BrightBlack,
                    101 => NamedColor::BrightRed,
                    102 => NamedColor::BrightGreen,
                    103 => NamedColor::BrightYellow,
                    104 => NamedColor::BrightBlue,
                    105 => NamedColor::BrightMagenta,
                    106 => NamedColor::BrightCyan,
                    107 => NamedColor::BrightWhite,
                    _ => unreachable!(),
                };
                return Some(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Named(color),
                )));
            }

            _ => {}
        }
        i += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the inline parser handles basic cursor movement.
    #[test]
    fn inline_cursor_up() {
        let input = b"\x1b[5A";
        let actions = parse_and_collect(input);
        assert_eq!(actions, vec![Action::CursorUp(5)]);
    }

    /// Test that SGR bold is parsed correctly.
    #[test]
    fn inline_sgr_bold() {
        let input = b"\x1b[1m";
        let actions = parse_and_collect(input);
        assert_eq!(actions, vec![Action::SetAttribute(Attribute::Bold)]);
    }

    /// Test that SGR reset is parsed correctly.
    #[test]
    fn inline_sgr_reset() {
        let input = b"\x1b[0m";
        let actions = parse_and_collect(input);
        assert_eq!(actions, vec![Action::ResetAttributes]);
    }

    /// Test print characters.
    #[test]
    fn inline_print() {
        let input = b"Hello";
        let actions = parse_and_collect(input);
        assert_eq!(
            actions,
            vec![
                Action::Print('H'),
                Action::Print('e'),
                Action::Print('l'),
                Action::Print('l'),
                Action::Print('o'),
            ]
        );
    }
}
