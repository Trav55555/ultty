//! Button widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    ButtonStyle as ProtoButtonStyle, ButtonWidget, Role, Semantics, Style as ProtoStyle,
};

/// Clickable button.
#[derive(Debug)]
pub struct Button {
    label: String,
    id: Option<String>,
    style: ButtonStyle,
    flex_grow: f32,
    description: Option<String>,
}

/// Button style variant.
#[derive(Debug, Clone, Copy, Default)]
pub enum ButtonStyle {
    #[default]
    Default,
    Primary,
    Danger,
    Success,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            id: None,
            style: ButtonStyle::default(),
            flex_grow: 0.0,
            description: None,
        }
    }

    /// Longer help text for assistive technology. The button's label is
    /// its accessible name; wrap it in [`super::Semantic`] to name it
    /// otherwise (an icon button, say).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn primary(mut self) -> Self {
        self.style = ButtonStyle::Primary;
        self
    }

    pub fn danger(mut self) -> Self {
        self.style = ButtonStyle::Danger;
        self
    }

    pub fn success(mut self) -> Self {
        self.style = ButtonStyle::Success;
        self
    }

    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.flex_grow = grow;
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
    fn constraints(&self) -> ultty_protocol::Constraints {
        let label_width = self.label.chars().count() as f32 + 4.0;
        ultty_protocol::Constraints {
            width: ultty_protocol::Dimension::Cells(label_width),
            height: ultty_protocol::Dimension::Cells(1.0),
            flex_grow: self.flex_grow,
            ..Default::default()
        }
    }

    fn widget_id(&self) -> Option<&str> {
        Button::widget_id(self)
    }

    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        let is_focused = super::is_widget_focused(self.widget_id());
        let (fg, bg) = if is_focused {
            (theme.button.focused_fg, theme.button.focused_bg)
        } else {
            (theme.button.fg, theme.button.bg)
        };
        ProtoStyle::new().fg(fg).bg(bg).bold()
    }

    fn content(&self) -> ultty_protocol::Content {
        let proto_style = match self.style {
            ButtonStyle::Default => ProtoButtonStyle::Default,
            ButtonStyle::Primary => ProtoButtonStyle::Primary,
            ButtonStyle::Danger => ProtoButtonStyle::Danger,
            ButtonStyle::Success => ProtoButtonStyle::Success,
        };
        ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Button(ButtonWidget {
            label: self.label.clone(),
            style: proto_style,
        }))
    }

    fn semantics(&self) -> Semantics {
        Semantics {
            role: Role::Button,
            label: Some(self.label.clone()),
            description: self.description.clone(),
            ..Semantics::default()
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let is_focused = super::is_widget_focused(self.widget_id());

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
            Key::Left | Key::Up => Some(FocusAction::Previous),
            Key::Right | Key::Down => Some(FocusAction::Next),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::Content;

    #[test]
    fn button_is_a_button_named_by_its_label_which_the_content_already_says() {
        let semantics = Button::new("Run").semantics();
        assert_eq!(semantics.role, Role::Button);
        assert_eq!(semantics.label.as_deref(), Some("Run"));
        let messages = crate::widgets::render_view_messages(&Button::new("Run")).unwrap();
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, ultty_protocol::AppMessage::SetSemantics { .. })),
            "the terminal derives the same from the button content"
        );

        let described = Button::new("Run").description("Runs every test");
        assert_eq!(
            described.semantics().description.as_deref(),
            Some("Runs every test")
        );
    }

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
            Content::Widget(w) => match w {
                ultty_protocol::WidgetContent::Button(b) => {
                    assert_eq!(b.label, "OK");
                    assert_eq!(b.style, ultty_protocol::ButtonStyle::Default);
                }
                _ => panic!("Expected Button widget"),
            },
            _ => panic!("Expected Widget content"),
        }
    }

    #[test]
    fn button_content_primary() {
        let btn = Button::new("Submit").primary();
        let content = <Button as View>::content(&btn);
        match content {
            Content::Widget(w) => match w {
                ultty_protocol::WidgetContent::Button(b) => {
                    assert_eq!(b.label, "Submit");
                    assert_eq!(b.style, ultty_protocol::ButtonStyle::Primary);
                }
                _ => panic!("Expected Button widget"),
            },
            _ => panic!("Expected Widget content"),
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
            focused_id: None,
        };
        assert!(matches!(btn.on_key(&key), Some(FocusAction::Consumed)));
    }

    #[test]
    fn button_on_key_tab() {
        let mut btn = Button::new("X");
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: Default::default(),
            focused_id: None,
        };
        assert!(matches!(btn.on_key(&key), Some(FocusAction::Next)));
    }

    #[test]
    fn button_on_key_unhandled() {
        let mut btn = Button::new("X");
        let key = KeyEvent {
            key: Key::Char('a'),
            modifiers: Default::default(),
            focused_id: None,
        };
        assert!(btn.on_key(&key).is_none());
    }

    #[test]
    fn button_is_registered_by_tree_walk_not_by_rendering() {
        use crate::focus::FocusManager;

        let btn = Button::new("OK").id("submit");

        let old = FocusManager::set_current(FocusManager::new());
        let backend = ratatui::backend::TestBackend::new(10, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| btn.render_ansi(frame, frame.area()))
            .unwrap();
        let _ = <Button as View>::style(&btn);
        let registered_by_render = FocusManager::with_current(|fm| fm.focusable_ids().to_vec());
        FocusManager::set_current(old);
        assert!(registered_by_render.is_empty());

        let mut fm = FocusManager::new();
        fm.register_view_tree(&btn);
        fm.focus_next();
        assert_eq!(fm.focused_id(), Some("submit"));
    }

    #[test]
    fn focused_button_style_uses_focused_colors() {
        use crate::focus::FocusManager;

        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("btn".to_string());
            fm.focus("btn");
        });

        let btn = Button::new("OK").id("btn");
        let style = <Button as View>::style(&btn);
        let theme = crate::theme::Theme::dark();
        assert_eq!(style.fg, Some(theme.button.focused_fg));
        assert_eq!(style.bg, Some(theme.button.focused_bg));

        FocusManager::set_current(old);
    }
}
