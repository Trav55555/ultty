//! Shared logic for binary toggle controls (Checkbox, Toggle).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};

/// Internal binary toggle state shared by Checkbox and Toggle widgets.
#[derive(Debug)]
pub(crate) struct BinaryToggle {
    pub state: bool,
    pub label: String,
    pub id: Option<String>,
    pub prefix_on: &'static str,
    pub prefix_off: &'static str,
}

impl BinaryToggle {
    pub fn new(
        label: impl Into<String>,
        prefix_on: &'static str,
        prefix_off: &'static str,
    ) -> Self {
        Self {
            state: false,
            label: label.into(),
            id: None,
            prefix_on,
            prefix_off,
        }
    }

    pub fn toggle(&mut self) {
        self.state = !self.state;
    }

    /// A control with `role` (checkbox or switch), named by its label,
    /// with its state; the `[x] ` drawn before the label is not its name.
    pub fn semantics_as(&self, role: ultty_protocol::Role) -> ultty_protocol::Semantics {
        ultty_protocol::Semantics {
            role,
            label: Some(self.label.clone()),
            toggled: if self.state {
                ultty_protocol::Toggled::True
            } else {
                ultty_protocol::Toggled::False
            },
            ..Default::default()
        }
    }

    fn prefix(&self) -> &'static str {
        if self.state {
            self.prefix_on
        } else {
            self.prefix_off
        }
    }
}

impl View for BinaryToggle {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn style(&self) -> ultty_protocol::Style {
        let theme = Theme::current();
        let is_focused = super::is_widget_focused(self.widget_id());
        let fg = if is_focused {
            theme.control.indicator
        } else if self.state {
            theme.control.active_fg
        } else {
            theme.control.fg
        };
        ultty_protocol::Style::new().fg(fg).bold()
    }

    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::Text(ultty_protocol::TextContent {
            text: format!("{}{}", self.prefix(), self.label),
            wrap: ultty_protocol::TextWrap::None,
            align: ultty_protocol::TextAlign::Left,
            overflow: ultty_protocol::TextOverflow::Clip,
        })
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &KeyEvent) -> Option<FocusAction> {
        match &key.key {
            Key::Char(' ') | Key::Enter => {
                self.toggle();
                Some(FocusAction::Consumed)
            }
            Key::Tab if key.modifiers.shift => Some(FocusAction::Previous),
            Key::Tab => Some(FocusAction::Next),
            Key::Left | Key::Up => Some(FocusAction::Previous),
            Key::Right | Key::Down => Some(FocusAction::Next),
            _ => None,
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let is_focused = super::is_widget_focused(self.widget_id());

        let theme = Theme::current();
        let fg_color = if self.state {
            to_ratatui_color(theme.control.active_fg)
        } else {
            to_ratatui_color(theme.control.fg)
        };
        let label_style = Style::default().fg(fg_color).add_modifier(Modifier::BOLD);
        let prefix_style = if is_focused {
            Style::default()
                .fg(to_ratatui_color(theme.control.indicator))
                .add_modifier(Modifier::BOLD)
        } else {
            label_style
        };
        let line = Line::from(vec![
            Span::styled(self.prefix(), prefix_style),
            Span::styled(self.label.as_str(), label_style),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_toggle_starts_off() {
        let bt = BinaryToggle::new("Test", "[x] ", "[ ] ");
        assert!(!bt.state);
    }

    #[test]
    fn binary_toggle_toggle() {
        let mut bt = BinaryToggle::new("Test", "(*) ", "( ) ");
        bt.toggle();
        assert!(bt.state);
        bt.toggle();
        assert!(!bt.state);
    }

    #[test]
    fn binary_toggle_prefix() {
        let mut bt = BinaryToggle::new("Test", "ON ", "OFF ");
        assert_eq!(bt.prefix(), "OFF ");
        bt.state = true;
        assert_eq!(bt.prefix(), "ON ");
    }
}
