//! Spinner widget for loading indicators.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Constraints, Content, Dimension, SpinnerStyle, SpinnerWidget, WidgetContent};

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
    fn constraints(&self) -> Constraints {
        Constraints {
            width: Dimension::Cells(1.0),
            height: Dimension::Cells(1.0),
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        Content::Widget(WidgetContent::Spinner(SpinnerWidget {
            label: self.label.clone(),
            style: SpinnerStyle::Braille,
        }))
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

    #[test]
    fn content_returns_spinner_widget() {
        let spinner = Spinner::new();
        match spinner.content() {
            Content::Widget(WidgetContent::Spinner(sw)) => {
                assert_eq!(sw.style, SpinnerStyle::Braille);
                assert!(sw.label.is_none());
            }
            _ => panic!("Expected Spinner widget content"),
        }
    }

    #[test]
    fn frame_wraps() {
        let spinner = Spinner::new().frame(FRAMES.len() + 1);
        assert_eq!(spinner.frame, 1);
    }

    #[test]
    fn content_preserves_label() {
        let spinner = Spinner::new().label("Loading");
        match spinner.content() {
            Content::Widget(WidgetContent::Spinner(sw)) => {
                assert_eq!(sw.label.as_deref(), Some("Loading"));
            }
            _ => panic!("Expected Spinner widget content"),
        }
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
