//! Text widgets.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{expand_tabs, expand_tabs_from_column, View};
use crate::edit::{self, EditOp};
use crate::theme::{to_ratatui_color, Theme};
use crate::vim::{KeyParser, VimConfig, VimHandler, VimResult};
use crate::vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator, VimState};
use crate::{impl_vim_ext, impl_vim_handler_accessors};

/// Plain text widget.
#[derive(Debug, Clone)]
pub struct Text {
    content: String,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: expand_tabs(&content.into()).into_owned(),
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

impl View for Text {
    fn constraints(&self) -> ultty_protocol::Constraints {
        let line_count = crate::overrides::text_or(&self.content)
            .lines()
            .count()
            .max(1);
        ultty_protocol::Constraints {
            display: ultty_protocol::Display::Flex,
            width: ultty_protocol::Dimension::Percent(100.0),
            height: ultty_protocol::Dimension::Cells(line_count as f32),
            flex_shrink: 1.0,
            ..Default::default()
        }
    }

    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::Text(ultty_protocol::TextContent::new(crate::overrides::text_or(
            &self.content,
        )))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let paragraph = Paragraph::new(crate::overrides::text_or(&self.content));
        frame.render_widget(paragraph, area);
    }
}

/// Rich text with styled spans.
#[derive(Debug, Clone, Default)]
pub struct RichText {
    spans: Vec<(String, ultty_protocol::Style)>,
    fill_width: bool,
    column: usize,
}

impl RichText {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn span(mut self, text: impl Into<String>, style: ultty_protocol::Style) -> Self {
        let text = text.into();
        let text = expand_tabs_from_column(&text, &mut self.column).into_owned();
        self.spans.push((text, style));
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

    /// The spans drawn: replacement text from a user override (ADR 0023)
    /// becomes one span in the first span's style.
    fn shown_spans(&self) -> std::borrow::Cow<'_, [(String, ultty_protocol::Style)]> {
        match crate::overrides::replacement_text() {
            Some(text) => {
                let style = self
                    .spans
                    .first()
                    .map(|(_, s)| s.clone())
                    .unwrap_or_default();
                std::borrow::Cow::Owned(vec![(text.to_string(), style)])
            }
            None => std::borrow::Cow::Borrowed(&self.spans),
        }
    }
}

impl View for RichText {
    fn constraints(&self) -> ultty_protocol::Constraints {
        let width = if self.fill_width {
            ultty_protocol::Dimension::Percent(100.0)
        } else {
            let total_chars: usize = self
                .shown_spans()
                .iter()
                .map(|(t, _)| t.chars().count())
                .sum();
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
        for (text, style) in self.shown_spans().iter() {
            rich = rich.span(text.clone(), style.clone());
        }
        ultty_protocol::Content::RichText(rich)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Modifier, Style as RStyle};
        use ratatui::text::Span;

        let spans: Vec<Span> = self
            .shown_spans()
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
    password: bool,
    /// Whether the input shows (and says) its vim mode; a host that edits
    /// insert-style, such as `ulttui-lua`, turns it off.
    show_mode: bool,
    accessible: super::Accessible,
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
            password: false,
            show_mode: true,
            accessible: super::Accessible::default(),
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

    /// Put the cursor after the first `cursor` grapheme clusters (clamped to
    /// the value). [`value`](Self::value) puts it at the end.
    pub fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = edit::char_index_from_cursor(&self.value, cursor);
        self
    }

    /// An input that is always in insert mode and shows no vim mode: for a
    /// host that does its own editing, where vim's modes would be a
    /// surprise.
    pub fn without_vim_mode(mut self) -> Self {
        self.show_mode = false;
        self.vim_state.mode = VimMode::Insert;
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

    /// Mask the value on screen and keep it from assistive technology.
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }

    /// The accessible name, such as "Message". An input is never named by
    /// its value.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.accessible.label = Some(label.into());
        self
    }

    /// Longer help text for assistive technology; the placeholder when
    /// not set.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.accessible.description = Some(description.into());
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

    /// Apply an editing operation at the cursor, by grapheme cluster.
    fn edit(&mut self, op: &EditOp) {
        let cursor = edit::cursor_from_char_index(&self.value, self.cursor);
        let (value, cursor) = edit::apply(&self.value, cursor, op);
        self.cursor = edit::char_index_from_cursor(&value, cursor);
        if value != self.value {
            self.history.reset();
        }
        self.value = value;
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

    fn mode_name(&self) -> &'static str {
        match self.vim_state.mode {
            VimMode::Normal => "normal",
            VimMode::Insert => "insert",
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => "visual",
            VimMode::Command => "command",
            VimMode::Search => "search",
        }
    }

    fn mode_indicator(&self) -> &'static str {
        if !self.show_mode {
            return "";
        }
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

    /// What was typed (never a password), described by the placeholder and
    /// the vim mode, for assistive technology.
    fn semantics(&self) -> ultty_protocol::Semantics {
        let mut description = Vec::new();
        if !self.placeholder.is_empty() {
            description.push(self.placeholder.clone());
        }
        if self.show_mode {
            description.push(format!("vim {} mode", self.mode_name()));
        }
        self.accessible.apply(ultty_protocol::Semantics {
            role: ultty_protocol::Role::Input,
            value_text: (!self.password).then(|| self.value.clone()),
            description: (!description.is_empty()).then(|| description.join(". ")),
            ..Default::default()
        })
    }

    /// Focus is a bar down the input's left edge, in the focus colour and
    /// thick in insert mode (so the mode is visible), thin in any other; an
    /// unfocused input keeps the cell, in its own colour, so focus moving
    /// does not move the text.
    fn style(&self) -> ultty_protocol::Style {
        use ultty_protocol::{BorderEdges, BorderStyle, BorderType};

        let theme = Theme::current();
        let focused = super::is_widget_focused(self.widget_id());
        let thick = focused && self.vim_state.mode == VimMode::Insert;
        let mut style = ultty_protocol::Style::new()
            .bg(theme.input.bg)
            .border(BorderStyle {
                style: if thick {
                    BorderType::Thick
                } else {
                    BorderType::Thin
                },
                edges: BorderEdges {
                    left: true,
                    ..BorderEdges::NONE
                },
                radius: 0,
            });
        style.border_color = Some(if focused {
            theme.input.focused_border
        } else {
            theme.input.bg
        });
        style
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

    /// The value and placeholder as they are, and where the caret is while
    /// the input has focus; the backend drops the caret for a terminal that
    /// cannot take one.
    fn content(&self) -> ultty_protocol::Content {
        let is_focused = super::is_widget_focused(self.widget_id());
        let cursor = (is_focused && !self.multiline)
            .then(|| edit::cursor_from_char_index(&self.value, self.cursor) as u32);
        ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(
            ultty_protocol::InputWidget {
                value: self.value.clone(),
                placeholder: self.placeholder.clone(),
                multiline: self.multiline,
                password: self.password,
                cursor,
            },
        ))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Modifier, Style as RStyle};
        use ratatui::text::Span;
        use ratatui::widgets::{Block, Borders};

        let is_focused = super::is_widget_focused(self.widget_id());

        let theme = Theme::current();
        let masked = self
            .password
            .then(|| "•".repeat(edit::grapheme_count(&self.value)));
        let (display_text, style) = if self.value.is_empty() {
            (
                self.placeholder.as_str(),
                RStyle::default().fg(to_ratatui_color(theme.input.placeholder_fg)),
            )
        } else {
            (
                masked.as_deref().unwrap_or(self.value.as_str()),
                RStyle::default().fg(to_ratatui_color(theme.input.fg)),
            )
        };

        let mode_indicator = self.mode_indicator();

        let mode_span = Span::styled(
            mode_indicator,
            RStyle::default().add_modifier(Modifier::BOLD),
        );
        let mut spans = vec![mode_span];
        if is_focused && !self.multiline {
            // The cursor is the cell under it drawn reversed (a space past
            // the end of the text).
            let cursor = edit::cursor_from_char_index(&self.value, self.cursor);
            let shown = masked.as_deref().unwrap_or(self.value.as_str());
            let (before, under, after) = edit::split_at_cursor(shown, cursor);
            let under = if under.is_empty() { " " } else { under };
            spans.push(Span::styled(before.to_string(), style));
            spans.push(Span::styled(
                under.to_string(),
                style.add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::styled(after.to_string(), style));
            if self.value.is_empty() {
                spans.push(Span::styled(self.placeholder.clone(), style));
            }
        } else {
            spans.push(Span::styled(display_text.to_string(), style));
        }
        let line = Line::from(spans);

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

    fn accepts_text(&self) -> bool {
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
            if let Some(op) = crate::edit::op_for_key(key) {
                // Home, End and the word and line deletions mean the whole
                // value, which a multi-line input leaves to vim.
                let line_op = !matches!(
                    op,
                    EditOp::Insert(_)
                        | EditOp::Backspace
                        | EditOp::Delete
                        | EditOp::Left
                        | EditOp::Right
                );
                if !(self.multiline && line_op) {
                    self.edit(&op);
                    return Some(super::FocusAction::Consumed);
                }
            }
            match key.key {
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
    fn native_content_is_the_users_text_only() {
        let input = TextInput::new().placeholder("Name").value("ann");
        match <TextInput as View>::content(&input) {
            ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(w)) => {
                assert_eq!(w.value, "ann");
                assert_eq!(w.placeholder, "Name");
            }
            _ => panic!("Expected input widget content"),
        }
    }

    #[test]
    fn the_vim_mode_is_in_the_description_and_a_bar_down_the_edge() {
        use ultty_protocol::BorderType;

        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("name".to_string());
            fm.focus("name");
        });
        let normal = TextInput::new().id("name").placeholder("Name");
        assert_eq!(
            <TextInput as View>::semantics(&normal)
                .description
                .as_deref(),
            Some("Name. vim normal mode")
        );
        let style = <TextInput as View>::style(&normal);
        assert_eq!(style.border.style, BorderType::Thin);
        assert!(style.border.edges.left && !style.border.edges.right);

        let mut insert = TextInput::new().id("name");
        insert.vim_state.mode = VimMode::Insert;
        assert_eq!(
            <TextInput as View>::style(&insert).border.style,
            BorderType::Thick
        );

        // An input with no vim mode says none and is always inserting.
        let plain = TextInput::new().id("name").without_vim_mode();
        assert_eq!(<TextInput as View>::semantics(&plain).description, None);
        assert_eq!(
            <TextInput as View>::style(&plain).border.style,
            BorderType::Thick
        );
        FocusManager::set_current(old);

        // Not focused: the bar is there, in the background colour.
        let blurred = TextInput::new().id("other");
        let style = <TextInput as View>::style(&blurred);
        assert_eq!(style.border_color, style.bg);
    }

    fn typed(input: &mut TextInput, key: Key, ctrl: bool, alt: bool) {
        let event = KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                alt,
                ..Modifiers::default()
            },
            focused_id: None,
        };
        assert!(matches!(
            input.on_key(&event),
            Some(super::super::FocusAction::Consumed)
        ));
    }

    #[test]
    fn insert_mode_edits_at_the_cursor_by_grapheme() {
        let mut input = TextInput::new().value("héllo wörld");
        input.vim_state.mode = VimMode::Insert;

        typed(&mut input, Key::Home, false, false);
        typed(&mut input, Key::Right, false, false);
        typed(&mut input, Key::Char('X'), false, false);
        assert_eq!(input.current_value(), "hXéllo wörld");

        typed(&mut input, Key::Delete, false, false);
        assert_eq!(input.current_value(), "hXllo wörld");
        typed(&mut input, Key::Backspace, false, false);
        assert_eq!(input.current_value(), "hllo wörld");

        typed(&mut input, Key::End, false, false);
        typed(&mut input, Key::Char('w'), true, false);
        assert_eq!(input.current_value(), "hllo ");
        typed(&mut input, Key::Char('u'), true, false);
        assert_eq!(input.current_value(), "");
    }

    #[test]
    fn insert_mode_never_splits_a_grapheme() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let mut input = TextInput::new().value(format!("a{family}b"));
        input.vim_state.mode = VimMode::Insert;

        typed(&mut input, Key::Left, false, false);
        typed(&mut input, Key::Backspace, false, false);
        assert_eq!(input.current_value(), "ab");
        assert_eq!(input.cursor_position(), 1);
    }

    #[test]
    fn insert_mode_word_motions_and_ctrl_a_e() {
        let mut input = TextInput::new().value("one two");
        input.vim_state.mode = VimMode::Insert;
        typed(&mut input, Key::Left, true, false);
        assert_eq!(input.cursor_position(), 4);
        typed(&mut input, Key::Char('a'), true, false);
        assert_eq!(input.cursor_position(), 0);
        typed(&mut input, Key::Right, false, true);
        assert_eq!(input.cursor_position(), 3);
        typed(&mut input, Key::Char('e'), true, false);
        assert_eq!(input.cursor_position(), 7);
    }

    #[test]
    fn cursor_builder_counts_graphemes_and_clamps() {
        let input = TextInput::new().value("ae\u{301}b").cursor(2);
        assert_eq!(input.cursor_position(), 3);
        assert_eq!(TextInput::new().value("ab").cursor(9).cursor_position(), 2);
    }

    #[test]
    fn focused_native_content_sends_the_caret_in_the_value() {
        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("name".to_string());
            fm.focus("name");
        });
        let widget_of = |input: &TextInput| match <TextInput as View>::content(input) {
            ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(w)) => w,
            _ => panic!("Expected input widget content"),
        };

        // The caret counts clusters of the value, nothing else.
        let input = widget_of(&TextInput::new().id("name").value("héllo").cursor(2));
        assert_eq!((input.value.as_str(), input.cursor), ("héllo", Some(2)));
        let end = widget_of(&TextInput::new().id("name").value("hi"));
        assert_eq!(end.cursor, Some(2));
        let accent = widget_of(&TextInput::new().id("name").value("ae\u{301}b").cursor(2));
        assert_eq!(accent.cursor, Some(2));
        // Empty: the placeholder shows, the caret at its start.
        let empty = widget_of(&TextInput::new().id("name").placeholder("Name"));
        assert_eq!(
            (
                empty.value.as_str(),
                empty.placeholder.as_str(),
                empty.cursor
            ),
            ("", "Name", Some(0))
        );
        // Not focused: no caret.
        let blurred = widget_of(&TextInput::new().id("other").value("hi"));
        assert_eq!((blurred.value.as_str(), blurred.cursor), ("hi", None));
        let secret = widget_of(&TextInput::new().id("name").password().value("pw"));
        assert_eq!((secret.value.as_str(), secret.cursor), ("pw", Some(2)));

        FocusManager::set_current(old);
    }

    #[test]
    fn focused_ansi_render_reverses_the_cell_under_the_cursor() {
        use ratatui::style::Modifier;

        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("name".to_string());
            fm.focus("name");
        });
        let input = TextInput::new().id("name").value("héllo").cursor(1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 3)).unwrap();
        terminal
            .draw(|frame| input.render_ansi(frame, frame.area()))
            .unwrap();
        FocusManager::set_current(old);

        let buffer = terminal.backend().buffer();
        let reversed: Vec<&str> = (0..20)
            .map(|x| &buffer[(x, 1)])
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .map(|cell| cell.symbol())
            .collect();
        assert_eq!(reversed, ["é"]);
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
