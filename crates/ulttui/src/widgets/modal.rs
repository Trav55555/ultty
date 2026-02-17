//! Modal/Dialog widget.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, BorderStyle, Constraints, Dimension, Display, FlexDirection, RegionId,
    Style as ProtoStyle, TextContent,
};

/// A modal dialog overlay.
pub struct Modal {
    title: String,
    body: String,
    body_view: Option<Box<dyn View>>,
    buttons: Vec<String>,
    selected_button: usize,
    visible: bool,
}

impl Default for Modal {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            body_view: None,
            buttons: vec!["OK".into()],
            selected_button: 0,
            visible: true,
        }
    }
}

impl std::fmt::Debug for Modal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Modal")
            .field("title", &self.title)
            .field("visible", &self.visible)
            .field("selected_button", &self.selected_button)
            .field("has_body_view", &self.body_view.is_some())
            .finish()
    }
}

impl Modal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn buttons(mut self, buttons: Vec<impl Into<String>>) -> Self {
        self.buttons = buttons.into_iter().map(Into::into).collect();
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn body_view(mut self, view: impl View + 'static) -> Self {
        self.body_view = Some(Box::new(view));
        self
    }

    pub fn selected_button_index(&self) -> usize {
        self.selected_button
    }

    pub fn selected_button_label(&self) -> Option<&str> {
        self.buttons.get(self.selected_button).map(|s| s.as_str())
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    fn select_next_button(&mut self) {
        if !self.buttons.is_empty() {
            self.selected_button = (self.selected_button + 1) % self.buttons.len();
        }
    }

    fn select_prev_button(&mut self) {
        if !self.buttons.is_empty() {
            if self.selected_button == 0 {
                self.selected_button = self.buttons.len() - 1;
            } else {
                self.selected_button -= 1;
            }
        }
    }
}

impl View for Modal {
    fn constraints(&self) -> Constraints {
        if !self.visible {
            return Constraints {
                display: Display::None,
                ..Default::default()
            };
        }

        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_self: ultty_protocol::AlignSelf::Center,
            width: Dimension::Cells(50.0),
            height: Dimension::Cells(10.0),
            margin: ultty_protocol::Edges::all(Dimension::Auto),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new()
            .border(BorderStyle::rounded())
            .bg(theme.background)
    }

    fn children_count(&self) -> usize {
        if !self.visible {
            return 0;
        }
        // body text + button row
        2
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        match index {
            0 => {
                // Body text or body_view
                if let Some(ref body_view) = self.body_view {
                    // Create container with body_view's constraints/style
                    send_message(&AppMessage::CreateRegion {
                        id: region_id,
                        parent,
                        insert_index: -1,
                        constraints: body_view.constraints(),
                        style: body_view.style(),
                    })?;
                    // Render body_view's children into the container
                    for i in 0..body_view.children_count() {
                        body_view.render_child_native(i, send_message, region_id, next_id)?;
                    }
                } else {
                    // Existing text body behavior
                    send_message(&AppMessage::CreateRegion {
                        id: region_id,
                        parent,
                        insert_index: -1,
                        constraints: Constraints {
                            display: Display::Flex,
                            ..Default::default()
                        },
                        style: ProtoStyle::default(),
                    })?;
                    send_message(&AppMessage::SetContent {
                        id: region_id,
                        content: ultty_protocol::Content::Text(TextContent::new(&self.body)),
                    })?;
                }
            }
            1 => {
                // Button row
                send_message(&AppMessage::CreateRegion {
                    id: region_id,
                    parent,
                    insert_index: -1,
                    constraints: Constraints {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        width: Dimension::Percent(100.0),
                        gap: ultty_protocol::Size {
                            width: Dimension::Cells(2.0),
                            height: Dimension::Auto,
                        },
                        ..Default::default()
                    },
                    style: ProtoStyle::default(),
                })?;

                for (i, label) in self.buttons.iter().enumerate() {
                    let btn_id = RegionId::new(*next_id);
                    *next_id += 1;
                    let theme = Theme::current();
                    let btn_style = if i == self.selected_button {
                        ProtoStyle::new()
                            .fg(theme.button.focused_fg)
                            .bg(theme.button.focused_bg)
                            .bold()
                    } else {
                        ProtoStyle::new().fg(theme.button.fg).bg(theme.button.bg)
                    };
                    send_message(&AppMessage::CreateRegion {
                        id: btn_id,
                        parent: region_id,
                        insert_index: -1,
                        constraints: Constraints::default(),
                        style: btn_style,
                    })?;
                    send_message(&AppMessage::SetContent {
                        id: btn_id,
                        content: ultty_protocol::Content::Text(TextContent::new(format!(
                            "[ {} ]",
                            label
                        ))),
                    })?;
                }
            }
            _ => return Ok(None),
        }

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        // Center the modal in the available area
        let modal_width = 50u16.min(area.width.saturating_sub(4));
        let modal_height = 10u16.min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_area = Rect::new(x, y, modal_width, modal_height);

        // Clear the background
        frame.render_widget(Clear, modal_area);

        let theme = Theme::current();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title.as_str())
            .border_style(Style::default().fg(to_ratatui_color(theme.primary)));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        // Body text or body_view
        if inner.height > 2 {
            let body_area = Rect::new(
                inner.x,
                inner.y,
                inner.width,
                inner.height.saturating_sub(2),
            );
            if let Some(ref body_view) = self.body_view {
                body_view.render_ansi(frame, body_area);
            } else {
                let body = Paragraph::new(self.body.as_str()).wrap(Wrap { trim: true });
                frame.render_widget(body, body_area);
            }
        }

        // Buttons at bottom
        if inner.height >= 1 {
            let btn_y = inner.y + inner.height.saturating_sub(1);
            let mut btn_x = inner.x;
            for (i, label) in self.buttons.iter().enumerate() {
                let text = format!("[ {} ]", label);
                let style = if i == self.selected_button {
                    Style::default()
                        .fg(to_ratatui_color(theme.button.focused_fg))
                        .bg(to_ratatui_color(theme.button.focused_bg))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(to_ratatui_color(theme.button.fg))
                        .bg(to_ratatui_color(theme.button.bg))
                };
                let btn_width = text.len() as u16;
                if btn_x + btn_width <= inner.x + inner.width {
                    let btn_area = Rect::new(btn_x, btn_y, btn_width, 1);
                    frame.render_widget(Paragraph::new(Line::styled(text, style)), btn_area);
                    btn_x += btn_width + 2;
                }
            }
        }
    }

    fn is_focusable(&self) -> bool {
        self.visible
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<super::FocusAction> {
        use crate::event::Key;
        if !self.visible {
            return None;
        }
        match key.key {
            Key::Left | Key::Up | Key::Char('h') => {
                self.select_prev_button();
                Some(super::FocusAction::Consumed)
            }
            Key::Right | Key::Down | Key::Char('l') => {
                self.select_next_button();
                Some(super::FocusAction::Consumed)
            }
            Key::Tab => {
                if key.modifiers.shift {
                    self.select_prev_button();
                } else {
                    self.select_next_button();
                }
                Some(super::FocusAction::Consumed)
            }
            Key::Enter => Some(super::FocusAction::Consumed),
            Key::Escape => {
                self.hide();
                Some(super::FocusAction::Consumed)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, KeyEvent, Modifiers};
    use crate::widgets::FocusAction;

    #[test]
    fn modal_button_navigation() {
        let mut modal = Modal::new()
            .title("Test")
            .body("Hello")
            .buttons(vec!["OK", "Cancel", "Help"]);

        assert_eq!(modal.selected_button_index(), 0);
        modal.select_next_button();
        assert_eq!(modal.selected_button_index(), 1);
        modal.select_next_button();
        assert_eq!(modal.selected_button_index(), 2);
        modal.select_next_button();
        assert_eq!(modal.selected_button_index(), 0); // wraps

        modal.select_prev_button();
        assert_eq!(modal.selected_button_index(), 2); // wraps back
    }

    #[test]
    fn modal_visibility() {
        let mut modal = Modal::new();
        assert!(modal.is_visible());
        modal.hide();
        assert!(!modal.is_visible());
        modal.show();
        assert!(modal.is_visible());
    }

    #[test]
    fn modal_selected_label() {
        let modal = Modal::new().buttons(vec!["Yes", "No"]);
        assert_eq!(modal.selected_button_label(), Some("Yes"));
    }

    // --- New tests below ---

    #[test]
    fn modal_default_values() {
        let modal = Modal::default();
        assert!(modal.is_visible());
        assert_eq!(modal.selected_button_index(), 0);
        assert_eq!(modal.selected_button_label(), Some("OK"));
    }

    #[test]
    fn modal_builder_chain() {
        let modal = Modal::new()
            .title("Confirm")
            .body("Are you sure?")
            .buttons(vec!["Yes", "No", "Cancel"])
            .visible(false);

        assert!(!modal.is_visible());
        assert_eq!(modal.selected_button_label(), Some("Yes"));
    }

    #[test]
    fn modal_visible_builder() {
        let modal = Modal::new().visible(false);
        assert!(!modal.is_visible());

        let modal = Modal::new().visible(true);
        assert!(modal.is_visible());
    }

    #[test]
    fn modal_empty_buttons_navigation() {
        let mut modal = Modal::new().buttons(Vec::<String>::new());
        // Should not panic on empty buttons
        modal.select_next_button();
        assert_eq!(modal.selected_button_index(), 0);
        modal.select_prev_button();
        assert_eq!(modal.selected_button_index(), 0);
        assert_eq!(modal.selected_button_label(), None);
    }

    #[test]
    fn modal_single_button_wraps() {
        let mut modal = Modal::new().buttons(vec!["Only"]);
        assert_eq!(modal.selected_button_index(), 0);
        modal.select_next_button();
        assert_eq!(modal.selected_button_index(), 0);
        modal.select_prev_button();
        assert_eq!(modal.selected_button_index(), 0);
    }

    #[test]
    fn modal_prev_button_wraps_from_zero() {
        let mut modal = Modal::new().buttons(vec!["A", "B", "C"]);
        assert_eq!(modal.selected_button_index(), 0);
        modal.select_prev_button();
        assert_eq!(modal.selected_button_index(), 2);
    }

    #[test]
    fn modal_debug_format() {
        let modal = Modal::new().title("Test").visible(true);
        let debug = format!("{:?}", modal);
        assert!(debug.contains("Modal"));
        assert!(debug.contains("Test"));
        assert!(debug.contains("visible"));
    }

    #[test]
    fn modal_constraints_when_visible() {
        let modal = Modal::new();
        let c = <Modal as View>::constraints(&modal);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Column);
    }

    #[test]
    fn modal_constraints_when_hidden() {
        let modal = Modal::new().visible(false);
        let c = <Modal as View>::constraints(&modal);
        assert_eq!(c.display, Display::None);
    }

    #[test]
    fn modal_children_count_visible() {
        let modal = Modal::new();
        assert_eq!(<Modal as View>::children_count(&modal), 2);
    }

    #[test]
    fn modal_children_count_hidden() {
        let modal = Modal::new().visible(false);
        assert_eq!(<Modal as View>::children_count(&modal), 0);
    }

    #[test]
    fn modal_is_focusable_when_visible() {
        let modal = Modal::new();
        assert!(modal.is_focusable());
    }

    #[test]
    fn modal_not_focusable_when_hidden() {
        let modal = Modal::new().visible(false);
        assert!(!modal.is_focusable());
    }

    fn key(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers::default(),
            focused_id: None,
        }
    }

    fn key_with_modifiers(k: Key, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers,
            focused_id: None,
        }
    }

    #[test]
    fn modal_on_key_right_selects_next() {
        let mut modal = Modal::new().buttons(vec!["A", "B", "C"]);
        assert_eq!(modal.selected_button_index(), 0);

        let result = modal.on_key(&key(Key::Right));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 1);
    }

    #[test]
    fn modal_on_key_down_selects_next() {
        let mut modal = Modal::new().buttons(vec!["A", "B", "C"]);
        assert_eq!(modal.selected_button_index(), 0);
        let result = modal.on_key(&key(Key::Down));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 1);
    }

    #[test]
    fn modal_on_key_left_selects_prev() {
        let mut modal = Modal::new().buttons(vec!["A", "B", "C"]);
        modal.select_next_button(); // go to 1
        let result = modal.on_key(&key(Key::Left));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 0);
    }

    #[test]
    fn modal_on_key_up_selects_prev() {
        let mut modal = Modal::new().buttons(vec!["A", "B", "C"]);
        modal.select_next_button(); // go to 1
        let result = modal.on_key(&key(Key::Up));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 0);
    }

    #[test]
    fn modal_on_key_h_selects_prev() {
        let mut modal = Modal::new().buttons(vec!["A", "B"]);
        modal.select_next_button(); // go to 1
        let result = modal.on_key(&key(Key::Char('h')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 0);
    }

    #[test]
    fn modal_on_key_l_selects_next() {
        let mut modal = Modal::new().buttons(vec!["A", "B"]);
        let result = modal.on_key(&key(Key::Char('l')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 1);
    }

    #[test]
    fn modal_on_key_tab_selects_next() {
        let mut modal = Modal::new().buttons(vec!["A", "B"]);
        let result = modal.on_key(&key(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 1);
    }

    #[test]
    fn modal_on_key_shift_tab_selects_previous() {
        let mut modal = Modal::new().buttons(vec!["A", "B", "C"]);
        let result = modal.on_key(&key_with_modifiers(
            Key::Tab,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        ));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(modal.selected_button_index(), 2);
    }

    #[test]
    fn modal_on_key_enter_consumed() {
        let mut modal = Modal::new();
        let result = modal.on_key(&key(Key::Enter));
        assert!(matches!(result, Some(FocusAction::Consumed)));
    }

    #[test]
    fn modal_on_key_escape_hides() {
        let mut modal = Modal::new();
        assert!(modal.is_visible());
        let result = modal.on_key(&key(Key::Escape));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(!modal.is_visible());
    }

    #[test]
    fn modal_on_key_unhandled_returns_none() {
        let mut modal = Modal::new();
        let result = modal.on_key(&key(Key::Char('x')));
        assert!(result.is_none());
    }

    #[test]
    fn modal_on_key_when_hidden_returns_none() {
        let mut modal = Modal::new().visible(false);
        let result = modal.on_key(&key(Key::Right));
        assert!(result.is_none());
    }

    #[test]
    fn modal_style_has_border() {
        let modal = Modal::new();
        let style = <Modal as View>::style(&modal);
        assert_ne!(style.border.style, ultty_protocol::BorderType::None);
    }

    #[test]
    fn modal_selected_button_label_after_navigation() {
        let mut modal = Modal::new().buttons(vec!["Save", "Discard", "Cancel"]);
        assert_eq!(modal.selected_button_label(), Some("Save"));
        modal.select_next_button();
        assert_eq!(modal.selected_button_label(), Some("Discard"));
        modal.select_next_button();
        assert_eq!(modal.selected_button_label(), Some("Cancel"));
    }

    #[test]
    fn modal_with_body_view_children_count() {
        use super::super::Text;
        let modal = Modal::new().body_view(Text::new("Rich content"));
        assert_eq!(<Modal as View>::children_count(&modal), 2);
    }

    #[test]
    fn modal_body_string_and_view_independent() {
        use super::super::Text;
        let modal = Modal::new().body("plain text").body_view(Text::new("rich"));
        assert!(modal.body_view.is_some());
    }

    #[test]
    fn modal_debug_with_body_view() {
        use super::super::Text;
        let modal = Modal::new().body_view(Text::new("content"));
        let debug = format!("{:?}", modal);
        assert!(debug.contains("has_body_view"));
        assert!(debug.contains("true"));
    }
}
