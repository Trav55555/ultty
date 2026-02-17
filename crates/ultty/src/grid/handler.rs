//! Handler that applies ANSI parser actions to the grid.

use crate::grid::{CellFlags, Grid};
use crate::protocol::ansi::{Action, Attribute, Color as AnsiColor, Mode, NamedColor};

use super::cell::Color;

/// Applies parser actions to a grid.
///
/// This handler is the bridge between the ANSI parser and the terminal grid.
/// It translates parser [`Action`]s into grid mutations.
pub struct Handler<'a> {
    grid: &'a mut Grid,
}

impl<'a> Handler<'a> {
    /// Create a new handler for the given grid.
    pub fn new(grid: &'a mut Grid) -> Self {
        Self { grid }
    }

    /// Handle a single parser action.
    pub fn handle(&mut self, action: Action) {
        match action {
            Action::Print(c) => self.grid.write_char(c),

            Action::Execute(_code) => {
                // C0 controls handled separately
            }

            // Cursor movement
            Action::CursorUp(n) => self.grid.cursor_up(n),
            Action::CursorDown(n) => self.grid.cursor_down(n),
            Action::CursorForward(n) => self.grid.cursor_right(n),
            Action::CursorBack(n) => self.grid.cursor_left(n),
            Action::CursorNextLine(n) => {
                self.grid.cursor_down(n);
                self.grid.carriage_return();
            }
            Action::CursorPreviousLine(n) => {
                self.grid.cursor_up(n);
                self.grid.carriage_return();
            }
            Action::CursorHorizontalAbsolute(col) => {
                // CSI G — move to absolute column, leave row unchanged
                let max_col = self.grid.dimensions().cols.saturating_sub(1);
                self.grid.cursor_mut().position.col = col.saturating_sub(1).min(max_col);
                self.grid.cursor_mut().wrap_pending = false;
            }
            Action::CursorPosition { row, col } => {
                self.grid.set_cursor_position(row, col);
            }
            Action::CursorVerticalAbsolute(row) => {
                // CSI d — move to absolute row, leave column unchanged
                let max_row = self.grid.dimensions().rows.saturating_sub(1);
                self.grid.cursor_mut().position.row = row.saturating_sub(1).min(max_row);
                self.grid.cursor_mut().wrap_pending = false;
            }
            Action::CursorBackwardTab(n) => {
                self.grid.backward_tab(n);
            }
            Action::CursorForwardTab(n) => {
                self.grid.forward_tab(n);
            }
            Action::RepeatLastChar(n) => {
                self.grid.repeat_last_char(n);
            }
            Action::SetInsertMode(enabled) => {
                self.grid.set_insert_mode(enabled);
            }

            // Erase operations
            Action::EraseInDisplay(mode) => match mode {
                0 => self.grid.erase_below(),
                1 => self.grid.erase_above(),
                2 | 3 => self.grid.clear(),
                _ => {}
            },
            Action::EraseInLine(mode) => match mode {
                0 => self.grid.erase_line_right(),
                1 => self.grid.erase_line_left(),
                2 => self.grid.erase_line(),
                _ => {}
            },
            Action::EraseCharacters(n) => {
                let col = self.grid.cursor().position.col;
                let row = self.grid.cursor().position.row;
                let cols = self.grid.dimensions().cols;
                let bg = self.grid.attrs().bg();
                let end = col.saturating_add(n).min(cols);

                // Check for orphaned wide char at the start boundary
                if col > 0 && self.grid.cell(col, row).is_some_and(|c| c.is_wide_spacer()) {
                    if let Some(primary) = self.grid.cell_mut(col - 1, row) {
                        primary.erase(bg);
                    }
                }
                // Check for orphaned wide char spacer at the end boundary
                if end < cols && self.grid.cell(end, row).is_some_and(|c| c.is_wide_spacer()) {
                    if let Some(spacer) = self.grid.cell_mut(end, row) {
                        spacer.erase(bg);
                    }
                }

                for c in col..end {
                    if let Some(cell) = self.grid.cell_mut(c, row) {
                        cell.erase(bg);
                    }
                }
            }

            // Line operations
            Action::InsertLines(n) => {
                self.grid.insert_lines(n);
                self.grid.carriage_return();
            }
            Action::DeleteLines(n) => {
                self.grid.delete_lines(n);
                self.grid.carriage_return();
            }
            Action::DeleteCharacters(n) => {
                let row_idx = self.grid.cursor().position.row;
                let col = self.grid.cursor().position.col;
                let bg = self.grid.attrs().bg();
                if let Some(row) = self.grid.row_mut(row_idx) {
                    row.delete_cells(col, n, bg);
                }
            }
            Action::InsertCharacters(n) => {
                let row_idx = self.grid.cursor().position.row;
                let col = self.grid.cursor().position.col;
                let bg = self.grid.attrs().bg();
                if let Some(row) = self.grid.row_mut(row_idx) {
                    row.insert_cells(col, n, bg);
                }
            }

            // Scrolling
            Action::ScrollUp(n) => self.grid.scroll_up(n),
            Action::ScrollDown(n) => self.grid.scroll_down(n),
            Action::ReverseIndex => {
                // Move cursor up, scroll down if at top of scroll region
                let region = self.grid.effective_scroll_region();
                let cursor_row = self.grid.cursor().position.row;
                if cursor_row <= region.top {
                    self.grid.scroll_down(1);
                } else {
                    self.grid.cursor_up(1);
                }
            }
            Action::Index => {
                // Move cursor down, scroll up if at bottom of scroll region
                let region = self.grid.effective_scroll_region();
                let cursor_row = self.grid.cursor().position.row;
                if cursor_row >= region.bottom {
                    self.grid.scroll_up(1);
                } else {
                    self.grid.cursor_down(1);
                }
            }
            Action::NextLine => {
                // Move to beginning of next line (CR + Index)
                self.grid.carriage_return();
                let region = self.grid.effective_scroll_region();
                let cursor_row = self.grid.cursor().position.row;
                if cursor_row >= region.bottom {
                    self.grid.scroll_up(1);
                } else {
                    self.grid.cursor_down(1);
                }
            }
            Action::SetScrollingRegion { top, bottom } => {
                self.grid.set_scroll_region(top, bottom);
            }

            // Cursor save/restore
            Action::SaveCursorPosition => self.grid.save_cursor(false),
            Action::RestoreCursorPosition => {
                self.grid.restore_cursor();
            }

            // Full/soft reset and screen alignment (handled by TerminalHandler)
            Action::FullReset | Action::SoftReset | Action::ScreenAlignmentPattern => {}

            // Keypad modes (handled by TerminalHandler)
            Action::ApplicationKeypadMode | Action::NormalKeypadMode => {}

            // Text attributes
            Action::SetAttribute(attr) => self.apply_attribute(attr),
            Action::ResetAttributes => self.grid.reset_attrs(),

            // Modes
            Action::SetMode(mode) => self.set_mode(mode, true),
            Action::ResetMode(mode) => self.set_mode(mode, false),

            // Tabs
            Action::Tab => self.grid.tab(),
            Action::SetTabStop => self.grid.set_tab_stop(),
            Action::ClearTabStop(mode) => self.grid.clear_tab_stop(mode),

            // Terminal queries (handled by TerminalHandler which has response queue)
            Action::ReportCursorPosition
            | Action::DeviceAttributes
            | Action::SecondaryDeviceAttributes => {}

            // Control characters
            Action::Bell => {}
            Action::Newline => self.grid.newline(),
            Action::CarriageReturn => self.grid.carriage_return(),
            Action::Backspace => self.grid.backspace(),

            // Screen modes (handled by TerminalHandler which has alternate buffer)
            Action::EnterAlternateScreen | Action::LeaveAlternateScreen => {}

            // OSC/DCS sequences are handled by TerminalHandler, not the simple grid handler
            Action::SetTitle(_)
            | Action::SetWorkingDirectory(_)
            | Action::PromptStart
            | Action::CommandStart
            | Action::CommandExecuted
            | Action::CommandFinished(_)
            | Action::SetClipboard { .. }
            | Action::SixelImage { .. }
            | Action::SetCursorStyle(_)
            | Action::StartHyperlink(_)
            | Action::EndHyperlink
            | Action::Notification { .. }
            | Action::KittyKeyboardPush(_)
            | Action::KittyKeyboardPop
            | Action::KittyKeyboardQuery
            | Action::UnknownOsc(_)
            | Action::QueryForegroundColor
            | Action::QueryBackgroundColor
            | Action::QueryCursorColor
            | Action::RequestPrivateMode(_) => {}
        }
    }

    /// Apply a text attribute to the current style.
    fn apply_attribute(&mut self, attr: Attribute) {
        let attrs = *self.grid.attrs();
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
            Attribute::DoubleUnderline => {
                new_attrs.flags_mut().set(CellFlags::DOUBLE_UNDERLINE);
                new_attrs.flags_mut().set(CellFlags::UNDERLINE);
            }
            Attribute::Overline => new_attrs.flags_mut().set(CellFlags::OVERLINE),

            Attribute::NoBold => {
                // SGR 22: reset both BOLD and DIM per ECMA-48
                new_attrs.flags_mut().unset(CellFlags::BOLD);
                new_attrs.flags_mut().unset(CellFlags::DIM);
            }
            Attribute::NoItalic => new_attrs.flags_mut().unset(CellFlags::ITALIC),
            Attribute::NoUnderline => {
                new_attrs.flags_mut().unset(CellFlags::UNDERLINE);
                new_attrs.flags_mut().unset(CellFlags::DOUBLE_UNDERLINE);
            }
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

        self.grid.set_attrs(new_attrs);
    }

    /// Set or reset a terminal mode.
    fn set_mode(&mut self, mode: Mode, enabled: bool) {
        match mode {
            Mode::CursorVisible => {
                self.grid.cursor_mut().visible = enabled;
            }
            // All other modes handled by TerminalHandler
            Mode::ApplicationCursorKeys
            | Mode::BracketedPaste
            | Mode::FocusReporting
            | Mode::MouseX10
            | Mode::MouseNormal
            | Mode::MouseButtonEvent
            | Mode::MouseAnyEvent
            | Mode::MouseUtf8
            | Mode::MouseSgr
            | Mode::MouseUrxvt
            | Mode::OriginMode
            | Mode::AutoWrap
            | Mode::ReverseVideo
            | Mode::CursorBlink
            | Mode::ColumnMode132
            | Mode::AlternateScreen
            | Mode::AlternateScreenClear
            | Mode::AlternateScreenSaveCursor
            | Mode::AlternateScrollMode
            | Mode::SynchronizedUpdate => {}
        }
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
    use crate::grid::Dimensions;
    use crate::protocol::ansi::AnsiParser;

    fn process(grid: &mut Grid, input: &[u8]) {
        let mut parser = AnsiParser::new();
        let mut handler = Handler::new(grid);
        parser.feed(input, |action| handler.handle(action));
    }

    #[test]
    fn print_hello() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"Hello");

        assert_eq!(grid.cell(0, 0).unwrap().char(), 'H');
        assert_eq!(grid.cell(1, 0).unwrap().char(), 'e');
        assert_eq!(grid.cell(2, 0).unwrap().char(), 'l');
        assert_eq!(grid.cell(3, 0).unwrap().char(), 'l');
        assert_eq!(grid.cell(4, 0).unwrap().char(), 'o');
    }

    #[test]
    fn cursor_movement() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[10;20H"); // Move to row 10, col 20

        assert_eq!(grid.cursor().position.row, 9); // 0-indexed
        assert_eq!(grid.cursor().position.col, 19);
    }

    #[test]
    fn text_with_color() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[31mRed\x1b[0m Normal");

        // "Red" should have red foreground
        assert_eq!(grid.cell(0, 0).unwrap().char(), 'R');
        assert_eq!(grid.cell(0, 0).unwrap().fg(), Color::Named(1)); // Red

        // " Normal" should have default color after reset
        assert_eq!(grid.cell(4, 0).unwrap().char(), 'N');
        assert_eq!(grid.cell(4, 0).unwrap().fg(), Color::Default);
    }

    #[test]
    fn newline_and_carriage_return() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"Line1\r\nLine2");

        assert_eq!(grid.row(0).unwrap().text_trimmed(), "Line1");
        assert_eq!(grid.row(1).unwrap().text_trimmed(), "Line2");
    }

    #[test]
    fn erase_display() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        process(&mut grid, b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC");
        process(&mut grid, b"\x1b[2J"); // Clear screen

        assert!(grid.row(0).unwrap().is_blank());
        assert!(grid.row(1).unwrap().is_blank());
        assert!(grid.row(2).unwrap().is_blank());
    }

    #[test]
    fn bold_and_italic() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[1;3mBoldItalic\x1b[0m");

        let cell = grid.cell(0, 0).unwrap();
        assert!(cell.has_flag(CellFlags::BOLD));
        assert!(cell.has_flag(CellFlags::ITALIC));
    }

    #[test]
    fn cursor_visibility() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        assert!(grid.cursor().visible);

        process(&mut grid, b"\x1b[?25l"); // Hide cursor
        assert!(!grid.cursor().visible);

        process(&mut grid, b"\x1b[?25h"); // Show cursor
        assert!(grid.cursor().visible);
    }

    #[test]
    fn save_restore_cursor() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[10;20H"); // Move to position
        process(&mut grid, b"\x1b7"); // Save
        process(&mut grid, b"\x1b[1;1H"); // Move to home
        process(&mut grid, b"\x1b8"); // Restore

        assert_eq!(grid.cursor().position.row, 9);
        assert_eq!(grid.cursor().position.col, 19);
    }

    #[test]
    fn scroll_up() {
        let mut grid = Grid::new(Dimensions::new(10, 3));
        process(&mut grid, b"Line1\r\nLine2\r\nLine3");
        process(&mut grid, b"\x1b[1S"); // Scroll up 1

        // Line1 scrolled off, Line2 is now first
        assert!(grid.row(0).unwrap().text_trimmed().starts_with("Line2"));
    }

    #[test]
    fn true_color() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[38;2;100;150;200mX");

        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.fg(), Color::Rgb(100, 150, 200));
    }

    #[test]
    fn osc_66_preserves_colors() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Set fg and bg colors, then print a block char via OSC 66
        // ESC[38;2;122;132;120m - fg gray
        // ESC[48;2;64;73;74m - bg darker gray
        // OSC 66;w=1;▀ ST - print upper half block
        process(
            &mut grid,
            b"\x1b[38;2;122;132;120m\x1b[48;2;64;73;74m\x1b]66;w=1;\xe2\x96\x80\x1b\\",
        );

        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.char(), '▀');
        assert_eq!(cell.fg(), Color::Rgb(122, 132, 120));
        assert_eq!(cell.bg(), Color::Rgb(64, 73, 74));
    }

    #[test]
    fn space_with_background() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Set bg color and print a space
        process(&mut grid, b"\x1b[48;2;64;73;74m ");

        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.char(), ' ');
        assert_eq!(cell.bg(), Color::Rgb(64, 73, 74));
    }

    #[test]
    fn cha_sets_column_only() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Move to row 5, col 10
        process(&mut grid, b"\x1b[5;10H");
        assert_eq!(grid.cursor().position.row, 4);
        assert_eq!(grid.cursor().position.col, 9);

        // CHA (CSI 20 G) — set column to 20, row stays at 4
        process(&mut grid, b"\x1b[20G");
        assert_eq!(grid.cursor().position.row, 4); // unchanged
        assert_eq!(grid.cursor().position.col, 19); // 0-indexed
    }

    #[test]
    fn cha_clamps_to_grid_width() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        // CHA to column 999 — should clamp to col 9 (0-indexed)
        process(&mut grid, b"\x1b[999G");
        assert_eq!(grid.cursor().position.col, 9);
    }

    #[test]
    fn insert_lines_resets_column() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Move to row 5, col 10
        process(&mut grid, b"\x1b[5;10H");
        assert_eq!(grid.cursor().position.col, 9);

        // Insert 1 line — column should reset to 0
        process(&mut grid, b"\x1b[L");
        assert_eq!(grid.cursor().position.col, 0);
    }

    #[test]
    fn delete_lines_resets_column() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[5;10H");
        assert_eq!(grid.cursor().position.col, 9);

        // Delete 1 line — column should reset to 0
        process(&mut grid, b"\x1b[M");
        assert_eq!(grid.cursor().position.col, 0);
    }

    #[test]
    fn vpa_sets_row_only() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Move to row 5, col 20
        process(&mut grid, b"\x1b[5;20H");
        assert_eq!(grid.cursor().position.row, 4);
        assert_eq!(grid.cursor().position.col, 19);

        // VPA (CSI 10 d) — set row to 10, col stays at 19
        process(&mut grid, b"\x1b[10d");
        assert_eq!(grid.cursor().position.row, 9);
        assert_eq!(grid.cursor().position.col, 19);
    }

    #[test]
    fn vpa_clamps_to_grid_height() {
        let mut grid = Grid::new(Dimensions::new(80, 5));
        process(&mut grid, b"\x1b[999d");
        assert_eq!(grid.cursor().position.row, 4);
    }

    #[test]
    fn cbt_moves_backward_one_tab_stop() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        // Move to col 20
        process(&mut grid, b"\x1b[1;21H");
        assert_eq!(grid.cursor().position.col, 20);

        // CBT (CSI Z) — backward 1 tab stop to col 16
        process(&mut grid, b"\x1b[Z");
        assert_eq!(grid.cursor().position.col, 16);
    }

    #[test]
    fn cbt_moves_backward_multiple_stops() {
        let mut grid = Grid::new(Dimensions::new(80, 24));
        process(&mut grid, b"\x1b[1;25H");
        assert_eq!(grid.cursor().position.col, 24);

        // CBT 3 — backward 3 tab stops: 24 → 16 → 8 → 0
        process(&mut grid, b"\x1b[3Z");
        assert_eq!(grid.cursor().position.col, 0);
    }

    #[test]
    fn erase_preserves_background_color() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        // Write some text
        process(&mut grid, b"Hello");
        // Set background to red (SGR 41)
        process(&mut grid, b"\x1b[41m");
        // Move to start and erase line
        process(&mut grid, b"\x1b[1;1H\x1b[2K");

        // All cells on row 0 should have red background
        for col in 0..10 {
            let cell = grid.cell(col, 0).unwrap();
            assert_eq!(cell.bg(), Color::Named(1), "col {} should have red bg", col);
        }
    }

    #[test]
    fn erase_right_preserves_background() {
        let mut grid = Grid::new(Dimensions::new(10, 5));
        process(&mut grid, b"ABCDE");
        // Set background to green (SGR 42), move to col 2, erase right
        process(&mut grid, b"\x1b[42m\x1b[1;3H\x1b[K");

        // Cells 0-1 should have default bg (not erased)
        assert_eq!(grid.cell(0, 0).unwrap().bg(), Color::Default);
        assert_eq!(grid.cell(1, 0).unwrap().bg(), Color::Default);
        // Cells 2+ should have green bg
        for col in 2..10 {
            assert_eq!(
                grid.cell(col, 0).unwrap().bg(),
                Color::Named(2),
                "col {}",
                col
            );
        }
    }
}
