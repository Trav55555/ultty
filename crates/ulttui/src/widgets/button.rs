//! Button widget.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;

/// Clickable button.
#[derive(Debug)]
pub struct Button {
    label: String,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    pub fn on_click<F, M>(self, _handler: F) -> Self
    where
        F: Fn() -> M + 'static,
    {
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

impl View for Button {
    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let styled_label = format!("[ {} ]", self.label);
        let style = Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD);

        let paragraph = Paragraph::new(Line::styled(styled_label, style));
        frame.render_widget(paragraph, area);
    }
}
