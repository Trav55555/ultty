//! CSI (Control Sequence Introducer) sequence handling.

use super::{Action, Mode, State};
use super::AnsiParser;

impl AnsiParser {
    /// CSI entry - just entered CSI sequence.
    pub(super) fn csi_entry<F>(&mut self, byte: u8, callback: &mut F)
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
    pub(super) fn csi_param<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Digit - accumulate parameter
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u16;
                self.current_param =
                    Some(self.current_param.unwrap_or(0).saturating_mul(10).saturating_add(digit));
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
            b'd' => callback(Action::CursorVerticalAbsolute(p1.max(1))),
            b'Z' => callback(Action::CursorBackwardTab(p1.max(1))),
            b'I' => callback(Action::CursorForwardTab(p1.max(1))),
            b'b' => callback(Action::RepeatLastChar(p1.max(1))),
            // HPA — Horizontal Position Absolute (alias for CHA)
            b'`' => callback(Action::CursorHorizontalAbsolute(p1.max(1))),
            // HPR — Horizontal Position Relative (alias for CUF)
            b'a' => callback(Action::CursorForward(p1.max(1))),

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
                for &param in &self.params {
                    if let Some(action) = self.private_mode_set(param) {
                        callback(action);
                    }
                }
            }
            b'l' if self.is_private => {
                for &param in &self.params {
                    if let Some(action) = self.private_mode_reset(param) {
                        callback(action);
                    }
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
}

#[cfg(test)]
mod tests {
    use super::super::{Action, Attribute, Mode, AnsiParser};

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
        assert_eq!(
            parse(b"\x1b[G"),
            vec![Action::CursorHorizontalAbsolute(1)]
        );
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
        assert_eq!(
            parse(b"\x1b[?1006h"),
            vec![Action::SetMode(Mode::MouseSgr)]
        );
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
        assert_eq!(
            parse(b"\x1b[?7h"),
            vec![Action::SetMode(Mode::AutoWrap)]
        );
    }

    #[test]
    fn decset_origin_mode() {
        assert_eq!(
            parse(b"\x1b[?6h"),
            vec![Action::SetMode(Mode::OriginMode)]
        );
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
        assert_eq!(
            parse(b"\x1b[?9h"),
            vec![Action::SetMode(Mode::MouseX10)]
        );
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
        assert_eq!(parse(b"\x1b[<u"), vec![Action::KittyKeyboardPop]);
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
    fn csi_invalid_byte_resets() {
        // Invalid byte in CSI entry resets to ground
        // 0x01 is a control character that triggers reset
        let actions = parse(b"\x1b[\x01");
        // The invalid byte causes a reset; should not crash
        // 0x01 in CSI entry hits the default arm -> reset
        assert!(actions.is_empty());
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
}
