//! ANSI/VT100 escape sequence parser for legacy applications.
//!
//! This parser converts raw terminal escape sequences into structured
//! [`Action`] values that can be processed by the terminal emulator.
//!
//! # Example
//!
//! ```rust
//! use ultty_vt::ansi::{AnsiParser, Action};
//!
//! let mut parser = AnsiParser::new();
//! let mut actions = Vec::new();
//!
//! parser.feed(b"\x1b[1;1H", |action| actions.push(action));
//!
//! assert_eq!(actions.len(), 1);
//! ```

mod csi;
mod osc;
mod sgr;

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

    /// Move cursor to absolute row (CSI d)
    CursorVerticalAbsolute(u16),

    /// Move cursor N tab stops backward (CSI Z)
    CursorBackwardTab(u16),

    /// Move cursor N tab stops forward (CSI I)
    CursorForwardTab(u16),

    /// Repeat last printed character N times (CSI b)
    RepeatLastChar(u16),

    /// Set/reset insert mode (IRM) — CSI 4 h / CSI 4 l
    SetInsertMode(bool),

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

    /// Kitty keyboard protocol: pop this many entries (CSI < n u, n >= 1)
    KittyKeyboardPop(u16),

    /// Kitty keyboard protocol: query mode (CSI ? u)
    KittyKeyboardQuery,

    /// Kitty keyboard protocol: change the current flags in place
    /// (CSI = flags ; mode u). Mode 1 replaces them, 2 sets the given bits,
    /// 3 clears them.
    KittyKeyboardSet { flags: u8, mode: u8 },

    /// xterm modifyOtherKeys level (XTMODKEYS: CSI > 4 ; level m, and
    /// CSI > 4 n to turn it off). 0 is off.
    SetModifyOtherKeys(u8),

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

    /// Query a dynamic colour (OSC 10 / 11 / 12 with `?`). The reply ends
    /// with the terminator the query used.
    QueryDynamicColor {
        color: DynamicColor,
        terminator: StringTerminator,
    },

    /// Change a palette entry (OSC 4 ; index ; spec)
    SetPaletteColor { index: u8, rgb: [u8; 3] },

    /// Query a palette entry (OSC 4 ; index ; ?)
    QueryPaletteColor {
        index: u8,
        terminator: StringTerminator,
    },

    /// Reset one palette entry, or all of them for `None` (OSC 104)
    ResetPaletteColor(Option<u8>),

    /// Request DEC private mode status (DECRQM - CSI ? Ps $ p)
    RequestPrivateMode(u16),

    /// Request ANSI mode status (DECRQM - CSI Ps $ p)
    RequestAnsiMode(u16),

    /// Device status report, operating status (DSR - CSI 5 n)
    DeviceStatusReport,

    /// Extended cursor position report (DECXCPR - CSI ? 6 n)
    ReportExtendedCursorPosition,

    /// Report the terminal's name and version (XTVERSION - CSI > 0 q)
    RequestTerminalVersion,

    /// Window manipulation and reports (XTWINOPS - CSI Ps ; ... t)
    WindowOp(WindowOp),

    /// Request a setting (DECRQSS - DCS $ q Pt ST); holds `Pt`, such as
    /// `m` for SGR or `r` for DECSTBM.
    RequestStatusString(String),

    /// Request terminfo capabilities (XTGETTCAP - DCS + q Pt ST); holds
    /// `Pt`, hex-encoded names separated by `;`.
    RequestTermcap(String),

    /// Full reset (RIS — ESC c)
    FullReset,

    /// Soft reset (DECSTR — CSI ! p)
    SoftReset,

    /// Screen alignment pattern (DECALN — ESC # 8)
    ScreenAlignmentPattern,

    /// Ultty in-band protocol data: the base64url text of an
    /// `ESC _ U ... ESC \` string, without the `U`. Undecoded, so this crate
    /// stays free of protocol dependencies. Strings longer than
    /// [`MAX_IN_BAND_APC`] are dropped.
    InBand(Vec<u8>),

    /// Application keypad mode (DECKPAM — ESC =)
    ApplicationKeypadMode,

    /// Normal keypad mode (DECKPNM — ESC >)
    NormalKeypadMode,
}

/// Output produced by the ANSI parser's span-aware feed path.
///
/// `Action` preserves the existing per-operation parser contract, while
/// `PrintAscii` borrows a contiguous printable ASCII run from the input buffer.
/// The span form is only emitted from the ground state with the normal ASCII
/// character set active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsiOutput<'a> {
    /// A regular parser action.
    Action(Action),
    /// A contiguous run of printable ASCII bytes.
    PrintAscii(&'a [u8]),
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

    DoubleUnderline,
    /// SGR 4:3
    CurlyUnderline,
    /// SGR 4:4
    DottedUnderline,
    /// SGR 4:5
    DashedUnderline,
    Overline,

    /// Underline color (SGR 58); `Default` (SGR 59) follows the foreground.
    UnderlineColor(Color),

    // Disable specific attributes
    NoBold,
    NoItalic,
    NoUnderline,
    NoBlink,
    NoReverse,
    NoHidden,
    NoStrikethrough,
    NoOverline,
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
    /// Grapheme cluster processing (CSI ?2027h/l) - multi-codepoint
    /// graphemes occupy one cell with their Unicode width
    GraphemeClustering,
    /// Allow 80 <-> 132 column switching (CSI ?40h/l). DECCOLM is ignored
    /// unless this is set, as in xterm.
    AllowColumnModeSwitch,
    /// Save the cursor as DECSC does on set, restore it as DECRC does on
    /// reset (CSI ?1048h/l)
    SaveCursor,
    /// Line feed/new line mode, the ANSI mode 20 (CSI 20h/l): LF, VT and FF
    /// also return the carriage
    LineFeedNewLine,
}

/// The colours OSC 10, 11 and 12 name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DynamicColor {
    /// Default foreground (OSC 10)
    Foreground,
    /// Default background (OSC 11)
    Background,
    /// Cursor (OSC 12)
    Cursor,
}

impl DynamicColor {
    /// The OSC number that names this colour.
    pub fn osc_code(self) -> u8 {
        match self {
            DynamicColor::Foreground => 10,
            DynamicColor::Background => 11,
            DynamicColor::Cursor => 12,
        }
    }
}

/// How a string sequence such as an OSC ended. xterm answers a query with
/// the same terminator, and some applications only accept that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StringTerminator {
    /// BEL (0x07)
    Bel,
    /// ST (`ESC \`)
    St,
}

impl StringTerminator {
    /// The terminator's bytes, as text.
    pub fn as_str(self) -> &'static str {
        match self {
            StringTerminator::Bel => "\x07",
            StringTerminator::St => "\x1b\\",
        }
    }
}

/// The XTWINOPS (`CSI Ps t`) operations Ultty acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowOp {
    /// Report the text area size in pixels (`CSI 14 t`).
    ReportTextAreaPixels,
    /// Report the character cell size in pixels (`CSI 16 t`).
    ReportCellPixels,
    /// Report the text area size in characters (`CSI 18 t`).
    ReportTextAreaChars,
    /// Push the title on the title stack (`CSI 22 ; Ps t`); `Ps` is 0
    /// (icon and window), 1 (icon) or 2 (window).
    PushTitle(u16),
    /// Pop the title from the title stack (`CSI 23 ; Ps t`).
    PopTitle(u16),
}

/// Parser state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// Normal state, processing printable characters
    Ground,
    /// Saw ESC, waiting for next byte
    Escape,
    /// In CSI sequence (ESC [), collecting parameters
    CsiEntry,
    /// Collecting CSI parameters (digits, `;` and `:`), after any private marker
    CsiParam,
    /// Saw a CSI intermediate byte (0x20-0x2F); expecting the final byte
    CsiIntermediate,
    /// Inside a malformed CSI sequence; consumed up to its final byte
    CsiIgnore,
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
    /// ESC # sequence (DECALN, etc.)
    EscapeHash,
}

/// ANSI parser state machine.
///
/// The parser processes bytes incrementally and emits [`Action`] values
/// through a callback. It maintains internal state to handle multi-byte
/// escape sequences.
/// Longest in-band APC string kept, in base64 bytes. Senders split frames
/// into strings of at most 64 KiB of data (87,382 base64 bytes); this leaves
/// room for senders that pick a slightly larger chunk.
pub const MAX_IN_BAND_APC: usize = 128 * 1024;

/// Classification of the APC string being parsed, from its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApcKind {
    /// No byte seen yet.
    Unknown,
    /// `U`: Ultty in-band protocol data, collected.
    InBand,
    /// An in-band string that exceeded [`MAX_IN_BAND_APC`]; dropped.
    Oversized,
    /// Any other APC (kitty graphics and so on); consumed and ignored.
    Other,
}

pub struct AnsiParser {
    pub(super) state: State,
    pub(super) params: Vec<u16>,
    /// Bit `i` set: `params[i]` followed a `:`, so it is a sub-parameter of
    /// the parameter before it.
    pub(super) subparams: u32,
    pub(super) current_param: Option<u16>,
    /// The parameter being collected followed a `:`.
    pub(super) param_is_sub: bool,
    /// CSI private marker: `?`, `>`, `<` or `=`.
    pub(super) marker: Option<u8>,
    /// Intermediate byte for CSI sequences (e.g., space in `CSI Ps SP q`)
    pub(super) intermediate: Option<u8>,
    pub(super) osc_buffer: Vec<u8>,
    pub(super) dcs_buffer: Vec<u8>,
    pub(super) dcs_params: Vec<u16>,
    /// What the current APC string is; see [`ApcKind`].
    pub(super) apc_kind: ApcKind,
    /// Text of an in-band APC string being collected.
    pub(super) apc_buffer: Vec<u8>,
    /// Code point bits of the UTF-8 sequence being decoded
    utf8_codepoint: u32,
    /// Number of remaining bytes expected in current UTF-8 sequence
    utf8_remaining: u8,
    /// Allowed range of the next continuation byte. Narrower than 80-BF
    /// right after E0, ED, F0 and F4, which is what rules out overlong
    /// forms, surrogates and code points above U+10FFFF.
    utf8_lower: u8,
    utf8_upper: u8,
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
            subparams: 0,
            current_param: None,
            param_is_sub: false,
            marker: None,
            intermediate: None,
            osc_buffer: Vec::new(),
            dcs_buffer: Vec::new(),
            dcs_params: Vec::new(),
            apc_kind: ApcKind::Unknown,
            apc_buffer: Vec::new(),
            utf8_codepoint: 0,
            utf8_remaining: 0,
            utf8_lower: 0x80,
            utf8_upper: 0xBF,
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

    /// Feed bytes into the parser, batching ground-state printable ASCII spans.
    ///
    /// This preserves the same semantics as [`Self::feed`] for controls,
    /// escapes, UTF-8, and character-set translations, but emits contiguous
    /// printable ASCII runs as borrowed spans to reduce callback dispatch on
    /// large plain-text PTY output.
    pub fn feed_with_ascii_spans<'a, F>(&mut self, data: &'a [u8], mut callback: F)
    where
        F: FnMut(AnsiOutput<'a>),
    {
        let mut index = 0;
        while index < data.len() {
            // In-band payloads are long runs without ESC; copy them whole.
            if self.state == State::ApcString && self.apc_kind != ApcKind::Unknown {
                let run = data[index..]
                    .iter()
                    .position(|&b| b == 0x1B)
                    .unwrap_or(data.len() - index);
                if run > 0 {
                    self.apc_extend(&data[index..index + run]);
                    index += run;
                    continue;
                }
            }
            let byte = data[index];
            if self.can_emit_ascii_span() {
                if is_printable_ascii(byte) {
                    let start = index;
                    index += 1;
                    while index < data.len() && is_printable_ascii(data[index]) {
                        index += 1;
                    }
                    callback(AnsiOutput::PrintAscii(&data[start..index]));
                    continue;
                }
                if byte >= 0xC2 {
                    let len = print_utf8_run(&data[index..], &mut callback);
                    if len > 0 {
                        index += len;
                        continue;
                    }
                }
            }

            self.process_byte(byte, &mut |action| {
                callback(AnsiOutput::Action(action));
            });
            index += 1;
        }
    }

    fn can_emit_ascii_span(&self) -> bool {
        self.state == State::Ground
            && self.utf8_remaining == 0
            && !self.is_graphics_charset_active()
    }

    /// Process a single byte.
    pub(super) fn process_byte<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match self.state {
            State::Ground => self.ground(byte, callback),
            State::Escape => self.escape(byte, callback),
            State::CsiEntry => self.csi_entry(byte, callback),
            State::CsiParam => self.csi_param(byte, callback),
            State::CsiIntermediate => self.csi_intermediate(byte, callback),
            State::CsiIgnore => self.csi_ignore(byte, callback),
            State::OscString => self.osc_string(byte, callback),
            State::OscStringEsc => self.osc_string_esc(byte, callback),
            State::DcsString => self.dcs_string(byte, callback),
            State::DcsStringEsc => self.dcs_string_esc(byte, callback),
            State::ApcString => self.apc_string(byte),
            State::ApcStringEsc => self.apc_string_esc(byte, callback),
            State::PmString => self.ignored_string(byte, State::PmStringEsc),
            State::PmStringEsc => self.ignored_string_esc(byte, State::PmString),
            State::SosString => self.ignored_string(byte, State::SosStringEsc),
            State::SosStringEsc => self.ignored_string_esc(byte, State::SosString),
            State::CharsetDesignateG0 | State::CharsetDesignateG1 => {
                self.charset_designate(byte, callback)
            }
            State::EscapeHash => {
                if byte == b'8' {
                    callback(Action::ScreenAlignmentPattern);
                }
                self.reset();
            }
        }
    }

    /// Reset parser to ground state.
    /// Note: charset state is preserved across resets (only cleared on hard reset).
    pub(super) fn reset(&mut self) {
        self.state = State::Ground;
        self.clear_csi();
        self.osc_buffer.clear();
        self.dcs_buffer.clear();
        self.dcs_params.clear();
        self.utf8_remaining = 0;
        // Note: g0/g1 charset state is intentionally NOT reset here
        // as it persists across escape sequences
    }

    /// Ground state - normal character processing.
    fn ground<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.utf8_remaining > 0 {
            if (self.utf8_lower..=self.utf8_upper).contains(&byte) {
                self.utf8_codepoint = (self.utf8_codepoint << 6) | (byte & 0x3F) as u32;
                self.utf8_remaining -= 1;
                self.utf8_lower = 0x80;
                self.utf8_upper = 0xBF;
                if self.utf8_remaining == 0 {
                    // The ranges above admit only scalar values.
                    let c = char::from_u32(self.utf8_codepoint).unwrap_or('\u{FFFD}');
                    callback(Action::Print(c));
                }
                return;
            }
            // The sequence so far is a maximal subpart of an ill-formed
            // sequence: one U+FFFD for it, then this byte on its own.
            callback(Action::Print('\u{FFFD}'));
            self.utf8_remaining = 0;
        }

        match byte {
            // ESC - start escape sequence
            0x1B => {
                self.state = State::Escape;
            }
            0x00..=0x1F => self.control(byte, callback),
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
            0xC2..=0xF4 => self.utf8_start(byte),
            // A continuation byte without a lead, an overlong lead (C0, C1)
            // or a lead past U+10FFFF (F5-FF).
            _ => callback(Action::Print('\u{FFFD}')),
        }
    }

    /// Start decoding a multi-byte UTF-8 sequence at lead byte `lead`
    /// (C2-F4), following the Unicode Standard's table of well-formed
    /// byte sequences (section 3.9, table 3-7).
    fn utf8_start(&mut self, lead: u8) {
        let (remaining, bits) = match lead {
            0xC2..=0xDF => (1, lead & 0x1F),
            0xE0..=0xEF => (2, lead & 0x0F),
            _ => (3, lead & 0x07),
        };
        self.utf8_remaining = remaining;
        self.utf8_codepoint = bits as u32;
        (self.utf8_lower, self.utf8_upper) = match lead {
            0xE0 => (0xA0, 0xBF),
            0xED => (0x80, 0x9F),
            0xF0 => (0x90, 0xBF),
            0xF4 => (0x80, 0x8F),
            _ => (0x80, 0xBF),
        };
    }

    /// Execute a C0 control other than ESC. Shared by the ground state and
    /// the CSI states, which execute controls without ending the sequence.
    pub(super) fn control<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            0x07 => callback(Action::Bell),
            0x08 => callback(Action::Backspace),
            0x09 => callback(Action::Tab),
            0x0A..=0x0C => callback(Action::Newline),
            0x0D => callback(Action::CarriageReturn),
            0x0E => self.using_g1 = true,
            0x0F => self.using_g1 = false,
            _ => callback(Action::Execute(byte)),
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
                self.clear_csi();
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
            // Full reset (RIS — ESC c)
            b'c' => {
                // A hard reset also forgets charset designations (G0/G1 back
                // to ASCII, shift-in), unlike the per-sequence reset below.
                self.g0_charset_graphics = false;
                self.g1_charset_graphics = false;
                self.using_g1 = false;
                callback(Action::FullReset);
                self.reset();
            }
            // Application keypad mode (DECKPAM — ESC =)
            b'=' => {
                callback(Action::ApplicationKeypadMode);
                self.reset();
            }
            // Normal keypad mode (DECKPNM — ESC >)
            b'>' => {
                callback(Action::NormalKeypadMode);
                self.reset();
            }
            // ESC # sequences (DECALN, etc.)
            b'#' => {
                self.state = State::EscapeHash;
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
                self.apc_kind = ApcKind::Unknown;
                self.apc_buffer.clear();
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
            0x60 => Some('\u{25C6}'),
            // Checkerboard
            0x61 => Some('\u{2592}'),
            // Horizontal tab symbol
            0x62 => Some('\u{2409}'),
            // Form feed symbol
            0x63 => Some('\u{240C}'),
            // Carriage return symbol
            0x64 => Some('\u{240D}'),
            // Line feed symbol
            0x65 => Some('\u{240A}'),
            // Degree symbol
            0x66 => Some('\u{00B0}'),
            // Plus/minus
            0x67 => Some('\u{00B1}'),
            // Newline symbol
            0x68 => Some('\u{2424}'),
            // Vertical tab symbol
            0x69 => Some('\u{240B}'),
            // Lower right corner
            0x6A => Some('\u{2518}'),
            // Upper right corner
            0x6B => Some('\u{2510}'),
            // Upper left corner
            0x6C => Some('\u{250C}'),
            // Lower left corner
            0x6D => Some('\u{2514}'),
            // Crossing lines (plus)
            0x6E => Some('\u{253C}'),
            // Scan line 1 (top)
            0x6F => Some('\u{23BA}'),
            // Scan line 3
            0x70 => Some('\u{23BB}'),
            // Horizontal line (middle)
            0x71 => Some('\u{2500}'),
            // Scan line 7
            0x72 => Some('\u{23BC}'),
            // Scan line 9 (bottom)
            0x73 => Some('\u{23BD}'),
            // Left tee
            0x74 => Some('\u{251C}'),
            // Right tee
            0x75 => Some('\u{2524}'),
            // Bottom tee
            0x76 => Some('\u{2534}'),
            // Top tee
            0x77 => Some('\u{252C}'),
            // Vertical line
            0x78 => Some('\u{2502}'),
            // Less than or equal
            0x79 => Some('\u{2264}'),
            // Greater than or equal
            0x7A => Some('\u{2265}'),
            // Pi
            0x7B => Some('\u{03C0}'),
            // Not equal
            0x7C => Some('\u{2260}'),
            // UK pound
            0x7D => Some('\u{00A3}'),
            // Bullet/centered dot
            0x7E => Some('\u{00B7}'),
            // Characters before 0x5F are passed through unchanged
            _ => None,
        }
    }
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

fn is_printable_ascii(byte: u8) -> bool {
    (0x20..=0x7E).contains(&byte)
}

/// Print the run of whole, well-formed multi-byte UTF-8 characters at the
/// start of `bytes`, returning how many bytes it took.
///
/// Each decodes to the one `Print` the byte-wise state machine would reach
/// from the ground state; whatever ends the run (ASCII, a sequence split
/// across reads, ill-formed bytes) is left to it. Kept out of line so the
/// byte loop it is called from stays small for escape-heavy input.
#[inline(never)]
fn print_utf8_run<'a, F>(bytes: &'a [u8], callback: &mut F) -> usize
where
    F: FnMut(AnsiOutput<'a>),
{
    let mut index = 0;
    while let Some(&lead) = bytes.get(index) {
        let len = match lead {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => break,
        };
        let Some(c) = bytes
            .get(index..index + len)
            .and_then(|seq| std::str::from_utf8(seq).ok())
            .and_then(|s| s.chars().next())
        else {
            break;
        };
        callback(AnsiOutput::Action(Action::Print(c)));
        index += len;
    }
    index
}

#[cfg(test)]
mod tests;
