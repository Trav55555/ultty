//! Toggle switch widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};

/// A toggle switch with a label.
#[derive(Debug)]
pub struct Toggle {
    on: bool,
    label: String,
}

impl Toggle {
    /// Create a new toggle in the off state with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            on: false,
            label: label.into(),
        }
    }

    /// Set the initial on/off state.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    /// Returns whether the toggle is currently on.
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Toggle the on/off state.
    pub fn toggle(&mut self) {
        self.on = !self.on;
    }
}

impl View for Toggle {
    fn content(&self) -> ultty_protocol::Content {
        let prefix = if self.on { "(*) " } else { "( ) " };
        ultty_protocol::Content::Text(ultty_protocol::TextContent {
            text: format!("{}{}", prefix, self.label),
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
            Key::Tab => Some(FocusAction::Next),
            _ => None,
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let prefix = if self.on { "(*) " } else { "( ) " };
        let text = format!("{}{}", prefix, self.label);
        let style = Style::default().add_modifier(Modifier::BOLD);
        let paragraph = Paragraph::new(Line::from(text)).style(style);
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_toggle_is_off() {
        let t = Toggle::new("Dark mode");
        assert!(!t.is_on());
    }

    #[test]
    fn toggle_on_builder() {
        let t = Toggle::new("Dark mode").on(true);
        assert!(t.is_on());
    }

    #[test]
    fn toggle_flips_state() {
        let mut t = Toggle::new("Test");
        assert!(!t.is_on());
        t.toggle();
        assert!(t.is_on());
        t.toggle();
        assert!(!t.is_on());
    }

    #[test]
    fn content_off() {
        let t = Toggle::new("Label");
        match t.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.starts_with("( ) "));
                assert!(tc.text.contains("Label"));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn content_on() {
        let t = Toggle::new("Label").on(true);
        match t.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.starts_with("(*) "));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn is_focusable() {
        let t = Toggle::new("Test");
        assert!(t.is_focusable());
    }

    #[test]
    fn space_toggles() {
        let mut t = Toggle::new("Test");
        let key = KeyEvent {
            key: Key::Char(' '),
            modifiers: crate::event::Modifiers::default(),
        };
        let result = t.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(t.is_on());
    }

    #[test]
    fn enter_toggles() {
        let mut t = Toggle::new("Test");
        let key = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
        };
        let result = t.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(t.is_on());
    }

    #[test]
    fn tab_moves_focus() {
        let mut t = Toggle::new("Test");
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers::default(),
        };
        let result = t.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Next)));
    }
}
