//! CSI (Control Sequence Introducer) sequence handling.
//!
//! The states follow the DEC/xterm parser: an optional private marker
//! (`<`, `=`, `>` or `?`) first, then parameters separated by `;` with
//! `:` sub-parameters (ITU T.416, used by SGR 4:3 and 38:2::r:g:b), then
//! intermediates, then the final byte. C0 controls inside a sequence are
//! executed and the sequence continues; CAN and SUB cancel it. Anything
//! malformed sends the parser to [`State::CsiIgnore`], which swallows the
//! rest of the sequence, so no byte of a control sequence is ever printed.

use super::AnsiParser;
use super::{Action, Mode, State, WindowOp};

/// Maximum number of CSI parameters kept per sequence.
///
/// xterm keeps 30; extra parameters are parsed and ignored so a hostile
/// `CSI 1;1;1;...` can never grow the parameter vector without bound.
pub const MAX_CSI_PARAMS: usize = 32;

// Every kept parameter has a bit in `AnsiParser::subparams`.
const _: () = assert!(MAX_CSI_PARAMS <= u32::BITS as usize);

/// A parameter that the action stores as a byte, saturated rather than
/// wrapped.
fn byte_param(value: u16) -> u8 {
    value.min(u8::MAX as u16) as u8
}

impl AnsiParser {
    /// Forget everything about the CSI sequence being collected.
    pub(super) fn clear_csi(&mut self) {
        self.params.clear();
        self.subparams = 0;
        self.current_param = None;
        self.param_is_sub = false;
        self.marker = None;
        self.intermediate = None;
    }

    /// Record a finished CSI parameter, ignoring any past [`MAX_CSI_PARAMS`].
    fn push_csi_param(&mut self, value: u16) {
        if self.params.len() < MAX_CSI_PARAMS {
            if self.param_is_sub {
                self.subparams |= 1 << self.params.len();
            }
            self.params.push(value);
        }
    }

    /// Finish the parameter being collected, if there is one. A trailing
    /// `:` counts as an empty (zero) sub-parameter.
    fn finish_csi_param(&mut self) {
        if self.current_param.is_some() || self.param_is_sub {
            let value = self.current_param.take().unwrap_or(0);
            self.push_csi_param(value);
        }
        self.param_is_sub = false;
    }

    /// Bytes every CSI state treats alike. Returns `true` if `byte` was
    /// one of them and has been handled.
    fn csi_common<F>(&mut self, byte: u8, callback: &mut F) -> bool
    where
        F: FnMut(Action),
    {
        match byte {
            // CAN and SUB cancel the sequence.
            0x18 | 0x1A => self.reset(),
            // ESC aborts it and starts a new escape sequence.
            0x1B => {
                self.clear_csi();
                self.state = State::Escape;
            }
            // Other C0 controls are executed in place (VT500, xterm).
            0x00..=0x1F => self.control(byte, callback),
            // DEL is ignored everywhere.
            0x7F => {}
            _ => return false,
        }
        true
    }

    /// CSI entry - just entered CSI sequence.
    pub(super) fn csi_entry<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.csi_common(byte, callback) {
            return;
        }
        match byte {
            // Private marker: `?` (DEC), `>`, `<` and `=` (xterm, kitty).
            0x3C..=0x3F => {
                self.marker = Some(byte);
                self.state = State::CsiParam;
            }
            b'0'..=b'9' | b';' | b':' => {
                self.state = State::CsiParam;
                self.csi_param(byte, callback);
            }
            0x20..=0x2F => {
                self.intermediate = Some(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                self.dispatch_csi(byte, callback);
                self.reset();
            }
            _ => self.state = State::CsiIgnore,
        }
    }

    /// CSI param - collecting parameters.
    pub(super) fn csi_param<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.csi_common(byte, callback) {
            return;
        }
        match byte {
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u16;
                self.current_param = Some(
                    self.current_param
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit),
                );
            }
            b';' => {
                let value = self.current_param.take().unwrap_or(0);
                self.push_csi_param(value);
                self.param_is_sub = false;
            }
            b':' => {
                let value = self.current_param.take().unwrap_or(0);
                self.push_csi_param(value);
                self.param_is_sub = true;
            }
            0x20..=0x2F => {
                self.finish_csi_param();
                self.intermediate = Some(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                self.finish_csi_param();
                self.dispatch_csi(byte, callback);
                self.reset();
            }
            // A private marker after the first byte, or a non-ASCII byte.
            _ => self.state = State::CsiIgnore,
        }
    }

    /// Consume the run of digits and `;` at the start of `bytes` while in
    /// the parameter state, returning how many bytes it took. These are
    /// most of the bytes of SGR-heavy output, and handling them here keeps
    /// them off the per-byte state machine call. Everything else (`:`,
    /// controls, the final byte) is left to [`Self::csi_param`].
    #[inline]
    pub(super) fn csi_param_run(&mut self, bytes: &[u8]) -> usize {
        let mut taken = 0;
        for &byte in bytes {
            match byte {
                b'0'..=b'9' => {
                    let digit = u16::from(byte - b'0');
                    self.current_param = Some(
                        self.current_param
                            .unwrap_or(0)
                            .saturating_mul(10)
                            .saturating_add(digit),
                    );
                }
                b';' => {
                    let value = self.current_param.take().unwrap_or(0);
                    self.push_csi_param(value);
                    self.param_is_sub = false;
                }
                _ => break,
            }
            taken += 1;
        }
        taken
    }

    /// CSI intermediate - saw an intermediate byte (e.g. space in DECSCUSR).
    pub(super) fn csi_intermediate<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.csi_common(byte, callback) {
            return;
        }
        match byte {
            0x40..=0x7E => {
                self.dispatch_csi(byte, callback);
                self.reset();
            }
            // A second intermediate (none we know of uses two), a parameter
            // after an intermediate, or a non-ASCII byte.
            _ => self.state = State::CsiIgnore,
        }
    }

    /// CSI ignore - a malformed sequence; consume it up to its final byte.
    pub(super) fn csi_ignore<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.csi_common(byte, callback) {
            return;
        }
        if let 0x40..=0x7E = byte {
            self.reset();
        }
    }

    /// Dispatch a complete CSI sequence according to its marker and
    /// intermediate. Sequences we do not know are ignored.
    fn dispatch_csi<F>(&mut self, cmd: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        // Only SGR defines sub-parameters.
        if self.subparams != 0 && cmd != b'm' {
            return;
        }
        let p1 = self.params.first().copied().unwrap_or(1);
        let first_or_zero = self.params.first().copied().unwrap_or(0);

        match (self.marker, self.intermediate, cmd) {
            (None, None, _) => self.dispatch_plain_csi(cmd, callback),

            // DEC private modes
            (Some(b'?'), None, b'h') => {
                for &param in &self.params {
                    if let Some(action) = self.private_mode_set(param) {
                        callback(action);
                    }
                }
            }
            (Some(b'?'), None, b'l') => {
                for &param in &self.params {
                    if let Some(action) = self.private_mode_reset(param) {
                        callback(action);
                    }
                }
            }
            // DECSED / DECSEL: no cell is protected, so they erase like ED/EL.
            (Some(b'?'), None, b'J') => {
                callback(Action::EraseInDisplay(first_or_zero.min(3) as u8))
            }
            (Some(b'?'), None, b'K') => callback(Action::EraseInLine(first_or_zero.min(2) as u8)),
            // DECXCPR (CSI ? 6 n)
            (Some(b'?'), None, b'n') if p1 == 6 => callback(Action::ReportExtendedCursorPosition),
            // DECRQM - Request DEC Private Mode (CSI ? Ps $ p)
            (Some(b'?'), Some(b'$'), b'p') => callback(Action::RequestPrivateMode(p1)),
            // DECRQM for ANSI modes (CSI Ps $ p)
            (None, Some(b'$'), b'p') => callback(Action::RequestAnsiMode(p1)),

            // XTVERSION (CSI > Ps q)
            (Some(b'>'), None, b'q') => callback(Action::RequestTerminalVersion),

            // Kitty keyboard protocol
            (Some(b'?'), None, b'u') => callback(Action::KittyKeyboardQuery),
            (Some(b'>'), None, b'u') => {
                callback(Action::KittyKeyboardPush(byte_param(first_or_zero)))
            }
            (Some(b'<'), None, b'u') => callback(Action::KittyKeyboardPop(p1.max(1))),
            (Some(b'='), None, b'u') => {
                let mode = self.params.get(1).copied().filter(|&m| m != 0).unwrap_or(1);
                callback(Action::KittyKeyboardSet {
                    flags: byte_param(first_or_zero),
                    mode: byte_param(mode),
                });
            }

            // Secondary device attributes (CSI > c)
            (Some(b'>'), None, b'c') => callback(Action::SecondaryDeviceAttributes),

            // Cursor style (DECSCUSR) - CSI Ps SP q
            (None, Some(b' '), b'q') => callback(Action::SetCursorStyle(byte_param(first_or_zero))),

            // Soft reset (DECSTR — CSI ! p)
            (None, Some(b'!'), b'p') => {
                // Like RIS, a soft reset designates ASCII into G0/G1 again.
                self.g0_charset_graphics = false;
                self.g1_charset_graphics = false;
                self.using_g1 = false;
                callback(Action::SoftReset);
            }

            // XTMODKEYS for modifyOtherKeys (resource 4); vim and neovim
            // send `CSI > 4 ; 2 m` at startup. A missing level resets it.
            (Some(b'>'), None, b'm') if p1 == 4 => callback(Action::SetModifyOtherKeys(
                byte_param(self.params.get(1).copied().unwrap_or(0)),
            )),
            (Some(b'>'), None, b'n') if p1 == 4 => callback(Action::SetModifyOtherKeys(0)),

            // Everything else, including the other XTMODKEYS resources,
            // `CSI = c` (DA3) and `CSI ? Ps s` (XTSAVE), is not ours to act on.
            _ => {}
        }
    }

    /// Dispatch a CSI sequence without marker or intermediate.
    fn dispatch_plain_csi<F>(&mut self, cmd: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        let p1 = self.params.first().copied().unwrap_or(1);
        let p2 = self.params.get(1).copied().unwrap_or(1);
        let first_or_zero = self.params.first().copied().unwrap_or(0);

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
            b'd' => callback(Action::CursorVerticalAbsolute(p1.max(1))),
            b'Z' => callback(Action::CursorBackwardTab(p1.max(1))),
            b'I' => callback(Action::CursorForwardTab(p1.max(1))),
            b'b' => callback(Action::RepeatLastChar(p1.max(1))),
            // HPA — Horizontal Position Absolute (alias for CHA)
            b'`' => callback(Action::CursorHorizontalAbsolute(p1.max(1))),
            // HPR — Horizontal Position Relative (alias for CUF)
            b'a' => callback(Action::CursorForward(p1.max(1))),

            // Erase operations (default to 0, not 1)
            b'J' => callback(Action::EraseInDisplay(first_or_zero.min(3) as u8)),
            b'K' => callback(Action::EraseInLine(first_or_zero.min(2) as u8)),
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
                top: first_or_zero,
                bottom: self.params.get(1).copied().unwrap_or(0),
            }),

            // Cursor save/restore
            b's' => callback(Action::SaveCursorPosition),
            b'u' => callback(Action::RestoreCursorPosition),

            // SGR (text attributes)
            b'm' => self.dispatch_sgr(callback),

            // Tab clear
            b'g' => callback(Action::ClearTabStop(byte_param(first_or_zero))),

            // Status reports (DSR, CPR, DECXCPR)
            b'n' if p1 == 6 => callback(Action::ReportCursorPosition),
            b'n' if p1 == 5 => callback(Action::DeviceStatusReport),

            // XTWINOPS - CSI Ps ; ... t
            b't' => {
                let p2 = self.params.get(1).copied().unwrap_or(0);
                let op = match self.params.first().copied().unwrap_or(0) {
                    14 => Some(WindowOp::ReportTextAreaPixels),
                    16 => Some(WindowOp::ReportCellPixels),
                    18 => Some(WindowOp::ReportTextAreaChars),
                    22 => Some(WindowOp::PushTitle(p2)),
                    23 => Some(WindowOp::PopTitle(p2)),
                    _ => None,
                };
                if let Some(op) = op {
                    callback(Action::WindowOp(op));
                }
            }

            // Primary device attributes
            b'c' => callback(Action::DeviceAttributes),

            // ANSI SM/RM (non-private mode set/reset)
            b'h' | b'l' => {
                let set = cmd == b'h';
                for &param in &self.params {
                    match param {
                        4 => callback(Action::SetInsertMode(set)),
                        20 if set => callback(Action::SetMode(Mode::LineFeedNewLine)),
                        20 => callback(Action::ResetMode(Mode::LineFeedNewLine)),
                        _ => {}
                    }
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
            3 => Some(Action::SetMode(Mode::ColumnMode132)),
            40 => Some(Action::SetMode(Mode::AllowColumnModeSwitch)),
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
            1048 => Some(Action::SetMode(Mode::SaveCursor)),
            1049 => Some(Action::SetMode(Mode::AlternateScreenSaveCursor)),
            2004 => Some(Action::SetMode(Mode::BracketedPaste)),
            2026 => Some(Action::SetMode(Mode::SynchronizedUpdate)),
            2027 => Some(Action::SetMode(Mode::GraphemeClustering)),
            _ => None,
        }
    }

    /// Handle private mode reset (CSI ? ... l).
    fn private_mode_reset(&self, param: u16) -> Option<Action> {
        match param {
            1 => Some(Action::ResetMode(Mode::ApplicationCursorKeys)),
            3 => Some(Action::ResetMode(Mode::ColumnMode132)),
            40 => Some(Action::ResetMode(Mode::AllowColumnModeSwitch)),
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
            1048 => Some(Action::ResetMode(Mode::SaveCursor)),
            1049 => Some(Action::ResetMode(Mode::AlternateScreenSaveCursor)),
            2004 => Some(Action::ResetMode(Mode::BracketedPaste)),
            2026 => Some(Action::ResetMode(Mode::SynchronizedUpdate)),
            2027 => Some(Action::ResetMode(Mode::GraphemeClustering)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Action, AnsiParser, Attribute, Mode};

    /// Feed raw bytes through the parser and collect emitted actions.
    fn parse(input: &[u8]) -> Vec<Action> {
        let mut parser = AnsiParser::new();
        let mut actions = Vec::new();
        parser.feed(input, |a| actions.push(a));
        actions
    }

    // ------------------------------------------------------------------
    // Cursor movement: CUU (A), CUD (B), CUF (C), CUB (D)
    // ------------------------------------------------------------------

    #[test]
    fn cuu_with_param() {
        assert_eq!(parse(b"\x1b[5A"), vec![Action::CursorUp(5)]);
    }

    #[test]
    fn cuu_default_is_one() {
        assert_eq!(parse(b"\x1b[A"), vec![Action::CursorUp(1)]);
    }

    #[test]
    fn cuu_zero_treated_as_one() {
        // CSI 0 A - param 0 should be clamped to 1
        assert_eq!(parse(b"\x1b[0A"), vec![Action::CursorUp(1)]);
    }

    #[test]
    fn cud_with_param() {
        assert_eq!(parse(b"\x1b[3B"), vec![Action::CursorDown(3)]);
    }

    #[test]
    fn cud_default() {
        assert_eq!(parse(b"\x1b[B"), vec![Action::CursorDown(1)]);
    }

    #[test]
    fn cuf_with_param() {
        assert_eq!(parse(b"\x1b[10C"), vec![Action::CursorForward(10)]);
    }

    #[test]
    fn cuf_default() {
        assert_eq!(parse(b"\x1b[C"), vec![Action::CursorForward(1)]);
    }

    #[test]
    fn cub_with_param() {
        assert_eq!(parse(b"\x1b[7D"), vec![Action::CursorBack(7)]);
    }

    #[test]
    fn cub_default() {
        assert_eq!(parse(b"\x1b[D"), vec![Action::CursorBack(1)]);
    }

    // ------------------------------------------------------------------
    // Cursor movement: CNL (E), CPL (F), CHA (G), CUP (H/f)
    // ------------------------------------------------------------------

    #[test]
    fn cnl_with_param() {
        assert_eq!(parse(b"\x1b[4E"), vec![Action::CursorNextLine(4)]);
    }

    #[test]
    fn cpl_with_param() {
        assert_eq!(parse(b"\x1b[2F"), vec![Action::CursorPreviousLine(2)]);
    }

    #[test]
    fn cha_with_param() {
        assert_eq!(
            parse(b"\x1b[15G"),
            vec![Action::CursorHorizontalAbsolute(15)]
        );
    }

    #[test]
    fn cha_default() {
        assert_eq!(parse(b"\x1b[G"), vec![Action::CursorHorizontalAbsolute(1)]);
    }

    #[test]
    fn cup_with_params() {
        assert_eq!(
            parse(b"\x1b[10;20H"),
            vec![Action::CursorPosition { row: 10, col: 20 }]
        );
    }

    #[test]
    fn cup_default() {
        assert_eq!(
            parse(b"\x1b[H"),
            vec![Action::CursorPosition { row: 1, col: 1 }]
        );
    }

    #[test]
    fn cup_f_variant() {
        assert_eq!(
            parse(b"\x1b[5;10f"),
            vec![Action::CursorPosition { row: 5, col: 10 }]
        );
    }

    #[test]
    fn cup_row_only() {
        // CSI 5 H - only row specified, col defaults to 1
        assert_eq!(
            parse(b"\x1b[5H"),
            vec![Action::CursorPosition { row: 5, col: 1 }]
        );
    }

    // ------------------------------------------------------------------
    // Erase sequences: ED (J), EL (K), ECH (X)
    // ------------------------------------------------------------------

    #[test]
    fn ed_default_zero() {
        assert_eq!(parse(b"\x1b[J"), vec![Action::EraseInDisplay(0)]);
    }

    #[test]
    fn ed_explicit_values() {
        assert_eq!(parse(b"\x1b[0J"), vec![Action::EraseInDisplay(0)]);
        assert_eq!(parse(b"\x1b[1J"), vec![Action::EraseInDisplay(1)]);
        assert_eq!(parse(b"\x1b[2J"), vec![Action::EraseInDisplay(2)]);
        assert_eq!(parse(b"\x1b[3J"), vec![Action::EraseInDisplay(3)]);
    }

    #[test]
    fn ed_clamped_to_3() {
        // Values > 3 should be clamped
        assert_eq!(parse(b"\x1b[9J"), vec![Action::EraseInDisplay(3)]);
    }

    #[test]
    fn el_default_zero() {
        assert_eq!(parse(b"\x1b[K"), vec![Action::EraseInLine(0)]);
    }

    #[test]
    fn el_explicit_values() {
        assert_eq!(parse(b"\x1b[0K"), vec![Action::EraseInLine(0)]);
        assert_eq!(parse(b"\x1b[1K"), vec![Action::EraseInLine(1)]);
        assert_eq!(parse(b"\x1b[2K"), vec![Action::EraseInLine(2)]);
    }

    #[test]
    fn el_clamped_to_2() {
        assert_eq!(parse(b"\x1b[9K"), vec![Action::EraseInLine(2)]);
    }

    #[test]
    fn ech_with_param() {
        assert_eq!(parse(b"\x1b[5X"), vec![Action::EraseCharacters(5)]);
    }

    #[test]
    fn ech_default() {
        assert_eq!(parse(b"\x1b[X"), vec![Action::EraseCharacters(1)]);
    }

    // ------------------------------------------------------------------
    // Mode set/reset: DECSET (CSI ? ... h) / DECRST (CSI ? ... l)
    // ------------------------------------------------------------------

    #[test]
    fn decset_cursor_visible() {
        assert_eq!(
            parse(b"\x1b[?25h"),
            vec![Action::SetMode(Mode::CursorVisible)]
        );
    }

    #[test]
    fn decrst_cursor_visible() {
        assert_eq!(
            parse(b"\x1b[?25l"),
            vec![Action::ResetMode(Mode::CursorVisible)]
        );
    }

    #[test]
    fn decset_application_cursor_keys() {
        assert_eq!(
            parse(b"\x1b[?1h"),
            vec![Action::SetMode(Mode::ApplicationCursorKeys)]
        );
    }

    #[test]
    fn decset_alternate_screen_1049() {
        assert_eq!(
            parse(b"\x1b[?1049h"),
            vec![Action::SetMode(Mode::AlternateScreenSaveCursor)]
        );
    }

    #[test]
    fn decrst_alternate_screen_1049() {
        assert_eq!(
            parse(b"\x1b[?1049l"),
            vec![Action::ResetMode(Mode::AlternateScreenSaveCursor)]
        );
    }

    #[test]
    fn decset_bracketed_paste() {
        assert_eq!(
            parse(b"\x1b[?2004h"),
            vec![Action::SetMode(Mode::BracketedPaste)]
        );
    }

    #[test]
    fn decrst_bracketed_paste() {
        assert_eq!(
            parse(b"\x1b[?2004l"),
            vec![Action::ResetMode(Mode::BracketedPaste)]
        );
    }

    #[test]
    fn decset_mouse_sgr() {
        assert_eq!(parse(b"\x1b[?1006h"), vec![Action::SetMode(Mode::MouseSgr)]);
    }

    #[test]
    fn decset_focus_reporting() {
        assert_eq!(
            parse(b"\x1b[?1004h"),
            vec![Action::SetMode(Mode::FocusReporting)]
        );
    }

    #[test]
    fn decset_synchronized_update() {
        assert_eq!(
            parse(b"\x1b[?2026h"),
            vec![Action::SetMode(Mode::SynchronizedUpdate)]
        );
    }

    #[test]
    fn decset_auto_wrap() {
        assert_eq!(parse(b"\x1b[?7h"), vec![Action::SetMode(Mode::AutoWrap)]);
    }

    #[test]
    fn decset_origin_mode() {
        assert_eq!(parse(b"\x1b[?6h"), vec![Action::SetMode(Mode::OriginMode)]);
    }

    #[test]
    fn decset_reverse_video() {
        assert_eq!(
            parse(b"\x1b[?5h"),
            vec![Action::SetMode(Mode::ReverseVideo)]
        );
    }

    #[test]
    fn decset_column_mode() {
        assert_eq!(
            parse(b"\x1b[?3h"),
            vec![Action::SetMode(Mode::ColumnMode132)]
        );
    }

    #[test]
    fn decset_cursor_blink() {
        assert_eq!(
            parse(b"\x1b[?12h"),
            vec![Action::SetMode(Mode::CursorBlink)]
        );
    }

    #[test]
    fn decset_alternate_screen_47() {
        assert_eq!(
            parse(b"\x1b[?47h"),
            vec![Action::SetMode(Mode::AlternateScreen)]
        );
    }

    #[test]
    fn decset_alternate_screen_clear_1047() {
        assert_eq!(
            parse(b"\x1b[?1047h"),
            vec![Action::SetMode(Mode::AlternateScreenClear)]
        );
    }

    #[test]
    fn decset_mouse_x10() {
        assert_eq!(parse(b"\x1b[?9h"), vec![Action::SetMode(Mode::MouseX10)]);
    }

    #[test]
    fn decset_mouse_normal() {
        assert_eq!(
            parse(b"\x1b[?1000h"),
            vec![Action::SetMode(Mode::MouseNormal)]
        );
    }

    #[test]
    fn decset_mouse_button_event() {
        assert_eq!(
            parse(b"\x1b[?1002h"),
            vec![Action::SetMode(Mode::MouseButtonEvent)]
        );
    }

    #[test]
    fn decset_mouse_any_event() {
        assert_eq!(
            parse(b"\x1b[?1003h"),
            vec![Action::SetMode(Mode::MouseAnyEvent)]
        );
    }

    #[test]
    fn decset_mouse_utf8() {
        assert_eq!(
            parse(b"\x1b[?1005h"),
            vec![Action::SetMode(Mode::MouseUtf8)]
        );
    }

    #[test]
    fn decset_mouse_urxvt() {
        assert_eq!(
            parse(b"\x1b[?1015h"),
            vec![Action::SetMode(Mode::MouseUrxvt)]
        );
    }

    #[test]
    fn decset_alternate_scroll_mode() {
        assert_eq!(
            parse(b"\x1b[?1007h"),
            vec![Action::SetMode(Mode::AlternateScrollMode)]
        );
    }

    #[test]
    fn decset_unknown_mode_no_action() {
        // Unknown private mode should produce no action
        let actions = parse(b"\x1b[?9999h");
        assert!(actions.is_empty());
    }

    // ------------------------------------------------------------------
    // CSI with parameters and defaults
    // ------------------------------------------------------------------

    #[test]
    fn csi_large_param() {
        assert_eq!(parse(b"\x1b[999A"), vec![Action::CursorUp(999)]);
    }

    #[test]
    fn csi_multiple_semicolons_default() {
        // CSI ; H means row=0, col=0 (both default) -> clamped to 1,1
        assert_eq!(
            parse(b"\x1b[;H"),
            vec![Action::CursorPosition { row: 1, col: 1 }]
        );
    }

    // ------------------------------------------------------------------
    // Line operations
    // ------------------------------------------------------------------

    #[test]
    fn insert_lines() {
        assert_eq!(parse(b"\x1b[3L"), vec![Action::InsertLines(3)]);
    }

    #[test]
    fn delete_lines() {
        assert_eq!(parse(b"\x1b[2M"), vec![Action::DeleteLines(2)]);
    }

    #[test]
    fn insert_characters() {
        assert_eq!(parse(b"\x1b[4@"), vec![Action::InsertCharacters(4)]);
    }

    #[test]
    fn delete_characters() {
        assert_eq!(parse(b"\x1b[3P"), vec![Action::DeleteCharacters(3)]);
    }

    // ------------------------------------------------------------------
    // Scrolling
    // ------------------------------------------------------------------

    #[test]
    fn scroll_up() {
        assert_eq!(parse(b"\x1b[5S"), vec![Action::ScrollUp(5)]);
    }

    #[test]
    fn scroll_down() {
        assert_eq!(parse(b"\x1b[3T"), vec![Action::ScrollDown(3)]);
    }

    #[test]
    fn set_scrolling_region() {
        assert_eq!(
            parse(b"\x1b[5;20r"),
            vec![Action::SetScrollingRegion { top: 5, bottom: 20 }]
        );
    }

    // ------------------------------------------------------------------
    // Cursor save/restore
    // ------------------------------------------------------------------

    #[test]
    fn save_cursor_via_csi() {
        assert_eq!(parse(b"\x1b[s"), vec![Action::SaveCursorPosition]);
    }

    #[test]
    fn restore_cursor_via_csi() {
        assert_eq!(parse(b"\x1b[u"), vec![Action::RestoreCursorPosition]);
    }

    // ------------------------------------------------------------------
    // Cursor style (DECSCUSR)
    // ------------------------------------------------------------------

    #[test]
    fn set_cursor_style_blinking_block() {
        assert_eq!(parse(b"\x1b[1 q"), vec![Action::SetCursorStyle(1)]);
    }

    #[test]
    fn set_cursor_style_steady_bar() {
        assert_eq!(parse(b"\x1b[6 q"), vec![Action::SetCursorStyle(6)]);
    }

    // ------------------------------------------------------------------
    // Report cursor position
    // ------------------------------------------------------------------

    #[test]
    fn report_cursor_position() {
        assert_eq!(parse(b"\x1b[6n"), vec![Action::ReportCursorPosition]);
    }

    // ------------------------------------------------------------------
    // Device attributes
    // ------------------------------------------------------------------

    #[test]
    fn primary_device_attributes() {
        assert_eq!(parse(b"\x1b[c"), vec![Action::DeviceAttributes]);
    }

    #[test]
    fn secondary_device_attributes() {
        assert_eq!(parse(b"\x1b[>c"), vec![Action::SecondaryDeviceAttributes]);
    }

    // ------------------------------------------------------------------
    // Tab clear
    // ------------------------------------------------------------------

    #[test]
    fn tab_clear_current() {
        assert_eq!(parse(b"\x1b[0g"), vec![Action::ClearTabStop(0)]);
    }

    #[test]
    fn tab_clear_all() {
        assert_eq!(parse(b"\x1b[3g"), vec![Action::ClearTabStop(3)]);
    }

    // ------------------------------------------------------------------
    // DECRQM - Request Private Mode
    // ------------------------------------------------------------------

    #[test]
    fn request_private_mode() {
        assert_eq!(
            parse(b"\x1b[?2026$p"),
            vec![Action::RequestPrivateMode(2026)]
        );
    }

    // ------------------------------------------------------------------
    // Kitty keyboard protocol
    // ------------------------------------------------------------------

    #[test]
    fn kitty_keyboard_push() {
        assert_eq!(parse(b"\x1b[>1u"), vec![Action::KittyKeyboardPush(1)]);
    }

    #[test]
    fn kitty_keyboard_pop() {
        assert_eq!(parse(b"\x1b[<u"), vec![Action::KittyKeyboardPop(1)]);
    }

    #[test]
    fn kitty_keyboard_query() {
        assert_eq!(parse(b"\x1b[?u"), vec![Action::KittyKeyboardQuery]);
    }

    // ------------------------------------------------------------------
    // Malformed CSI sequences
    // ------------------------------------------------------------------

    #[test]
    fn csi_aborted_by_esc() {
        // ESC during CSI should abort and restart in escape state
        // ESC [ 5 ESC [ 3 B -> only CursorDown(3) from the second sequence
        assert_eq!(parse(b"\x1b[5\x1b[3B"), vec![Action::CursorDown(3)]);
    }

    #[test]
    fn c0_inside_csi_is_executed_and_the_sequence_continues() {
        assert_eq!(
            parse(b"\x1b[\x011\nK"),
            vec![
                Action::Execute(0x01),
                Action::Newline,
                Action::EraseInLine(1)
            ]
        );
    }

    #[test]
    fn csi_sgr_dispatches_via_m() {
        // Verify CSI m dispatches SGR
        assert_eq!(
            parse(b"\x1b[1m"),
            vec![Action::SetAttribute(Attribute::Bold)]
        );
    }

    #[test]
    fn csi_params_are_capped_and_extra_ones_ignored() {
        // 100 parameters: only the first MAX_CSI_PARAMS are kept, the
        // sequence still dispatches, and the parser returns to ground.
        let mut data = b"\x1b[".to_vec();
        for _ in 0..100 {
            data.extend_from_slice(b"1;");
        }
        data.extend_from_slice(b"4mX");

        let mut parser = AnsiParser::new();
        let mut actions = Vec::new();
        let mut max_params = 0;
        parser.feed(&data, |a| {
            actions.push(a);
        });
        // Re-run byte by byte to observe the parameter vector while collecting
        let mut parser = AnsiParser::new();
        for &byte in &data {
            parser.feed(&[byte], |_| {});
            max_params = max_params.max(parser.params.len());
        }

        assert!(max_params <= super::MAX_CSI_PARAMS);
        assert!(actions.contains(&Action::SetAttribute(super::super::Attribute::Bold)));
        assert_eq!(actions.last(), Some(&Action::Print('X')));
    }

    #[test]
    fn csi_unknown_final_byte_ignored() {
        // Unknown CSI final byte (e.g., '|') should produce no action
        let actions = parse(b"\x1b[5|");
        assert!(actions.is_empty());
    }

    #[test]
    fn csi_followed_by_normal_text() {
        // After CSI completion, parser should return to ground
        assert_eq!(
            parse(b"\x1b[1AX"),
            vec![Action::CursorUp(1), Action::Print('X')]
        );
    }

    #[test]
    fn csi_private_semicolon_no_preceding_digit() {
        // CSI ? ; 25 h - first param defaults to 0, second param is 25 (show cursor)
        // p1 = 0 doesn't match any mode, but p2 = 25 enables cursor visibility
        let actions = parse(b"\x1b[?;25h");
        assert_eq!(actions, vec![Action::SetMode(Mode::CursorVisible)]);
    }

    #[test]
    fn csi_private_mode_set_multiple_params() {
        // CSI ? 1049 ; 2004 h - enable alternate screen + bracketed paste
        let actions = parse(b"\x1b[?1049;2004h");
        assert_eq!(
            actions,
            vec![
                Action::SetMode(Mode::AlternateScreenSaveCursor),
                Action::SetMode(Mode::BracketedPaste),
            ]
        );
    }

    #[test]
    fn csi_private_mode_reset_multiple_params() {
        // CSI ? 1049 ; 2004 l - disable alternate screen + bracketed paste
        let actions = parse(b"\x1b[?1049;2004l");
        assert_eq!(
            actions,
            vec![
                Action::ResetMode(Mode::AlternateScreenSaveCursor),
                Action::ResetMode(Mode::BracketedPaste),
            ]
        );
    }

    #[test]
    fn csi_param_saturates_on_overflow() {
        // A huge numeric parameter should saturate to u16::MAX, not wrap
        let actions = parse(b"\x1b[99999A");
        assert_eq!(actions, vec![Action::CursorUp(u16::MAX)]);
    }

    // ------------------------------------------------------------------
    // Private markers, intermediates, sub-parameters, controls
    // ------------------------------------------------------------------

    #[test]
    fn xtmodkeys_is_not_sgr() {
        // vim and neovim send this at startup.
        assert_eq!(parse(b"\x1b[>4;2m"), vec![Action::SetModifyOtherKeys(2)]);
        assert_eq!(parse(b"\x1b[>4;1m"), vec![Action::SetModifyOtherKeys(1)]);
        assert_eq!(parse(b"\x1b[>4m"), vec![Action::SetModifyOtherKeys(0)]);
        assert_eq!(parse(b"\x1b[>4n"), vec![Action::SetModifyOtherKeys(0)]);
        // Other resources (1 cursor keys, 2 function keys) are ignored.
        assert_eq!(parse(b"\x1b[>1;2m"), vec![]);
    }

    #[test]
    fn kitty_keyboard_forms() {
        assert_eq!(parse(b"\x1b[?u"), vec![Action::KittyKeyboardQuery]);
        assert_eq!(parse(b"\x1b[>5u"), vec![Action::KittyKeyboardPush(5)]);
        assert_eq!(parse(b"\x1b[>u"), vec![Action::KittyKeyboardPush(0)]);
        assert_eq!(parse(b"\x1b[<u"), vec![Action::KittyKeyboardPop(1)]);
        assert_eq!(
            parse(b"\x1b[=1;1u"),
            vec![Action::KittyKeyboardSet { flags: 1, mode: 1 }]
        );
        assert_eq!(
            parse(b"\x1b[=3;2u"),
            vec![Action::KittyKeyboardSet { flags: 3, mode: 2 }]
        );
        // The mode defaults to 1.
        assert_eq!(
            parse(b"\x1b[=7u"),
            vec![Action::KittyKeyboardSet { flags: 7, mode: 1 }]
        );
        assert_eq!(parse(b"\x1b[u"), vec![Action::RestoreCursorPosition]);
    }

    #[test]
    fn private_markers_do_not_select_plain_commands() {
        // XTSAVE / XTRESTORE are not DECSC / DECSTBM.
        assert_eq!(parse(b"\x1b[?1049s"), vec![]);
        assert_eq!(parse(b"\x1b[?1r"), vec![]);
        // DA3 is not DA1; xterm's title-mode CSI > T is not SD.
        assert_eq!(parse(b"\x1b[=c"), vec![]);
        assert_eq!(parse(b"\x1b[>1T"), vec![]);
        // SL (CSI Ps SP @) is not ICH.
        assert_eq!(parse(b"\x1b[2 @"), vec![]);
    }

    #[test]
    fn sub_parameters_outside_sgr_are_ignored() {
        assert_eq!(parse(b"\x1b[2:3H"), vec![]);
    }

    #[test]
    fn malformed_sequences_are_consumed_to_their_final_byte() {
        // A marker after a parameter, a parameter after an intermediate,
        // and a non-ASCII byte: none of the tail is printed.
        assert_eq!(parse(b"\x1b[1?2hX"), vec![Action::Print('X')]);
        assert_eq!(parse(b"\x1b[1 2qX"), vec![Action::Print('X')]);
        assert_eq!(parse(b"\x1b[1\xff2mX"), vec![Action::Print('X')]);
    }

    #[test]
    fn can_and_sub_cancel_a_sequence() {
        assert_eq!(
            parse(b"\x1b[1\x18AX"),
            vec![Action::Print('A'), Action::Print('X')]
        );
        assert_eq!(
            parse(b"\x1b[1\x1aAX"),
            vec![Action::Print('A'), Action::Print('X')]
        );
    }

    #[test]
    fn controls_execute_in_every_csi_state() {
        assert_eq!(
            parse(b"\x1b[\r?\x0825\x07h"),
            vec![
                Action::CarriageReturn,
                Action::Backspace,
                Action::Bell,
                Action::SetMode(Mode::CursorVisible),
            ]
        );
        assert_eq!(
            parse(b"\x1b[2 \tq"),
            vec![Action::Tab, Action::SetCursorStyle(2)]
        );
    }
}
