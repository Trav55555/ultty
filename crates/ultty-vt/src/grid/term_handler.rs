//! Handler that applies ANSI parser actions to the Terminal.

use crate::ansi::{
    Action, Attribute, Color as AnsiColor, DynamicColor, Mode, NamedColor, WindowOp,
};
use crate::grid::{
    decode_sixel, format_color_spec, Cell, CellFlags, CursorShape, MouseEncoding, MouseMode,
    Terminal, TerminalImage,
};

use super::cell::{Color, UnderlineStyle};

/// Primary device attributes: a VT220-class terminal (62) with sixel
/// graphics (4) and ANSI colour (22). Nothing it does not implement.
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?62;4;22c";

/// This crate's version as one number, `major * 10000 + minor * 100 +
/// patch` (the form VTE uses), for the secondary device attributes.
fn version_number() -> u32 {
    let part = |s: &str| s.parse::<u32>().unwrap_or(0);
    part(env!("CARGO_PKG_VERSION_MAJOR")) * 10_000
        + part(env!("CARGO_PKG_VERSION_MINOR")) * 100
        + part(env!("CARGO_PKG_VERSION_PATCH"))
}

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

    /// The terminal this handler applies actions to.
    pub fn terminal(&self) -> &Terminal {
        self.terminal
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

            // Protocol data for the embedder; it does not touch the grid.
            Action::InBand(_) => {}

            Action::InlineAnchor {
                session_id,
                surface_id,
                rows,
            } => self.terminal.place_anchor(crate::grid::Anchor {
                session_id,
                surface_id,
                rows,
            }),

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
                // CSI G — move to absolute column, leave row unchanged
                let max_col = self.terminal.grid().dimensions().cols.saturating_sub(1);
                self.terminal.grid_mut().cursor_mut().position.col =
                    col.saturating_sub(1).min(max_col);
                self.terminal.grid_mut().cursor_mut().wrap_pending = false;
            }
            Action::CursorPosition { row, col } => {
                // When origin mode (DECOM) is enabled, cursor positions are relative
                // to the scroll region, not the screen
                let row = if self.terminal.modes().origin_mode {
                    let region = self.terminal.grid().effective_scroll_region();
                    let region_height = region.bottom.saturating_sub(region.top) + 1;
                    // Clamp row to region height, then offset by region.top
                    // (row is 1-based, so region_height is the max valid value)
                    row.min(region_height).saturating_add(region.top)
                } else {
                    row
                };
                self.terminal.grid_mut().set_cursor_position(row, col);
            }
            Action::CursorVerticalAbsolute(row) => {
                // CSI d — move to absolute row, leave column unchanged
                // Respects origin mode like CursorPosition
                let row = if self.terminal.modes().origin_mode {
                    let region = self.terminal.grid().effective_scroll_region();
                    let region_height = region.bottom.saturating_sub(region.top) + 1;
                    row.min(region_height).saturating_add(region.top)
                } else {
                    row
                };
                let max_row = self.terminal.grid().dimensions().rows.saturating_sub(1);
                self.terminal.grid_mut().cursor_mut().position.row =
                    row.saturating_sub(1).min(max_row);
                self.terminal.grid_mut().cursor_mut().wrap_pending = false;
            }
            Action::CursorBackwardTab(n) => {
                self.terminal.grid_mut().backward_tab(n);
            }
            Action::CursorForwardTab(n) => {
                self.terminal.grid_mut().forward_tab(n);
            }
            Action::RepeatLastChar(n) => {
                self.terminal.grid_mut().repeat_last_char(n);
            }
            Action::SetInsertMode(enabled) => {
                self.terminal.grid_mut().set_insert_mode(enabled);
                self.terminal.modes_mut().insert_mode = enabled;
            }

            // Erase operations
            Action::EraseInDisplay(mode) => match mode {
                0 => self.terminal.grid_mut().erase_below(),
                1 => self.terminal.grid_mut().erase_above(),
                2 => self.terminal.grid_mut().clear(),
                // ED 3 clears saved lines only (xterm).
                3 => self.terminal.buffer_mut().clear_history(),
                _ => {}
            },
            Action::EraseInLine(mode) => match mode {
                0 => self.terminal.grid_mut().erase_line_right(),
                1 => self.terminal.grid_mut().erase_line_left(),
                2 => self.terminal.grid_mut().erase_line(),
                _ => {}
            },
            Action::EraseCharacters(n) => {
                let pos = self.terminal.grid().cursor().position;
                self.terminal
                    .grid_mut()
                    .erase_cells(pos.row, pos.col, pos.col.saturating_add(n));
            }

            // Line operations
            Action::InsertLines(n) => {
                self.terminal.grid_mut().insert_lines(n);
                self.terminal.grid_mut().carriage_return();
            }
            Action::DeleteLines(n) => {
                self.terminal.grid_mut().delete_lines(n);
                self.terminal.grid_mut().carriage_return();
            }
            Action::DeleteCharacters(n) => {
                let row_idx = self.terminal.grid().cursor().position.row;
                let col = self.terminal.grid().cursor().position.col;
                let bg = self.terminal.grid().attrs().bg();
                if let Some(row) = self.terminal.grid_mut().row_mut(row_idx) {
                    row.delete_cells(col, n, bg);
                }
            }
            Action::InsertCharacters(n) => {
                let row_idx = self.terminal.grid().cursor().position.row;
                let col = self.terminal.grid().cursor().position.col;
                let bg = self.terminal.grid().attrs().bg();
                if let Some(row) = self.terminal.grid_mut().row_mut(row_idx) {
                    row.insert_cells(col, n, bg);
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
                // When origin mode is active, home to top of scroll region
                if self.terminal.modes().origin_mode {
                    let region = self.terminal.grid().effective_scroll_region();
                    self.terminal.grid_mut().cursor_mut().position.row = region.top;
                }
            }

            // Cursor save/restore (DECSC/DECRC)
            Action::SaveCursorPosition => {
                let origin = self.terminal.modes().origin_mode;
                self.terminal.grid_mut().save_cursor(origin);
            }
            Action::RestoreCursorPosition => {
                if let Some(origin) = self.terminal.grid_mut().restore_cursor() {
                    self.terminal.modes_mut().origin_mode = origin;
                }
            }

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
                let (row, col) = self.cursor_report_position();
                self.reply(format!("\x1b[{row};{col}R"));
            }
            Action::ReportExtendedCursorPosition => {
                let (row, col) = self.cursor_report_position();
                self.reply(format!("\x1b[?{row};{col}R"));
            }
            Action::DeviceStatusReport => self.reply("\x1b[0n".to_string()),
            Action::ReportColorScheme => {
                let report = self.terminal.color_scheme_report();
                self.terminal.queue_response(report);
            }
            Action::DeviceAttributes => {
                self.terminal
                    .queue_response(PRIMARY_DEVICE_ATTRIBUTES.to_vec());
            }
            Action::SecondaryDeviceAttributes => {
                // CSI > Pp ; Pv ; Pc c: VT220 (1), our version, no ROM cartridge.
                self.reply(format!("\x1b[>1;{};0c", version_number()));
            }
            Action::RequestTerminalVersion => {
                self.reply(format!("\x1bP>|ultty {}\x1b\\", env!("CARGO_PKG_VERSION")));
            }
            Action::WindowOp(op) => self.window_op(op),
            Action::RequestStatusString(request) => self.report_status_string(&request),
            Action::RequestTermcap(request) => self.report_termcap(&request),
            Action::RequestAnsiMode(mode) => {
                let status = self.ansi_mode_status(mode);
                self.reply(format!("\x1b[{mode};{status}$y"));
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
                if self.terminal.modes().linefeed_newline {
                    self.terminal.grid_mut().carriage_return();
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
            Action::Notification { title: _, body: _ } | Action::Progress(_) => {
                // Handled by the app, which shows them; no terminal state.
            }
            Action::KittyKeyboardPush(flags) => {
                self.terminal.push_kitty_keyboard_mode(flags);
            }
            Action::KittyKeyboardPop(count) => {
                self.terminal.pop_kitty_keyboard_modes(count);
            }
            Action::KittyKeyboardSet { flags, mode } => {
                self.terminal.set_kitty_keyboard_mode(flags, mode);
            }
            Action::SetModifyOtherKeys(level) => {
                // Levels above 2 are xterm's 3, which adds nothing we send.
                self.terminal.modes_mut().modify_other_keys = level.min(2);
            }
            Action::KittyKeyboardQuery => {
                // Send current keyboard mode flags
                let flags = self.terminal.modes().kitty_keyboard_flags;
                let response = format!("\x1b[?{}u", flags);
                self.terminal.queue_response(response.into_bytes());
            }
            // Colour queries answer from the theme the embedder set with
            // `Terminal::set_theme_colors`, in order with every other reply,
            // so a query followed by a DA1 sentinel is never misread.
            Action::QueryDynamicColor { color, terminator } => {
                let theme = self.terminal.palette().theme();
                let rgb = match color {
                    DynamicColor::Foreground => theme.foreground,
                    DynamicColor::Background => theme.background,
                    DynamicColor::Cursor => theme.cursor,
                };
                self.reply(format!(
                    "\x1b]{};{}{}",
                    color.osc_code(),
                    format_color_spec(rgb),
                    terminator.as_str()
                ));
            }
            Action::QueryPaletteColor { index, terminator } => {
                let rgb = self.terminal.palette().color(index);
                self.reply(format!(
                    "\x1b]4;{index};{}{}",
                    format_color_spec(rgb),
                    terminator.as_str()
                ));
            }
            Action::SetPaletteColor { index, rgb } => self.terminal.palette_mut().set(index, rgb),
            Action::ResetPaletteColor(index) => self.terminal.palette_mut().reset(index),
            Action::RequestPrivateMode(mode) => {
                let status = self.private_mode_status(mode);
                self.reply(format!("\x1b[?{mode};{status}$y"));
            }
            Action::UnknownOsc(_) => {
                // Ignore unknown OSC sequences
            }

            // Full reset (RIS — ESC c)
            Action::FullReset => {
                self.terminal.set_grapheme_clustering(true);
                self.terminal.grid_mut().clear();
                self.terminal.grid_mut().reset_attrs();
                self.terminal.grid_mut().reset_scroll_region();
                self.terminal.grid_mut().set_auto_wrap(true);
                self.terminal.grid_mut().set_insert_mode(false);
                self.terminal.grid_mut().cursor_mut().position =
                    crate::grid::CursorPosition::origin();
                self.terminal.grid_mut().cursor_mut().visible = true;
                self.terminal.grid_mut().cursor_mut().wrap_pending = false;
                self.terminal.grid_mut().reset_tab_stops();
                *self.terminal.modes_mut() = crate::grid::terminal::TerminalModes::default();
                if self.terminal.is_alternate_screen() {
                    self.terminal.leave_alternate_screen();
                }
                self.terminal.reset_auxiliary_state();
            }

            // Soft reset (DECSTR — CSI ! p). Per DEC and xterm it leaves the
            // cursor and the screen where they are and resets: SGR, margins,
            // DECTCEM, IRM, DECOM, DECAWM (on, xterm's default), DECCKM,
            // DECKPAM and the saved cursor. The parser resets charsets.
            Action::SoftReset => {
                let grid = self.terminal.grid_mut();
                grid.reset_attrs();
                grid.reset_scroll_region();
                grid.set_auto_wrap(true);
                grid.set_insert_mode(false);
                grid.reset_saved_cursor();
                grid.cursor_mut().visible = true;
                grid.cursor_mut().wrap_pending = false;
                let modes = self.terminal.modes_mut();
                modes.origin_mode = false;
                modes.insert_mode = false;
                modes.auto_wrap = true;
                modes.application_cursor_keys = false;
                modes.application_keypad = false;
            }

            // Screen alignment pattern (DECALN — ESC # 8)
            Action::ScreenAlignmentPattern => {
                self.terminal.grid_mut().fill_with_char('E');
                self.terminal.grid_mut().reset_scroll_region();
                self.terminal.grid_mut().cursor_mut().position =
                    crate::grid::CursorPosition::origin();
                self.terminal.grid_mut().cursor_mut().wrap_pending = false;
            }

            // Keypad modes
            Action::ApplicationKeypadMode => {
                self.terminal.modes_mut().application_keypad = true;
            }
            Action::NormalKeypadMode => {
                self.terminal.modes_mut().application_keypad = false;
            }
        }
    }

    fn reply(&mut self, reply: String) {
        self.terminal.queue_response(reply.into_bytes());
    }

    /// The cursor as CPR reports it: 1-based, and relative to the scroll
    /// region in origin mode.
    fn cursor_report_position(&self) -> (u16, u16) {
        let grid = self.terminal.grid();
        let position = grid.cursor().position;
        let row = if self.terminal.modes().origin_mode {
            position
                .row
                .saturating_sub(grid.effective_scroll_region().top)
        } else {
            position.row
        };
        (row + 1, position.col + 1)
    }

    /// DECRQM status for a DEC private mode: 1 set, 2 reset, 0 unknown.
    fn private_mode_status(&self, mode: u16) -> u8 {
        let terminal = &*self.terminal;
        let modes = terminal.modes();
        let grid = terminal.grid();
        let set = match mode {
            1 => modes.application_cursor_keys,
            3 => modes.column_mode_132,
            5 => modes.reverse_video,
            6 => modes.origin_mode,
            7 => grid.auto_wrap(),
            9 => modes.mouse_mode == MouseMode::X10,
            12 => grid.cursor().blinking,
            25 => grid.cursor().visible,
            40 => modes.allow_column_mode_switch,
            47 | 1047 | 1049 => terminal.is_alternate_screen(),
            1000 => modes.mouse_mode == MouseMode::Normal,
            1002 => modes.mouse_mode == MouseMode::ButtonEvent,
            1003 => modes.mouse_mode == MouseMode::AnyEvent,
            1004 => modes.focus_reporting,
            1005 => modes.mouse_encoding == MouseEncoding::Utf8,
            1006 => modes.mouse_encoding == MouseEncoding::Sgr,
            1007 => modes.alternate_scroll,
            1015 => modes.mouse_encoding == MouseEncoding::Urxvt,
            // An action rather than a state: report it reset.
            1048 => false,
            2004 => modes.bracketed_paste,
            2026 => modes.synchronized_update,
            2027 => grid.grapheme_clustering(),
            2031 => modes.color_scheme_updates,
            _ => return 0,
        };
        if set {
            1
        } else {
            2
        }
    }

    /// DECRQM status for an ANSI mode: 1 set, 2 reset, 3 permanently set,
    /// 4 permanently reset, 0 unknown.
    fn ansi_mode_status(&self, mode: u16) -> u8 {
        let set = match mode {
            // KAM: the keyboard never locks.
            2 => return 4,
            4 => self.terminal.grid().insert_mode(),
            // SRM: there is no local echo.
            12 => return 3,
            20 => self.terminal.modes().linefeed_newline,
            _ => return 0,
        };
        if set {
            1
        } else {
            2
        }
    }

    fn window_op(&mut self, op: WindowOp) {
        let dims = self.terminal.dimensions();
        let cell = self.terminal.cell_pixel_size();
        match op {
            WindowOp::ReportTextAreaChars => {
                self.reply(format!("\x1b[8;{};{}t", dims.rows, dims.cols));
            }
            WindowOp::ReportCellPixels => {
                if let Some((width, height)) = cell {
                    self.reply(format!("\x1b[6;{height};{width}t"));
                }
            }
            WindowOp::ReportTextAreaPixels => {
                if let Some((width, height)) = cell {
                    let height = u32::from(height) * u32::from(dims.rows);
                    let width = u32::from(width) * u32::from(dims.cols);
                    self.reply(format!("\x1b[4;{height};{width}t"));
                }
            }
            // There is one title, the window's; icon-only (1) requests leave
            // it alone so a balanced push/pop of icon and window titles
            // restores it once.
            WindowOp::PushTitle(which) if which != 1 => self.terminal.push_title(),
            WindowOp::PopTitle(which) if which != 1 => {
                self.terminal.pop_title();
            }
            WindowOp::PushTitle(_) | WindowOp::PopTitle(_) => {}
        }
    }

    /// Answer DECRQSS for SGR, DECSTBM and DECSCUSR.
    fn report_status_string(&mut self, request: &str) {
        let grid = self.terminal.grid();
        let setting = match request {
            "m" => Some(format!("{}m", sgr_report(grid.attrs()))),
            "r" => {
                let region = grid.effective_scroll_region();
                Some(format!("{};{}r", region.top + 1, region.bottom + 1))
            }
            " q" => Some(format!("{} q", cursor_style_number(grid.cursor()))),
            _ => None,
        };
        self.reply(match setting {
            Some(setting) => format!("\x1bP1$r{setting}\x1b\\"),
            None => "\x1bP0$r\x1b\\".to_string(),
        });
    }

    /// Answer XTGETTCAP, one reply per requested name.
    fn report_termcap(&mut self, request: &str) {
        for name_hex in request.split(';') {
            let answer = decode_hex(name_hex).and_then(|name| {
                termcap_value(&name, self.terminal.terminal_name()).map(|value| (name, value))
            });
            self.reply(match answer {
                Some((name, value)) => {
                    format!("\x1bP1+r{}={}\x1b\\", encode_hex(&name), encode_hex(&value))
                }
                // Echo the name only when it is plain hex: anything else
                // came from the program and, sent back as input, would be
                // typed into whatever reads the terminal (a shell).
                None if !name_hex.is_empty() && name_hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    format!("\x1bP0+r{name_hex}\x1b\\")
                }
                None => "\x1bP0+r\x1b\\".to_string(),
            });
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
            Attribute::Underline => new_attrs.set_underline_style(UnderlineStyle::Single),
            Attribute::Blink => new_attrs.flags_mut().set(CellFlags::BLINK),
            Attribute::Reverse => new_attrs.flags_mut().set(CellFlags::REVERSE),
            Attribute::Hidden => new_attrs.flags_mut().set(CellFlags::HIDDEN),
            Attribute::Strikethrough => new_attrs.flags_mut().set(CellFlags::STRIKETHROUGH),
            Attribute::DoubleUnderline => new_attrs.set_underline_style(UnderlineStyle::Double),
            Attribute::CurlyUnderline => new_attrs.set_underline_style(UnderlineStyle::Curly),
            Attribute::DottedUnderline => new_attrs.set_underline_style(UnderlineStyle::Dotted),
            Attribute::DashedUnderline => new_attrs.set_underline_style(UnderlineStyle::Dashed),
            Attribute::UnderlineColor(color) => {
                new_attrs.set_underline_color(convert_color(color));
            }
            Attribute::Overline => new_attrs.flags_mut().set(CellFlags::OVERLINE),

            Attribute::NoBold => {
                // SGR 22: reset both BOLD and DIM per ECMA-48
                new_attrs.flags_mut().unset(CellFlags::BOLD);
                new_attrs.flags_mut().unset(CellFlags::DIM);
            }
            Attribute::NoItalic => new_attrs.flags_mut().unset(CellFlags::ITALIC),
            Attribute::NoUnderline => new_attrs.set_underline_style(UnderlineStyle::None),
            Attribute::NoBlink => new_attrs.flags_mut().unset(CellFlags::BLINK),
            Attribute::NoReverse => new_attrs.flags_mut().unset(CellFlags::REVERSE),
            Attribute::NoHidden => new_attrs.flags_mut().unset(CellFlags::HIDDEN),
            Attribute::NoStrikethrough => new_attrs.flags_mut().unset(CellFlags::STRIKETHROUGH),
            Attribute::NoOverline => new_attrs.flags_mut().unset(CellFlags::OVERLINE),

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
                // Setting or resetting DECOM homes the cursor to the new
                // origin: the region's top-left, or the screen's.
                self.terminal.modes_mut().origin_mode = enabled;
                let top = if enabled {
                    self.terminal.grid().effective_scroll_region().top
                } else {
                    0
                };
                let cursor = self.terminal.grid_mut().cursor_mut();
                cursor.position = crate::grid::CursorPosition::new(0, top);
                cursor.wrap_pending = false;
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
            Mode::AllowColumnModeSwitch => {
                self.terminal.modes_mut().allow_column_mode_switch = enabled;
            }
            Mode::ColumnMode132 => {
                // xterm ignores DECCOLM unless mode 40 allows it. Without the
                // gate, `reset` would shrink every window to 80 columns.
                if !self.terminal.modes().allow_column_mode_switch {
                    return;
                }
                self.terminal.modes_mut().column_mode_132 = enabled;
                // Request the resize from the app.
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
                    self.terminal.enter_alternate_screen_clear();
                } else {
                    self.terminal.leave_alternate_screen();
                }
            }
            Mode::AlternateScreenSaveCursor => {
                if enabled {
                    let origin = self.terminal.modes().origin_mode;
                    self.terminal.grid_mut().save_cursor(origin);
                    self.terminal.enter_alternate_screen_clear();
                } else {
                    self.terminal.leave_alternate_screen();
                    self.terminal.grid_mut().restore_cursor();
                }
            }
            Mode::SaveCursor => {
                if enabled {
                    let origin = self.terminal.modes().origin_mode;
                    self.terminal.grid_mut().save_cursor(origin);
                } else if let Some(origin) = self.terminal.grid_mut().restore_cursor() {
                    self.terminal.modes_mut().origin_mode = origin;
                }
            }
            Mode::LineFeedNewLine => {
                self.terminal.modes_mut().linefeed_newline = enabled;
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
            Mode::GraphemeClustering => {
                self.terminal.set_grapheme_clustering(enabled);
            }
            Mode::ColorSchemeUpdates => {
                self.terminal.modes_mut().color_scheme_updates = enabled;
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

        // Size the image in cells with the cell size the embedder reported,
        // the one XTWINOPS 16 tells applications, so an image drawn for it
        // covers exactly the cells it was made for. Without one, assume a
        // common 8x16 cell.
        let (cell_pixel_width, cell_pixel_height) = self
            .terminal
            .cell_pixel_size()
            .filter(|&(w, h)| w > 0 && h > 0)
            .map_or((8, 16), |(w, h)| (u32::from(w), u32::from(h)));

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

    /// Write a contiguous printable ASCII span to the active terminal buffer.
    pub fn handle_ascii_span(&mut self, bytes: &[u8]) {
        if !self.terminal.is_alternate_screen() {
            self.terminal.buffer_mut().write_ascii_span(bytes);
        } else {
            self.terminal.grid_mut().write_ascii_span(bytes);
        }
    }
}

/// The SGR parameters that recreate `attrs` from a reset, for DECRQSS.
fn sgr_report(attrs: &Cell) -> String {
    let mut params = vec!["0".to_string()];
    let underline = match attrs.underline_style() {
        UnderlineStyle::None => None,
        UnderlineStyle::Single => Some("4"),
        UnderlineStyle::Double => Some("21"),
        UnderlineStyle::Curly => Some("4:3"),
        UnderlineStyle::Dotted => Some("4:4"),
        UnderlineStyle::Dashed => Some("4:5"),
    };
    let flag = |flag, param: &'static str| attrs.has_flag(flag).then_some(param);
    let attributes = [
        flag(CellFlags::BOLD, "1"),
        flag(CellFlags::DIM, "2"),
        flag(CellFlags::ITALIC, "3"),
        underline,
        flag(CellFlags::BLINK, "5"),
        flag(CellFlags::REVERSE, "7"),
        flag(CellFlags::HIDDEN, "8"),
        flag(CellFlags::STRIKETHROUGH, "9"),
        flag(CellFlags::OVERLINE, "53"),
    ];
    params.extend(attributes.into_iter().flatten().map(str::to_string));
    for (color, base) in [(attrs.fg(), 30), (attrs.bg(), 40)] {
        match color {
            Color::Default => {}
            Color::Named(n) if n < 8 => params.push((base + u16::from(n)).to_string()),
            Color::Named(n) => params.push((base + 60 + u16::from(n % 16 - 8)).to_string()),
            Color::Indexed(n) => params.push(format!("{};5;{n}", base + 8)),
            Color::Rgb(r, g, b) => params.push(format!("{};2;{r};{g};{b}", base + 8)),
        }
    }
    match attrs.underline_color() {
        Color::Default => {}
        Color::Rgb(r, g, b) => params.push(format!("58:2::{r}:{g}:{b}")),
        Color::Named(n) => params.push(format!("58:5:{n}")),
        Color::Indexed(n) => params.push(format!("58:5:{n}")),
    }
    params.join(";")
}

/// The DECSCUSR number for a cursor's shape and blinking.
fn cursor_style_number(cursor: &crate::grid::Cursor) -> u8 {
    let steady = u8::from(!cursor.blinking);
    match cursor.shape {
        CursorShape::Block => 1 + steady,
        CursorShape::Underline => 3 + steady,
        CursorShape::Bar => 5 + steady,
    }
}

/// The terminfo capabilities XTGETTCAP answers. Only what is true of
/// Ultty: the `TERM` in use (`terminal_name`), 256 indexed colours, and
/// 24-bit direct colour.
fn termcap_value(name: &str, terminal_name: &str) -> Option<String> {
    match name {
        "TN" | "name" => Some(terminal_name.to_string()),
        "Co" | "colors" => Some("256".to_string()),
        "RGB" => Some("8/8/8".to_string()),
        _ => None,
    }
}

/// Decode XTGETTCAP's hex-encoded name; `None` unless it is whole bytes of
/// hex digits forming UTF-8.
fn decode_hex(hex: &str) -> Option<String> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

fn encode_hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02X}")).collect()
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
    use crate::ansi::AnsiParser;
    use crate::grid::{cell::Color, Dimensions};

    fn process(terminal: &mut Terminal, input: &[u8]) {
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(terminal);
        parser.feed(input, |action| handler.handle(action));
    }

    fn cursor_col(term: &Terminal) -> u16 {
        term.grid().cursor().position.col
    }

    #[test]
    fn xtmodkeys_sets_the_modify_other_keys_level() {
        let mut term = Terminal::new(Dimensions::new(10, 5));
        process(&mut term, b"\x1b[>4;2m");
        assert_eq!(term.modes().modify_other_keys, 2);
        assert!(term.grid().attrs().flags().is_empty(), "not SGR");
        process(&mut term, b"\x1b[>4;1m");
        assert_eq!(term.modes().modify_other_keys, 1);
        process(&mut term, b"\x1b[>4m");
        assert_eq!(term.modes().modify_other_keys, 0);
        process(&mut term, b"\x1b[>4;2m\x1b[>4n");
        assert_eq!(term.modes().modify_other_keys, 0);
    }

    #[test]
    fn maximal_cursor_motion_parameters_clamp_instead_of_overflowing() {
        let mut term = Terminal::new(Dimensions::new(10, 5));
        process(&mut term, b"\x1b[3;3H\x1b[65535B\x1b[65535C");
        assert_eq!(term.grid().cursor().position.row, 4);
        assert_eq!(term.grid().cursor().position.col, 9);
    }

    #[test]
    fn screen_alignment_replaces_wide_chars_with_plain_cells() {
        let mut term = Terminal::new(Dimensions::new(3, 2));
        process(&mut term, "\x1b[2G\u{4e2d}\x1b#8\u{4e2d}".as_bytes());
        assert_eq!(term.check_integrity(), Ok(()));
        assert_eq!(term.grid().cell(2, 0).unwrap().char(), 'E');
    }

    #[test]
    fn wide_char_overlapping_the_next_wide_char_clears_its_spacer() {
        let mut term = Terminal::new(Dimensions::new(6, 1));
        // Wide char at cols 1-2, then a wide char at cols 0-1 overwrites its
        // first half; col 2 must not keep a spacer with no primary.
        process(&mut term, "a\u{4e2d}\r\u{4e2d}".as_bytes());
        assert_eq!(term.check_integrity(), Ok(()));
        assert_eq!(term.grid().cell(2, 0).unwrap().char(), ' ');
    }

    #[test]
    fn partial_erases_never_split_a_wide_char() {
        // Cursor on the spacer of a wide char at cols 2-3.
        for erase in [&b"\x1b[J"[..], b"\x1b[K", b"\x1b[1K", b"\x1b[1J", b"\x1b[X"] {
            let mut term = Terminal::new(Dimensions::new(6, 2));
            process(&mut term, "ab\u{4e2d}cd\x1b[1;4H".as_bytes());
            process(&mut term, erase);
            assert_eq!(term.check_integrity(), Ok(()), "after {erase:?}");
            assert!(
                !term.grid().cell(2, 0).unwrap().is_wide(),
                "after {erase:?}"
            );
        }
    }

    #[test]
    fn copied_text_keeps_graphemes_and_skips_wide_spacers() {
        use crate::grid::{SelectionMode, SelectionPoint};

        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(
            &mut term,
            "e\u{301}\u{4e2d}\u{6587}\u{2764}\u{fe0f}!".as_bytes(),
        );
        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(19, 0));
        assert_eq!(
            term.selected_text().as_deref(),
            Some("e\u{301}\u{4e2d}\u{6587}\u{2764}\u{fe0f}!")
        );
    }

    #[test]
    fn combining_mark_survives_scroll() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        process(&mut term, "a\r\ne\u{301}\r\nx\r\ny".as_bytes());
        // One line scrolled off; the accented line is now the top row.
        assert_eq!(term.grid().grapheme(0, 0).as_deref(), Some("e\u{301}"));
    }

    #[test]
    fn combining_mark_survives_into_scrollback_and_reflow() {
        let mut term = Terminal::new(Dimensions::new(10, 2));
        process(&mut term, "e\u{301}\r\nx\r\ny".as_bytes());
        let mut history = term.buffer().clone();
        history.scroll_up(1);
        let top = history.visible_row(0).expect("history row");
        assert_eq!(top.copy_text(0, 0), "e\u{301}");
        assert_eq!(top.text_trimmed(), "e\u{301}");

        term.resize(Dimensions::new(6, 2));
        let mut history = term.buffer().clone();
        history.scroll_up(1);
        let top = history.visible_row(0).expect("history row after reflow");
        assert_eq!(top.copy_text(0, 0), "e\u{301}");
    }

    #[test]
    fn insert_and_delete_chars_move_combining_marks() {
        let mut term = Terminal::new(Dimensions::new(10, 2));
        process(&mut term, "ae\u{301}".as_bytes());
        process(&mut term, b"\x1b[1G\x1b[@"); // ICH 1 at column 0
        assert_eq!(term.grid().grapheme(0, 2).as_deref(), Some("e\u{301}"));
        assert_eq!(term.grid().grapheme(0, 1).as_deref(), Some("a"));
        process(&mut term, b"\x1b[1G\x1b[2P"); // DCH 2 at column 0
        assert_eq!(term.grid().grapheme(0, 0).as_deref(), Some("e\u{301}"));
        assert!(term.grid().combining_chars(0, 2).is_none());
    }

    #[test]
    fn two_combining_marks_stack_on_one_base_cell() {
        let mut term = Terminal::new(Dimensions::new(10, 2));
        process(&mut term, "e\u{301}\u{303}x".as_bytes());
        assert_eq!(
            term.grid().grapheme(0, 0).as_deref(),
            Some("e\u{301}\u{303}")
        );
        assert_eq!(term.grid().cell(1, 0).unwrap().char(), 'x');
        assert_eq!(cursor_col(&term), 2);
    }

    #[test]
    fn four_person_family_occupies_one_wide_cell() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(&mut term, family.as_bytes());
        assert_eq!(cursor_col(&term), 2);
        assert_eq!(term.grid().grapheme(0, 0).as_deref(), Some(family));
        assert!(term.grid().cell(1, 0).unwrap().is_wide_spacer());
    }

    #[test]
    fn zwj_sequence_occupies_one_wide_cell() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(&mut term, family.as_bytes());
        assert_eq!(cursor_col(&term), 2);
        assert_eq!(term.grid().grapheme(0, 0).as_deref(), Some(family));
        assert!(term.grid().cell(1, 0).unwrap().is_wide_spacer());
    }

    #[test]
    fn emoji_presentation_selector_widens_narrow_base() {
        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(&mut term, "\u{2764}\u{fe0f}x".as_bytes());
        assert!(term.grid().cell(0, 0).unwrap().is_wide());
        assert!(term.grid().cell(1, 0).unwrap().is_wide_spacer());
        assert_eq!(term.grid().cell(2, 0).unwrap().char(), 'x');
        assert_eq!(cursor_col(&term), 3);
    }

    #[test]
    fn regional_indicators_pair_into_flags() {
        let mut term = Terminal::new(Dimensions::new(20, 2));
        // Two flags: US then DE.
        process(&mut term, "\u{1f1fa}\u{1f1f8}\u{1f1e9}\u{1f1ea}".as_bytes());
        assert_eq!(cursor_col(&term), 4);
        assert_eq!(
            term.grid().grapheme(0, 0).as_deref(),
            Some("\u{1f1fa}\u{1f1f8}")
        );
        assert_eq!(
            term.grid().grapheme(0, 2).as_deref(),
            Some("\u{1f1e9}\u{1f1ea}")
        );
    }

    #[test]
    fn skin_tone_modifier_joins_its_base() {
        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(&mut term, "\u{1f44d}\u{1f3fd}".as_bytes());
        assert_eq!(cursor_col(&term), 2);
    }

    #[test]
    fn cursor_motion_breaks_the_cluster() {
        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(&mut term, "\u{1f1fa}\x1b[1;6H\u{1f1f8}".as_bytes());
        assert_eq!(term.grid().grapheme(0, 0).as_deref(), Some("\u{1f1fa}"));
        assert_eq!(term.grid().grapheme(0, 5).as_deref(), Some("\u{1f1f8}"));
    }

    #[test]
    fn cluster_continuation_at_right_margin_does_not_wrap() {
        let mut term = Terminal::new(Dimensions::new(4, 2));
        process(&mut term, "ab\u{1f468}\u{200d}\u{1f469}".as_bytes());
        assert_eq!(term.grid().cursor().position.row, 0);
        assert_eq!(
            term.grid().grapheme(0, 2).as_deref(),
            Some("\u{1f468}\u{200d}\u{1f469}")
        );
    }

    #[test]
    fn mode_2027_reset_restores_per_codepoint_widths() {
        let mut term = Terminal::new(Dimensions::new(20, 2));
        process(&mut term, b"\x1b[?2027$p");
        assert_eq!(term.take_responses(), vec![b"\x1b[?2027;1$y".to_vec()]);

        process(&mut term, b"\x1b[?2027l\x1b[?2027$p");
        assert_eq!(term.take_responses(), vec![b"\x1b[?2027;2$y".to_vec()]);
        process(&mut term, "\u{1f468}\u{200d}\u{1f469}".as_bytes());
        assert_eq!(cursor_col(&term), 4);

        process(&mut term, b"\x1bc");
        assert!(term.grid().grapheme_clustering());
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

    /// What Claude Code's full mouse mode (1000, 1002, 1003, 1006) needs: a
    /// drag is the held button plus 32, motion with no button is 35, and
    /// button-event mode (1002 alone) reports only drags.
    #[test]
    fn mouse_motion_reports_drags_and_buttonless_motion_by_mode() {
        use crate::grid::{MouseButton, MouseModifiers};
        let mut term = Terminal::new(Dimensions::new(80, 24));
        let none = MouseModifiers::default();

        process(&mut term, b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
        let drag = term.encode_mouse_event(MouseButton::Left, 3, 4, true, true, none);
        assert_eq!(drag.as_deref(), Some(&b"\x1b[<32;4;5M"[..]));
        let hover = term.encode_mouse_event(MouseButton::Release, 3, 4, true, true, none);
        assert_eq!(hover.as_deref(), Some(&b"\x1b[<35;4;5M"[..]));

        process(&mut term, b"\x1b[?1003l\x1b[?1002h");
        let drag = term.encode_mouse_event(MouseButton::Right, 3, 4, true, true, none);
        assert_eq!(drag.as_deref(), Some(&b"\x1b[<34;4;5M"[..]));
        let hover = term.encode_mouse_event(MouseButton::Release, 3, 4, true, true, none);
        assert_eq!(hover, None, "1002 reports motion only with a button held");

        process(&mut term, b"\x1b[?1002l\x1b[?1000h");
        let drag = term.encode_mouse_event(MouseButton::Left, 3, 4, true, true, none);
        assert_eq!(drag, None, "1000 reports no motion");
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

    #[test]
    fn cha_moves_column_only() {
        let mut term = Terminal::new(Dimensions::new(80, 24));
        // Move to row 10, col 5
        process(&mut term, b"\x1b[10;5H");
        assert_eq!(term.grid().cursor().position.row, 9);
        assert_eq!(term.grid().cursor().position.col, 4);

        // CHA to column 30 — row should stay at 9
        process(&mut term, b"\x1b[30G");
        assert_eq!(term.grid().cursor().position.row, 9);
        assert_eq!(term.grid().cursor().position.col, 29);
    }

    #[test]
    fn cha_clamps_to_grid_width() {
        let mut term = Terminal::new(Dimensions::new(10, 5));
        // CHA to column 999 — should clamp to rightmost column
        process(&mut term, b"\x1b[999G");
        assert_eq!(term.grid().cursor().position.col, 9);
    }

    #[test]
    fn cha_clears_wrap_pending() {
        let mut term = Terminal::new(Dimensions::new(5, 3));
        // Fill row to trigger wrap_pending
        process(&mut term, b"ABCDE");
        assert!(term.grid().cursor().wrap_pending);

        // CHA should clear wrap_pending
        process(&mut term, b"\x1b[1G");
        assert!(!term.grid().cursor().wrap_pending);
        assert_eq!(term.grid().cursor().position.col, 0);
    }

    #[test]
    fn stale_scroll_region_survives_reflow_resize_without_panic() {
        let mut term = Terminal::new(Dimensions::new(80, 40));
        process(&mut term, b"\x1b[32;40r");
        assert_eq!(
            term.grid().scroll_region().map(|r| (r.top, r.bottom)),
            Some((31, 39))
        );

        // Width change goes through Buffer::reflow_all -> Grid::set_rows
        term.resize(Dimensions::new(60, 20));

        process(&mut term, b"\x1bM");
        process(&mut term, b"\x1b[S");
        process(&mut term, b"\x1b[T");
        process(&mut term, b"\n\n\n");
        assert!(term.grid().cursor().position.row < 20);
    }

    #[test]
    fn ris_resets_charset_designation() {
        // Charset state lives in the parser, so keep one parser across steps.
        let mut term = Terminal::new(Dimensions::new(10, 3));
        let mut parser = AnsiParser::new();

        let mut handler = TerminalHandler::new(&mut term);
        parser.feed(b"\x1b(0a", |action| handler.handle(action));
        // 'a' in DEC Special Graphics is a checkerboard, not a letter
        assert_ne!(term.grid().cell(0, 0).unwrap().char(), 'a');

        let mut handler = TerminalHandler::new(&mut term);
        parser.feed(b"\x1bca", |action| handler.handle(action));
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'a');
    }

    #[test]
    fn ris_clears_alternate_screen_images_and_hyperlinks() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        process(
            &mut term,
            b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        );
        assert_eq!(term.all_hyperlinks().len(), 1);
        process(&mut term, b"\x1b[?1049h");
        process(&mut term, b"ALT");
        process(&mut term, b"\x1b[?1049l");

        process(&mut term, b"\x1bc");

        assert!(term.all_hyperlinks().is_empty());
        assert!(!term.is_alternate_screen());
        term.enter_alternate_screen();
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), ' ');
    }
}
