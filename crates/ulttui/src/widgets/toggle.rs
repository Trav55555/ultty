//! Toggle switch widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};

/// A toggle switch with a label.
#[derive(Debug)]
pub struct Toggle {
    on: bool,
    label: String,
    id: Option<String>,
}

impl Toggle {
    /// Create a new toggle in the off state with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            on: false,
            label: label.into(),
            id: None,
        }
    }

    /// Set the initial on/off state.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
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
    fn widget_id(&self) -> Option<&str> {
        Toggle::widget_id(self)
    }

    fn style(&self) -> ultty_protocol::Style {
        let theme = Theme::current();
        let is_focused = super::register_and_check_focus(self.widget_id());
        let fg = if is_focused {
            theme.control.indicator
        } else if self.on {
            theme.control.active_fg
        } else {
            theme.control.fg
        };
        ultty_protocol::Style::new().fg(fg).bold()
    }

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
            Key::Tab if key.modifiers.shift => Some(FocusAction::Previous),
            Key::Tab => Some(FocusAction::Next),
            Key::Left | Key::Up => Some(FocusAction::Previous),
            Key::Right | Key::Down => Some(FocusAction::Next),
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

        let prefix = if self.on { "(*) " } else { "( ) " };
        let theme = Theme::current();
        let fg_color = if self.on {
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
    use crate::focus::FocusManager;

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
    fn toggle_with_id() {
        let t = Toggle::new("Dark mode").id("theme");
        assert_eq!(t.widget_id(), Some("theme"));
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
            focused_id: None,
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
            focused_id: None,
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
            focused_id: None,
        };
        let result = t.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Next)));
    }

    #[test]
    fn shift_tab_moves_focus_previous() {
        let mut t = Toggle::new("Test");
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers {
                shift: true,
                ..crate::event::Modifiers::default()
            },
            focused_id: None,
        };
        let result = t.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    #[test]
    fn focused_toggle_style_uses_indicator_color() {
        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("theme".to_string());
            fm.focus("theme");
        });

        let t = Toggle::new("Theme").id("theme");
        let style = <Toggle as View>::style(&t);
        let theme = Theme::dark();
        assert_eq!(style.fg, Some(theme.control.indicator));

        FocusManager::set_current(old);
    }
}
