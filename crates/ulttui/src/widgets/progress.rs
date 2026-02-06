//! Progress bar widget.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Content, TextContent};

/// A horizontal progress bar.
#[derive(Debug)]
pub struct ProgressBar {
    value: f32,
    label: Option<String>,
}

impl ProgressBar {
    pub fn new(value: f32) -> Self {
        Self {
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
    fn content(&self) -> Content {
        let pct = self.value as u32;
        let text = match &self.label {
            Some(label) => format!("{label} [{pct}%]"),
            None => format!("[{pct}%]"),
        };
        Content::Text(TextContent::new(text))
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
        let content = bar.content();
        match content {
            Content::Text(tc) => assert!(tc.text.contains("Loading")),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn content_shows_percentage() {
        let bar = ProgressBar::new(42.0);
        let content = bar.content();
        match content {
            Content::Text(tc) => assert!(tc.text.contains("42%")),
            _ => panic!("Expected Text content"),
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
