//! Toggle switch widget.

use ratatui::layout::Rect;
use ratatui::Frame;

use super::binary_toggle::BinaryToggle;
use super::{FocusAction, View};
use crate::event::KeyEvent;

/// A toggle switch with a label.
#[derive(Debug)]
pub struct Toggle(BinaryToggle);

impl Toggle {
    /// Create a new toggle in the off state with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self(BinaryToggle::new(label, "(*) ", "( ) "))
    }

    /// Set the initial on/off state.
    pub fn on(mut self, on: bool) -> Self {
        self.0.state = on;
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.0.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.0.id.as_deref()
    }

    /// Returns whether the toggle is currently on.
    pub fn is_on(&self) -> bool {
        self.0.state
    }

    /// Toggle the on/off state.
    pub fn toggle(&mut self) {
        self.0.toggle();
    }
}

impl View for Toggle {
    fn widget_id(&self) -> Option<&str> {
        self.0.widget_id()
    }
    fn style(&self) -> ultty_protocol::Style {
        self.0.style()
    }
    fn content(&self) -> ultty_protocol::Content {
        self.0.content()
    }
    fn semantics(&self) -> ultty_protocol::Semantics {
        self.0.semantics_as(ultty_protocol::Role::Switch)
    }
    fn is_focusable(&self) -> bool {
        self.0.is_focusable()
    }
    fn on_key(&mut self, key: &KeyEvent) -> Option<FocusAction> {
        self.0.on_key(key)
    }
    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        self.0.render_ansi(frame, area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, KeyEvent};
    use crate::focus::FocusManager;
    use crate::theme::Theme;

    #[test]
    fn toggle_is_a_switch_with_its_state() {
        use ultty_protocol::{Role, Toggled};
        let semantics = Toggle::new("Dark mode").on(true).semantics();
        assert_eq!(semantics.role, Role::Switch);
        assert_eq!(semantics.label.as_deref(), Some("Dark mode"));
        assert_eq!(semantics.toggled, Toggled::True);
        assert_eq!(Toggle::new("Dark mode").semantics().toggled, Toggled::False);
    }

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
