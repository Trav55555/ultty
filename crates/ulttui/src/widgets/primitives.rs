//! Layout primitives that use theme design tokens.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::{render_view_to_native, View};
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Constraints, Content, Dimension, Display, FlexDirection, RegionId,
    Style as ProtoStyle,
};

use super::typography::Title;

/// Uniform padding wrapper. Wraps a child with padding from theme tokens.
pub struct Inset<V: View> {
    child: V,
    padding: f32,
}

impl<V: View> Inset<V> {
    pub fn new(child: V, padding: f32) -> Self {
        Self { child, padding }
    }
}

impl<V: View> View for Inset<V> {
    fn constraints(&self) -> Constraints {
        let mut c = self.child.constraints();
        c.padding = ultty_protocol::Edges::all(Dimension::Cells(self.padding));
        c
    }

    fn style(&self) -> ProtoStyle {
        self.child.style()
    }

    fn content(&self) -> Content {
        self.child.content()
    }

    fn children_count(&self) -> usize {
        self.child.children_count()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        self.child
            .render_child_native(index, send_message, parent, next_id)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let p = self.padding as u16;
        let inner = Rect::new(
            area.x + p,
            area.y + p,
            area.width.saturating_sub(p * 2),
            area.height.saturating_sub(p * 2),
        );
        self.child.render_ansi(frame, inner);
    }

    fn widget_id(&self) -> Option<&str> {
        self.child.widget_id()
    }

    fn is_focusable(&self) -> bool {
        self.child.is_focusable()
    }
}

/// Flexible empty space that grows to fill available room.
pub struct Spacer;

impl View for Spacer {
    fn constraints(&self) -> Constraints {
        Constraints {
            flex_grow: 1.0,
            ..Default::default()
        }
    }
}

/// Grouped content section with optional title and surface background.
pub struct Section {
    title: Option<String>,
    children: Vec<Box<dyn View>>,
}

impl Section {
    pub fn new() -> Self {
        Self {
            title: None,
            children: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn child<V: View + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }
}

impl Default for Section {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Section {
    fn constraints(&self) -> Constraints {
        let theme = Theme::current();
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            width: Dimension::Percent(100.0),
            gap: ultty_protocol::Size {
                width: Dimension::Auto,
                height: Dimension::Cells(theme.tokens.space_sm),
            },
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new().bg(theme.tokens.surface_0)
    }

    fn children_count(&self) -> usize {
        let title_count = if self.title.is_some() { 1 } else { 0 };
        title_count + self.children.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        if let Some(title_text) = &self.title {
            if index == 0 {
                let title = Title::new(title_text.clone());
                return Ok(Some(render_view_to_native(
                    &title,
                    send_message,
                    parent,
                    next_id,
                    None,
                    false,
                )?));
            }
            // Title occupies index 0; children start at index 1
            let child_index = index - 1;
            if let Some(child) = self.children.get(child_index) {
                Ok(Some(render_view_to_native(
                    child.as_ref(),
                    send_message,
                    parent,
                    next_id,
                    None,
                    false,
                )?))
            } else {
                Ok(None)
            }
        } else if let Some(child) = self.children.get(index) {
            Ok(Some(render_view_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
                None,
                false,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let bg = ratatui::widgets::Block::default()
            .style(ratatui::style::Style::default().bg(to_ratatui_color(theme.tokens.surface_0)));
        frame.render_widget(bg, area);

        let mut constraints = Vec::new();
        if self.title.is_some() {
            constraints.push(Constraint::Length(1));
        }
        for _ in &self.children {
            constraints.push(Constraint::Min(1));
        }

        if constraints.is_empty() {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        let mut chunk_idx = 0;
        if let Some(title_text) = &self.title {
            let title = Title::new(title_text.clone());
            title.render_ansi(frame, chunks[chunk_idx]);
            chunk_idx += 1;
        }
        for child in &self.children {
            if chunk_idx < chunks.len() {
                child.render_ansi(frame, chunks[chunk_idx]);
                chunk_idx += 1;
            }
        }
    }
}

/// Elevated card with `surface_1` background and subtle border.
pub struct Card {
    children: Vec<Box<dyn View>>,
    max_width: Option<f32>,
    align_center: bool,
}

impl Card {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            max_width: None,
            align_center: false,
        }
    }

    pub fn child<V: View + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    pub fn max_width(mut self, cells: f32) -> Self {
        self.max_width = Some(cells);
        self
    }

    pub fn center(mut self) -> Self {
        self.align_center = true;
        self
    }
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Card {
    fn constraints(&self) -> Constraints {
        let theme = Theme::current();
        let mut c = Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            padding: ultty_protocol::Edges::all(Dimension::Cells(theme.tokens.space_md)),
            gap: ultty_protocol::Size {
                width: Dimension::Auto,
                height: Dimension::Cells(theme.tokens.space_sm),
            },
            ..Default::default()
        };
        if let Some(mw) = self.max_width {
            c.width = Dimension::Cells(mw);
            c.max_width = Dimension::Cells(mw);
            c.flex_grow = 0.0;
        } else {
            c.width = Dimension::Percent(100.0);
        }
        if self.align_center {
            c.align_self = ultty_protocol::AlignSelf::Center;
        }
        c
    }

    fn style(&self) -> ProtoStyle {
        let theme = Theme::current();
        ProtoStyle::new()
            .bg(theme.tokens.surface_1)
            .border(ultty_protocol::BorderStyle::thin())
    }

    fn children_count(&self) -> usize {
        self.children.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        if let Some(child) = self.children.get(index) {
            Ok(Some(render_view_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
                None,
                false,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(
                ratatui::style::Style::default().fg(to_ratatui_color(theme.tokens.border_subtle)),
            )
            .style(ratatui::style::Style::default().bg(to_ratatui_color(theme.tokens.surface_1)));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.children.is_empty() {
            return;
        }

        let constraints: Vec<Constraint> =
            self.children.iter().map(|_| Constraint::Min(1)).collect();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        for (child, chunk) in self.children.iter().zip(chunks.iter()) {
            child.render_ansi(frame, *chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::Text;

    #[test]
    fn spacer_has_flex_grow() {
        let s = Spacer;
        assert_eq!(s.constraints().flex_grow, 1.0);
    }

    #[test]
    fn inset_adds_padding() {
        let inner = Text::new("hello");
        let inset = Inset::new(inner, 2.0);
        let c = inset.constraints();
        assert_eq!(c.padding.top, Dimension::Cells(2.0));
        assert_eq!(c.padding.right, Dimension::Cells(2.0));
        assert_eq!(c.padding.bottom, Dimension::Cells(2.0));
        assert_eq!(c.padding.left, Dimension::Cells(2.0));
    }

    #[test]
    fn section_children_count_without_title() {
        let s = Section::new().child(Text::new("a")).child(Text::new("b"));
        assert_eq!(s.children_count(), 2);
    }

    #[test]
    fn section_children_count_with_title() {
        let s = Section::new()
            .title("Header")
            .child(Text::new("a"))
            .child(Text::new("b"));
        assert_eq!(s.children_count(), 3);
    }

    #[test]
    fn card_has_surface_1_bg() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let c = Card::new();
        let style = c.style();
        assert_eq!(style.bg, Some(theme.tokens.surface_1));
    }

    #[test]
    fn card_children_count() {
        let c = Card::new()
            .child(Text::new("a"))
            .child(Text::new("b"))
            .child(Text::new("c"));
        assert_eq!(c.children_count(), 3);
    }
}
