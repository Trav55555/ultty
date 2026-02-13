use ratatui::layout::Rect;
use ratatui::Frame;

use super::View;
use ultty_protocol::{
    AlignItems, AppMessage, Constraints, Dimension, Display, FlexDirection, RegionId,
    Style as ProtoStyle,
};

/// Docked layout container.
///
/// Arranges children in a docked layout with optional top, bottom, left, right
/// regions surrounding a center content area.
pub struct DockLayout {
    top: Option<Box<dyn View>>,
    bottom: Option<Box<dyn View>>,
    left: Option<Box<dyn View>>,
    right: Option<Box<dyn View>>,
    center: Option<Box<dyn View>>,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl DockLayout {
    pub fn new() -> Self {
        Self {
            top: None,
            bottom: None,
            left: None,
            right: None,
            center: None,
        }
    }

    pub fn dock_top<V: View + 'static>(mut self, view: V) -> Self {
        self.top = Some(Box::new(view));
        self
    }

    pub fn dock_bottom<V: View + 'static>(mut self, view: V) -> Self {
        self.bottom = Some(Box::new(view));
        self
    }

    pub fn dock_left<V: View + 'static>(mut self, view: V) -> Self {
        self.left = Some(Box::new(view));
        self
    }

    pub fn dock_right<V: View + 'static>(mut self, view: V) -> Self {
        self.right = Some(Box::new(view));
        self
    }

    pub fn center<V: View + 'static>(mut self, view: V) -> Self {
        self.center = Some(Box::new(view));
        self
    }

    fn has_middle(&self) -> bool {
        self.left.is_some() || self.center.is_some() || self.right.is_some()
    }

    fn render_view_to_native(
        child: &dyn View,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<RegionId> {
        Self::render_view_to_native_with(child, send_message, parent, next_id, None)
    }

    fn render_view_to_native_with(
        child: &dyn View,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
        constraint_overrides: Option<Constraints>,
    ) -> anyhow::Result<RegionId> {
        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        let constraints = constraint_overrides.unwrap_or_else(|| child.constraints());

        send_message(&AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints,
            style: child.style(),
        })?;

        let content = child.content();
        if content != ultty_protocol::Content::None {
            send_message(&AppMessage::SetContent {
                id: region_id,
                content,
            })?;
        }

        if let Some((sx, sy)) = child.scroll_offset() {
            send_message(&AppMessage::Scroll {
                id: region_id,
                x: sx,
                y: sy,
                animate: false,
            })?;
        }

        for i in 0..child.children_count() {
            child.render_child_native(i, send_message, region_id, next_id)?;
        }

        Ok(region_id)
    }

    fn dimension_to_cells(d: &Dimension, total: u16, fallback: u16) -> u16 {
        match d {
            Dimension::Cells(v) => (*v as u16).min(total),
            Dimension::Percent(p) => ((total as f32) * (*p / 100.0)) as u16,
            _ => fallback.min(total),
        }
    }
}

impl View for DockLayout {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            overflow: ultty_protocol::Overflow {
                x: ultty_protocol::OverflowAxis::Hidden,
                y: ultty_protocol::OverflowAxis::Hidden,
            },
            ..Default::default()
        }
    }

    fn children_count(&self) -> usize {
        let mut count = 0;
        if self.top.is_some() {
            count += 1;
        }
        if self.has_middle() {
            count += 1;
        }
        if self.bottom.is_some() {
            count += 1;
        }
        count
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let mut current = 0usize;

        if let Some(top) = &self.top {
            if index == current {
                return Ok(Some(Self::render_view_to_native(
                    top.as_ref(),
                    send_message,
                    parent,
                    next_id,
                )?));
            }
            current += 1;
        }

        if self.has_middle() {
            if index == current {
                let middle_id = RegionId::new(*next_id);
                *next_id += 1;
                send_message(&AppMessage::CreateRegion {
                    id: middle_id,
                    parent,
                    insert_index: -1,
                    constraints: Constraints {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        width: Dimension::Percent(100.0),
                        flex_grow: 1.0,
                        flex_shrink: 1.0,
                        min_height: Dimension::Cells(0.0),
                        align_items: AlignItems::Stretch,
                        ..Default::default()
                    },
                    style: ProtoStyle::default(),
                })?;

                if let Some(left) = &self.left {
                    Self::render_view_to_native(left.as_ref(), send_message, middle_id, next_id)?;
                }
                if let Some(center) = &self.center {
                    let mut center_constraints = center.constraints();
                    center_constraints.width = Dimension::Auto;
                    center_constraints.flex_grow = 1.0;
                    center_constraints.flex_shrink = 1.0;
                    center_constraints.min_height = Dimension::Cells(0.0);
                    Self::render_view_to_native_with(
                        center.as_ref(),
                        send_message,
                        middle_id,
                        next_id,
                        Some(center_constraints),
                    )?;
                }
                if let Some(right) = &self.right {
                    Self::render_view_to_native(right.as_ref(), send_message, middle_id, next_id)?;
                }

                return Ok(Some(middle_id));
            }
            current += 1;
        }

        if let Some(bottom) = &self.bottom {
            if index == current {
                return Ok(Some(Self::render_view_to_native(
                    bottom.as_ref(),
                    send_message,
                    parent,
                    next_id,
                )?));
            }
        }

        Ok(None)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let mut middle = area;

        if let Some(top) = &self.top {
            let h = Self::dimension_to_cells(&top.constraints().height, middle.height, 1);
            let top_area = Rect::new(middle.x, middle.y, middle.width, h);
            top.render_ansi(frame, top_area);
            middle = Rect::new(
                middle.x,
                middle.y + h,
                middle.width,
                middle.height.saturating_sub(h),
            );
        }

        if let Some(bottom) = &self.bottom {
            let h = Self::dimension_to_cells(&bottom.constraints().height, middle.height, 1);
            let bottom_area = Rect::new(
                middle.x,
                middle.y + middle.height.saturating_sub(h),
                middle.width,
                h,
            );
            bottom.render_ansi(frame, bottom_area);
            middle = Rect::new(
                middle.x,
                middle.y,
                middle.width,
                middle.height.saturating_sub(h),
            );
        }

        if middle.width == 0 || middle.height == 0 {
            return;
        }

        let mut center_area = middle;

        if let Some(left) = &self.left {
            let w = Self::dimension_to_cells(&left.constraints().width, center_area.width, 20);
            let left_area = Rect::new(center_area.x, center_area.y, w, center_area.height);
            left.render_ansi(frame, left_area);
            center_area = Rect::new(
                center_area.x + w,
                center_area.y,
                center_area.width.saturating_sub(w),
                center_area.height,
            );
        }

        if let Some(right) = &self.right {
            let w = Self::dimension_to_cells(&right.constraints().width, center_area.width, 20);
            let right_area = Rect::new(
                center_area.x + center_area.width.saturating_sub(w),
                center_area.y,
                w,
                center_area.height,
            );
            right.render_ansi(frame, right_area);
            center_area = Rect::new(
                center_area.x,
                center_area.y,
                center_area.width.saturating_sub(w),
                center_area.height,
            );
        }

        if let Some(center) = &self.center {
            center.render_ansi(frame, center_area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DockLayout;
    use crate::widgets::{Text, View};
    use ultty_protocol::{Dimension, Display, FlexDirection};

    #[test]
    fn dock_empty_has_no_children() {
        let dock = DockLayout::new();
        assert_eq!(<DockLayout as View>::children_count(&dock), 0);
    }

    #[test]
    fn dock_center_only() {
        let dock = DockLayout::new().center(Text::new("center"));
        assert_eq!(<DockLayout as View>::children_count(&dock), 1);
    }

    #[test]
    fn dock_top_and_center() {
        let dock = DockLayout::new()
            .dock_top(Text::new("top"))
            .center(Text::new("center"));
        assert_eq!(<DockLayout as View>::children_count(&dock), 2);
    }

    #[test]
    fn dock_all_regions() {
        let dock = DockLayout::new()
            .dock_top(Text::new("top"))
            .dock_bottom(Text::new("bottom"))
            .dock_left(Text::new("left"))
            .center(Text::new("center"))
            .dock_right(Text::new("right"));
        assert_eq!(<DockLayout as View>::children_count(&dock), 3);
    }

    #[test]
    fn dock_constraints_are_flex_column() {
        let dock = DockLayout::new();
        let c = <DockLayout as View>::constraints(&dock);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Column);
        assert_eq!(c.width, Dimension::Percent(100.0));
        assert_eq!(c.flex_grow, 1.0);
    }

    #[test]
    fn dock_left_right_center() {
        let dock = DockLayout::new()
            .dock_left(Text::new("left"))
            .center(Text::new("center"))
            .dock_right(Text::new("right"));
        assert_eq!(<DockLayout as View>::children_count(&dock), 1);
    }
}
