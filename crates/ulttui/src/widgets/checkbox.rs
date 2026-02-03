//! Checkbox widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};

/// A toggleable checkbox with a label.
#[derive(Debug)]
pub struct Checkbox {
    checked: bool,
    label: String,
}

impl Checkbox {
    /// Create a new unchecked checkbox with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            checked: false,
            label: label.into(),
        }
    }

    /// Set the initial checked state.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Returns whether the checkbox is currently checked.
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// Toggle the checked state.
    pub fn toggle(&mut self) {
        self.checked = !self.checked;
    }
}

impl View for Checkbox {
    fn content(&self) -> ultty_protocol::Content {
        let prefix = if self.checked { "[x] " } else { "[ ] " };
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
        let prefix = if self.checked { "[x] " } else { "[ ] " };
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
    fn new_checkbox_is_unchecked() {
        let cb = Checkbox::new("Accept terms");
        assert!(!cb.is_checked());
    }

    #[test]
    fn checkbox_checked_builder() {
        let cb = Checkbox::new("Accept").checked(true);
        assert!(cb.is_checked());
    }

    #[test]
    fn toggle_flips_state() {
        let mut cb = Checkbox::new("Test");
        assert!(!cb.is_checked());
        cb.toggle();
        assert!(cb.is_checked());
        cb.toggle();
        assert!(!cb.is_checked());
    }

    #[test]
    fn content_unchecked() {
        let cb = Checkbox::new("Label");
        match cb.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.starts_with("[ ] "));
                assert!(tc.text.contains("Label"));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn content_checked() {
        let cb = Checkbox::new("Label").checked(true);
        match cb.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.starts_with("[x] "));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn is_focusable() {
        let cb = Checkbox::new("Test");
        assert!(cb.is_focusable());
    }

    #[test]
    fn space_toggles() {
        let mut cb = Checkbox::new("Test");
        let key = KeyEvent {
            key: Key::Char(' '),
            modifiers: crate::event::Modifiers::default(),
        };
        let result = cb.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(cb.is_checked());
    }

    #[test]
    fn enter_toggles() {
        let mut cb = Checkbox::new("Test");
        let key = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
        };
        let result = cb.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(cb.is_checked());
    }

    #[test]
    fn tab_moves_focus() {
        let mut cb = Checkbox::new("Test");
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers::default(),
        };
        let result = cb.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Next)));
    }
}
