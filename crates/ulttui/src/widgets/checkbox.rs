//! Checkbox widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};

/// A toggleable checkbox with a label.
#[derive(Debug)]
pub struct Checkbox {
    checked: bool,
    label: String,
    id: Option<String>,
}

impl Checkbox {
    /// Create a new unchecked checkbox with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            checked: false,
            label: label.into(),
            id: None,
        }
    }

    /// Set the initial checked state.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
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
    fn widget_id(&self) -> Option<&str> {
        Checkbox::widget_id(self)
    }

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
        use ratatui::text::Span;

        // Register with focus manager if we have an ID
        if let Some(id) = self.widget_id() {
            crate::focus::FocusManager::with_current_mut(|fm| fm.register(id.to_string()));
        }
        let is_focused = self
            .widget_id()
            .map(|id| crate::focus::FocusManager::with_current(|fm| fm.is_focused(id)))
            .unwrap_or(false);

        let prefix = if self.checked { "[x] " } else { "[ ] " };
        let theme = Theme::current();
        let fg_color = if self.checked {
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
            Span::styled(prefix, prefix_style),
            Span::styled(self.label.as_str(), label_style),
        ]);
        let paragraph = Paragraph::new(line);
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
    fn checkbox_with_id() {
        let cb = Checkbox::new("Accept").id("terms");
        assert_eq!(cb.widget_id(), Some("terms"));
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
            focused_id: None,
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
            focused_id: None,
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
            focused_id: None,
        };
        let result = cb.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Next)));
    }
}
