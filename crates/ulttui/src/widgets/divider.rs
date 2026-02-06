//! Divider/separator widget.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Constraints, Content, Dimension, TextContent};

/// Divider widget for visual separation.
#[derive(Debug, Clone, Copy)]
pub struct Divider {
    horizontal: bool,
    ch: char,
}

impl Divider {
    pub fn horizontal() -> Self {
        Self {
            horizontal: true,
            ch: '─',
        }
    }

    pub fn vertical() -> Self {
        Self {
            horizontal: false,
            ch: '│',
        }
    }

    pub fn char(mut self, ch: char) -> Self {
        self.ch = ch;
        self
    }
}

impl View for Divider {
    fn constraints(&self) -> Constraints {
        if self.horizontal {
            Constraints {
                height: Dimension::cells(1.0),
                ..Default::default()
            }
        } else {
            Constraints {
                width: Dimension::cells(1.0),
                ..Default::default()
            }
        }
    }

    fn content(&self) -> Content {
        Content::Text(TextContent::new(self.ch.to_string()))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let style = Style::default().fg(to_ratatui_color(theme.muted));

        let content = if self.horizontal {
            self.ch.to_string().repeat(area.width as usize)
        } else {
            std::iter::repeat_n(self.ch, area.height as usize)
                .map(|ch| ch.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let paragraph = Paragraph::new(content).style(style);
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_default_char() {
        let divider = Divider::horizontal();
        let content = divider.content();
        match content {
            Content::Text(tc) => assert_eq!(tc.text, "─"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn vertical_default_char() {
        let divider = Divider::vertical();
        let content = divider.content();
        match content {
            Content::Text(tc) => assert_eq!(tc.text, "│"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn custom_char() {
        let divider = Divider::horizontal().char('=');
        let content = divider.content();
        match content {
            Content::Text(tc) => assert_eq!(tc.text, "="),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn not_focusable() {
        let divider = Divider::horizontal();
        assert!(!divider.is_focusable());
    }

    #[test]
    fn horizontal_constraints() {
        let divider = Divider::horizontal();
        let constraints = divider.constraints();
        assert_eq!(constraints.height, Dimension::cells(1.0));
    }
}
