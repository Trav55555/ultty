//! Container widgets.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::{render_view_to_native, View};
use crate::theme::to_ratatui_color;
use ultty_protocol::{
    AppMessage, BorderStyle, BorderType, Color, Constraints, Dimension, Display, Edges,
    FlexDirection, RegionId, Style as ProtoStyle,
};

/// Extract padding values as (top, right, bottom, left) in cell units.
fn extract_cells(edges: &Edges<Dimension>) -> (u16, u16, u16, u16) {
    let to_u16 = |d: &Dimension| match d {
        Dimension::Cells(n) => *n as u16,
        _ => 0,
    };
    (
        to_u16(&edges.top),
        to_u16(&edges.right),
        to_u16(&edges.bottom),
        to_u16(&edges.left),
    )
}

fn map_border_type(bt: BorderType) -> ratatui::widgets::BorderType {
    match bt {
        BorderType::Thin => ratatui::widgets::BorderType::Plain,
        BorderType::Thick => ratatui::widgets::BorderType::Thick,
        BorderType::Double => ratatui::widgets::BorderType::Double,
        BorderType::Rounded => ratatui::widgets::BorderType::Rounded,
        _ => ratatui::widgets::BorderType::Plain,
    }
}

fn render_style_block(style: &ProtoStyle, frame: &mut Frame, area: Rect) -> Rect {
    if style.bg.is_none() && style.border.style == BorderType::None {
        return area;
    }

    let mut block = ratatui::widgets::Block::default();
    if let Some(bg) = style.bg {
        block = block.style(ratatui::style::Style::default().bg(to_ratatui_color(bg)));
    }

    if style.border.style != BorderType::None {
        let mut borders = ratatui::widgets::Borders::NONE;
        let edges = &style.border.edges;
        if edges.top {
            borders |= ratatui::widgets::Borders::TOP;
        }
        if edges.right {
            borders |= ratatui::widgets::Borders::RIGHT;
        }
        if edges.bottom {
            borders |= ratatui::widgets::Borders::BOTTOM;
        }
        if edges.left {
            borders |= ratatui::widgets::Borders::LEFT;
        }
        block = block
            .borders(borders)
            .border_type(map_border_type(style.border.style));
        if let Some(bc) = style.border_color {
            block = block.border_style(ratatui::style::Style::default().fg(to_ratatui_color(bc)));
        }
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// Vertical flex container.
pub struct Column {
    constraints: Constraints,
    style: ProtoStyle,
    children: Vec<Box<dyn View>>,
    gap: u16,
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Column")
            .field("constraints", &self.constraints)
            .field("style", &self.style)
            .field("children_count", &self.children.len())
            .field("gap", &self.gap)
            .finish()
    }
}

impl Column {
    pub fn new() -> Self {
        Self {
            constraints: Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                width: Dimension::Percent(100.0),
                ..Default::default()
            },
            style: ProtoStyle::default(),
            children: Vec::new(),
            gap: 0,
        }
    }

    pub fn child<V: View + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.constraints.gap.height = Dimension::Cells(gap);
        self.gap = gap as u16;
        self
    }

    pub fn padding(mut self, padding: f32) -> Self {
        self.constraints = self.constraints.padding_all(Dimension::Cells(padding));
        self
    }

    pub fn justify_center(mut self) -> Self {
        self.constraints.justify_content = ultty_protocol::JustifyContent::Center;
        self
    }

    pub fn justify_between(mut self) -> Self {
        self.constraints.justify_content = ultty_protocol::JustifyContent::SpaceBetween;
        self
    }

    pub fn align_center(mut self) -> Self {
        self.constraints.align_items = ultty_protocol::AlignItems::Center;
        self
    }

    pub fn align_stretch(mut self) -> Self {
        self.constraints.align_items = ultty_protocol::AlignItems::Stretch;
        self
    }

    pub fn width(mut self, w: Dimension) -> Self {
        self.constraints.width = w;
        self
    }

    pub fn height(mut self, h: Dimension) -> Self {
        self.constraints.height = h;
        self
    }

    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.constraints.flex_grow = grow;
        self
    }

    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.constraints.flex_shrink = shrink;
        self
    }

    pub fn overflow_y_hidden(mut self) -> Self {
        self.constraints.overflow.y = ultty_protocol::OverflowAxis::Hidden;
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.style.bg = Some(color);
        self
    }

    pub fn border(mut self, border: BorderStyle) -> Self {
        self.style.border = border;
        self
    }

    pub fn border_color(mut self, color: Color) -> Self {
        self.style.border_color = Some(color);
        self
    }

    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}

impl View for Column {
    fn constraints(&self) -> Constraints {
        self.constraints.clone()
    }

    fn style(&self) -> ProtoStyle {
        self.style.clone()
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
                true,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if self.children.is_empty() {
            render_style_block(&self.style, frame, area);
            return;
        }

        let bordered = render_style_block(&self.style, frame, area);

        let pad = extract_cells(&self.constraints.padding);
        let inner = Rect::new(
            bordered.x + pad.3,
            bordered.y + pad.0,
            bordered.width.saturating_sub(pad.1 + pad.3),
            bordered.height.saturating_sub(pad.0 + pad.2),
        );

        // Reserve space for gaps between children
        let total_gap = self.gap * (self.children.len().saturating_sub(1)) as u16;
        let available = inner.height.saturating_sub(total_gap);

        let constraints: Vec<Constraint> = self
            .children
            .iter()
            .map(|child| {
                let c = child.constraints();
                match c.height {
                    Dimension::Cells(n) => Constraint::Length(n as u16),
                    Dimension::Percent(p) => Constraint::Percentage(p as u16),
                    _ => {
                        if c.flex_grow > 0.0 {
                            Constraint::Fill(1)
                        } else {
                            Constraint::Min(1)
                        }
                    }
                }
            })
            .collect();

        let no_gap_area = Rect::new(inner.x, inner.y, inner.width, available);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .constraints(constraints)
            .split(no_gap_area);

        let mut y_offset = 0u16;
        for (i, (child, chunk)) in self.children.iter().zip(chunks.iter()).enumerate() {
            let shifted = Rect::new(inner.x, inner.y + y_offset, chunk.width, chunk.height);
            child.render_ansi(frame, shifted);
            y_offset += chunk.height;
            if i < self.children.len() - 1 {
                y_offset += self.gap;
            }
        }
    }
}

/// Horizontal flex container.
pub struct Row {
    constraints: Constraints,
    style: ProtoStyle,
    children: Vec<Box<dyn View>>,
    gap: u16,
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("constraints", &self.constraints)
            .field("style", &self.style)
            .field("children_count", &self.children.len())
            .field("gap", &self.gap)
            .finish()
    }
}

impl Row {
    pub fn new() -> Self {
        Self {
            constraints: Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                width: Dimension::Percent(100.0),
                ..Default::default()
            },
            style: ProtoStyle::default(),
            children: Vec::new(),
            gap: 0,
        }
    }

    pub fn child<V: View + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.constraints.gap.width = Dimension::Cells(gap);
        self.gap = gap as u16;
        self
    }

    pub fn justify_center(mut self) -> Self {
        self.constraints.justify_content = ultty_protocol::JustifyContent::Center;
        self
    }

    pub fn justify_between(mut self) -> Self {
        self.constraints.justify_content = ultty_protocol::JustifyContent::SpaceBetween;
        self
    }

    pub fn align_center(mut self) -> Self {
        self.constraints.align_items = ultty_protocol::AlignItems::Center;
        self
    }

    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.constraints.flex_grow = grow;
        self
    }

    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.constraints.flex_shrink = shrink;
        self
    }

    pub fn height(mut self, h: Dimension) -> Self {
        self.constraints.height = h;
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.style.bg = Some(color);
        self
    }

    pub fn border(mut self, border: BorderStyle) -> Self {
        self.style.border = border;
        self
    }

    pub fn border_color(mut self, color: Color) -> Self {
        self.style.border_color = Some(color);
        self
    }

    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}

impl View for Row {
    fn constraints(&self) -> Constraints {
        self.constraints.clone()
    }

    fn style(&self) -> ProtoStyle {
        self.style.clone()
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
                true,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if self.children.is_empty() {
            render_style_block(&self.style, frame, area);
            return;
        }

        let bordered = render_style_block(&self.style, frame, area);

        let pad = extract_cells(&self.constraints.padding);
        let inner = Rect::new(
            bordered.x + pad.3,
            bordered.y + pad.0,
            bordered.width.saturating_sub(pad.1 + pad.3),
            bordered.height.saturating_sub(pad.0 + pad.2),
        );

        let total_gap = self.gap * (self.children.len().saturating_sub(1)) as u16;
        let available = inner.width.saturating_sub(total_gap);

        let constraints: Vec<Constraint> = self
            .children
            .iter()
            .map(|child| {
                let c = child.constraints();
                match c.width {
                    Dimension::Cells(n) => Constraint::Length(n as u16),
                    Dimension::Percent(p) => Constraint::Percentage(p as u16),
                    _ => {
                        if c.flex_grow > 0.0 {
                            Constraint::Fill(1)
                        } else {
                            Constraint::Min(1)
                        }
                    }
                }
            })
            .collect();

        let no_gap_area = Rect::new(inner.x, inner.y, available, inner.height);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .margin(0)
            .constraints(constraints)
            .split(no_gap_area);

        let mut x_offset = 0u16;
        for (i, (child, chunk)) in self.children.iter().zip(chunks.iter()).enumerate() {
            let shifted = Rect::new(inner.x + x_offset, inner.y, chunk.width, chunk.height);
            child.render_ansi(frame, shifted);
            x_offset += chunk.width;
            if i < self.children.len() - 1 {
                x_offset += self.gap;
            }
        }
    }
}

/// Z-ordered stack container.
pub struct Stack {
    constraints: Constraints,
    children: Vec<Box<dyn View>>,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("constraints", &self.constraints)
            .field("children_count", &self.children.len())
            .finish()
    }
}

impl Stack {
    pub fn new() -> Self {
        Self {
            constraints: Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                width: Dimension::Percent(100.0),
                height: Dimension::Percent(100.0),
                ..Default::default()
            },
            children: Vec::new(),
        }
    }

    pub fn child<V: View + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}

impl View for Stack {
    fn constraints(&self) -> Constraints {
        self.constraints.clone()
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
                true,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        for child in &self.children {
            child.render_ansi(frame, area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::Text;

    #[test]
    fn column_children_count() {
        let col = Column::new()
            .child(Text::new("a"))
            .child(Text::new("b"))
            .child(Text::new("c"));
        assert_eq!(<Column as View>::children_count(&col), 3);
    }

    #[test]
    fn column_constraints() {
        let col = Column::new().gap(2.0).padding(1.0);
        let c = <Column as View>::constraints(&col);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Column);
        assert_eq!(c.gap.height, Dimension::Cells(2.0));
        assert_eq!(c.padding.top, Dimension::Cells(1.0));
    }

    #[test]
    fn column_bg_sets_style() {
        let col = Column::new().bg(ultty_protocol::Color::Rgb(30, 30, 36));
        let style = <Column as View>::style(&col);
        assert_eq!(style.bg, Some(ultty_protocol::Color::Rgb(30, 30, 36)));
    }

    #[test]
    fn column_border_sets_style() {
        let col = Column::new().border(ultty_protocol::BorderStyle::thin());
        let style = <Column as View>::style(&col);
        assert_eq!(style.border, ultty_protocol::BorderStyle::thin());
    }

    #[test]
    fn column_border_color_sets_style() {
        let col = Column::new().border_color(ultty_protocol::Color::Rgb(58, 224, 221));
        let style = <Column as View>::style(&col);
        assert_eq!(
            style.border_color,
            Some(ultty_protocol::Color::Rgb(58, 224, 221))
        );
    }

    #[test]
    fn column_default_style_is_empty() {
        let col = Column::new();
        let style = <Column as View>::style(&col);
        assert_eq!(style, ultty_protocol::Style::default());
    }

    #[test]
    fn row_children_count() {
        let row = Row::new().child(Text::new("x")).child(Text::new("y"));
        assert_eq!(<Row as View>::children_count(&row), 2);
    }

    #[test]
    fn row_constraints() {
        let row = Row::new().gap(3.0);
        let c = <Row as View>::constraints(&row);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Row);
        assert_eq!(c.gap.width, Dimension::Cells(3.0));
    }

    #[test]
    fn row_bg_sets_style() {
        let row = Row::new().bg(ultty_protocol::Color::Rgb(255, 93, 110));
        let style = <Row as View>::style(&row);
        assert_eq!(style.bg, Some(ultty_protocol::Color::Rgb(255, 93, 110)));
    }

    #[test]
    fn row_height_sets_constraint() {
        let row = Row::new().height(Dimension::Cells(1.0));
        let c = <Row as View>::constraints(&row);
        assert_eq!(c.height, Dimension::Cells(1.0));
    }

    #[test]
    fn row_default_style_is_empty() {
        let row = Row::new();
        let style = <Row as View>::style(&row);
        assert_eq!(style, ultty_protocol::Style::default());
    }

    #[test]
    fn stack_children_count() {
        let stack = Stack::new().child(Text::new("a"));
        assert_eq!(<Stack as View>::children_count(&stack), 1);
    }

    #[test]
    fn column_empty() {
        let col = Column::new();
        assert_eq!(<Column as View>::children_count(&col), 0);
    }

    #[test]
    fn extract_cells_works() {
        let edges = Edges::all(Dimension::Cells(2.0));
        let (t, r, b, l) = extract_cells(&edges);
        assert_eq!((t, r, b, l), (2, 2, 2, 2));
    }

    #[test]
    fn extract_cells_auto() {
        let edges = Edges::all(Dimension::Auto);
        let (t, r, b, l) = extract_cells(&edges);
        assert_eq!((t, r, b, l), (0, 0, 0, 0));
    }
}
