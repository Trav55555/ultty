//! Button widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::Style as ProtoStyle;

/// Clickable button.
#[derive(Debug)]
pub struct Button {
    label: String,
    id: Option<String>,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            id: None,
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

impl View for Button {
    fn widget_id(&self) -> Option<&str> {
        Button::widget_id(self)
    }

    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new()
            .fg(theme.button.fg)
            .bg(theme.button.bg)
            .bold()
    }

    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::Text(ultty_protocol::TextContent::new(format!(
            "[ {} ]",
            self.label
        )))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        // Register with focus manager if we have an ID
        if let Some(id) = self.widget_id() {
            crate::focus::FocusManager::with_current_mut(|fm| fm.register(id.to_string()));
        }
        let is_focused = self
            .widget_id()
            .map(|id| crate::focus::FocusManager::with_current(|fm| fm.is_focused(id)))
            .unwrap_or(false);

        let styled_label = format!("[ {} ]", self.label);
        let theme = Theme::current();
        let (fg, bg) = if is_focused {
            (
                to_ratatui_color(theme.button.focused_fg),
                to_ratatui_color(theme.button.focused_bg),
            )
        } else {
            (
                to_ratatui_color(theme.button.fg),
                to_ratatui_color(theme.button.bg),
            )
        };
        let style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);

        let paragraph = Paragraph::new(Line::styled(styled_label, style));
        frame.render_widget(paragraph, area);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &KeyEvent) -> Option<FocusAction> {
        match key.key {
            Key::Enter => Some(FocusAction::Consumed),
            Key::Tab if key.modifiers.shift => Some(FocusAction::Previous),
            Key::Tab => Some(FocusAction::Next),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::Content;

    #[test]
    fn button_label() {
        let btn = Button::new("Click me");
        assert_eq!(btn.label(), "Click me");
    }

    #[test]
    fn button_content() {
        let btn = Button::new("OK");
        let content = <Button as View>::content(&btn);
        match content {
            Content::Text(tc) => assert_eq!(tc.text, "[ OK ]"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn button_style() {
        let btn = Button::new("X");
        let style = <Button as View>::style(&btn);
        let theme = crate::theme::Theme::dark();
        assert!(style.bold);
        assert_eq!(style.fg, Some(theme.button.fg));
        assert_eq!(style.bg, Some(theme.button.bg));
    }

    #[test]
    fn button_is_focusable() {
        let btn = Button::new("X");
        assert!(btn.is_focusable());
    }

    #[test]
    fn button_with_id() {
        let btn = Button::new("OK").id("submit");
        assert_eq!(btn.widget_id(), Some("submit"));
    }

    #[test]
    fn button_on_key_enter() {
        let mut btn = Button::new("X");
        let key = KeyEvent {
            key: Key::Enter,
            modifiers: Default::default(),
        };
        assert!(matches!(btn.on_key(&key), Some(FocusAction::Consumed)));
    }

    #[test]
    fn button_on_key_tab() {
        let mut btn = Button::new("X");
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: Default::default(),
        };
        assert!(matches!(btn.on_key(&key), Some(FocusAction::Next)));
    }

    #[test]
    fn button_on_key_unhandled() {
        let mut btn = Button::new("X");
        let key = KeyEvent {
            key: Key::Char('a'),
            modifiers: Default::default(),
        };
        assert!(btn.on_key(&key).is_none());
    }

    #[test]
    fn button_registers_with_focus_manager() {
        use crate::focus::FocusManager;

        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| fm.begin_frame());

        let btn = Button::new("OK").id("submit");
        if let Some(id) = btn.widget_id() {
            FocusManager::with_current_mut(|fm| fm.register(id.to_string()));
        }
        FocusManager::with_current_mut(|fm| fm.focus_next());

        let focused = FocusManager::with_current(|fm| fm.focused_id().map(str::to_string));
        assert_eq!(focused.as_deref(), Some("submit"));

        FocusManager::set_current(old);
    }

    #[test]
    fn focused_button_is_detected() {
        use crate::focus::FocusManager;

        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("btn".to_string());
            fm.focus("btn");
        });

        let btn = Button::new("OK").id("btn");
        let is_focused = btn
            .widget_id()
            .map(|id| FocusManager::with_current(|fm| fm.is_focused(id)))
            .unwrap_or(false);
        assert!(is_focused);

        FocusManager::set_current(old);
    }
}
