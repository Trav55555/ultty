//! Shared state widgets for empty, loading, and error UI.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Content, SpinnerStyle, SpinnerWidget, Style as ProtoStyle, WidgetContent};

const LOADING_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const LOADING_FRAME_PERIOD: Duration = Duration::from_millis(80);

/// Spinner frame for the given time since the animation epoch.
fn frame_for_elapsed(elapsed: Duration) -> usize {
    let ticks = elapsed.as_millis() / LOADING_FRAME_PERIOD.as_millis();
    (ticks % LOADING_FRAMES.len() as u128) as usize
}

/// Process-wide animation epoch so every spinner shares one phase.
fn clock_frame() -> usize {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    frame_for_elapsed(EPOCH.get_or_init(Instant::now).elapsed())
}

/// A reusable empty-state widget.
#[derive(Debug, Clone)]
pub struct EmptyState {
    id: Option<String>,
    title: String,
    detail: Option<String>,
    hint: Option<String>,
}

impl EmptyState {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: None,
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
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

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

/// A reusable loading-state widget (spinner plus label).
///
/// Animation differs by backend:
/// - **Native (Ultty)**: the terminal animates the spinner widget itself; no
///   app involvement is needed.
/// - **ANSI**: each render picks the frame from a monotonic clock, so the
///   spinner advances whenever the app re-renders. The runner only renders
///   when something changed, so an otherwise idle screen needs a re-render
///   trigger such as `Sub::every(Duration::from_millis(80), ..)` to animate
///   smoothly. Use [`LoadingState::frame`] to pin a specific frame instead
///   (for example in snapshot tests).
#[derive(Debug, Clone)]
pub struct LoadingState {
    id: Option<String>,
    label: String,
    frame: Option<usize>,
}

impl LoadingState {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: None,
            label: label.into(),
            frame: None,
        }
    }

    /// Pin the spinner to a specific frame instead of following the clock.
    pub fn frame(mut self, frame: usize) -> Self {
        self.frame = Some(frame % LOADING_FRAMES.len());
        self
    }

    fn current_frame(&self) -> usize {
        self.frame.unwrap_or_else(clock_frame)
    }
}

impl View for LoadingState {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

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
        let text = format!("{} {}", LOADING_FRAMES[self.current_frame()], self.label);
        frame.render_widget(Paragraph::new(text).style(style), area);
    }
}

/// A reusable error-state widget.
#[derive(Debug, Clone)]
pub struct ErrorState {
    id: Option<String>,
    message: String,
    hint: Option<String>,
}

impl ErrorState {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(message: impl Into<String>) -> Self {
        Self {
            id: None,
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
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// An alert, named by the error and described by the hint.
    fn semantics(&self) -> ultty_protocol::Semantics {
        ultty_protocol::Semantics {
            role: ultty_protocol::Role::Alert,
            label: Some(format!("Error: {}", self.message)),
            description: self.hint.clone(),
            ..Default::default()
        }
    }

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
        assert_eq!(loading.current_frame(), 2);
    }

    #[test]
    fn loading_frame_advances_with_clock_when_not_pinned() {
        assert_eq!(frame_for_elapsed(Duration::ZERO), 0);
        assert_eq!(frame_for_elapsed(LOADING_FRAME_PERIOD), 1);
        assert_eq!(frame_for_elapsed(LOADING_FRAME_PERIOD * 3), 3);
        let full_cycle = LOADING_FRAME_PERIOD * LOADING_FRAMES.len() as u32;
        assert_eq!(frame_for_elapsed(full_cycle), 0);
        assert_eq!(frame_for_elapsed(full_cycle + LOADING_FRAME_PERIOD * 2), 2);

        let unpinned = LoadingState::new("Loading");
        assert!(unpinned.current_frame() < LOADING_FRAMES.len());
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
