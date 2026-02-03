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

    /// Reverse Index (ESC M) - move cursor up, scroll down if at top
    ReverseIndex,

    /// Index (ESC D) - move cursor down, scroll up if at bottom
    Index,

    /// Next Line (ESC E) - CR + LF
    NextLine,

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

    /// Device attributes request (CSI c or CSI 0 c) - Primary DA
    DeviceAttributes,

    /// Secondary device attributes request (CSI > c or CSI > 0 c) - DA2
    SecondaryDeviceAttributes,

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

    /// Set cursor style (DECSCUSR - CSI Ps SP q)
    /// 0/1 = blinking block, 2 = steady block, 3 = blinking underline,
    /// 4 = steady underline, 5 = blinking bar, 6 = steady bar
    SetCursorStyle(u8),

    // === OSC Sequences ===
    /// Set window/icon title (OSC 0, 1, 2)
    SetTitle(String),

    /// Set working directory (OSC 7)
    SetWorkingDirectory(String),

    /// Shell integration: prompt start (OSC 133;A)
    PromptStart,

    /// Shell integration: command start (OSC 133;B)
    CommandStart,

    /// Shell integration: command executed (OSC 133;C)
    CommandExecuted,

    /// Shell integration: command finished with exit code (OSC 133;D;exitcode)
    CommandFinished(Option<i32>),

    /// Set clipboard content (OSC 52)
    SetClipboard { clipboard: char, data: String },

    /// Start hyperlink (OSC 8 with URI)
    StartHyperlink(String),

    /// End hyperlink (OSC 8 with empty URI)
    EndHyperlink,

    /// Desktop notification (OSC 9 or OSC 777)
    Notification { title: String, body: String },

    /// Kitty keyboard protocol: push mode with flags (CSI > flags u)
    KittyKeyboardPush(u8),

    /// Kitty keyboard protocol: pop mode (CSI < u)
    KittyKeyboardPop,

    /// Kitty keyboard protocol: query mode (CSI ? u)
    KittyKeyboardQuery,

    /// Sixel image data (DCS q ... ST)
    /// Contains: aspect ratio params and raw sixel data
    SixelImage {
        /// Pixel aspect ratio parameter
        aspect_ratio: u16,
        /// Background color mode (0=device default, 1=no change, 2=set to 0)
        background: u16,
        /// Horizontal grid size
        horizontal_grid: u16,
        /// Raw sixel data bytes
        data: Vec<u8>,
    },

    /// Unknown OSC sequence (for debugging)
    UnknownOsc(String),

    /// Query foreground color (OSC 10 ; ?)
    QueryForegroundColor,

    /// Query background color (OSC 11 ; ?)
    QueryBackgroundColor,

    /// Query cursor color (OSC 12 ; ?)
    QueryCursorColor,

    /// Request DEC private mode status (DECRQM - CSI ? Ps $ p)
    RequestPrivateMode(u16),
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
    /// X10 mouse tracking - button press only (CSI ?9h/l)
    MouseX10,
    /// Normal mouse tracking - press and release (CSI ?1000h/l)
    MouseNormal,
    /// Button event mouse tracking - with motion (CSI ?1002h/l)
    MouseButtonEvent,
    /// Any event mouse tracking - all motion (CSI ?1003h/l)
    MouseAnyEvent,
    /// UTF-8 mouse encoding (CSI ?1005h/l)
    MouseUtf8,
    /// SGR extended mouse encoding (CSI ?1006h/l)
    MouseSgr,
    /// URXVT mouse encoding (CSI ?1015h/l)
    MouseUrxvt,
    /// Origin mode (CSI ?6h/l)
    OriginMode,
    /// Auto-wrap mode (CSI ?7h/l)
    AutoWrap,
    /// Reverse video mode (CSI ?5h/l) - swap fg/bg
    ReverseVideo,
    /// Alternate screen buffer (CSI ?47h/l)
    AlternateScreen,
    /// Alternate screen buffer with clear (CSI ?1047h/l)
    AlternateScreenClear,
    /// Save cursor and switch to alternate screen (CSI ?1049h/l)
    AlternateScreenSaveCursor,
    /// Cursor blink (CSI ?12h/l)
    CursorBlink,
    /// Column mode - 132 vs 80 columns (CSI ?3h/l)
    ColumnMode132,
    /// Alternate scroll mode (CSI ?1007h/l) - scroll alternate screen instead of sending keys
    AlternateScrollMode,
    /// Synchronized update mode (CSI ?2026h/l) - batch updates for flicker-free rendering
    SynchronizedUpdate,
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
    /// Saw ESC while in OSC, expecting \ for ST (String Terminator)
    OscStringEsc,
    /// In DCS sequence (ESC P)
    DcsString,
    /// Saw ESC while in DCS, expecting \ for ST
    DcsStringEsc,
    /// In APC sequence (ESC _) - used by Kitty graphics protocol
    ApcString,
    /// Saw ESC while in APC, expecting \ for ST
    ApcStringEsc,
    /// In PM sequence (ESC ^) - Privacy Message, rarely used
    PmString,
    /// Saw ESC while in PM, expecting \ for ST
    PmStringEsc,
    /// In SOS sequence (ESC X) - Start of String, rarely used
    SosString,
    /// Saw ESC while in SOS, expecting \ for ST
    SosStringEsc,
    /// Character set designation for G0 (ESC ()
    CharsetDesignateG0,
    /// Character set designation for G1 (ESC ))
    CharsetDesignateG1,
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
    /// Intermediate byte for CSI sequences (e.g., space in `CSI Ps SP q`)
    intermediate: Option<u8>,
    osc_buffer: Vec<u8>,
    dcs_buffer: Vec<u8>,
    dcs_params: Vec<u16>,
    /// Buffer for accumulating UTF-8 multi-byte sequences
    utf8_buffer: Vec<u8>,
    /// Number of remaining bytes expected in current UTF-8 sequence
    utf8_remaining: u8,
    /// Active character set for G0 (false = ASCII, true = DEC Special Graphics)
    g0_charset_graphics: bool,
    /// Active character set for G1
    g1_charset_graphics: bool,
    /// Which charset is currently active (false = G0, true = G1)
    using_g1: bool,
}

impl AnsiParser {
    /// Create a new ANSI parser.
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: Vec::with_capacity(16),
            current_param: None,
            is_private: false,
            intermediate: None,
            osc_buffer: Vec::new(),
            dcs_buffer: Vec::new(),
            dcs_params: Vec::new(),
            utf8_buffer: Vec::with_capacity(4),
            utf8_remaining: 0,
            g0_charset_graphics: false,
            g1_charset_graphics: false,
            using_g1: false,
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
            State::OscStringEsc => self.osc_string_esc(byte, callback),
            State::DcsString => self.dcs_string(byte, callback),
            State::DcsStringEsc => self.dcs_string_esc(byte, callback),
            State::ApcString => self.apc_string(byte),
            State::ApcStringEsc => self.apc_string_esc(byte),
            State::PmString => self.ignored_string(byte, State::PmStringEsc),
            State::PmStringEsc => self.ignored_string_esc(byte, State::PmString),
            State::SosString => self.ignored_string(byte, State::SosStringEsc),
            State::SosStringEsc => self.ignored_string_esc(byte, State::SosString),
            State::CharsetDesignateG0 | State::CharsetDesignateG1 => {
                self.charset_designate(byte, callback)
            }
        }
    }

    /// Reset parser to ground state.
    /// Note: charset state is preserved across resets (only cleared on hard reset).
    fn reset(&mut self) {
        self.state = State::Ground;
        self.params.clear();
        self.current_param = None;
        self.is_private = false;
        self.intermediate = None;
        self.osc_buffer.clear();
        self.dcs_buffer.clear();
        self.dcs_params.clear();
        self.utf8_buffer.clear();
        self.utf8_remaining = 0;
        // Note: g0/g1 charset state is intentionally NOT reset here
        // as it persists across escape sequences
    }

    /// Ground state - normal character processing.
    fn ground<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        // If we're in the middle of a UTF-8 sequence, handle continuation
        if self.utf8_remaining > 0 {
            if byte & 0xC0 == 0x80 {
                // Valid continuation byte
                self.utf8_buffer.push(byte);
                self.utf8_remaining -= 1;

                if self.utf8_remaining == 0 {
                    // Complete sequence - decode and emit
                    if let Ok(s) = std::str::from_utf8(&self.utf8_buffer) {
                        for c in s.chars() {
                            callback(Action::Print(c));
                        }
                    }
                    self.utf8_buffer.clear();
                }
                return;
            } else {
                // Invalid continuation - emit replacement char and process this byte normally
                callback(Action::Print('\u{FFFD}'));
                self.utf8_buffer.clear();
                self.utf8_remaining = 0;
                // Fall through to process current byte
            }
        }

        match byte {
            // ESC - start escape sequence
            0x1B => {
                self.state = State::Escape;
            }
            // C0 control codes
            0x07 => callback(Action::Bell),
            0x08 => callback(Action::Backspace),
            0x09 => callback(Action::Tab),
            0x0A..=0x0C => callback(Action::Newline),
            0x0D => callback(Action::CarriageReturn),
            0x0E => {
                self.using_g1 = true;
            }
            0x0F => {
                self.using_g1 = false;
            }
            // Other C0 controls
            0x00..=0x1F => callback(Action::Execute(byte)),
            0x20..=0x7E => {
                let ch = if self.is_graphics_charset_active() {
                    Self::translate_dec_graphics(byte).unwrap_or(byte as char)
                } else {
                    byte as char
                };
                callback(Action::Print(ch));
            }
            // DEL - ignore
            0x7F => {}
            // UTF-8 continuation byte without lead - invalid, emit replacement
            0x80..=0xBF => {
                callback(Action::Print('\u{FFFD}'));
            }
            // UTF-8 2-byte sequence lead (0xC0-0xDF)
            0xC0..=0xDF => {
                self.utf8_buffer.clear();
                self.utf8_buffer.push(byte);
                self.utf8_remaining = 1;
            }
            // UTF-8 3-byte sequence lead (0xE0-0xEF)
            0xE0..=0xEF => {
                self.utf8_buffer.clear();
                self.utf8_buffer.push(byte);
                self.utf8_remaining = 2;
            }
            // UTF-8 4-byte sequence lead (0xF0-0xF7)
            0xF0..=0xF7 => {
                self.utf8_buffer.clear();
                self.utf8_buffer.push(byte);
                self.utf8_remaining = 3;
            }
            // Invalid UTF-8 lead bytes (0xF8-0xFF) - emit replacement
            0xF8..=0xFF => {
                callback(Action::Print('\u{FFFD}'));
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
                self.dcs_buffer.clear();
                self.dcs_params.clear();
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
            // Reverse Index (ESC M) - move cursor up, scroll down if at top of scroll region
            b'M' => {
                callback(Action::ReverseIndex);
                self.reset();
            }
            // Index (ESC D) - move cursor down, scroll up if at bottom of scroll region
            b'D' => {
                callback(Action::Index);
                self.reset();
            }
            // Next Line (ESC E) - move to beginning of next line, scroll if needed
            b'E' => {
                callback(Action::NextLine);
                self.reset();
            }
            // Another ESC - restart escape
            0x1B => {
                // Stay in escape state
            }
            // G0 character set designation (ESC ()
            b'(' => {
                self.state = State::CharsetDesignateG0;
            }
            // G1 character set designation (ESC ))
            b')' => {
                self.state = State::CharsetDesignateG1;
            }
            // APC - Application Program Command (ESC _)
            // Used by Kitty graphics protocol
            b'_' => {
                self.state = State::ApcString;
            }
            // PM - Privacy Message (ESC ^)
            b'^' => {
                self.state = State::PmString;
            }
            // SOS - Start of String (ESC X)
            b'X' => {
                self.state = State::SosString;
            }
            // Anything else - abort and return to ground
            _ => {
                self.reset();
            }
        }
    }

    /// Character set designation (ESC ( X or ESC ) X)
    /// Handles G0/G1 character set selection used by ncurses applications.
    fn charset_designate<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        let _ = callback; // May be used in future for charset change notifications

        // Determine if this is G0 or G1 based on the state we entered from
        let is_g0 = self.state == State::CharsetDesignateG0;

        // Common character set designators:
        // 'B' - ASCII (USASCII)
        // '0' - DEC Special Graphics (line drawing)
        // 'A' - UK National (treated as ASCII)
        // '<' - DEC Supplemental (treated as ASCII)
        match byte {
            b'0' => {
                if is_g0 {
                    self.g0_charset_graphics = true;
                } else {
                    self.g1_charset_graphics = true;
                }
            }
            b'B' | b'A' | b'<' | b'>' | b'1' | b'2' => {
                // ASCII or other charset - disable line drawing
                if is_g0 {
                    self.g0_charset_graphics = false;
                } else {
                    self.g1_charset_graphics = false;
                }
            }
            _ => {
                // Unknown charset designator - treat as ASCII
                tracing::debug!("Unknown charset designator: {}", byte as char);
                if is_g0 {
                    self.g0_charset_graphics = false;
                } else {
                    self.g1_charset_graphics = false;
                }
            }
        }
        self.reset();
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
            // Kitty keyboard protocol prefix indicators
            b'>' | b'<' => {
                // Store as intermediate for Kitty keyboard handling
                self.intermediate = Some(byte);
                self.state = State::CsiParam;
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
            // Intermediate bytes (e.g., space in DECSCUSR)
            0x20..=0x2F => {
                self.intermediate = Some(byte);
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
            // Intermediate bytes (e.g., space in DECSCUSR)
            0x20..=0x2F => {
                self.intermediate = Some(byte);
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
            b'u' => {
                // Check for Kitty keyboard protocol sequences
                match self.intermediate {
                    Some(b'>') => {
                        // CSI > flags u - push keyboard mode
                        let flags = self.params.first().copied().unwrap_or(0) as u8;
                        callback(Action::KittyKeyboardPush(flags));
                    }
                    Some(b'<') => {
                        // CSI < u - pop keyboard mode
                        callback(Action::KittyKeyboardPop);
                    }
                    None if self.is_private => {
                        // CSI ? u - query keyboard mode
                        callback(Action::KittyKeyboardQuery);
                    }
                    _ => {
                        // Regular restore cursor position
                        callback(Action::RestoreCursorPosition);
                    }
                }
            }

            // SGR (text attributes)
            b'm' => self.dispatch_sgr(callback),

            // Tab clear
            b'g' => callback(Action::ClearTabStop(
                self.params.first().copied().unwrap_or(0) as u8,
            )),

            // Cursor position report
            b'n' if p1 == 6 => callback(Action::ReportCursorPosition),

            // Device attributes
            b'c' => {
                if self.intermediate == Some(b'>') {
                    callback(Action::SecondaryDeviceAttributes);
                } else {
                    callback(Action::DeviceAttributes);
                }
            }

            // Cursor style (DECSCUSR) - CSI Ps SP q
            b'q' if self.intermediate == Some(b' ') => {
                let style = self.params.first().copied().unwrap_or(0) as u8;
                callback(Action::SetCursorStyle(style));
            }

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

            // DECRQM - Request DEC Private Mode (CSI ? Ps $ p)
            b'p' if self.is_private && self.intermediate == Some(b'$') => {
                callback(Action::RequestPrivateMode(p1));
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
            3 => Some(Action::SetMode(Mode::ColumnMode132)),
            5 => Some(Action::SetMode(Mode::ReverseVideo)),
            6 => Some(Action::SetMode(Mode::OriginMode)),
            7 => Some(Action::SetMode(Mode::AutoWrap)),
            9 => Some(Action::SetMode(Mode::MouseX10)),
            12 => Some(Action::SetMode(Mode::CursorBlink)),
            25 => Some(Action::SetMode(Mode::CursorVisible)),
            47 => Some(Action::SetMode(Mode::AlternateScreen)),
            1000 => Some(Action::SetMode(Mode::MouseNormal)),
            1002 => Some(Action::SetMode(Mode::MouseButtonEvent)),
            1003 => Some(Action::SetMode(Mode::MouseAnyEvent)),
            1004 => Some(Action::SetMode(Mode::FocusReporting)),
            1005 => Some(Action::SetMode(Mode::MouseUtf8)),
            1006 => Some(Action::SetMode(Mode::MouseSgr)),
            1015 => Some(Action::SetMode(Mode::MouseUrxvt)),
            1007 => Some(Action::SetMode(Mode::AlternateScrollMode)),
            1047 => Some(Action::SetMode(Mode::AlternateScreenClear)),
            1049 => Some(Action::SetMode(Mode::AlternateScreenSaveCursor)),
            2004 => Some(Action::SetMode(Mode::BracketedPaste)),
            2026 => Some(Action::SetMode(Mode::SynchronizedUpdate)),
            _ => None,
        }
    }

    /// Handle private mode reset (CSI ? ... l).
    fn private_mode_reset(&self, param: u16) -> Option<Action> {
        match param {
            1 => Some(Action::ResetMode(Mode::ApplicationCursorKeys)),
            3 => Some(Action::ResetMode(Mode::ColumnMode132)),
            5 => Some(Action::ResetMode(Mode::ReverseVideo)),
            6 => Some(Action::ResetMode(Mode::OriginMode)),
            7 => Some(Action::ResetMode(Mode::AutoWrap)),
            9 => Some(Action::ResetMode(Mode::MouseX10)),
            12 => Some(Action::ResetMode(Mode::CursorBlink)),
            25 => Some(Action::ResetMode(Mode::CursorVisible)),
            47 => Some(Action::ResetMode(Mode::AlternateScreen)),
            1000 => Some(Action::ResetMode(Mode::MouseNormal)),
            1002 => Some(Action::ResetMode(Mode::MouseButtonEvent)),
            1003 => Some(Action::ResetMode(Mode::MouseAnyEvent)),
            1004 => Some(Action::ResetMode(Mode::FocusReporting)),
            1005 => Some(Action::ResetMode(Mode::MouseUtf8)),
            1006 => Some(Action::ResetMode(Mode::MouseSgr)),
            1015 => Some(Action::ResetMode(Mode::MouseUrxvt)),
            1007 => Some(Action::ResetMode(Mode::AlternateScrollMode)),
            1047 => Some(Action::ResetMode(Mode::AlternateScreenClear)),
            1049 => Some(Action::ResetMode(Mode::AlternateScreenSaveCursor)),
            2004 => Some(Action::ResetMode(Mode::BracketedPaste)),
            2026 => Some(Action::ResetMode(Mode::SynchronizedUpdate)),
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
    fn osc_string<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // ESC - might be start of ST (String Terminator = ESC \)
            0x1B => {
                self.state = State::OscStringEsc;
            }
            // BEL also terminates OSC
            0x07 => {
                self.dispatch_osc(callback);
                self.reset();
            }
            _ => {
                self.osc_buffer.push(byte);
            }
        }
    }

    /// OSC string ESC state - waiting for backslash to complete ST.
    fn osc_string_esc<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                self.dispatch_osc(callback);
                self.reset();
            }
            // Any other byte - was not a valid ST, dispatch and reprocess
            _ => {
                self.dispatch_osc(callback);
                self.reset();
                // Reprocess this byte from ground state
                self.process_byte(byte, callback);
            }
        }
    }

    /// Dispatch OSC sequence based on accumulated buffer.
    fn dispatch_osc<F>(&mut self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        let content = String::from_utf8_lossy(&self.osc_buffer);

        // OSC format: code ; data
        if let Some(semicolon_pos) = content.find(';') {
            let code = &content[..semicolon_pos];
            let data = &content[semicolon_pos + 1..];

            match code {
                // Set window title (OSC 0, 1, 2)
                "0" | "1" | "2" => {
                    callback(Action::SetTitle(data.to_string()));
                }
                // Set working directory (OSC 7)
                // Format: file://hostname/path
                "7" => {
                    callback(Action::SetWorkingDirectory(data.to_string()));
                }
                // Clipboard (OSC 52)
                // Format: clipboard;base64data
                "52" => {
                    if let Some(semi_pos) = data.find(';') {
                        let clipboard = data.chars().next().unwrap_or('c');
                        let b64data = &data[semi_pos + 1..];
                        // Decode base64
                        if let Ok(decoded) = base64_decode(b64data) {
                            if let Ok(text) = String::from_utf8(decoded) {
                                callback(Action::SetClipboard {
                                    clipboard,
                                    data: text,
                                });
                            }
                        }
                    }
                }
                // Hyperlinks (OSC 8)
                // Format: 8;params;uri (params typically empty or id=xxx)
                "8" => {
                    // data format: params;uri
                    if let Some(semi_pos) = data.find(';') {
                        let _params = &data[..semi_pos];
                        let uri = &data[semi_pos + 1..];
                        if uri.is_empty() {
                            // End hyperlink
                            callback(Action::EndHyperlink);
                        } else {
                            // Start hyperlink
                            callback(Action::StartHyperlink(uri.to_string()));
                        }
                    }
                }
                // Desktop notification - iTerm2 style (OSC 9)
                "9" => {
                    callback(Action::Notification {
                        title: String::new(),
                        body: data.to_string(),
                    });
                }
                // Shell integration (OSC 133)
                "133" => {
                    self.dispatch_shell_integration(data, callback);
                }
                // Desktop notification - rxvt/urxvt style (OSC 777)
                // Format: 777;notify;title;body
                "777" => {
                    let parts: Vec<&str> = data.splitn(3, ';').collect();
                    if parts.first() == Some(&"notify") {
                        let title = parts.get(1).unwrap_or(&"").to_string();
                        let body = parts.get(2).unwrap_or(&"").to_string();
                        callback(Action::Notification { title, body });
                    }
                }
                // Foreground color query/set (OSC 10)
                "10" => {
                    if data == "?" {
                        callback(Action::QueryForegroundColor);
                    }
                    // Setting colors not implemented - ignore
                }
                // Background color query/set (OSC 11)
                "11" => {
                    if data == "?" {
                        callback(Action::QueryBackgroundColor);
                    }
                    // Setting colors not implemented - ignore
                }
                // Cursor color query/set (OSC 12)
                "12" => {
                    if data == "?" {
                        callback(Action::QueryCursorColor);
                    }
                    // Setting colors not implemented - ignore
                }
                // Kitty character width protocol (OSC 66)
                // Format: 66;w=N;CHAR or 66;s=N;CHAR
                // We print the character and ignore the width hint (monospace terminal)
                "66" => {
                    // data format: w=N;CHAR or s=N;CHAR
                    // Find the second semicolon to get the character(s)
                    if let Some(semi_pos) = data.find(';') {
                        let chars = &data[semi_pos + 1..];
                        // Print each character
                        for c in chars.chars() {
                            callback(Action::Print(c));
                        }
                    }
                }
                _ => {
                    // Unknown OSC - log for debugging
                    tracing::debug!("Unknown OSC sequence: {}", content);
                }
            }
        } else if let Ok(code) = content.parse::<u16>() {
            // Some OSC sequences have no data (just the code)
            if code == 133 {
                // Bare OSC 133 - treat as prompt start
                callback(Action::PromptStart);
            }
        }
    }

    /// Check if the currently active charset is DEC Special Graphics.
    fn is_graphics_charset_active(&self) -> bool {
        if self.using_g1 {
            self.g1_charset_graphics
        } else {
            self.g0_charset_graphics
        }
    }

    /// Translate a byte to DEC Special Graphics character.
    /// Returns None if the byte doesn't map to a special character (passthrough).
    fn translate_dec_graphics(byte: u8) -> Option<char> {
        // DEC Special Graphics character set mapping
        // Characters 0x5F-0x7E map to line-drawing and other symbols
        match byte {
            // Blank (space)
            0x5F => Some(' '),
            // Diamond
            0x60 => Some('◆'),
            // Checkerboard
            0x61 => Some('▒'),
            // Horizontal tab symbol
            0x62 => Some('␉'),
            // Form feed symbol
            0x63 => Some('␌'),
            // Carriage return symbol
            0x64 => Some('␍'),
            // Line feed symbol
            0x65 => Some('␊'),
            // Degree symbol
            0x66 => Some('°'),
            // Plus/minus
            0x67 => Some('±'),
            // Newline symbol
            0x68 => Some('␤'),
            // Vertical tab symbol
            0x69 => Some('␋'),
            // Lower right corner
            0x6A => Some('┘'),
            // Upper right corner
            0x6B => Some('┐'),
            // Upper left corner
            0x6C => Some('┌'),
            // Lower left corner
            0x6D => Some('└'),
            // Crossing lines (plus)
            0x6E => Some('┼'),
            // Scan line 1 (top)
            0x6F => Some('⎺'),
            // Scan line 3
            0x70 => Some('⎻'),
            // Horizontal line (middle)
            0x71 => Some('─'),
            // Scan line 7
            0x72 => Some('⎼'),
            // Scan line 9 (bottom)
            0x73 => Some('⎽'),
            // Left tee
            0x74 => Some('├'),
            // Right tee
            0x75 => Some('┤'),
            // Bottom tee
            0x76 => Some('┴'),
            // Top tee
            0x77 => Some('┬'),
            // Vertical line
            0x78 => Some('│'),
            // Less than or equal
            0x79 => Some('≤'),
            // Greater than or equal
            0x7A => Some('≥'),
            // Pi
            0x7B => Some('π'),
            // Not equal
            0x7C => Some('≠'),
            // UK pound
            0x7D => Some('£'),
            // Bullet/centered dot
            0x7E => Some('·'),
            // Characters before 0x5F are passed through unchanged
            _ => None,
        }
    }

    /// Parse shell integration OSC 133 sequences.
    fn dispatch_shell_integration<F>(&self, data: &str, callback: &mut F)
    where
        F: FnMut(Action),
    {
        // OSC 133 ; type [; args]
        let mut parts = data.splitn(2, ';');
        let cmd_type = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("");

        match cmd_type {
            "A" => callback(Action::PromptStart),
            "B" => callback(Action::CommandStart),
            "C" => callback(Action::CommandExecuted),
            "D" => {
                let exit_code = args.parse::<i32>().ok();
                callback(Action::CommandFinished(exit_code));
            }
            _ => {
                tracing::debug!("Unknown shell integration type: {}", cmd_type);
            }
        }
    }

    /// DCS string state.
    fn dcs_string<F>(&mut self, byte: u8, _callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // ESC - might be start of ST (String Terminator = ESC \)
            0x1B => {
                self.state = State::DcsStringEsc;
            }
            // CAN/SUB abort the sequence
            0x18 | 0x1A => {
                self.reset();
            }
            _ => {
                // Accumulate DCS content
                self.dcs_buffer.push(byte);
            }
        }
    }

    /// DCS string ESC state - waiting for backslash to complete ST.
    fn dcs_string_esc<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                self.dispatch_dcs(callback);
                self.reset();
            }
            // Any other byte - was not a valid ST, dispatch and reprocess
            _ => {
                self.dispatch_dcs(callback);
                self.reset();
                // Reprocess this byte from ground state
                self.process_byte(byte, callback);
            }
        }
    }

    /// APC string state - accumulating APC content (Kitty graphics, etc.).
    /// We currently just ignore APC content but need to consume it properly.
    fn apc_string(&mut self, byte: u8) {
        match byte {
            // ESC - might be start of ST (String Terminator = ESC \)
            0x1B => {
                self.state = State::ApcStringEsc;
            }
            // Just consume the byte - we don't process APC content yet
            _ => {}
        }
    }

    /// APC string ESC state - waiting for backslash to complete ST.
    fn apc_string_esc(&mut self, byte: u8) {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                self.reset();
            }
            // Any other byte - not a valid ST, continue in APC mode
            _ => {
                self.state = State::ApcString;
            }
        }
    }

    /// Generic handler for ignored string sequences (PM, SOS, etc.)
    fn ignored_string(&mut self, byte: u8, esc_state: State) {
        if byte == 0x1B {
            self.state = esc_state;
        }
        // Otherwise just consume the byte
    }

    /// Generic handler for ESC in ignored string sequences.
    fn ignored_string_esc(&mut self, byte: u8, string_state: State) {
        match byte {
            b'\\' => self.reset(),
            _ => self.state = string_state,
        }
    }

    /// Dispatch DCS sequence (primarily Sixel).
    fn dispatch_dcs<F>(&mut self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.dcs_buffer.is_empty() {
            return;
        }

        // Check for Sixel: first character should be 'q' after params
        // DCS format: ESC P params q <sixel data> ST
        // When we get here, params are in dcs_params, buffer starts with 'q' or params

        // Find the 'q' that starts sixel data
        let content = &self.dcs_buffer;
        if let Some(q_pos) = content.iter().position(|&b| b == b'q') {
            // Parse params before 'q' (semicolon-separated)
            let param_bytes = &content[..q_pos];
            let mut params = Vec::new();
            let mut current = 0u16;

            for &b in param_bytes {
                match b {
                    b'0'..=b'9' => {
                        current = current.saturating_mul(10).saturating_add((b - b'0') as u16);
                    }
                    b';' => {
                        params.push(current);
                        current = 0;
                    }
                    _ => {}
                }
            }
            params.push(current); // Push last param

            // Get sixel data after 'q'
            let sixel_data = content[q_pos + 1..].to_vec();

            if !sixel_data.is_empty() {
                callback(Action::SixelImage {
                    aspect_ratio: params.first().copied().unwrap_or(0),
                    background: params.get(1).copied().unwrap_or(0),
                    horizontal_grid: params.get(2).copied().unwrap_or(0),
                    data: sixel_data,
                });
            }
        }
    }
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple base64 decoder for OSC 52 clipboard data.
fn base64_decode(input: &str) -> Result<Vec<u8>, &'static str> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn char_to_val(c: u8) -> Option<u8> {
        ALPHABET.iter().position(|&x| x == c).map(|p| p as u8)
    }

    let input: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'=' && b != b'\n' && b != b'\r')
        .collect();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);

    for chunk in input.chunks(4) {
        let vals: Vec<u8> = chunk.iter().filter_map(|&c| char_to_val(c)).collect();
        if vals.len() >= 2 {
            output.push((vals[0] << 2) | (vals[1] >> 4));
            if vals.len() >= 3 {
                output.push((vals[1] << 4) | (vals[2] >> 2));
                if vals.len() >= 4 {
                    output.push((vals[2] << 6) | vals[3]);
                }
            }
        }
    }

    Ok(output)
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
            vec![Action::Print('▀')]
        );

        // Block character with BEL terminator
        assert_eq!(
            parse(b"\x1b]66;w=1;\xE2\x96\x88\x07"),
            vec![Action::Print('█')]
        );

        // Multiple characters
        assert_eq!(
            parse(b"\x1b]66;w=1;\xE2\x96\x80\xE2\x96\x84\x1b\\"),
            vec![Action::Print('▀'), Action::Print('▄')]
        );

        // Screen width (s=N) format
        assert_eq!(
            parse(b"\x1b]66;s=2;\xE2\x96\x84\x1b\\"),
            vec![Action::Print('▄')]
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
        // 2-byte UTF-8: é (U+00E9) = 0xC3 0xA9
        assert_eq!(parse(b"\xC3\xA9"), vec![Action::Print('é')]);
        // â (U+00E2) = 0xC3 0xA2
        assert_eq!(parse(b"\xC3\xA2"), vec![Action::Print('â')]);
        // Mixed ASCII and UTF-8
        assert_eq!(
            parse(b"caf\xC3\xA9"),
            vec![
                Action::Print('c'),
                Action::Print('a'),
                Action::Print('f'),
                Action::Print('é')
            ]
        );
    }

    #[test]
    fn utf8_three_byte_chars() {
        // 3-byte UTF-8: 中 (U+4E2D) = 0xE4 0xB8 0xAD
        assert_eq!(parse(b"\xE4\xB8\xAD"), vec![Action::Print('中')]);
        // € (U+20AC) = 0xE2 0x82 0xAC
        assert_eq!(parse(b"\xE2\x82\xAC"), vec![Action::Print('€')]);
    }

    #[test]
    fn utf8_four_byte_chars() {
        // 4-byte UTF-8: 😀 (U+1F600) = 0xF0 0x9F 0x98 0x80
        assert_eq!(parse(b"\xF0\x9F\x98\x80"), vec![Action::Print('😀')]);
        // 🦀 (U+1F980) = 0xF0 0x9F 0xA6 0x80
        assert_eq!(parse(b"\xF0\x9F\xA6\x80"), vec![Action::Print('🦀')]);
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
            vec![Action::Print('é'), Action::SetAttribute(Attribute::Bold)]
        );
    }

    #[test]
    fn dec_special_graphics_line_drawing() {
        // ESC ( 0 switches to DEC Special Graphics (G0)
        // 'x' (0x78) should become vertical line │
        // 'q' (0x71) should become horizontal line ─
        // 'l' (0x6C) should become upper left corner ┌
        // ESC ( B switches back to ASCII
        assert_eq!(
            parse(b"\x1b(0lqx\x1b(B"),
            vec![
                Action::Print('┌'), // l = upper left corner
                Action::Print('─'), // q = horizontal line
                Action::Print('│'), // x = vertical line
            ]
        );
    }

    #[test]
    fn dec_special_graphics_corners() {
        // All four corners
        assert_eq!(
            parse(b"\x1b(0lkjm\x1b(B"),
            vec![
                Action::Print('┌'), // l = upper left
                Action::Print('┐'), // k = upper right
                Action::Print('┘'), // j = lower right
                Action::Print('└'), // m = lower left
            ]
        );
    }

    #[test]
    fn dec_special_graphics_tees_and_cross() {
        // Tee characters and crossing lines
        assert_eq!(
            parse(b"\x1b(0tuvwn\x1b(B"),
            vec![
                Action::Print('├'), // t = left tee
                Action::Print('┤'), // u = right tee
                Action::Print('┴'), // v = bottom tee
                Action::Print('┬'), // w = top tee
                Action::Print('┼'), // n = crossing lines
            ]
        );
    }

    #[test]
    fn dec_special_graphics_switch_back() {
        // After ESC ( B, characters should be normal ASCII
        assert_eq!(
            parse(b"\x1b(0x\x1b(Bx"),
            vec![
                Action::Print('│'), // x in graphics mode = vertical line
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
                Action::Print('│'), // x after SO with G1=graphics
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
}
