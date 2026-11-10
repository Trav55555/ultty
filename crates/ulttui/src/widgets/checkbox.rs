//! Checkbox widget.

use ratatui::layout::Rect;
use ratatui::Frame;

use super::binary_toggle::BinaryToggle;
use super::{FocusAction, View};
use crate::event::KeyEvent;

/// A toggleable checkbox with a label.
#[derive(Debug)]
pub struct Checkbox(BinaryToggle);

impl Checkbox {
    /// Create a new unchecked checkbox with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self(BinaryToggle::new(label, "[x] ", "[ ] "))
    }

    /// Set the initial checked state.
    pub fn checked(mut self, checked: bool) -> Self {
        self.0.state = checked;
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.0.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.0.id.as_deref()
    }

    /// Returns whether the checkbox is currently checked.
    pub fn is_checked(&self) -> bool {
        self.0.state
    }

    /// Toggle the checked state.
    pub fn toggle(&mut self) {
        self.0.toggle();
    }
}

impl View for Checkbox {
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
        self.0.semantics_as(ultty_protocol::Role::Checkbox)
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
    fn checkbox_is_a_checkbox_named_by_its_label_with_its_state() {
        use ultty_protocol::{Role, Toggled};
        let semantics = Checkbox::new("Accept terms").semantics();
        assert_eq!(semantics.role, Role::Checkbox);
        assert_eq!(semantics.label.as_deref(), Some("Accept terms"));
        assert_eq!(semantics.toggled, Toggled::False);
        let mut checked = Checkbox::new("Accept terms");
        checked.toggle();
        assert_eq!(checked.semantics().toggled, Toggled::True);
    }

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

    #[test]
    fn shift_tab_moves_focus_previous() {
        let mut cb = Checkbox::new("Test");
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers {
                shift: true,
                ..crate::event::Modifiers::default()
            },
            focused_id: None,
        };
        let result = cb.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    #[test]
    fn focused_checkbox_style_uses_indicator_color() {
        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("terms".to_string());
            fm.focus("terms");
        });

        let cb = Checkbox::new("Accept").id("terms");
        let style = <Checkbox as View>::style(&cb);
        let theme = Theme::dark();
        assert_eq!(style.fg, Some(theme.control.indicator));

        FocusManager::set_current(old);
    }
}
