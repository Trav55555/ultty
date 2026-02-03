//! Modal/Dialog widget.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::View;
use ultty_protocol::{
    AppMessage, BorderStyle, Color as ProtoColor, Constraints, Dimension, Display, FlexDirection,
    NamedColor, RegionId, Style as ProtoStyle, TextContent,
};

/// A modal dialog overlay.
pub struct Modal {
    title: String,
    body: String,
    buttons: Vec<String>,
    selected_button: usize,
    visible: bool,
}

impl Default for Modal {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
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
            width: Dimension::Cells(50.0),
            max_height: Dimension::Cells(20.0),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        ProtoStyle::new()
            .border(BorderStyle::rounded())
            .bg(ProtoColor::Named(NamedColor::Black))
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
                // Body text
                send_message(&AppMessage::CreateRegion {
                    id: region_id,
                    parent,
                    insert_index: -1,
                    constraints: Constraints {
                        width: Dimension::Percent(100.0),
                        ..Default::default()
                    },
                    style: ProtoStyle::default(),
                })?;
                send_message(&AppMessage::SetContent {
                    id: region_id,
                    content: ultty_protocol::Content::Text(TextContent::new(&self.body)),
                })?;
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
                    let btn_style = if i == self.selected_button {
                        ProtoStyle::new()
                            .fg(ProtoColor::Named(NamedColor::Black))
                            .bg(ProtoColor::Named(NamedColor::White))
                            .bold()
                    } else {
                        ProtoStyle::new()
                            .fg(ProtoColor::Named(NamedColor::White))
                            .bg(ProtoColor::Named(NamedColor::BrightBlack))
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

        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title.as_str())
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        // Body text
        if inner.height > 2 {
            let body_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(2));
            let body = Paragraph::new(self.body.as_str()).wrap(Wrap { trim: true });
            frame.render_widget(body, body_area);
        }

        // Buttons at bottom
        if inner.height >= 1 {
            let btn_y = inner.y + inner.height.saturating_sub(1);
            let mut btn_x = inner.x;
            for (i, label) in self.buttons.iter().enumerate() {
                let text = format!("[ {} ]", label);
                let style = if i == self.selected_button {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::DarkGray)
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
            Key::Left | Key::Char('h') => {
                self.select_prev_button();
                Some(super::FocusAction::Consumed)
            }
            Key::Right | Key::Char('l') => {
                self.select_next_button();
                Some(super::FocusAction::Consumed)
            }
            Key::Tab => {
                self.select_next_button();
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
}
