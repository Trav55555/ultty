//! Progress bar widget.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Content, ProgressWidget, WidgetContent};

/// A horizontal progress bar.
#[derive(Debug)]
pub struct ProgressBar {
    id: Option<String>,
    value: f32,
    label: Option<String>,
}

impl ProgressBar {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(value: f32) -> Self {
        Self {
            id: None,
            value: value.clamp(0.0, 100.0),
            label: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

impl View for ProgressBar {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn content(&self) -> Content {
        Content::Widget(WidgetContent::Progress(ProgressWidget {
            value: self.value / 100.0,
            label: self.label.clone(),
            animated: false,
            indeterminate: false,
        }))
    }

    fn semantics(&self) -> ultty_protocol::Semantics {
        ultty_protocol::Semantics {
            role: ultty_protocol::Role::Progress,
            label: self.label.clone(),
            value: Some(ultty_protocol::SemanticValue {
                now: self.value / 100.0,
                min: 0.0,
                max: 1.0,
            }),
            ..Default::default()
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let suffix = format!(" {:.0}%", self.value);
        let prefix = match &self.label {
            Some(label) => format!("{label} "),
            None => String::new(),
        };

        let reserved = prefix.len() + suffix.len();
        let bar_width = area.width.saturating_sub(reserved as u16) as usize;
        let filled = ((self.value / 100.0) * bar_width as f32) as usize;
        let empty = bar_width.saturating_sub(filled);

        let bar = "=".repeat(filled) + &"-".repeat(empty);
        let display = format!("{prefix}{bar}{suffix}");

        let style = Style::default().fg(to_ratatui_color(theme.primary));
        let paragraph = Paragraph::new(display).style(style);
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(widget: &impl View, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                widget.render_ansi(frame, frame.area());
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn new_clamps_value() {
        assert_eq!(ProgressBar::new(120.0).value(), 100.0);
        assert_eq!(ProgressBar::new(-5.0).value(), 0.0);
    }

    #[test]
    fn label_builder() {
        let bar = ProgressBar::new(10.0).label("Loading");
        match bar.content() {
            Content::Widget(WidgetContent::Progress(pw)) => {
                assert_eq!(pw.label.as_deref(), Some("Loading"));
            }
            _ => panic!("Expected Progress widget content"),
        }
    }

    #[test]
    fn content_returns_progress_widget() {
        let bar = ProgressBar::new(42.0);
        match bar.content() {
            Content::Widget(WidgetContent::Progress(pw)) => {
                assert!((pw.value - 0.42).abs() < 0.01);
                assert!(!pw.animated);
                assert!(!pw.indeterminate);
            }
            _ => panic!("Expected Progress widget content"),
        }
    }

    #[test]
    fn content_value_normalized() {
        let bar = ProgressBar::new(100.0);
        match bar.content() {
            Content::Widget(WidgetContent::Progress(pw)) => {
                assert!((pw.value - 1.0).abs() < f32::EPSILON);
            }
            _ => panic!("Expected Progress widget content"),
        }

        let bar_zero = ProgressBar::new(0.0);
        match bar_zero.content() {
            Content::Widget(WidgetContent::Progress(pw)) => {
                assert!((pw.value).abs() < f32::EPSILON);
            }
            _ => panic!("Expected Progress widget content"),
        }
    }

    #[test]
    fn not_focusable() {
        let bar = ProgressBar::new(10.0);
        assert!(!bar.is_focusable());
    }

    #[test]
    fn zero_percent() {
        let bar = ProgressBar::new(0.0);
        let buffer = render(&bar, 40, 1);
        assert!(buffer.contains("0%"));
    }

    #[test]
    fn hundred_percent() {
        let bar = ProgressBar::new(100.0);
        let buffer = render(&bar, 40, 1);
        assert!(buffer.contains("100%"));
    }
}
