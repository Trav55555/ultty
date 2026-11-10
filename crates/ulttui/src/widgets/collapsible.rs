use ratatui::layout::Rect;
use ratatui::style::Style as RStyle;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Constraints, Dimension, Display, FlexDirection, RegionId, Style as ProtoStyle,
    TextContent,
};

pub struct Collapsible {
    header: Box<dyn View>,
    body: Box<dyn View>,
    expanded: bool,
    id: Option<String>,
}

impl Collapsible {
    pub fn new(header: impl View + 'static, body: impl View + 'static) -> Self {
        Self {
            header: Box::new(header),
            body: Box::new(body),
            expanded: false,
            id: None,
        }
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl std::fmt::Debug for Collapsible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collapsible")
            .field("expanded", &self.expanded)
            .field("id", &self.id)
            .finish()
    }
}

impl View for Collapsible {
    /// A tree item, expanded or collapsed, holding its header and body.
    fn semantics(&self) -> ultty_protocol::Semantics {
        ultty_protocol::Semantics {
            role: ultty_protocol::Role::TreeItem,
            expanded: if self.expanded {
                ultty_protocol::Expanded::Expanded
            } else {
                ultty_protocol::Expanded::Collapsed
            },
            ..Default::default()
        }
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        ProtoStyle::default()
    }

    fn children_count(&self) -> usize {
        if self.expanded {
            2
        } else {
            1
        }
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        match index {
            0 => {
                send_message(&AppMessage::CreateRegion {
                    id: region_id,
                    parent,
                    insert_index: -1,
                    constraints: Constraints {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        width: Dimension::Percent(100.0),
                        ..Default::default()
                    },
                    style: ProtoStyle::default(),
                })?;

                let chevron_id = RegionId::new(*next_id);
                *next_id += 1;
                let chevron = if self.expanded { "▾ " } else { "▸ " };
                send_message(&AppMessage::CreateRegion {
                    id: chevron_id,
                    parent: region_id,
                    insert_index: -1,
                    constraints: Constraints::default(),
                    style: ProtoStyle::default(),
                })?;
                let content = ultty_protocol::Content::Text(TextContent::new(chevron));
                // The chevron is what `expanded` already says.
                let decoration = ultty_protocol::Semantics {
                    hidden: true,
                    ..Default::default()
                };
                super::send_semantics(send_message, chevron_id, decoration, &content)?;
                send_message(&AppMessage::SetContent {
                    id: chevron_id,
                    content,
                })?;

                for i in 0..self.header.children_count() {
                    self.header
                        .render_child_native(i, send_message, region_id, next_id)?;
                }
            }
            1 if self.expanded => {
                send_message(&AppMessage::CreateRegion {
                    id: region_id,
                    parent,
                    insert_index: -1,
                    constraints: self.body.constraints(),
                    style: self.body.style(),
                })?;
                for i in 0..self.body.children_count() {
                    self.body
                        .render_child_native(i, send_message, region_id, next_id)?;
                }
            }
            _ => return Ok(None),
        }

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let theme = Theme::current();
        let chevron = if self.expanded { "▾ " } else { "▸ " };

        let chevron_area = Rect::new(area.x, area.y, 2.min(area.width), 1);
        frame.render_widget(
            Paragraph::new(Line::styled(
                chevron,
                RStyle::default().fg(to_ratatui_color(theme.muted)),
            )),
            chevron_area,
        );

        if area.width > 2 {
            let header_area = Rect::new(area.x + 2, area.y, area.width - 2, 1);
            self.header.render_ansi(frame, header_area);
        }

        if self.expanded && area.height > 1 {
            let body_area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
            self.body.render_ansi(frame, body_area);
        }
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        visit(self.header.as_ref());
        if self.expanded {
            visit(self.body.as_ref());
        }
    }

    fn is_focusable(&self) -> bool {
        self.id.is_some()
    }

    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::Text;

    #[test]
    fn collapsible_default_collapsed() {
        let c = Collapsible::new(Text::new("Header"), Text::new("Body"));
        assert!(!c.expanded);
    }

    #[test]
    fn collapsible_expanded() {
        let c = Collapsible::new(Text::new("H"), Text::new("B")).expanded(true);
        assert!(c.expanded);
    }

    #[test]
    fn collapsible_children_count_collapsed() {
        let c = Collapsible::new(Text::new("H"), Text::new("B"));
        assert_eq!(<Collapsible as View>::children_count(&c), 1);
    }

    #[test]
    fn collapsible_children_count_expanded() {
        let c = Collapsible::new(Text::new("H"), Text::new("B")).expanded(true);
        assert_eq!(<Collapsible as View>::children_count(&c), 2);
    }

    #[test]
    fn collapsible_with_id() {
        let c = Collapsible::new(Text::new("H"), Text::new("B")).id("my-section");
        assert!(c.is_focusable());
        assert_eq!(c.widget_id(), Some("my-section"));
    }

    #[test]
    fn collapsible_debug() {
        let c = Collapsible::new(Text::new("H"), Text::new("B")).expanded(true);
        let debug = format!("{:?}", c);
        assert!(debug.contains("Collapsible"));
        assert!(debug.contains("expanded"));
    }

    #[test]
    fn collapsible_constraints_are_column() {
        let c = Collapsible::new(Text::new("H"), Text::new("B"));
        let constraints = <Collapsible as View>::constraints(&c);
        assert_eq!(constraints.flex_direction, FlexDirection::Column);
    }
}
