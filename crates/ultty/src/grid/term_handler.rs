//! Handler that applies ANSI parser actions to the Terminal.

use crate::grid::{decode_sixel, CellFlags, MouseEncoding, MouseMode, Terminal, TerminalImage};
use crate::protocol::ansi::{Action, Attribute, Color as AnsiColor, Mode, NamedColor};

use super::cell::Color;

/// Applies parser actions to a Terminal.
///
/// This handler is the bridge between the ANSI parser and the terminal.
/// It translates parser [`Action`]s into terminal mutations, properly
/// handling alternate screen, modes, and scrollback.
pub struct TerminalHandler<'a> {
    terminal: &'a mut Terminal,
}

impl<'a> TerminalHandler<'a> {
    /// Create a new handler for the given terminal.
    pub fn new(terminal: &'a mut Terminal) -> Self {
        Self { terminal }
    }

    /// Handle a single parser action.
    pub fn handle(&mut self, action: Action) {
        match action {
            Action::Print(c) => {
                if !self.terminal.is_alternate_screen() {
                    self.terminal.buffer_mut().write_char(c);
                } else {
                    self.terminal.grid_mut().write_char(c);
                }
            }

            Action::Execute(_code) => {
                // C0 controls handled separately
            }

            // Cursor movement
            Action::CursorUp(n) => self.terminal.grid_mut().cursor_up(n),
            Action::CursorDown(n) => self.terminal.grid_mut().cursor_down(n),
            Action::CursorForward(n) => self.terminal.grid_mut().cursor_right(n),
            Action::CursorBack(n) => self.terminal.grid_mut().cursor_left(n),
            Action::CursorNextLine(n) => {
                self.terminal.grid_mut().cursor_down(n);
                self.terminal.grid_mut().carriage_return();
            }
            Action::CursorPreviousLine(n) => {
                self.terminal.grid_mut().cursor_up(n);
                self.terminal.grid_mut().carriage_return();
            }
            Action::CursorHorizontalAbsolute(col) => {
                let row = self.terminal.grid().cursor().position.row + 1;
                self.terminal.grid_mut().set_cursor_position(row, col);
            }
            Action::CursorPosition { row, col } => {
                // When origin mode (DECOM) is enabled, cursor positions are relative
                // to the scroll region, not the screen
                let row = if self.terminal.modes().origin_mode {
                    let region = self.terminal.grid().effective_scroll_region();
                    // Add scroll region top offset (row is 1-based, region.top is 0-based)
                    row.saturating_add(region.top)
                } else {
                    row
                };
                self.terminal.grid_mut().set_cursor_position(row, col);
            }

            // Erase operations
            Action::EraseInDisplay(mode) => match mode {
                0 => self.terminal.grid_mut().erase_below(),
                1 => self.terminal.grid_mut().erase_above(),
                2 | 3 => self.terminal.grid_mut().clear(),
                _ => {}
            },
            Action::EraseInLine(mode) => match mode {
                0 => self.terminal.grid_mut().erase_line_right(),
                1 => self.terminal.grid_mut().erase_line_left(),
                2 => self.terminal.grid_mut().erase_line(),
                _ => {}
            },
            Action::EraseCharacters(n) => {
                let col = self.terminal.grid().cursor().position.col;
                let row = self.terminal.grid().cursor().position.row;
                for i in 0..n {
                    if let Some(cell) = self.terminal.grid_mut().cell_mut(col + i, row) {
                        cell.clear();
                    }
                }
            }

            // Line operations
            Action::InsertLines(n) => {
                self.terminal.grid_mut().insert_lines(n);
            }
            Action::DeleteLines(n) => {
                self.terminal.grid_mut().delete_lines(n);
            }
            Action::DeleteCharacters(n) => {
                let row_idx = self.terminal.grid().cursor().position.row;
                let col = self.terminal.grid().cursor().position.col;
                if let Some(row) = self.terminal.grid_mut().row_mut(row_idx) {
                    row.delete_cells(col, n);
                }
            }
            Action::InsertCharacters(n) => {
                let row_idx = self.terminal.grid().cursor().position.row;
                let col = self.terminal.grid().cursor().position.col;
                if let Some(row) = self.terminal.grid_mut().row_mut(row_idx) {
                    row.insert_cells(col, n);
                }
            }

            // Scrolling (use buffer for scrollback on main screen)
            Action::ScrollUp(n) => {
                if !self.terminal.is_alternate_screen() {
                    self.terminal.buffer_mut().scroll_grid_up(n);
                } else {
                    self.terminal.grid_mut().scroll_up(n);
                }
            }
            Action::ScrollDown(n) => self.terminal.grid_mut().scroll_down(n),
            Action::ReverseIndex => {
                // Move cursor up, scroll down if at top of scroll region
                let grid = self.terminal.grid_mut();
                let region = grid.effective_scroll_region();
                let cursor_row = grid.cursor().position.row;
                if cursor_row <= region.top {
                    grid.scroll_down(1);
                } else {
                    grid.cursor_up(1);
                }
            }
            Action::Index => {
                // Move cursor down, scroll up if at bottom of scroll region
                let grid = self.terminal.grid_mut();
                let region = grid.effective_scroll_region();
                let cursor_row = grid.cursor().position.row;
                if cursor_row >= region.bottom {
                    if !self.terminal.is_alternate_screen() {
                        self.terminal.buffer_mut().scroll_grid_up(1);
                    } else {
                        self.terminal.grid_mut().scroll_up(1);
                    }
                } else {
                    self.terminal.grid_mut().cursor_down(1);
                }
            }
            Action::NextLine => {
                // Move to beginning of next line (CR + Index)
                self.terminal.grid_mut().carriage_return();
                let grid = self.terminal.grid_mut();
                let region = grid.effective_scroll_region();
                let cursor_row = grid.cursor().position.row;
                if cursor_row >= region.bottom {
                    if !self.terminal.is_alternate_screen() {
                        self.terminal.buffer_mut().scroll_grid_up(1);
                    } else {
                        self.terminal.grid_mut().scroll_up(1);
                    }
                } else {
                    self.terminal.grid_mut().cursor_down(1);
                }
            }
            Action::SetScrollingRegion { top, bottom } => {
                self.terminal.grid_mut().set_scroll_region(top, bottom);
            }

            // Cursor save/restore
            Action::SaveCursorPosition => self.terminal.grid_mut().cursor_mut().save(),
            Action::RestoreCursorPosition => self.terminal.grid_mut().cursor_mut().restore(),

            // Text attributes
            Action::SetAttribute(attr) => self.apply_attribute(attr),
            Action::ResetAttributes => self.terminal.grid_mut().reset_attrs(),

            // Modes
            Action::SetMode(mode) => self.set_mode(mode, true),
            Action::ResetMode(mode) => self.set_mode(mode, false),

            // Tabs
            Action::Tab => self.terminal.grid_mut().tab(),
            Action::SetTabStop => self.terminal.grid_mut().set_tab_stop(),
            Action::ClearTabStop(mode) => self.terminal.grid_mut().clear_tab_stop(mode),

            // Terminal queries
            Action::ReportCursorPosition => {
                // Send cursor position report: CSI row ; col R
                let row = self.terminal.grid().cursor().position.row + 1; // 1-based
                let col = self.terminal.grid().cursor().position.col + 1; // 1-based
                let response = format!("\x1b[{};{}R", row, col);
                self.terminal.queue_response(response.into_bytes());
            }
            Action::DeviceAttributes => {
                // Send primary device attributes (DA1)
                // We identify as VT220 with various capabilities:
                // 1 = 132 columns, 2 = printer port, 6 = selective erase,
                // 22 = ANSI color, 29 = ANSI text locator
                let response = b"\x1b[?62;1;2;6;22;29c".to_vec();
                self.terminal.queue_response(response);
            }
            Action::SecondaryDeviceAttributes => {
                // Send secondary device attributes (DA2)
                // Format: CSI > Pp ; Pv ; Pc c
                // Pp=0 (VT100-compatible), Pv=1 (version), Pc=0 (no special options)
                let response = b"\x1b[>0;1;0c".to_vec();
                self.terminal.queue_response(response);
            }

            // Control characters
            Action::Bell => {
                // Bell is handled by the event loop emitting TerminalEvent::Bell
            }

            // Cursor style (DECSCUSR)
            Action::SetCursorStyle(style) => {
                use crate::grid::CursorShape;
                let cursor = self.terminal.grid_mut().cursor_mut();
                match style {
                    0 | 1 => {
                        cursor.shape = CursorShape::Block;
                        cursor.blinking = true;
                    }
                    2 => {
                        cursor.shape = CursorShape::Block;
                        cursor.blinking = false;
                    }
                    3 => {
                        cursor.shape = CursorShape::Underline;
                        cursor.blinking = true;
                    }
                    4 => {
                        cursor.shape = CursorShape::Underline;
                        cursor.blinking = false;
                    }
                    5 => {
                        cursor.shape = CursorShape::Bar;
                        cursor.blinking = true;
                    }
                    6 => {
                        cursor.shape = CursorShape::Bar;
                        cursor.blinking = false;
                    }
                    _ => {} // Unknown style, ignore
                }
            }
            Action::Newline => {
                if !self.terminal.is_alternate_screen() {
                    self.terminal.buffer_mut().newline();
                } else {
                    self.terminal.grid_mut().newline();
                }
            }
            Action::CarriageReturn => self.terminal.grid_mut().carriage_return(),
            Action::Backspace => self.terminal.grid_mut().backspace(),

            // Screen modes
            Action::EnterAlternateScreen => {
                self.terminal.enter_alternate_screen();
            }
            Action::LeaveAlternateScreen => {
                self.terminal.leave_alternate_screen();
            }

            // OSC sequences
            Action::SetTitle(title) => {
                self.terminal.set_title(title);
            }
            Action::SetWorkingDirectory(path) => {
                self.terminal.set_working_directory(path);
            }
            Action::PromptStart => {
                self.terminal.mark_prompt_start();
            }
            Action::CommandStart => {
                self.terminal.mark_command_start();
            }
            Action::CommandExecuted => {
                self.terminal.mark_command_executed();
            }
            Action::CommandFinished(exit_code) => {
                self.terminal.mark_command_finished(exit_code);
            }
            Action::SetClipboard {
                clipboard: _,
                data: _,
            } => {
                // Clipboard is handled at the app level, not terminal level
                // The terminal handler ignores this and the app will handle it
            }
            Action::SixelImage {
                aspect_ratio: _,
                background,
                horizontal_grid: _,
                data,
            } => {
                self.handle_sixel_image(&data, background);
            }
            Action::StartHyperlink(uri) => {
                self.terminal.start_hyperlink(uri);
                // Also set the hyperlink_id on the grid's current attrs
                let id = self.terminal.current_hyperlink_id();
                self.terminal.grid_mut().set_hyperlink_id(id);
            }
            Action::EndHyperlink => {
                self.terminal.end_hyperlink();
                self.terminal.grid_mut().set_hyperlink_id(0);
            }
            Action::Notification { title: _, body: _ } => {
                // Notification is handled at the app level, not terminal level
                // The terminal handler ignores this and the app will handle it
            }
            Action::KittyKeyboardPush(flags) => {
                self.terminal.push_kitty_keyboard_mode(flags);
            }
            Action::KittyKeyboardPop => {
                self.terminal.pop_kitty_keyboard_mode();
            }
            Action::KittyKeyboardQuery => {
                // Send current keyboard mode flags
                let flags = self.terminal.modes().kitty_keyboard_flags;
                let response = format!("\x1b[?{}u", flags);
                self.terminal.queue_response(response.into_bytes());
            }
            Action::QueryForegroundColor => {
                // Respond with foreground color in XParseColor format
                // Format: OSC 10 ; rgb:RRRR/GGGG/BBBB ST
                // Using a typical light foreground for dark themes
                let response = b"\x1b]10;rgb:d4d4/d4d4/d4d4\x1b\\".to_vec();
                self.terminal.queue_response(response);
            }
            Action::QueryBackgroundColor => {
                // Respond with background color in XParseColor format
                // Format: OSC 11 ; rgb:RRRR/GGGG/BBBB ST
                // Using a typical dark background
                let response = b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\".to_vec();
                self.terminal.queue_response(response);
            }
            Action::QueryCursorColor => {
                // Respond with cursor color in XParseColor format
                // Format: OSC 12 ; rgb:RRRR/GGGG/BBBB ST
                let response = b"\x1b]12;rgb:d4d4/d4d4/d4d4\x1b\\".to_vec();
                self.terminal.queue_response(response);
            }
            Action::RequestPrivateMode(mode) => {
                // DECRQM response format: CSI ? Ps ; Pm $ y
                // Pm: 1=set, 2=reset, 3=permanently set, 4=permanently reset, 0=not recognized
                let modes = self.terminal.modes();
                let status = match mode {
                    1 => if modes.application_cursor_keys { 1 } else { 2 },
                    7 => if modes.auto_wrap { 1 } else { 2 },
                    25 => if self.terminal.grid().cursor().visible { 1 } else { 2 },
                    1000 | 1002 | 1003 => {
                        let active = matches!(modes.mouse_mode,
                            crate::grid::terminal::MouseMode::Normal |
                            crate::grid::terminal::MouseMode::ButtonEvent |
                            crate::grid::terminal::MouseMode::AnyEvent
                        );
                        if active { 1 } else { 2 }
                    }
                    1004 => if modes.focus_reporting { 1 } else { 2 },
                    1006 => if matches!(modes.mouse_encoding, crate::grid::terminal::MouseEncoding::Sgr) { 1 } else { 2 },
                    2004 => if modes.bracketed_paste { 1 } else { 2 },
                    2026 => if modes.synchronized_update { 1 } else { 2 },
                    2027 => 2, // Unicode core spec - not implemented
                    // Mode not recognized
                    _ => 0,
                };
                let response = format!("\x1b[?{};{}$y", mode, status);
                self.terminal.queue_response(response.into_bytes());
            }
            Action::UnknownOsc(_) => {
                // Ignore unknown OSC sequences
            }
        }
    }

    /// Apply a text attribute to the current style.
    fn apply_attribute(&mut self, attr: Attribute) {
        let attrs = *self.terminal.grid().attrs();
        let mut new_attrs = attrs;

        match attr {
            Attribute::Bold => new_attrs.flags_mut().set(CellFlags::BOLD),
            Attribute::Dim => new_attrs.flags_mut().set(CellFlags::DIM),
            Attribute::Italic => new_attrs.flags_mut().set(CellFlags::ITALIC),
            Attribute::Underline => new_attrs.flags_mut().set(CellFlags::UNDERLINE),
            Attribute::Blink => new_attrs.flags_mut().set(CellFlags::BLINK),
            Attribute::Reverse => new_attrs.flags_mut().set(CellFlags::REVERSE),
            Attribute::Hidden => new_attrs.flags_mut().set(CellFlags::HIDDEN),
            Attribute::Strikethrough => new_attrs.flags_mut().set(CellFlags::STRIKETHROUGH),

            Attribute::NoBold => new_attrs.flags_mut().unset(CellFlags::BOLD),
            Attribute::NoItalic => new_attrs.flags_mut().unset(CellFlags::ITALIC),
            Attribute::NoUnderline => new_attrs.flags_mut().unset(CellFlags::UNDERLINE),
            Attribute::NoBlink => new_attrs.flags_mut().unset(CellFlags::BLINK),
            Attribute::NoReverse => new_attrs.flags_mut().unset(CellFlags::REVERSE),
            Attribute::NoHidden => new_attrs.flags_mut().unset(CellFlags::HIDDEN),
            Attribute::NoStrikethrough => new_attrs.flags_mut().unset(CellFlags::STRIKETHROUGH),

            Attribute::ForegroundColor(color) => {
                new_attrs.set_fg(convert_color(color));
            }
            Attribute::BackgroundColor(color) => {
                new_attrs.set_bg(convert_color(color));
            }
        }

        self.terminal.grid_mut().set_attrs(new_attrs);
    }

    /// Set or reset a terminal mode.
    fn set_mode(&mut self, mode: Mode, enabled: bool) {
        match mode {
            Mode::CursorVisible => {
                self.terminal.grid_mut().cursor_mut().visible = enabled;
            }
            Mode::ApplicationCursorKeys => {
                self.terminal.modes_mut().application_cursor_keys = enabled;
            }
            Mode::BracketedPaste => {
                self.terminal.modes_mut().bracketed_paste = enabled;
            }
            Mode::FocusReporting => {
                self.terminal.modes_mut().focus_reporting = enabled;
            }
            // Mouse tracking modes
            Mode::MouseX10 => {
                self.terminal.modes_mut().mouse_mode = if enabled {
                    MouseMode::X10
                } else {
                    MouseMode::None
                };
            }
            Mode::MouseNormal => {
                self.terminal.modes_mut().mouse_mode = if enabled {
                    MouseMode::Normal
                } else {
                    MouseMode::None
                };
            }
            Mode::MouseButtonEvent => {
                self.terminal.modes_mut().mouse_mode = if enabled {
                    MouseMode::ButtonEvent
                } else {
                    MouseMode::None
                };
            }
            Mode::MouseAnyEvent => {
                self.terminal.modes_mut().mouse_mode = if enabled {
                    MouseMode::AnyEvent
                } else {
                    MouseMode::None
                };
            }
            // Mouse encoding modes
            Mode::MouseUtf8 => {
                if enabled {
                    self.terminal.modes_mut().mouse_encoding = MouseEncoding::Utf8;
                } else {
                    self.terminal.modes_mut().mouse_encoding = MouseEncoding::X10;
                }
            }
            Mode::MouseSgr => {
                if enabled {
                    self.terminal.modes_mut().mouse_encoding = MouseEncoding::Sgr;
                } else {
                    self.terminal.modes_mut().mouse_encoding = MouseEncoding::X10;
                }
            }
            Mode::MouseUrxvt => {
                if enabled {
                    self.terminal.modes_mut().mouse_encoding = MouseEncoding::Urxvt;
                } else {
                    self.terminal.modes_mut().mouse_encoding = MouseEncoding::X10;
                }
            }
            Mode::OriginMode => {
                self.terminal.modes_mut().origin_mode = enabled;
            }
            Mode::AutoWrap => {
                self.terminal.modes_mut().auto_wrap = enabled;
                self.terminal.grid_mut().set_auto_wrap(enabled);
            }
            Mode::ReverseVideo => {
                self.terminal.modes_mut().reverse_video = enabled;
            }
            Mode::CursorBlink => {
                self.terminal.grid_mut().cursor_mut().blinking = enabled;
            }
            Mode::ColumnMode132 => {
                // 132 column mode (DECCOLM) - request resize from app
                let columns = if enabled { 132 } else { 80 };
                self.terminal.request_column_resize(columns);
                // Clear the screen when switching column modes (per DECCOLM spec)
                self.terminal.grid_mut().clear();
                self.terminal.grid_mut().cursor_mut().position =
                    crate::grid::CursorPosition::origin();
            }
            Mode::AlternateScreen => {
                if enabled {
                    self.terminal.enter_alternate_screen();
                } else {
                    self.terminal.leave_alternate_screen();
                }
            }
            Mode::AlternateScreenClear => {
                if enabled {
                    self.terminal.enter_alternate_screen();
                    self.terminal.grid_mut().clear();
                } else {
                    self.terminal.leave_alternate_screen();
                }
            }
            Mode::AlternateScreenSaveCursor => {
                if enabled {
                    self.terminal.grid_mut().cursor_mut().save();
                    self.terminal.enter_alternate_screen();
                    self.terminal.grid_mut().clear();
                } else {
                    self.terminal.leave_alternate_screen();
                    self.terminal.grid_mut().cursor_mut().restore();
                }
            }
            Mode::AlternateScrollMode => {
                self.terminal.modes_mut().alternate_scroll = enabled;
            }
            Mode::SynchronizedUpdate => {
                // Synchronized update mode - rendering is suppressed while active
                // to prevent partial frames. The event loop checks this flag and
                // defers rendering until the mode is disabled (ESU received).
                self.terminal.modes_mut().synchronized_update = enabled;
            }
        }
    }

    /// Handle a Sixel image.
    fn handle_sixel_image(&mut self, data: &[u8], background: u16) {
        // Decode the sixel data
        let Some((pixels, width, height)) = decode_sixel(data, background) else {
            tracing::warn!("Failed to decode sixel image");
            return;
        };

        // Get current cursor position for image placement
        let cursor = self.terminal.grid().cursor();
        let start_col = cursor.position.col;
        let start_row = cursor.position.row;

        // Calculate cell dimensions using typical terminal font metrics.
        // 8x16 is standard for most monospace fonts at common sizes.
        let cell_pixel_width = 8u32;
        let cell_pixel_height = 16u32;

        let cell_width = width.div_ceil(cell_pixel_width).min(u16::MAX as u32) as u16;
        let cell_height = height.div_ceil(cell_pixel_height).min(u16::MAX as u32) as u16;

        // Allocate image ID and create the image
        let id = self.terminal.images_mut().allocate_id();
        let image = TerminalImage {
            id,
            width,
            height,
            data: pixels,
            start_col,
            start_row,
            cell_width,
            cell_height,
        };

        tracing::debug!(
            "Decoded sixel image: {}x{} pixels, {}x{} cells at ({}, {})",
            width,
            height,
            cell_width,
            cell_height,
            start_col,
            start_row
        );

        // Add the image to the terminal
        self.terminal.add_image(image);
    }
}

/// Convert an ANSI parser color to a grid color.
fn convert_color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Default => Color::Default,
        AnsiColor::Named(named) => Color::Named(convert_named_color(named)),
        AnsiColor::Indexed(idx) => Color::Indexed(idx),
        AnsiColor::Rgb { r, g, b } => Color::Rgb(r, g, b),
    }
}

/// Convert a named color to its palette index.
fn convert_named_color(named: NamedColor) -> u8 {
    match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{cell::Color, Dimensions};
    use crate::protocol::ansi::AnsiParser;

    fn process(terminal: &mut Terminal, input: &[u8]) {
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(terminal);
        parser.feed(input, |action| handler.handle(action));
    }

    #[test]
    fn print_hello() {
        let mut term = Terminal::new(Dimensions::new(80, 24));
        process(&mut term, b"Hello");

        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'H');
        assert_eq!(term.grid().cell(4, 0).unwrap().char(), 'o');
    }

    #[test]
    fn alternate_screen_escape() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Write to main
        process(&mut term, b"Main");
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'M');

        // Enter alternate screen
        process(&mut term, b"\x1b[?1049h");
        assert!(term.is_alternate_screen());
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), ' ');

        // Write to alternate
        process(&mut term, b"Alt");
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'A');

        // Leave alternate screen
        process(&mut term, b"\x1b[?1049l");
        assert!(!term.is_alternate_screen());
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'M');
    }

    #[test]
    fn scroll_to_history() {
        let mut term = Terminal::new(Dimensions::new(10, 3));

        // Fill screen and scroll
        process(&mut term, b"Line1\r\n");
        process(&mut term, b"Line2\r\n");
        process(&mut term, b"Line3\r\n");
        process(&mut term, b"Line4\r\n");

        // Line1 should be in history now
        assert!(term.buffer().history_len() > 0);
    }

    #[test]
    fn modes_tracked() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        assert!(!term.modes().bracketed_paste);
        process(&mut term, b"\x1b[?2004h");
        assert!(term.modes().bracketed_paste);

        process(&mut term, b"\x1b[?2004l");
        assert!(!term.modes().bracketed_paste);
    }

    #[test]
    fn application_cursor_keys() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        assert!(!term.modes().application_cursor_keys);
        process(&mut term, b"\x1b[?1h");
        assert!(term.modes().application_cursor_keys);
    }

    #[test]
    fn mouse_tracking_modes() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Default: no mouse tracking
        assert_eq!(term.modes().mouse_mode, MouseMode::None);
        assert!(!term.mouse_tracking_enabled());

        // X10 mode (press only)
        process(&mut term, b"\x1b[?9h");
        assert_eq!(term.modes().mouse_mode, MouseMode::X10);
        assert!(term.mouse_tracking_enabled());
        process(&mut term, b"\x1b[?9l");
        assert_eq!(term.modes().mouse_mode, MouseMode::None);

        // Normal mode (press and release)
        process(&mut term, b"\x1b[?1000h");
        assert_eq!(term.modes().mouse_mode, MouseMode::Normal);
        assert!(term.mouse_tracking_enabled());
        assert!(!term.track_mouse_motion());
        process(&mut term, b"\x1b[?1000l");
        assert_eq!(term.modes().mouse_mode, MouseMode::None);

        // Button event mode (with motion when pressed)
        process(&mut term, b"\x1b[?1002h");
        assert_eq!(term.modes().mouse_mode, MouseMode::ButtonEvent);
        assert!(term.track_mouse_motion());
        process(&mut term, b"\x1b[?1002l");
        assert_eq!(term.modes().mouse_mode, MouseMode::None);

        // Any event mode (all motion)
        process(&mut term, b"\x1b[?1003h");
        assert_eq!(term.modes().mouse_mode, MouseMode::AnyEvent);
        assert!(term.track_mouse_motion());
        process(&mut term, b"\x1b[?1003l");
        assert_eq!(term.modes().mouse_mode, MouseMode::None);
    }

    #[test]
    fn mouse_encoding_modes() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Default: X10 encoding
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::X10);

        // UTF-8 encoding
        process(&mut term, b"\x1b[?1005h");
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::Utf8);
        process(&mut term, b"\x1b[?1005l");
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::X10);

        // SGR encoding
        process(&mut term, b"\x1b[?1006h");
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::Sgr);
        process(&mut term, b"\x1b[?1006l");
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::X10);

        // URXVT encoding
        process(&mut term, b"\x1b[?1015h");
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::Urxvt);
        process(&mut term, b"\x1b[?1015l");
        assert_eq!(term.modes().mouse_encoding, MouseEncoding::X10);
    }

    #[test]
    fn mouse_event_encoding_sgr() {
        use crate::grid::MouseModifiers;

        let mut term = Terminal::new(Dimensions::new(80, 24));
        let no_mods = MouseModifiers::default();

        // Enable SGR mouse mode
        process(&mut term, b"\x1b[?1000h"); // Normal tracking
        process(&mut term, b"\x1b[?1006h"); // SGR encoding

        // Test left button press at column 10, row 5
        let event = term
            .encode_mouse_event(crate::grid::MouseButton::Left, 10, 5, true, false, no_mods)
            .unwrap();
        assert_eq!(event, b"\x1b[<0;11;6M"); // 0-indexed to 1-indexed

        // Test left button release - SGR uses Left button with pressed=false
        let event = term
            .encode_mouse_event(crate::grid::MouseButton::Left, 10, 5, false, false, no_mods)
            .unwrap();
        assert_eq!(event, b"\x1b[<0;11;6m"); // lowercase 'm' for release

        // Test scroll up
        let event = term
            .encode_mouse_event(
                crate::grid::MouseButton::ScrollUp,
                10,
                5,
                true,
                false,
                no_mods,
            )
            .unwrap();
        assert_eq!(event, b"\x1b[<64;11;6M"); // scroll up is button code 64

        // Test motion tracking
        process(&mut term, b"\x1b[?1003h"); // Any event mode
        let event = term
            .encode_mouse_event(crate::grid::MouseButton::Left, 10, 5, true, true, no_mods)
            .unwrap();
        assert_eq!(event, b"\x1b[<32;11;6M"); // motion flag adds 32
    }

    #[test]
    fn mouse_event_with_modifiers() {
        use crate::grid::MouseModifiers;

        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Enable SGR mouse mode
        process(&mut term, b"\x1b[?1000h"); // Normal tracking
        process(&mut term, b"\x1b[?1006h"); // SGR encoding

        // Test shift+click (adds 4 to button code)
        let shift_mods = MouseModifiers::new(true, false, false, false);
        let event = term
            .encode_mouse_event(
                crate::grid::MouseButton::Left,
                10,
                5,
                true,
                false,
                shift_mods,
            )
            .unwrap();
        assert_eq!(event, b"\x1b[<4;11;6M"); // 0 + 4 = 4

        // Test ctrl+click (adds 16 to button code)
        let ctrl_mods = MouseModifiers::new(false, false, true, false);
        let event = term
            .encode_mouse_event(
                crate::grid::MouseButton::Left,
                10,
                5,
                true,
                false,
                ctrl_mods,
            )
            .unwrap();
        assert_eq!(event, b"\x1b[<16;11;6M"); // 0 + 16 = 16

        // Test alt+click (adds 8 to button code)
        let alt_mods = MouseModifiers::new(false, true, false, false);
        let event = term
            .encode_mouse_event(crate::grid::MouseButton::Left, 10, 5, true, false, alt_mods)
            .unwrap();
        assert_eq!(event, b"\x1b[<8;11;6M"); // 0 + 8 = 8

        // Test shift+ctrl+alt+click (adds 4 + 8 + 16 = 28)
        let all_mods = MouseModifiers::new(true, true, true, false);
        let event = term
            .encode_mouse_event(crate::grid::MouseButton::Left, 10, 5, true, false, all_mods)
            .unwrap();
        assert_eq!(event, b"\x1b[<28;11;6M"); // 0 + 4 + 8 + 16 = 28

        // Test shift+middle click
        let event = term
            .encode_mouse_event(
                crate::grid::MouseButton::Middle,
                10,
                5,
                true,
                false,
                shift_mods,
            )
            .unwrap();
        assert_eq!(event, b"\x1b[<5;11;6M"); // 1 + 4 = 5
    }

    #[test]
    fn alternate_scroll_mode() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Default: enabled (so mouse wheel works in less, vim, etc.)
        assert!(term.modes().alternate_scroll);

        // Disable alternate scroll mode
        process(&mut term, b"\x1b[?1007l");
        assert!(!term.modes().alternate_scroll);

        // Enable alternate scroll mode
        process(&mut term, b"\x1b[?1007h");
        assert!(term.modes().alternate_scroll);
    }

    #[test]
    fn synchronized_update_mode() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Default: disabled
        assert!(!term.modes().synchronized_update);

        // Enable synchronized update mode
        process(&mut term, b"\x1b[?2026h");
        assert!(term.modes().synchronized_update);

        // Disable synchronized update mode
        process(&mut term, b"\x1b[?2026l");
        assert!(!term.modes().synchronized_update);
    }

    #[test]
    fn background_colors_in_alternate_screen() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Enter alternate screen (mode 1049 saves cursor and enters alt screen)
        process(&mut term, b"\x1b[?1049h");
        assert!(term.is_alternate_screen());

        // Set RGB background color (48;2;R;G;B) and print a space
        // This simulates what OpenCode does to draw filled backgrounds
        process(&mut term, b"\x1b[48;2;64;73;74m ");

        // Verify the cell has the correct background color
        let cell = term.grid().cell(0, 0).unwrap();
        assert_eq!(cell.char(), ' ');
        assert_eq!(cell.bg(), Color::Rgb(64, 73, 74));

        // Print more characters with the same background
        process(&mut term, b"ABC");

        // All cells should have the background
        assert_eq!(term.grid().cell(1, 0).unwrap().bg(), Color::Rgb(64, 73, 74));
        assert_eq!(term.grid().cell(2, 0).unwrap().bg(), Color::Rgb(64, 73, 74));
        assert_eq!(term.grid().cell(3, 0).unwrap().bg(), Color::Rgb(64, 73, 74));
    }

    #[test]
    fn osc_66_in_alternate_screen() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Enter alternate screen
        process(&mut term, b"\x1b[?1049h");

        // Set colors and print via OSC 66
        // ESC[38;2;122;132;120m - fg gray
        // ESC[48;2;64;73;74m - bg darker gray
        // OSC 66;w=1;▀ ST - print upper half block
        process(
            &mut term,
            b"\x1b[38;2;122;132;120m\x1b[48;2;64;73;74m\x1b]66;w=1;\xe2\x96\x80\x1b\\",
        );

        let cell = term.grid().cell(0, 0).unwrap();
        assert_eq!(cell.char(), '▀');
        assert_eq!(cell.fg(), Color::Rgb(122, 132, 120));
        assert_eq!(cell.bg(), Color::Rgb(64, 73, 74));
    }
}
