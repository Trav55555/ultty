//! Container widgets.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::{render_view_to_native, View};
use crate::theme::to_ratatui_color;
use ultty_protocol::{
    AppMessage, BorderStyle, BorderType, Color, Constraints, Dimension, Display, Edges,
    FlexDirection, GridLine, GridPlacement, RegionId, Style as ProtoStyle, TrackSizingFunction,
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

/// The children an override does not hide (ADR 0023). The native layout
/// leaves hidden regions out (`display: none`); the ANSI layout of `Column`
/// and `Row` does the same with this.
fn visible_children(children: &[Box<dyn View>]) -> Vec<&dyn View> {
    children
        .iter()
        .map(|child| child.as_ref())
        .filter(|child| !crate::overrides::is_hidden(*child))
        .collect()
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
    id: Option<String>,
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
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new() -> Self {
        Self {
            id: None,
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

    pub fn justify_start(mut self) -> Self {
        self.constraints.justify_content = ultty_protocol::JustifyContent::Start;
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

    /// Free space split evenly around each child: half a share before the
    /// first and after the last, a whole share between two.
    pub fn justify_around(mut self) -> Self {
        self.constraints.justify_content = ultty_protocol::JustifyContent::SpaceAround;
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

    pub fn align_content_start(mut self) -> Self {
        self.constraints.align_content = ultty_protocol::AlignContent::Start;
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
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

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
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        if let Some(child) = self.children.get(index) {
            Ok(Some(render_view_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
                None,
            )?))
        } else {
            Ok(None)
        }
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        for child in &self.children {
            visit(child.as_ref());
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if self.children.is_empty() {
            render_style_block(&self.style, frame, area);
            return;
        }

        let bordered = render_style_block(&self.style, frame, area);
        let children = visible_children(&self.children);

        let pad = extract_cells(&self.constraints.padding);
        let inner = Rect::new(
            bordered.x + pad.3,
            bordered.y + pad.0,
            bordered.width.saturating_sub(pad.1 + pad.3),
            bordered.height.saturating_sub(pad.0 + pad.2),
        );

        // Reserve space for gaps between children
        let total_gap = self.gap * (children.len().saturating_sub(1)) as u16;
        let available = inner.height.saturating_sub(total_gap);

        let constraints: Vec<Constraint> = children
            .iter()
            .map(|child| {
                let c = crate::overrides::constraints_of(*child);
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
        for (i, (child, chunk)) in children.iter().zip(chunks.iter()).enumerate() {
            let shifted = Rect::new(inner.x, inner.y + y_offset, chunk.width, chunk.height);
            super::render_view_ansi(*child, frame, shifted);
            y_offset += chunk.height;
            if i < children.len() - 1 {
                y_offset += self.gap;
            }
        }
    }
}

/// Horizontal flex container.
pub struct Row {
    id: Option<String>,
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
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new() -> Self {
        Self {
            id: None,
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
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

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
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        if let Some(child) = self.children.get(index) {
            Ok(Some(render_view_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
                None,
            )?))
        } else {
            Ok(None)
        }
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        for child in &self.children {
            visit(child.as_ref());
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if self.children.is_empty() {
            render_style_block(&self.style, frame, area);
            return;
        }

        let bordered = render_style_block(&self.style, frame, area);
        let children = visible_children(&self.children);

        let pad = extract_cells(&self.constraints.padding);
        let inner = Rect::new(
            bordered.x + pad.3,
            bordered.y + pad.0,
            bordered.width.saturating_sub(pad.1 + pad.3),
            bordered.height.saturating_sub(pad.0 + pad.2),
        );

        let total_gap = self.gap * (children.len().saturating_sub(1)) as u16;
        let available = inner.width.saturating_sub(total_gap);

        let constraints: Vec<Constraint> = children
            .iter()
            .map(|child| {
                let c = crate::overrides::constraints_of(*child);
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
        for (i, (child, chunk)) in children.iter().zip(chunks.iter()).enumerate() {
            let shifted = Rect::new(inner.x + x_offset, inner.y, chunk.width, chunk.height);
            super::render_view_ansi(*child, frame, shifted);
            x_offset += chunk.width;
            if i < children.len() - 1 {
                x_offset += self.gap;
            }
        }
    }
}

/// Z-ordered stack container.
pub struct Stack {
    overlay_size: Option<(u16, u16)>,
    id: Option<String>,
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
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new() -> Self {
        Self {
            overlay_size: None,
            id: None,
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

    /// Overlay a bottom-centred opaque dialog over the existing child.
    pub fn overlay<V: View + 'static>(mut self, child: V, width: u16, height: u16) -> Self {
        self.constraints.display = Display::Grid;
        self.constraints.grid_template_columns =
            vec![TrackSizingFunction::Single(Dimension::Percent(100.0))];
        self.constraints.grid_template_rows =
            vec![TrackSizingFunction::Single(Dimension::Percent(100.0))];
        self.overlay_size = Some((width, height));
        self.children.push(Box::new(child));
        self
    }

    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}

impl View for Stack {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn constraints(&self) -> Constraints {
        self.constraints.clone()
    }

    fn children_count(&self) -> usize {
        self.children.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        if let Some(child) = self.children.get(index) {
            let overrides = self.overlay_size.map(|(width, height)| {
                let mut c = child.constraints();
                c.grid_column = GridPlacement {
                    start: GridLine::Line(1),
                    end: GridLine::Line(2),
                };
                c.grid_row = GridPlacement {
                    start: GridLine::Line(1),
                    end: GridLine::Line(2),
                };
                if index + 1 == self.children.len() {
                    c.width = Dimension::Cells(f32::from(width));
                    c.height = Dimension::Cells(f32::from(height));
                    c.margin.top = Dimension::Auto;
                    c.margin.left = Dimension::Auto;
                    c.margin.right = Dimension::Auto;
                }
                c
            });
            let id =
                render_view_to_native(child.as_ref(), send_message, parent, next_id, overrides)?;
            if self.overlay_size.is_some() && index + 1 == self.children.len() {
                super::send_semantics(
                    send_message,
                    id,
                    ultty_protocol::Semantics {
                        role: ultty_protocol::Role::Dialog,
                        modal: true,
                        ..Default::default()
                    },
                    &ultty_protocol::Content::None,
                )?;
            }
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        for child in &self.children {
            visit(child.as_ref());
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        for (index, child) in self.children.iter().enumerate() {
            let target = if let Some((width, height)) = self
                .overlay_size
                .filter(|_| index + 1 == self.children.len())
            {
                let width = width.min(area.width);
                let height = height.min(area.height);
                let rect = Rect::new(
                    area.x + (area.width - width) / 2,
                    area.y + area.height - height,
                    width,
                    height,
                );
                frame.render_widget(ratatui::widgets::Clear, rect);
                rect
            } else {
                area
            };
            super::render_view_ansi(child.as_ref(), frame, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::Text;

    #[test]
    fn stack_overlay_uses_same_grid_cell_and_modal_semantics() {
        let stack = Stack::new()
            .child(Text::new("underlay"))
            .overlay(Text::new("dialog"), 20, 4);
        let messages = super::super::render_view_messages(&stack).unwrap();
        let regions: Vec<_> = messages
            .iter()
            .filter_map(|message| match message {
                AppMessage::CreateRegion { constraints, .. } => Some(constraints),
                _ => None,
            })
            .collect();
        assert_eq!(regions[0].display, Display::Grid);
        assert_eq!(regions[1].grid_column.start, GridLine::Line(1));
        assert_eq!(regions[2].grid_column.start, GridLine::Line(1));
        assert_eq!(regions[2].grid_row.start, GridLine::Line(1));
        assert_eq!(regions[2].margin.top, Dimension::Auto);
        assert!(messages.iter().any(|message| matches!(message,
            AppMessage::SetSemantics { semantics, .. } if semantics.modal && semantics.role == ultty_protocol::Role::Dialog)));
    }

    #[test]
    fn stack_overlay_clears_only_dialog_rectangle_in_ansi() {
        let underlay = Column::new()
            .child(Text::new("top row"))
            .child(Text::new("abcdefghijklmnopqrst"));
        let stack = Stack::new().child(underlay).overlay(Text::new("box"), 6, 1);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| stack.render_ansi(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = (0..20).map(|x| buffer[(x, 1)].symbol()).collect::<String>();
        assert_eq!(row, "abcdefgbox   nopqrst");
    }

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
    fn row_padding_insets_children_in_both_modes() {
        let row = Row::new().padding(1.0).child(Text::new("ab"));
        let c = <Row as View>::constraints(&row);
        assert_eq!(c.padding.left, Dimension::Cells(1.0));
        assert_eq!(c.padding.bottom, Dimension::Cells(1.0));

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 3)).unwrap();
        terminal
            .draw(|frame| row.render_ansi(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 1)].symbol(), " ");
        assert_eq!(buffer[(1, 1)].symbol(), "a");
        assert_eq!(buffer[(2, 1)].symbol(), "b");
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
