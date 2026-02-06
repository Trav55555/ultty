//! Spinner widget for loading indicators.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Content, TextContent};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Animated spinner indicator.
#[derive(Debug, Clone)]
pub struct Spinner {
    frame: usize,
    label: Option<String>,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            frame: 0,
            label: None,
        }
    }

    pub fn frame(mut self, frame: usize) -> Self {
        self.frame = frame % FRAMES.len();
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn frame_count() -> usize {
        FRAMES.len()
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Spinner {
    fn content(&self) -> Content {
        let frame = FRAMES[self.frame];
        let text = match &self.label {
            Some(label) => format!("{frame} {label}"),
            None => frame.to_string(),
        };
        Content::Text(TextContent::new(text))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let display = match &self.label {
            Some(label) => format!("{} {label}", FRAMES[self.frame]),
            None => FRAMES[self.frame].to_string(),
        };
        let style = Style::default().fg(to_ratatui_color(theme.primary));
        let paragraph = Paragraph::new(display).style(style);
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_text(spinner: &Spinner) -> String {
        match spinner.content() {
            Content::Text(tc) => tc.text,
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn default_frame_zero() {
        let spinner = Spinner::new();
        let text = content_text(&spinner);
        assert!(text.starts_with(FRAMES[0]));
    }

    #[test]
    fn frame_wraps() {
        let spinner = Spinner::new().frame(FRAMES.len() + 1);
        let text = content_text(&spinner);
        assert!(text.starts_with(FRAMES[1]));
    }

    #[test]
    fn label_builder() {
        let spinner = Spinner::new().label("Loading");
        let text = content_text(&spinner);
        assert!(text.contains("Loading"));
    }

    #[test]
    fn frame_count_is_10() {
        assert_eq!(Spinner::frame_count(), 10);
    }

    #[test]
    fn not_focusable() {
        let spinner = Spinner::new();
        assert!(!spinner.is_focusable());
    }
}
