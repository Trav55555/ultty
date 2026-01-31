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
                let row = self.grid.cursor().position.row + 1; // 1-based
                self.grid.set_cursor_position(row, col);
            }
            Action::CursorPosition { row, col } => {
                self.grid.set_cursor_position(row, col);
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
                for i in 0..n {
                    if let Some(cell) = self.grid.cell_mut(col + i, row) {
                        cell.clear();
                    }
                }
            }

            // Line operations
            Action::InsertLines(n) => {
                // Insert n blank lines at cursor, pushing content down
                for _ in 0..n {
                    self.grid.scroll_down(1);
                }
            }
            Action::DeleteLines(n) => {
                // Delete n lines at cursor, pulling content up
                for _ in 0..n {
                    self.grid.scroll_up(1);
                }
            }
            Action::DeleteCharacters(n) => {
                let row_idx = self.grid.cursor().position.row;
                let col = self.grid.cursor().position.col;
                if let Some(row) = self.grid.row_mut(row_idx) {
                    row.delete_cells(col, n);
                }
            }
            Action::InsertCharacters(n) => {
                let row_idx = self.grid.cursor().position.row;
                let col = self.grid.cursor().position.col;
                if let Some(row) = self.grid.row_mut(row_idx) {
                    row.insert_cells(col, n);
                }
            }

            // Scrolling
            Action::ScrollUp(n) => self.grid.scroll_up(n),
            Action::ScrollDown(n) => self.grid.scroll_down(n),
            Action::SetScrollingRegion { top, bottom } => {
                self.grid.set_scroll_region(top, bottom);
            }

            // Cursor save/restore
            Action::SaveCursorPosition => self.grid.cursor_mut().save(),
            Action::RestoreCursorPosition => self.grid.cursor_mut().restore(),

            // Text attributes
            Action::SetAttribute(attr) => self.apply_attribute(attr),
            Action::ResetAttributes => self.grid.reset_attrs(),

            // Modes
            Action::SetMode(mode) => self.set_mode(mode, true),
            Action::ResetMode(mode) => self.set_mode(mode, false),

            // Tabs
            Action::Tab => self.grid.tab(),
            Action::SetTabStop => {
                // TODO: Implement custom tab stops
            }
            Action::ClearTabStop(_) => {
                // TODO: Implement tab stop clearing
            }

            // Terminal queries
            Action::ReportCursorPosition => {
                // TODO: Send cursor position report
            }
            Action::DeviceAttributes => {
                // TODO: Send device attributes
            }

            // Control characters
            Action::Bell => {
                // TODO: Trigger bell (audio or visual)
            }
            Action::Newline => self.grid.newline(),
            Action::CarriageReturn => self.grid.carriage_return(),
            Action::Backspace => self.grid.backspace(),

            // Screen modes
            Action::EnterAlternateScreen => {
                // TODO: Switch to alternate screen buffer
            }
            Action::LeaveAlternateScreen => {
                // TODO: Switch back to main screen buffer
            }

            // OSC/DCS sequences are handled by TerminalHandler, not the simple grid handler
            Action::SetTitle(_)
            | Action::SetWorkingDirectory(_)
            | Action::PromptStart
            | Action::CommandStart
            | Action::CommandExecuted
            | Action::CommandFinished(_)
            | Action::SetClipboard { .. }
            | Action::SixelImage { .. }
            | Action::UnknownOsc(_) => {}
        }
    }

    /// Apply a text attribute to the current style.
    fn apply_attribute(&mut self, attr: Attribute) {
        let attrs = self.grid.attrs().clone();
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

        self.grid.set_attrs(new_attrs);
    }

    /// Set or reset a terminal mode.
    fn set_mode(&mut self, mode: Mode, enabled: bool) {
        match mode {
            Mode::CursorVisible => {
                self.grid.cursor_mut().visible = enabled;
            }
            Mode::ApplicationCursorKeys => {
                // TODO: Track application cursor keys mode
            }
            Mode::BracketedPaste => {
                // TODO: Track bracketed paste mode
            }
            Mode::FocusReporting => {
                // TODO: Track focus reporting mode
            }
            Mode::MouseX10
            | Mode::MouseNormal
            | Mode::MouseButtonEvent
            | Mode::MouseAnyEvent
            | Mode::MouseUtf8
            | Mode::MouseSgr
            | Mode::MouseUrxvt => {
                // Mouse modes are handled by TerminalHandler, not the simple grid handler
            }
            Mode::OriginMode => {
                // TODO: Track origin mode
            }
            Mode::AutoWrap => {
                // TODO: Track auto-wrap mode
            }
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
}
