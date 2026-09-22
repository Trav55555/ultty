//! Text widgets.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use crate::vim::{KeyParser, VimConfig, VimHandler, VimResult};
use crate::{impl_vim_ext, impl_vim_handler_accessors};
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
    fn constraints(&self) -> ultty_protocol::Constraints {
        let line_count = self.content.lines().count().max(1);
        ultty_protocol::Constraints {
            display: ultty_protocol::Display::Flex,
            width: ultty_protocol::Dimension::Percent(100.0),
            height: ultty_protocol::Dimension::Cells(line_count as f32),
            flex_shrink: 1.0,
            ..Default::default()
        }
    }

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
    fill_width: bool,
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

    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }

    pub fn spans(&self) -> &[(String, ultty_protocol::Style)] {
        &self.spans
    }
}

impl View for RichText {
    fn constraints(&self) -> ultty_protocol::Constraints {
        let width = if self.fill_width {
            ultty_protocol::Dimension::Percent(100.0)
        } else {
            let total_chars: usize = self.spans.iter().map(|(t, _)| t.chars().count()).sum();
            ultty_protocol::Dimension::Cells(total_chars as f32)
        };
        ultty_protocol::Constraints {
            display: ultty_protocol::Display::Flex,
            width,
            max_width: ultty_protocol::Dimension::Percent(100.0),
            height: ultty_protocol::Dimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        }
    }

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
    id: Option<String>,
    placeholder: String,
    multiline: bool,
    rows: u16,
    cursor: usize,
    selection_start: Option<usize>,
    history: super::input_history::InputHistory,
    vim_state: VimState,
    vim_config: VimConfig,
    key_parser: KeyParser,
}

impl Default for TextInput {
    fn default() -> Self {
        Self {
            value: String::new(),
            id: None,
            placeholder: String::new(),
            multiline: false,
            rows: 1,
            cursor: 0,
            selection_start: None,
            history: Default::default(),
            vim_state: VimState::new(),
            vim_config: VimConfig::for_text_input(),
            key_parser: KeyParser::new(),
        }
    }
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self.cursor = self.value.chars().count();
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

    pub fn height(mut self, rows: u16) -> Self {
        self.rows = rows;
        self
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cursor_position(&self) -> usize {
        self.cursor
    }

    pub fn current_value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, val: impl Into<String>) {
        self.value = val.into();
        self.cursor = self.value.chars().count();
        self.history.reset();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.history.reset();
    }

    pub fn submit_value(&mut self) -> String {
        let submitted = self.value.clone();
        if !submitted.is_empty() {
            self.push_history(submitted.clone());
        }
        self.clear();
        submitted
    }

    pub fn push_history(&mut self, entry: impl Into<String>) {
        self.history.push(entry);
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
        self.cursor = self.cursor.min(len);

        for _ in 0..count {
            match motion {
                VimMotion::Left if self.cursor > 0 => {
                    self.cursor -= 1;
                }
                VimMotion::Right if self.cursor < len => {
                    self.cursor += 1;
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
                    if let Some(found) = find_char_after(&chars, self.cursor, *c) {
                        self.cursor = found;
                    }
                }
                VimMotion::FindCharBack(c) => {
                    if let Some(found) = find_char_before(&chars, self.cursor, *c) {
                        self.cursor = found;
                    }
                }
                VimMotion::TillChar(c) => {
                    if let Some(found) = find_char_after(&chars, self.cursor, *c) {
                        self.cursor = found - 1;
                    }
                }
                VimMotion::TillCharBack(c) => {
                    if let Some(found) = find_char_before(&chars, self.cursor, *c) {
                        self.cursor = found + 1;
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
        let pos = pos.min(chars.len());
        chars[..pos]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    fn find_line_end(&self, chars: &[char], pos: usize) -> usize {
        let pos = pos.min(chars.len());
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
        let (start, end) = clamp_range(start, end, chars.len());
        let deleted: String = chars[start..end].iter().collect();
        self.value = chars[..start].iter().chain(chars[end..].iter()).collect();
        if self.cursor > start {
            self.cursor = start;
        }
        deleted
    }

    /// Replace `start..end` (clamped to the value) with `map` applied to it.
    fn map_range(&mut self, start: usize, end: usize, map: impl Fn(char) -> String) {
        let chars: Vec<char> = self.value.chars().collect();
        let (start, end) = clamp_range(start, end, chars.len());
        let mapped: String = chars[start..end].iter().map(|c| map(*c)).collect();
        self.value = chars[..start].iter().collect::<String>()
            + &mapped
            + &chars[end..].iter().collect::<String>();
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
                self.map_range(start, end, |c| c.to_uppercase().collect());
                VimResult::Handled
            }
            VimOperator::Lowercase => {
                self.map_range(start, end, |c| c.to_lowercase().collect());
                VimResult::Handled
            }
            _ => VimResult::Ignored,
        }
    }

    fn mode_indicator(&self) -> &'static str {
        match self.vim_state.mode {
            VimMode::Normal => "[N] ",
            VimMode::Insert => "[I] ",
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => "[V] ",
            VimMode::Command => "[:] ",
            VimMode::Search => "[/] ",
        }
    }

    fn is_cursor_on_first_line(&self) -> bool {
        let chars: Vec<char> = self.value.chars().collect();
        self.find_line_start(&chars, self.cursor) == 0
    }

    fn is_cursor_on_last_line(&self) -> bool {
        let chars: Vec<char> = self.value.chars().collect();
        self.find_line_end(&chars, self.cursor) == chars.len()
    }

    fn navigate_history_up(&mut self) -> bool {
        if let Some(val) = self.history.navigate_up(&self.value) {
            self.value = val.to_string();
            self.cursor = self.value.chars().count();
            true
        } else {
            false
        }
    }

    fn navigate_history_down(&mut self) -> bool {
        if let Some(val) = self.history.navigate_down() {
            self.value = val.to_string();
            self.cursor = self.value.chars().count();
            true
        } else {
            false
        }
    }
}

impl View for TextInput {
    fn widget_id(&self) -> Option<&str> {
        TextInput::widget_id(self)
    }

    fn constraints(&self) -> ultty_protocol::Constraints {
        ultty_protocol::Constraints {
            display: ultty_protocol::Display::Flex,
            width: ultty_protocol::Dimension::Percent(100.0),
            height: if self.multiline {
                ultty_protocol::Dimension::Cells(self.rows as f32)
            } else {
                ultty_protocol::Dimension::Cells(1.0)
            },
            flex_shrink: 1.0,
            ..Default::default()
        }
    }

    fn content(&self) -> ultty_protocol::Content {
        let is_focused = super::is_widget_focused(self.widget_id());
        let focus_prefix = if is_focused { ">> " } else { "" };
        let mode_indicator = self.mode_indicator();
        ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(
            ultty_protocol::InputWidget {
                value: if self.value.is_empty() {
                    String::new()
                } else {
                    format!("{}{}{}", focus_prefix, mode_indicator, self.value)
                },
                placeholder: format!("{}{}{}", focus_prefix, mode_indicator, self.placeholder),
                multiline: self.multiline,
                password: false,
            },
        ))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Modifier, Style as RStyle};
        use ratatui::text::Span;
        use ratatui::widgets::{Block, Borders};

        let is_focused = super::is_widget_focused(self.widget_id());

        let theme = Theme::current();
        let (display_text, style) = if self.value.is_empty() {
            (
                self.placeholder.as_str(),
                RStyle::default().fg(to_ratatui_color(theme.input.placeholder_fg)),
            )
        } else {
            (
                self.value.as_str(),
                RStyle::default().fg(to_ratatui_color(theme.input.fg)),
            )
        };

        let mode_indicator = self.mode_indicator();

        let mode_span = Span::styled(
            mode_indicator,
            RStyle::default().add_modifier(Modifier::BOLD),
        );
        let text_span = Span::styled(display_text.to_string(), style);
        let line = Line::from(vec![mode_span, text_span]);

        let mut paragraph = Paragraph::new(line);
        if is_focused {
            let border_style = RStyle::default().fg(to_ratatui_color(theme.input.focused_border));
            paragraph = paragraph.block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style),
            );
        }
        frame.render_widget(paragraph, area);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<super::FocusAction> {
        use crate::event::Key;

        match key.key {
            Key::Tab if key.modifiers.shift => return Some(super::FocusAction::Previous),
            Key::Tab => return Some(super::FocusAction::Next),
            _ => {}
        }

        if self.vim_state.mode == VimMode::Insert {
            match key.key {
                Key::Char(c) => {
                    let chars: Vec<char> = self.value.chars().collect();
                    let at = self.cursor.min(chars.len());
                    let before: String = chars[..at].iter().collect();
                    let after: String = chars[at..].iter().collect();
                    self.value = format!("{}{}{}", before, c, after);
                    self.cursor = at + 1;
                    self.history.reset();
                    return Some(super::FocusAction::Consumed);
                }
                Key::Backspace if self.cursor > 0 => {
                    self.delete_range(self.cursor - 1, self.cursor);
                    self.history.reset();
                    return Some(super::FocusAction::Consumed);
                }
                Key::Up
                    if (!self.multiline || self.is_cursor_on_first_line())
                        && self.navigate_history_up() =>
                {
                    return Some(super::FocusAction::Consumed);
                }
                Key::Down
                    if (!self.multiline || self.is_cursor_on_last_line())
                        && self.navigate_history_down() =>
                {
                    return Some(super::FocusAction::Consumed);
                }
                _ => {}
            }
        }

        // Normal, visual and search modes (and Escape in insert mode) go
        // through the persistent vim parser so multi-key sequences such as
        // `gg`, `dw`, `3l` and `f{c}` complete across key events.
        let vim_key = crate::vim::to_vim_key(&key.key)?;
        match self.handle_vim_key(&vim_key, crate::vim::to_vim_modifiers(key.modifiers)) {
            VimResult::Ignored => None,
            _ => Some(super::FocusAction::Consumed),
        }
    }
}

impl VimHandler for TextInput {
    impl_vim_handler_accessors!(TextInput);

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

        VimResult::Handled
    }
}

impl_vim_ext!(TextInput, VimConfig::for_text_input());

/// Clamp `start..end` into `0..=len`, keeping `start <= end`.
fn clamp_range(start: usize, end: usize, len: usize) -> (usize, usize) {
    let start = start.min(len);
    (start, end.min(len).max(start))
}

/// Index of the first `target` strictly after `cursor`, if any.
fn find_char_after(chars: &[char], cursor: usize, target: char) -> Option<usize> {
    let from = cursor.saturating_add(1).min(chars.len());
    chars[from..]
        .iter()
        .position(|ch| *ch == target)
        .map(|pos| from + pos)
}

/// Index of the last `target` strictly before `cursor`, if any.
fn find_char_before(chars: &[char], cursor: usize, target: char) -> Option<usize> {
    let to = cursor.min(chars.len());
    chars[..to].iter().rposition(|ch| *ch == target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, KeyEvent, Modifiers};
    use crate::focus::FocusManager;
    use crate::vim::VimExt;

    fn key_event(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers::default(),
            focused_id: None,
        }
    }

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

    #[test]
    fn value_builder_counts_chars_not_bytes() {
        let input = TextInput::new().value("héllo");
        assert_eq!(input.cursor_position(), 5);
    }

    #[test]
    fn motions_on_multibyte_value_do_not_panic() {
        let motions = [
            VimMotion::Left,
            VimMotion::Right,
            VimMotion::Word,
            VimMotion::WordEnd,
            VimMotion::WordBack,
            VimMotion::LineStart,
            VimMotion::LineEnd,
            VimMotion::LineFirstNonBlank,
            VimMotion::DocumentStart,
            VimMotion::DocumentEnd,
            VimMotion::Up,
            VimMotion::Down,
            VimMotion::FindChar('l'),
            VimMotion::FindCharBack('h'),
            VimMotion::TillChar('o'),
            VimMotion::TillCharBack('é'),
        ];
        for motion in motions {
            let mut input = TextInput::new().value("héllo").multiline();
            input.move_cursor(&motion, 3);
            assert!(input.cursor_position() <= 5, "{motion:?}");
        }
    }

    #[test]
    fn find_char_on_empty_input_does_not_panic() {
        let mut input = TextInput::new();
        input.move_cursor(&VimMotion::FindChar('x'), 1);
        input.move_cursor(&VimMotion::TillChar('x'), 1);
        input.move_cursor(&VimMotion::FindCharBack('x'), 1);
        input.move_cursor(&VimMotion::TillCharBack('x'), 1);
        assert_eq!(input.cursor_position(), 0);
    }

    #[test]
    fn till_char_stops_before_match() {
        let mut input = TextInput::new().value("héllo");
        input.cursor = 0;
        input.move_cursor(&VimMotion::TillChar('o'), 1);
        assert_eq!(input.cursor_position(), 3);
        input.move_cursor(&VimMotion::TillCharBack('h'), 1);
        assert_eq!(input.cursor_position(), 1);
    }

    #[test]
    fn range_operators_clamp_out_of_range_indices() {
        let mut input = TextInput::new().value("héllo");
        input.delete_range(3, 99);
        assert_eq!(input.current_value(), "hél");

        let mut input = TextInput::new().value("héllo");
        input.apply_operator(&VimOperator::Uppercase, 2, 99);
        assert_eq!(input.current_value(), "héLLO");

        let mut input = TextInput::new().value("HÉLLO");
        input.apply_operator(&VimOperator::Lowercase, 99, 2);
        assert_eq!(input.current_value(), "HÉllo");
        assert!(input.is_cursor_on_first_line());
    }

    #[test]
    fn insert_with_stale_cursor_appends_at_end() {
        let mut input = TextInput::new().value("ab");
        input.vim_state.mode = VimMode::Insert;
        input.cursor = 10;
        input.on_key(&key_event(Key::Char('c')));
        assert_eq!(input.current_value(), "abc");
        assert_eq!(input.cursor_position(), 3);
    }

    #[test]
    fn normal_mode_operator_motion_completes_across_key_events() {
        let mut input = TextInput::new().value("hello world");
        input.cursor = 0;
        input.vim_state.mode = VimMode::Normal;

        assert!(matches!(
            input.on_key(&key_event(Key::Char('d'))),
            Some(crate::widgets::FocusAction::Consumed)
        ));
        assert_eq!(input.current_value(), "hello world", "pending operator");
        input.on_key(&key_event(Key::Char('w')));
        assert_eq!(input.current_value(), "world");
    }

    #[test]
    fn normal_mode_find_count_and_document_motions() {
        let mut input = TextInput::new().value("hello world");
        input.cursor = 0;
        input.vim_state.mode = VimMode::Normal;

        input.on_key(&key_event(Key::Char('f')));
        assert_eq!(input.cursor_position(), 0, "f waits for its character");
        input.on_key(&key_event(Key::Char('o')));
        assert_eq!(input.cursor_position(), 4);

        input.on_key(&key_event(Key::Char('3')));
        input.on_key(&key_event(Key::Char('l')));
        assert_eq!(input.cursor_position(), 7);

        input.on_key(&key_event(Key::Char('g')));
        assert_eq!(input.cursor_position(), 7, "lone g waits");
        input.on_key(&key_event(Key::Char('g')));
        assert_eq!(input.cursor_position(), 0);

        input.on_key(&key_event(Key::Char('G')));
        assert_eq!(input.cursor_position(), 11);
    }

    #[test]
    fn insert_and_normal_mode_switch_through_parser() {
        let mut input = TextInput::new().value("abc");
        input.vim_state.mode = VimMode::Normal;
        assert!(matches!(
            input.on_key(&key_event(Key::Char('i'))),
            Some(crate::widgets::FocusAction::Consumed)
        ));
        assert_eq!(input.vim_state.mode, VimMode::Insert);

        input.on_key(&key_event(Key::Char('x')));
        assert!(input.current_value().contains('x'));

        assert!(matches!(
            input.on_key(&key_event(Key::Escape)),
            Some(crate::widgets::FocusAction::Consumed)
        ));
        assert_eq!(input.vim_state.mode, VimMode::Normal);
        assert!(input.on_key(&key_event(Key::Char('q'))).is_none());
    }

    #[test]
    fn text_input_with_id() {
        let input = TextInput::new().id("name");
        assert_eq!(input.widget_id(), Some("name"));
    }

    #[test]
    fn native_content_includes_mode_indicator() {
        let input = TextInput::new().placeholder("Name");
        match <TextInput as View>::content(&input) {
            ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(w)) => {
                assert!(w.placeholder.starts_with("[N] "));
            }
            _ => panic!("Expected input widget content"),
        }
    }

    #[test]
    fn focused_native_content_includes_focus_prefix() {
        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("name".to_string());
            fm.focus("name");
        });

        let input = TextInput::new().id("name").placeholder("Name");
        match <TextInput as View>::content(&input) {
            ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(w)) => {
                assert!(w.placeholder.starts_with(">> [N] "));
            }
            _ => panic!("Expected input widget content"),
        }

        FocusManager::set_current(old);
    }

    #[test]
    fn test_history_navigation_up_down() {
        let mut input = TextInput::new();
        input.vim_state.mode = VimMode::Insert;
        input.push_history("first");
        input.push_history("second");
        input.set_value("draft");

        assert!(matches!(
            input.on_key(&key_event(Key::Up)),
            Some(super::super::FocusAction::Consumed)
        ));
        assert_eq!(input.current_value(), "second");

        assert!(matches!(
            input.on_key(&key_event(Key::Up)),
            Some(super::super::FocusAction::Consumed)
        ));
        assert_eq!(input.current_value(), "first");

        assert!(matches!(
            input.on_key(&key_event(Key::Down)),
            Some(super::super::FocusAction::Consumed)
        ));
        assert_eq!(input.current_value(), "second");

        assert!(matches!(
            input.on_key(&key_event(Key::Down)),
            Some(super::super::FocusAction::Consumed)
        ));
        assert_eq!(input.current_value(), "draft");
    }

    #[test]
    fn test_submit_value_adds_to_history() {
        let mut input = TextInput::new().value("echo hello");

        let submitted = input.submit_value();

        assert_eq!(submitted, "echo hello");
        assert_eq!(input.history.entries(), &["echo hello"]);
    }

    #[test]
    fn test_submit_value_clears_input() {
        let mut input = TextInput::new().value("echo hello");

        input.submit_value();

        assert_eq!(input.current_value(), "");
        assert_eq!(input.cursor_position(), 0);
    }

    #[test]
    fn test_set_value_moves_cursor_to_end() {
        let mut input = TextInput::new();

        input.set_value("hello world");

        assert_eq!(input.current_value(), "hello world");
        assert_eq!(input.cursor_position(), 11);
    }

    #[test]
    fn test_height_builder_sets_rows() {
        let input = TextInput::new().height(4);

        assert_eq!(input.rows(), 4);
    }

    #[test]
    fn test_vim_starts_in_insert_mode() {
        let input = TextInput::new().vim();
        assert_eq!(input.vim_state.mode, VimMode::Insert);
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
