//! Container widgets.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::View;
use ultty_protocol::{AppMessage, Constraints, Dimension, Display, FlexDirection, RegionId};

fn render_child_to_native(
    child: &dyn View,
    send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
    parent: RegionId,
    next_id: &mut u32,
) -> anyhow::Result<RegionId> {
    let region_id = RegionId::new(*next_id);
    *next_id += 1;

    send_message(&AppMessage::CreateRegion {
        id: region_id,
        parent,
        insert_index: -1,
        constraints: child.constraints(),
        style: child.style(),
    })?;

    let content = child.content();
    if content != ultty_protocol::Content::None {
        send_message(&AppMessage::SetContent {
            id: region_id,
            content,
        })?;
    }

    for i in 0..child.children_count() {
        child.render_child_native(i, send_message, region_id, next_id)?;
    }

    Ok(region_id)
}

/// Vertical flex container.
pub struct Column {
    constraints: Constraints,
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
                ..Default::default()
            },
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

    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}

impl View for Column {
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
            Ok(Some(render_child_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if self.children.is_empty() {
            return;
        }

        let constraints: Vec<Constraint> =
            self.children.iter().map(|_| Constraint::Min(1)).collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .constraints(constraints)
            .split(area);

        for (child, chunk) in self.children.iter().zip(chunks.iter()) {
            child.render_ansi(frame, *chunk);
        }
    }
}

/// Horizontal flex container.
pub struct Row {
    constraints: Constraints,
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
                ..Default::default()
            },
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

    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}

impl View for Row {
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
            Ok(Some(render_child_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
            )?))
        } else {
            Ok(None)
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if self.children.is_empty() {
            return;
        }

        let constraints: Vec<Constraint> = self
            .children
            .iter()
            .map(|_| Constraint::Ratio(1, self.children.len() as u32))
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .margin(0)
            .constraints(constraints)
            .split(area);

        for (child, chunk) in self.children.iter().zip(chunks.iter()) {
            child.render_ansi(frame, *chunk);
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
            constraints: Constraints::default(),
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
            Ok(Some(render_child_to_native(
                child.as_ref(),
                send_message,
                parent,
                next_id,
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
