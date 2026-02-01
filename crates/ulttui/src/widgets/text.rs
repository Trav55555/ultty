//! Text widgets.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::vim::{VimConfig, VimExt, VimHandler, VimResult};
use ultty_protocol::vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator, VimState};

/// Plain text widget.
#[derive(Debug, Clone)]
pub struct Text {
    content: String,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

impl View for Text {
    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::Text(ultty_protocol::TextContent::new(&self.content))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let paragraph = Paragraph::new(self.content.as_str());
        frame.render_widget(paragraph, area);
    }
}

/// Rich text with styled spans.
#[derive(Debug, Clone, Default)]
pub struct RichText {
    spans: Vec<(String, ultty_protocol::Style)>,
}

impl RichText {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn span(mut self, text: impl Into<String>, style: ultty_protocol::Style) -> Self {
        self.spans.push((text.into(), style));
        self
    }

    pub fn text(self, text: impl Into<String>) -> Self {
        self.span(text, ultty_protocol::Style::default())
    }

    pub fn spans(&self) -> &[(String, ultty_protocol::Style)] {
        &self.spans
    }
}

impl View for RichText {
    fn content(&self) -> ultty_protocol::Content {
        let mut rich = ultty_protocol::RichText::new();
        for (text, style) in &self.spans {
            rich = rich.span(text.clone(), style.clone());
        }
        ultty_protocol::Content::RichText(rich)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Modifier, Style as RStyle};
        use ratatui::text::Span;

        let spans: Vec<Span> = self
            .spans
            .iter()
            .map(|(text, style)| {
                let mut rstyle = RStyle::default();

                if let Some(fg) = &style.fg {
                    rstyle = rstyle.fg(convert_color(fg));
                }
                if let Some(bg) = &style.bg {
                    rstyle = rstyle.bg(convert_color(bg));
                }
                if style.bold {
                    rstyle = rstyle.add_modifier(Modifier::BOLD);
                }
                if style.italic {
                    rstyle = rstyle.add_modifier(Modifier::ITALIC);
                }
                if style.underline {
                    rstyle = rstyle.add_modifier(Modifier::UNDERLINED);
                }
                if style.dim {
                    rstyle = rstyle.add_modifier(Modifier::DIM);
                }

                Span::styled(text.clone(), rstyle)
            })
            .collect();

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }
}

fn convert_color(color: &ultty_protocol::Color) -> ratatui::style::Color {
    use ratatui::style::Color as RColor;
    use ultty_protocol::{Color, NamedColor};

    match color {
        Color::Named(named) => match named {
            NamedColor::Black => RColor::Black,
            NamedColor::Red => RColor::Red,
            NamedColor::Green => RColor::Green,
            NamedColor::Yellow => RColor::Yellow,
            NamedColor::Blue => RColor::Blue,
            NamedColor::Magenta => RColor::Magenta,
            NamedColor::Cyan => RColor::Cyan,
            NamedColor::White => RColor::White,
            NamedColor::BrightBlack => RColor::DarkGray,
            NamedColor::BrightRed => RColor::LightRed,
            NamedColor::BrightGreen => RColor::LightGreen,
            NamedColor::BrightYellow => RColor::LightYellow,
            NamedColor::BrightBlue => RColor::LightBlue,
            NamedColor::BrightMagenta => RColor::LightMagenta,
            NamedColor::BrightCyan => RColor::LightCyan,
            NamedColor::BrightWhite => RColor::White,
            NamedColor::Foreground | NamedColor::Background => RColor::Reset,
        },
        Color::Rgb(r, g, b) => RColor::Rgb(*r, *g, *b),
        Color::Indexed(idx) => RColor::Indexed(*idx),
    }
}

/// Text input widget with vim motion support.
#[derive(Debug, Clone)]
pub struct TextInput {
    value: String,
    placeholder: String,
    multiline: bool,
    cursor: usize,
    selection_start: Option<usize>,
    vim_state: VimState,
    vim_config: VimConfig,
}

impl Default for TextInput {
    fn default() -> Self {
        Self {
            value: String::new(),
            placeholder: String::new(),
            multiline: false,
            cursor: 0,
            selection_start: None,
            vim_state: VimState::new(),
            vim_config: VimConfig::for_text_input(),
        }
    }
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self.cursor = self.value.len();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn cursor_position(&self) -> usize {
        self.cursor
    }

    pub fn current_value(&self) -> &str {
        &self.value
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection_start.map(|start| {
            if start <= self.cursor {
                (start, self.cursor)
            } else {
                (self.cursor, start)
            }
        })
    }

    fn move_cursor(&mut self, motion: &VimMotion, count: usize) {
        let chars: Vec<char> = self.value.chars().collect();
        let len = chars.len();

        for _ in 0..count {
            match motion {
                VimMotion::Left => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                    }
                }
                VimMotion::Right => {
                    if self.cursor < len {
                        self.cursor += 1;
                    }
                }
                VimMotion::Word => {
                    self.cursor = self.find_next_word_start(&chars, self.cursor);
                }
                VimMotion::WordEnd => {
                    self.cursor = self.find_word_end(&chars, self.cursor);
                }
                VimMotion::WordBack => {
                    self.cursor = self.find_word_start(&chars, self.cursor);
                }
                VimMotion::LineStart => {
                    if self.multiline {
                        self.cursor = self.find_line_start(&chars, self.cursor);
                    } else {
                        self.cursor = 0;
                    }
                }
                VimMotion::LineEnd => {
                    if self.multiline {
                        self.cursor = self.find_line_end(&chars, self.cursor);
                    } else {
                        self.cursor = len;
                    }
                }
                VimMotion::LineFirstNonBlank => {
                    let start = if self.multiline {
                        self.find_line_start(&chars, self.cursor)
                    } else {
                        0
                    };
                    self.cursor = chars[start..]
                        .iter()
                        .position(|c| !c.is_whitespace())
                        .map(|p| start + p)
                        .unwrap_or(start);
                }
                VimMotion::DocumentStart => {
                    self.cursor = 0;
                }
                VimMotion::DocumentEnd => {
                    self.cursor = len;
                }
                VimMotion::Down if self.multiline => {
                    self.cursor = self.move_to_line(&chars, self.cursor, 1);
                }
                VimMotion::Up if self.multiline => {
                    self.cursor = self.move_to_line(&chars, self.cursor, -1);
                }
                VimMotion::FindChar(c) => {
                    if let Some(pos) = chars[self.cursor + 1..].iter().position(|ch| ch == c) {
                        self.cursor = self.cursor + 1 + pos;
                    }
                }
                VimMotion::FindCharBack(c) => {
                    if let Some(pos) = chars[..self.cursor].iter().rposition(|ch| ch == c) {
                        self.cursor = pos;
                    }
                }
                VimMotion::TillChar(c) => {
                    if let Some(pos) = chars[self.cursor + 1..].iter().position(|ch| ch == c) {
                        self.cursor = self.cursor + pos;
                    }
                }
                VimMotion::TillCharBack(c) => {
                    if let Some(pos) = chars[..self.cursor].iter().rposition(|ch| ch == c) {
                        self.cursor = pos + 1;
                    }
                }
                _ => {}
            }
        }
    }

    fn find_next_word_start(&self, chars: &[char], start: usize) -> usize {
        let len = chars.len();
        if start >= len {
            return len;
        }

        let mut pos = start;

        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }

        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }

        pos
    }

    fn find_word_end(&self, chars: &[char], start: usize) -> usize {
        let len = chars.len();
        if start >= len {
            return len;
        }

        let mut pos = start;

        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }

        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }

        if pos > 0 {
            pos - 1
        } else {
            pos
        }
    }

    fn find_word_start(&self, chars: &[char], start: usize) -> usize {
        if start == 0 {
            return 0;
        }

        let mut pos = start - 1;

        while pos > 0 && chars[pos].is_whitespace() {
            pos -= 1;
        }

        while pos > 0 && !chars[pos - 1].is_whitespace() {
            pos -= 1;
        }

        pos
    }

    fn find_line_start(&self, chars: &[char], pos: usize) -> usize {
        chars[..pos]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    fn find_line_end(&self, chars: &[char], pos: usize) -> usize {
        chars[pos..]
            .iter()
            .position(|c| *c == '\n')
            .map(|p| pos + p)
            .unwrap_or(chars.len())
    }

    fn move_to_line(&self, chars: &[char], pos: usize, delta: i32) -> usize {
        let current_line_start = self.find_line_start(chars, pos);
        let col = pos - current_line_start;

        let mut target_line_start = current_line_start;

        if delta > 0 {
            for _ in 0..delta {
                let line_end = self.find_line_end(chars, target_line_start);
                if line_end >= chars.len() {
                    break;
                }
                target_line_start = line_end + 1;
            }
        } else {
            for _ in 0..(-delta) {
                if target_line_start == 0 {
                    break;
                }
                target_line_start = self.find_line_start(chars, target_line_start - 1);
            }
        }

        let target_line_end = self.find_line_end(chars, target_line_start);
        let target_line_len = target_line_end - target_line_start;

        target_line_start + col.min(target_line_len)
    }

    fn delete_range(&mut self, start: usize, end: usize) -> String {
        let chars: Vec<char> = self.value.chars().collect();
        let deleted: String = chars[start..end].iter().collect();
        self.value = chars[..start].iter().chain(chars[end..].iter()).collect();
        if self.cursor > start {
            self.cursor = start;
        }
        deleted
    }

    fn apply_operator(&mut self, operator: &VimOperator, start: usize, end: usize) -> VimResult {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        match operator {
            VimOperator::Delete => {
                self.delete_range(start, end);
                VimResult::Handled
            }
            VimOperator::Change => {
                self.delete_range(start, end);
                self.vim_state.mode = VimMode::Insert;
                VimResult::ModeChanged(VimMode::Insert)
            }
            VimOperator::Yank => VimResult::Handled,
            VimOperator::Uppercase => {
                let chars: Vec<char> = self.value.chars().collect();
                let upper: String = chars[start..end]
                    .iter()
                    .flat_map(|c| c.to_uppercase())
                    .collect();
                self.value = chars[..start].iter().collect::<String>()
                    + &upper
                    + &chars[end..].iter().collect::<String>();
                VimResult::Handled
            }
            VimOperator::Lowercase => {
                let chars: Vec<char> = self.value.chars().collect();
                let lower: String = chars[start..end]
                    .iter()
                    .flat_map(|c| c.to_lowercase())
                    .collect();
                self.value = chars[..start].iter().collect::<String>()
                    + &lower
                    + &chars[end..].iter().collect::<String>();
                VimResult::Handled
            }
            _ => VimResult::Ignored,
        }
    }
}

impl View for TextInput {
    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(
            ultty_protocol::InputWidget {
                value: self.value.clone(),
                placeholder: self.placeholder.clone(),
                multiline: self.multiline,
                password: false,
            },
        ))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
        use ratatui::text::Span;

        let (display_text, style) = if self.value.is_empty() {
            (
                self.placeholder.as_str(),
                RStyle::default().fg(RColor::DarkGray),
            )
        } else {
            (self.value.as_str(), RStyle::default())
        };

        let mode_indicator = match self.vim_state.mode {
            VimMode::Normal => "[N] ",
            VimMode::Insert => "[I] ",
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => "[V] ",
            VimMode::Command => "[:] ",
            VimMode::Search => "[/] ",
        };

        let mode_span = Span::styled(
            mode_indicator,
            RStyle::default().add_modifier(Modifier::BOLD),
        );
        let text_span = Span::styled(display_text.to_string(), style);
        let line = Line::from(vec![mode_span, text_span]);

        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<super::FocusAction> {
        use crate::event::Key;

        if self.vim_state.mode == VimMode::Insert {
            match key.key {
                Key::Escape => {
                    self.vim_state.mode = VimMode::Normal;
                    return Some(super::FocusAction::Consumed);
                }
                Key::Char(c) => {
                    let chars: Vec<char> = self.value.chars().collect();
                    let before: String = chars[..self.cursor].iter().collect();
                    let after: String = chars[self.cursor..].iter().collect();
                    self.value = format!("{}{}{}", before, c, after);
                    self.cursor += 1;
                    return Some(super::FocusAction::Consumed);
                }
                Key::Backspace if self.cursor > 0 => {
                    self.delete_range(self.cursor - 1, self.cursor);
                    return Some(super::FocusAction::Consumed);
                }
                Key::Tab if key.modifiers.shift => return Some(super::FocusAction::Previous),
                Key::Tab => return Some(super::FocusAction::Next),
                _ => {}
            }
        } else {
            match key.key {
                Key::Char('i') => {
                    self.vim_state.mode = VimMode::Insert;
                    return Some(super::FocusAction::Consumed);
                }
                Key::Tab if key.modifiers.shift => return Some(super::FocusAction::Previous),
                Key::Tab => return Some(super::FocusAction::Next),
                _ => {}
            }
        }
        None
    }
}

impl VimHandler for TextInput {
    fn vim_state(&self) -> &VimState {
        &self.vim_state
    }

    fn vim_state_mut(&mut self) -> &mut VimState {
        &mut self.vim_state
    }

    fn handle_vim_action(&mut self, action: &VimAction) -> VimResult {
        match action {
            VimAction::InsertAtCursor => {
                self.vim_state.mode = VimMode::Insert;
                VimResult::ModeChanged(VimMode::Insert)
            }
            VimAction::InsertAfterCursor => {
                let chars: Vec<char> = self.value.chars().collect();
                if self.cursor < chars.len() {
                    self.cursor += 1;
                }
                self.vim_state.mode = VimMode::Insert;
                VimResult::ModeChanged(VimMode::Insert)
            }
            VimAction::InsertAtLineStart => {
                let chars: Vec<char> = self.value.chars().collect();
                let line_start = if self.multiline {
                    self.find_line_start(&chars, self.cursor)
                } else {
                    0
                };
                self.cursor = chars[line_start..]
                    .iter()
                    .position(|c| !c.is_whitespace())
                    .map(|p| line_start + p)
                    .unwrap_or(line_start);
                self.vim_state.mode = VimMode::Insert;
                VimResult::ModeChanged(VimMode::Insert)
            }
            VimAction::InsertAtLineEnd => {
                let chars: Vec<char> = self.value.chars().collect();
                self.cursor = if self.multiline {
                    self.find_line_end(&chars, self.cursor)
                } else {
                    chars.len()
                };
                self.vim_state.mode = VimMode::Insert;
                VimResult::ModeChanged(VimMode::Insert)
            }
            VimAction::DeleteChar => {
                let chars: Vec<char> = self.value.chars().collect();
                if self.cursor < chars.len() {
                    self.delete_range(self.cursor, self.cursor + 1);
                }
                VimResult::Handled
            }
            VimAction::DeleteCharBefore => {
                if self.cursor > 0 {
                    self.delete_range(self.cursor - 1, self.cursor);
                }
                VimResult::Handled
            }
            VimAction::DeleteLine => {
                let chars: Vec<char> = self.value.chars().collect();
                let start = if self.multiline {
                    self.find_line_start(&chars, self.cursor)
                } else {
                    0
                };
                let end = if self.multiline {
                    let line_end = self.find_line_end(&chars, self.cursor);
                    if line_end < chars.len() {
                        line_end + 1
                    } else {
                        line_end
                    }
                } else {
                    chars.len()
                };
                self.delete_range(start, end);
                VimResult::Handled
            }
            VimAction::ChangeLine => {
                let chars: Vec<char> = self.value.chars().collect();
                let start = if self.multiline {
                    let line_start = self.find_line_start(&chars, self.cursor);
                    chars[line_start..]
                        .iter()
                        .position(|c| !c.is_whitespace())
                        .map(|p| line_start + p)
                        .unwrap_or(line_start)
                } else {
                    0
                };
                let end = if self.multiline {
                    self.find_line_end(&chars, self.cursor)
                } else {
                    chars.len()
                };
                self.delete_range(start, end);
                self.vim_state.mode = VimMode::Insert;
                VimResult::ModeChanged(VimMode::Insert)
            }
            VimAction::Undo => VimResult::Handled,
            VimAction::Redo => VimResult::Handled,
            VimAction::EnterVisual => {
                self.selection_start = Some(self.cursor);
                self.vim_state.mode = VimMode::Visual;
                VimResult::ModeChanged(VimMode::Visual)
            }
            VimAction::EnterVisualLine => {
                let chars: Vec<char> = self.value.chars().collect();
                self.selection_start = Some(if self.multiline {
                    self.find_line_start(&chars, self.cursor)
                } else {
                    0
                });
                self.vim_state.mode = VimMode::VisualLine;
                VimResult::ModeChanged(VimMode::VisualLine)
            }
            VimAction::ExitToNormal => {
                self.selection_start = None;
                self.vim_state.reset_to_normal();
                VimResult::ModeChanged(VimMode::Normal)
            }
            _ => VimResult::Ignored,
        }
    }

    fn handle_vim_command(&mut self, command: &VimCommand) -> VimResult {
        let count = command.effective_count();
        let start_pos = self.cursor;

        self.move_cursor(&command.motion, count);

        if command.operator != VimOperator::None {
            return self.apply_operator(&command.operator, start_pos, self.cursor);
        }

        if self.vim_state.mode.is_visual() {}

        VimResult::Handled
    }
}

impl VimExt for TextInput {
    fn vim(self) -> Self {
        self
    }

    fn vim_with_config(mut self, config: VimConfig) -> Self {
        self.vim_config = config;
        if self.vim_config.start_in_insert {
            self.vim_state.mode = VimMode::Insert;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_movement() {
        let mut input = TextInput::new().value("hello world");
        input.cursor = 0;
        input.vim_state.mode = VimMode::Normal;

        input.move_cursor(&VimMotion::Word, 1);
        assert_eq!(input.cursor, 6);

        input.move_cursor(&VimMotion::LineEnd, 1);
        assert_eq!(input.cursor, 11);

        input.move_cursor(&VimMotion::LineStart, 1);
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn test_delete_word() {
        let mut input = TextInput::new().value("hello world");
        input.cursor = 0;
        input.vim_state.mode = VimMode::Normal;

        let cmd = VimCommand::operator(VimOperator::Delete, VimMotion::Word);
        input.handle_vim_command(&cmd);

        assert_eq!(input.current_value(), "world");
    }

    #[test]
    fn test_visual_selection() {
        let mut input = TextInput::new().value("hello");
        input.cursor = 0;
        input.vim_state.mode = VimMode::Normal;

        input.handle_vim_action(&VimAction::EnterVisual);
        assert_eq!(input.vim_state.mode, VimMode::Visual);
        assert_eq!(input.selection_start, Some(0));

        input.move_cursor(&VimMotion::Right, 3);
        assert_eq!(input.selection(), Some((0, 3)));
    }

    mod color_conversion {
        use super::*;
        use ratatui::style::Color as RColor;
        use ultty_protocol::{Color, NamedColor};

        #[test]
        fn named_colors_convert() {
            let cases = [
                (NamedColor::Black, RColor::Black),
                (NamedColor::Red, RColor::Red),
                (NamedColor::Green, RColor::Green),
                (NamedColor::Yellow, RColor::Yellow),
                (NamedColor::Blue, RColor::Blue),
                (NamedColor::Magenta, RColor::Magenta),
                (NamedColor::Cyan, RColor::Cyan),
                (NamedColor::White, RColor::White),
            ];
            for (named, expected) in cases {
                let result = convert_color(&Color::Named(named));
                assert_eq!(result, expected, "Failed for {:?}", named);
            }
        }

        #[test]
        fn bright_colors_convert() {
            let cases = [
                (NamedColor::BrightBlack, RColor::DarkGray),
                (NamedColor::BrightRed, RColor::LightRed),
                (NamedColor::BrightGreen, RColor::LightGreen),
                (NamedColor::BrightYellow, RColor::LightYellow),
                (NamedColor::BrightBlue, RColor::LightBlue),
                (NamedColor::BrightMagenta, RColor::LightMagenta),
                (NamedColor::BrightCyan, RColor::LightCyan),
                (NamedColor::BrightWhite, RColor::White),
            ];
            for (named, expected) in cases {
                let result = convert_color(&Color::Named(named));
                assert_eq!(result, expected, "Failed for {:?}", named);
            }
        }

        #[test]
        fn foreground_background_reset() {
            assert_eq!(
                convert_color(&Color::Named(NamedColor::Foreground)),
                RColor::Reset
            );
            assert_eq!(
                convert_color(&Color::Named(NamedColor::Background)),
                RColor::Reset
            );
        }

        #[test]
        fn rgb_colors_convert() {
            let color = Color::Rgb(255, 128, 64);
            let result = convert_color(&color);
            assert_eq!(result, RColor::Rgb(255, 128, 64));
        }

        #[test]
        fn rgb_edge_cases() {
            assert_eq!(convert_color(&Color::Rgb(0, 0, 0)), RColor::Rgb(0, 0, 0));
            assert_eq!(
                convert_color(&Color::Rgb(255, 255, 255)),
                RColor::Rgb(255, 255, 255)
            );
        }

        #[test]
        fn indexed_colors_convert() {
            for idx in [0u8, 15, 16, 231, 232, 255] {
                let color = Color::Indexed(idx);
                let result = convert_color(&color);
                assert_eq!(result, RColor::Indexed(idx));
            }
        }
    }
}
