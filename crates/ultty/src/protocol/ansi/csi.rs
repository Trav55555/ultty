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
}
