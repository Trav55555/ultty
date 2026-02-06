//! Shared state widgets for empty, loading, and error UI.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Content, SpinnerStyle, SpinnerWidget, Style as ProtoStyle, WidgetContent};

const LOADING_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A reusable empty-state widget.
#[derive(Debug, Clone)]
pub struct EmptyState {
    title: String,
    detail: Option<String>,
    hint: Option<String>,
}

impl EmptyState {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: None,
            hint: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn text(&self) -> String {
        let mut lines = vec![self.title.clone()];
        if let Some(detail) = &self.detail {
            lines.push(detail.clone());
        }
        if let Some(hint) = &self.hint {
            lines.push(format!("Hint: {}", hint));
        }
        lines.join("\n")
    }
}

impl View for EmptyState {
    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new().fg(theme.muted)
    }

    fn content(&self) -> Content {
        Content::Text(ultty_protocol::TextContent::new(self.text()))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let style = Style::default().fg(to_ratatui_color(theme.muted));
        frame.render_widget(Paragraph::new(self.text()).style(style), area);
    }
}

/// A reusable loading-state widget.
#[derive(Debug, Clone)]
pub struct LoadingState {
    label: String,
    frame: usize,
}

impl LoadingState {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            frame: 0,
        }
    }

    pub fn frame(mut self, frame: usize) -> Self {
        self.frame = frame % LOADING_FRAMES.len();
        self
    }
}

impl View for LoadingState {
    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new().fg(theme.primary).bold()
    }

    fn content(&self) -> Content {
        Content::Widget(WidgetContent::Spinner(SpinnerWidget {
            label: Some(self.label.clone()),
            style: SpinnerStyle::Braille,
        }))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let style = Style::default().fg(to_ratatui_color(theme.primary));
        let text = format!("{} {}", LOADING_FRAMES[self.frame], self.label);
        frame.render_widget(Paragraph::new(text).style(style), area);
    }
}

/// A reusable error-state widget.
#[derive(Debug, Clone)]
pub struct ErrorState {
    message: String,
    hint: Option<String>,
}

impl ErrorState {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn text(&self) -> String {
        let mut lines = vec![format!("Error: {}", self.message)];
        if let Some(hint) = &self.hint {
            lines.push(format!("Hint: {}", hint));
        }
        lines.join("\n")
    }
}

impl View for ErrorState {
    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new().fg(theme.error).bold()
    }

    fn content(&self) -> Content {
        Content::Text(ultty_protocol::TextContent::new(self.text()))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let style = Style::default()
            .fg(to_ratatui_color(theme.error))
            .add_modifier(Modifier::BOLD);
        frame.render_widget(Paragraph::new(Line::from(self.text())).style(style), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_content_contains_title() {
        let empty = EmptyState::new("No items");
        match empty.content() {
            Content::Text(tc) => assert!(tc.text.contains("No items")),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn empty_state_with_detail_and_hint() {
        let empty = EmptyState::new("No projects")
            .detail("Create your first project")
            .hint("Press n to create");
        match empty.content() {
            Content::Text(tc) => {
                assert!(tc.text.contains("No projects"));
                assert!(tc.text.contains("Create your first project"));
                assert!(tc.text.contains("Hint: Press n to create"));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn loading_state_content_uses_spinner_widget() {
        let loading = LoadingState::new("Loading data");
        match loading.content() {
            Content::Widget(WidgetContent::Spinner(sp)) => {
                assert_eq!(sp.label.as_deref(), Some("Loading data"));
                assert_eq!(sp.style, SpinnerStyle::Braille);
            }
            _ => panic!("Expected Spinner widget content"),
        }
    }

    #[test]
    fn loading_frame_wraps() {
        let loading = LoadingState::new("Loading").frame(LOADING_FRAMES.len() + 2);
        assert_eq!(loading.frame, 2);
    }

    #[test]
    fn error_state_content_contains_message() {
        let error = ErrorState::new("Request failed").hint("Retry in a moment");
        match error.content() {
            Content::Text(tc) => {
                assert!(tc.text.contains("Error: Request failed"));
                assert!(tc.text.contains("Hint: Retry in a moment"));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn state_widgets_not_focusable() {
        assert!(!EmptyState::new("x").is_focusable());
        assert!(!LoadingState::new("x").is_focusable());
        assert!(!ErrorState::new("x").is_focusable());
    }
}
