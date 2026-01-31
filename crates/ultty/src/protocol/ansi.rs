//! ANSI/VT100 escape sequence parser for legacy applications.
//!
//! This parser converts raw terminal escape sequences into structured
//! [`Action`] values that can be processed by the terminal emulator.
//!
//! # Example
//!
//! ```rust
//! use ultty::protocol::ansi::{AnsiParser, Action};
//!
//! let mut parser = AnsiParser::new();
//! let mut actions = Vec::new();
//!
//! parser.feed(b"\x1b[1;1H", |action| actions.push(action));
//!
//! assert_eq!(actions.len(), 1);
//! ```

use serde::{Deserialize, Serialize};

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

/// Parser state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Normal state, processing printable characters
    Ground,
    /// Saw ESC, waiting for next byte
    Escape,
    /// In CSI sequence (ESC [), collecting parameters
    CsiEntry,
    /// In CSI sequence with private marker (ESC [ ?)
    CsiPrivate,
    /// Collecting CSI parameters (digits and semicolons)
    CsiParam,
    /// In OSC sequence (ESC ])
    OscString,
    /// In DCS sequence (ESC P)
    DcsString,
}

/// ANSI parser state machine.
///
/// The parser processes bytes incrementally and emits [`Action`] values
/// through a callback. It maintains internal state to handle multi-byte
/// escape sequences.
pub struct AnsiParser {
    state: State,
    params: Vec<u16>,
    current_param: Option<u16>,
    is_private: bool,
    osc_buffer: Vec<u8>,
}

impl AnsiParser {
    /// Create a new ANSI parser.
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: Vec::with_capacity(16),
            current_param: None,
            is_private: false,
            osc_buffer: Vec::new(),
        }
    }

    /// Feed bytes into the parser.
    ///
    /// The parser will call the provided callback for each complete action
    /// it recognizes. Incomplete sequences are buffered internally.
    pub fn feed<F>(&mut self, data: &[u8], mut callback: F)
    where
        F: FnMut(Action),
    {
        for &byte in data {
            self.process_byte(byte, &mut callback);
        }
    }

    /// Process a single byte.
    fn process_byte<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match self.state {
            State::Ground => self.ground(byte, callback),
            State::Escape => self.escape(byte, callback),
            State::CsiEntry => self.csi_entry(byte, callback),
            State::CsiPrivate | State::CsiParam => self.csi_param(byte, callback),
            State::OscString => self.osc_string(byte, callback),
            State::DcsString => self.dcs_string(byte, callback),
        }
    }

    /// Reset parser to ground state.
    fn reset(&mut self) {
        self.state = State::Ground;
        self.params.clear();
        self.current_param = None;
        self.is_private = false;
        self.osc_buffer.clear();
    }

    /// Ground state - normal character processing.
    fn ground<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // ESC - start escape sequence
            0x1B => {
                self.state = State::Escape;
            }
            // C0 control codes
            0x07 => callback(Action::Bell),
            0x08 => callback(Action::Backspace),
            0x09 => callback(Action::Tab),
            0x0A | 0x0B | 0x0C => callback(Action::Newline),
            0x0D => callback(Action::CarriageReturn),
            // Other C0 controls
            0x00..=0x1F => callback(Action::Execute(byte)),
            // Printable ASCII
            0x20..=0x7E => callback(Action::Print(byte as char)),
            // DEL - ignore
            0x7F => {}
            // High bytes - could be UTF-8, for now treat as printable
            0x80..=0xFF => {
                // TODO: Proper UTF-8 handling
                callback(Action::Print(byte as char));
            }
        }
    }

    /// Escape state - just saw ESC.
    fn escape<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // CSI - Control Sequence Introducer
            b'[' => {
                self.state = State::CsiEntry;
                self.params.clear();
                self.current_param = None;
                self.is_private = false;
            }
            // OSC - Operating System Command
            b']' => {
                self.state = State::OscString;
                self.osc_buffer.clear();
            }
            // DCS - Device Control String
            b'P' => {
                self.state = State::DcsString;
            }
            // Save cursor (ESC 7)
            b'7' => {
                callback(Action::SaveCursorPosition);
                self.reset();
            }
            // Restore cursor (ESC 8)
            b'8' => {
                callback(Action::RestoreCursorPosition);
                self.reset();
            }
            // Set tab stop (ESC H)
            b'H' => {
                callback(Action::SetTabStop);
                self.reset();
            }
            // Another ESC - restart escape
            0x1B => {
                // Stay in escape state
            }
            // Anything else - abort and return to ground
            _ => {
                self.reset();
            }
        }
    }

    /// CSI entry - just entered CSI sequence.
    fn csi_entry<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Private mode indicator
            b'?' => {
                self.is_private = true;
                self.state = State::CsiPrivate;
            }
            // Digit - start parameter
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u16;
                self.current_param = Some(digit);
                self.state = State::CsiParam;
            }
            // Semicolon with no preceding digit
            b';' => {
                self.params.push(0);
                self.state = State::CsiParam;
            }
            // Final byte - no parameters
            b'@'..=b'~' => {
                self.dispatch_csi(byte, callback);
                self.reset();
            }
            // ESC - abort and restart
            0x1B => {
                self.state = State::Escape;
            }
            // Intermediate bytes - we don't handle these yet
            0x20..=0x2F => {
                // Skip intermediates for now
            }
            _ => {
                self.reset();
            }
        }
    }

    /// CSI param - collecting parameters.
    fn csi_param<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Digit - accumulate parameter
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u16;
                self.current_param = Some(self.current_param.unwrap_or(0) * 10 + digit);
            }
            // Semicolon - end current parameter
            b';' => {
                self.params.push(self.current_param.unwrap_or(0));
                self.current_param = None;
            }
            // Final byte - dispatch
            b'@'..=b'~' => {
                if let Some(p) = self.current_param {
                    self.params.push(p);
                }
                self.dispatch_csi(byte, callback);
                self.reset();
            }
            // ESC - abort and restart
            0x1B => {
                self.state = State::Escape;
            }
            // Intermediate bytes
            0x20..=0x2F => {
                // Skip for now
            }
            _ => {
                self.reset();
            }
        }
    }

    /// Dispatch a complete CSI sequence.
    fn dispatch_csi<F>(&mut self, cmd: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        let p1 = self.params.first().copied().unwrap_or(1);
        let p2 = self.params.get(1).copied().unwrap_or(1);

        match cmd {
            // Cursor movement
            b'A' => callback(Action::CursorUp(p1.max(1))),
            b'B' => callback(Action::CursorDown(p1.max(1))),
            b'C' => callback(Action::CursorForward(p1.max(1))),
            b'D' => callback(Action::CursorBack(p1.max(1))),
            b'E' => callback(Action::CursorNextLine(p1.max(1))),
            b'F' => callback(Action::CursorPreviousLine(p1.max(1))),
            b'G' => callback(Action::CursorHorizontalAbsolute(p1.max(1))),
            b'H' | b'f' => callback(Action::CursorPosition {
                row: p1.max(1),
                col: p2.max(1),
            }),

            // Erase operations (default to 0, not 1)
            b'J' => callback(Action::EraseInDisplay(
                self.params.first().copied().unwrap_or(0).min(3) as u8,
            )),
            b'K' => callback(Action::EraseInLine(
                self.params.first().copied().unwrap_or(0).min(2) as u8,
            )),
            b'X' => callback(Action::EraseCharacters(p1.max(1))),

            // Line operations
            b'L' => callback(Action::InsertLines(p1.max(1))),
            b'M' => callback(Action::DeleteLines(p1.max(1))),
            b'P' => callback(Action::DeleteCharacters(p1.max(1))),
            b'@' => callback(Action::InsertCharacters(p1.max(1))),

            // Scrolling
            b'S' => callback(Action::ScrollUp(p1.max(1))),
            b'T' => callback(Action::ScrollDown(p1.max(1))),
            b'r' => callback(Action::SetScrollingRegion {
                top: p1,
                bottom: p2,
            }),

            // Cursor save/restore
            b's' => callback(Action::SaveCursorPosition),
            b'u' => callback(Action::RestoreCursorPosition),

            // SGR (text attributes)
            b'm' => self.dispatch_sgr(callback),

            // Tab clear
            b'g' => callback(Action::ClearTabStop(
                self.params.first().copied().unwrap_or(0) as u8,
            )),

            // Cursor position report
            b'n' if p1 == 6 => callback(Action::ReportCursorPosition),

            // Device attributes
            b'c' => callback(Action::DeviceAttributes),

            // Mode set/reset
            b'h' if self.is_private => {
                if let Some(action) = self.private_mode_set(p1) {
                    callback(action);
                }
            }
            b'l' if self.is_private => {
                if let Some(action) = self.private_mode_reset(p1) {
                    callback(action);
                }
            }

            _ => {
                // Unknown CSI sequence - ignore
            }
        }
    }

    /// Handle private mode set (CSI ? ... h).
    fn private_mode_set(&self, param: u16) -> Option<Action> {
        match param {
            1 => Some(Action::SetMode(Mode::ApplicationCursorKeys)),
            6 => Some(Action::SetMode(Mode::OriginMode)),
            7 => Some(Action::SetMode(Mode::AutoWrap)),
            25 => Some(Action::SetMode(Mode::CursorVisible)),
            1004 => Some(Action::SetMode(Mode::FocusReporting)),
            1049 => Some(Action::EnterAlternateScreen),
            2004 => Some(Action::SetMode(Mode::BracketedPaste)),
            _ => None,
        }
    }

    /// Handle private mode reset (CSI ? ... l).
    fn private_mode_reset(&self, param: u16) -> Option<Action> {
        match param {
            1 => Some(Action::ResetMode(Mode::ApplicationCursorKeys)),
            6 => Some(Action::ResetMode(Mode::OriginMode)),
            7 => Some(Action::ResetMode(Mode::AutoWrap)),
            25 => Some(Action::ResetMode(Mode::CursorVisible)),
            1004 => Some(Action::ResetMode(Mode::FocusReporting)),
            1049 => Some(Action::LeaveAlternateScreen),
            2004 => Some(Action::ResetMode(Mode::BracketedPaste)),
            _ => None,
        }
    }

    /// Dispatch SGR (Select Graphic Rendition) sequence.
    fn dispatch_sgr<F>(&self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.params.is_empty() || self.params == [0] {
            callback(Action::ResetAttributes);
            return;
        }

        let mut i = 0;
        while i < self.params.len() {
            let p = self.params[i];
            match p {
                0 => callback(Action::ResetAttributes),
                1 => callback(Action::SetAttribute(Attribute::Bold)),
                2 => callback(Action::SetAttribute(Attribute::Dim)),
                3 => callback(Action::SetAttribute(Attribute::Italic)),
                4 => callback(Action::SetAttribute(Attribute::Underline)),
                5 => callback(Action::SetAttribute(Attribute::Blink)),
                7 => callback(Action::SetAttribute(Attribute::Reverse)),
                8 => callback(Action::SetAttribute(Attribute::Hidden)),
                9 => callback(Action::SetAttribute(Attribute::Strikethrough)),
                22 => callback(Action::SetAttribute(Attribute::NoBold)),
                23 => callback(Action::SetAttribute(Attribute::NoItalic)),
                24 => callback(Action::SetAttribute(Attribute::NoUnderline)),
                25 => callback(Action::SetAttribute(Attribute::NoBlink)),
                27 => callback(Action::SetAttribute(Attribute::NoReverse)),
                28 => callback(Action::SetAttribute(Attribute::NoHidden)),
                29 => callback(Action::SetAttribute(Attribute::NoStrikethrough)),

                // Foreground colors
                30..=37 => {
                    let color = Self::named_color_from_index((p - 30) as u8);
                    callback(Action::SetAttribute(Attribute::ForegroundColor(
                        Color::Named(color),
                    )));
                }
                38 => {
                    // Extended foreground color
                    if i + 2 < self.params.len() && self.params[i + 1] == 5 {
                        // 256-color: ESC[38;5;Nm
                        let idx = self.params[i + 2] as u8;
                        callback(Action::SetAttribute(Attribute::ForegroundColor(
                            Color::Indexed(idx),
                        )));
                        i += 2;
                    } else if i + 4 < self.params.len() && self.params[i + 1] == 2 {
                        // True color: ESC[38;2;R;G;Bm
                        let r = self.params[i + 2] as u8;
                        let g = self.params[i + 3] as u8;
                        let b = self.params[i + 4] as u8;
                        callback(Action::SetAttribute(Attribute::ForegroundColor(
                            Color::Rgb { r, g, b },
                        )));
                        i += 4;
                    }
                }
                39 => callback(Action::SetAttribute(Attribute::ForegroundColor(
                    Color::Default,
                ))),

                // Background colors
                40..=47 => {
                    let color = Self::named_color_from_index((p - 40) as u8);
                    callback(Action::SetAttribute(Attribute::BackgroundColor(
                        Color::Named(color),
                    )));
                }
                48 => {
                    // Extended background color
                    if i + 2 < self.params.len() && self.params[i + 1] == 5 {
                        let idx = self.params[i + 2] as u8;
                        callback(Action::SetAttribute(Attribute::BackgroundColor(
                            Color::Indexed(idx),
                        )));
                        i += 2;
                    } else if i + 4 < self.params.len() && self.params[i + 1] == 2 {
                        let r = self.params[i + 2] as u8;
                        let g = self.params[i + 3] as u8;
                        let b = self.params[i + 4] as u8;
                        callback(Action::SetAttribute(Attribute::BackgroundColor(
                            Color::Rgb { r, g, b },
                        )));
                        i += 4;
                    }
                }
                49 => callback(Action::SetAttribute(Attribute::BackgroundColor(
                    Color::Default,
                ))),

                // Bright foreground colors
                90..=97 => {
                    let color = Self::bright_color_from_index((p - 90) as u8);
                    callback(Action::SetAttribute(Attribute::ForegroundColor(
                        Color::Named(color),
                    )));
                }

                // Bright background colors
                100..=107 => {
                    let color = Self::bright_color_from_index((p - 100) as u8);
                    callback(Action::SetAttribute(Attribute::BackgroundColor(
                        Color::Named(color),
                    )));
                }

                _ => {}
            }
            i += 1;
        }
    }

    /// Convert color index (0-7) to NamedColor.
    fn named_color_from_index(idx: u8) -> NamedColor {
        match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            _ => NamedColor::White,
        }
    }

    /// Convert bright color index (0-7) to bright NamedColor.
    fn bright_color_from_index(idx: u8) -> NamedColor {
        match idx {
            0 => NamedColor::BrightBlack,
            1 => NamedColor::BrightRed,
            2 => NamedColor::BrightGreen,
            3 => NamedColor::BrightYellow,
            4 => NamedColor::BrightBlue,
            5 => NamedColor::BrightMagenta,
            6 => NamedColor::BrightCyan,
            7 => NamedColor::BrightWhite,
            _ => NamedColor::BrightWhite,
        }
    }

    /// OSC string state - accumulating OSC content.
    fn osc_string<F>(&mut self, byte: u8, _callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // ST (String Terminator) - ESC \
            0x1B => {
                // Might be start of ST, need to check next byte
                // For now, just reset
                self.reset();
            }
            // BEL also terminates OSC
            0x07 => {
                // TODO: Process OSC content
                self.reset();
            }
            _ => {
                self.osc_buffer.push(byte);
            }
        }
    }

    /// DCS string state.
    fn dcs_string<F>(&mut self, byte: u8, _callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            0x1B => {
                // Might be start of ST
                self.reset();
            }
            _ => {
                // Accumulate DCS content
            }
        }
    }
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(
            parse(b"Hi"),
            vec![Action::Print('H'), Action::Print('i')]
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
        assert_eq!(parse(b"\x1b[?1049h"), vec![Action::EnterAlternateScreen]);
        assert_eq!(parse(b"\x1b[?1049l"), vec![Action::LeaveAlternateScreen]);
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
}
