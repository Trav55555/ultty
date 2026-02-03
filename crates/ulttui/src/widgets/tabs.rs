//! Tabs widget for switching between views.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use ultty_protocol::{
    AppMessage, Color as ProtoColor, Constraints, Dimension, Display, FlexDirection, NamedColor,
    RegionId, Style as ProtoStyle, TextContent,
};

/// A tab bar widget.
#[derive(Default)]
pub struct Tabs {
    labels: Vec<String>,
    active: usize,
}

impl std::fmt::Debug for Tabs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tabs")
            .field("labels", &self.labels)
            .field("active", &self.active)
            .finish()
    }
}

impl Tabs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn labels(mut self, labels: Vec<impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    pub fn active(mut self, index: usize) -> Self {
        self.active = index;
        self
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_label(&self) -> Option<&str> {
        self.labels.get(self.active).map(|s| s.as_str())
    }

    pub fn tab_count(&self) -> usize {
        self.labels.len()
    }

    pub fn select_next(&mut self) {
        if !self.labels.is_empty() {
            self.active = (self.active + 1) % self.labels.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.labels.is_empty() {
            if self.active == 0 {
                self.active = self.labels.len() - 1;
            } else {
                self.active -= 1;
            }
        }
    }

    pub fn select(&mut self, index: usize) {
        if index < self.labels.len() {
            self.active = index;
        }
    }
}

impl View for Tabs {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            gap: ultty_protocol::Size {
                width: Dimension::Cells(1.0),
                height: Dimension::Auto,
            },
            ..Default::default()
        }
    }

    fn children_count(&self) -> usize {
        self.labels.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let label = match self.labels.get(index) {
            Some(l) => l,
            None => return Ok(None),
        };

        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        let is_active = index == self.active;
        let style = if is_active {
            ProtoStyle::new()
                .fg(ProtoColor::Named(NamedColor::Black))
                .bg(ProtoColor::Named(NamedColor::White))
                .bold()
        } else {
            ProtoStyle::new().fg(ProtoColor::Named(NamedColor::White))
        };

        send_message(&AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints: Constraints::default(),
            style,
        })?;

        let display_text = format!(" {} ", label);

        send_message(&AppMessage::SetContent {
            id: region_id,
            content: ultty_protocol::Content::Text(TextContent::new(display_text)),
        })?;

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let spans: Vec<Span> = self
            .labels
            .iter()
            .enumerate()
            .flat_map(|(i, label)| {
                let is_active = i == self.active;
                let style = if is_active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let mut spans = vec![Span::styled(format!(" {} ", label), style)];
                if i < self.labels.len() - 1 {
                    spans.push(Span::raw(" "));
                }
                spans
            })
            .collect();

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<super::FocusAction> {
        use crate::event::Key;
        match key.key {
            Key::Left | Key::Char('h') => {
                self.select_prev();
                Some(super::FocusAction::Consumed)
            }
            Key::Right | Key::Char('l') => {
                self.select_next();
                Some(super::FocusAction::Consumed)
            }
            Key::Char(c) if c.is_ascii_digit() => {
                let idx = c as usize - '0' as usize;
                if idx > 0 && idx <= self.labels.len() {
                    self.select(idx - 1);
                }
                Some(super::FocusAction::Consumed)
            }
            Key::Tab if key.modifiers.shift => Some(super::FocusAction::Previous),
            Key::Tab => Some(super::FocusAction::Next),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_navigation() {
        let mut tabs = Tabs::new().labels(vec!["One", "Two", "Three"]);
        assert_eq!(tabs.active_index(), 0);

        tabs.select_next();
        assert_eq!(tabs.active_index(), 1);

        tabs.select_next();
        assert_eq!(tabs.active_index(), 2);

        tabs.select_next();
        assert_eq!(tabs.active_index(), 0); // wraps

        tabs.select_prev();
        assert_eq!(tabs.active_index(), 2); // wraps back
    }

    #[test]
    fn tabs_select() {
        let mut tabs = Tabs::new().labels(vec!["A", "B", "C"]);
        tabs.select(2);
        assert_eq!(tabs.active_index(), 2);
        assert_eq!(tabs.active_label(), Some("C"));

        tabs.select(99); // out of bounds, no change
        assert_eq!(tabs.active_index(), 2);
    }

    #[test]
    fn tabs_empty() {
        let mut tabs = Tabs::new();
        tabs.select_next(); // should not panic
        tabs.select_prev();
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.active_label(), None);
    }
}
