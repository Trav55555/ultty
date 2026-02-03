//! Taffy integration for layout computation.

use std::collections::HashMap;

use taffy::prelude::*;
use taffy::Overflow;
use ultty_protocol::{
    AlignContent as ProtoAlignContent, AlignItems as ProtoAlignItems, AlignSelf as ProtoAlignSelf,
    Constraints, Dimension as ProtoDimension, Display as ProtoDisplay,
    FlexDirection as ProtoFlexDirection, FlexWrap as ProtoFlexWrap,
    JustifyContent as ProtoJustifyContent, OverflowAxis as ProtoOverflowAxis, RegionId,
};

use super::tree::SceneTree;

pub struct LayoutEngine {
    taffy: TaffyTree<()>,
    region_to_node: HashMap<RegionId, NodeId>,
    node_to_region: HashMap<NodeId, RegionId>,
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            taffy: TaffyTree::new(),
            region_to_node: HashMap::new(),
            node_to_region: HashMap::new(),
        }
    }

    pub fn sync(&mut self, scene: &SceneTree) {
        let mut seen = std::collections::HashSet::new();

        for region in scene.iter_depth_first() {
            seen.insert(region.id);

            if let Some(&node_id) = self.region_to_node.get(&region.id) {
                let style = constraints_to_style(&region.constraints);
                let _ = self.taffy.set_style(node_id, style);
            } else {
                let style = constraints_to_style(&region.constraints);
                if let Ok(node_id) = self.taffy.new_leaf(style) {
                    self.region_to_node.insert(region.id, node_id);
                    self.node_to_region.insert(node_id, region.id);
                }
            }
        }

        let stale: Vec<_> = self
            .region_to_node
            .iter()
            .filter(|(id, _)| !seen.contains(id))
            .map(|(id, node)| (*id, *node))
            .collect();

        for (region_id, node_id) in stale {
            let _ = self.taffy.remove(node_id);
            self.region_to_node.remove(&region_id);
            self.node_to_region.remove(&node_id);
        }

        for region in scene.iter_depth_first() {
            if let Some(&parent_node) = self.region_to_node.get(&region.id) {
                let child_nodes: Vec<_> = region
                    .children
                    .iter()
                    .filter_map(|child_id| self.region_to_node.get(child_id).copied())
                    .collect();

                let _ = self.taffy.set_children(parent_node, &child_nodes);
            }
        }
    }

    pub fn compute(&mut self, width: f32, height: f32) {
        if let Some(&root_node) = self.region_to_node.get(&RegionId::ROOT) {
            let size = Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            };
            let _ = self.taffy.compute_layout(root_node, size);
        }
    }

    pub fn get_layout(&self, region_id: RegionId) -> Option<taffy::Layout> {
        let node_id = self.region_to_node.get(&region_id)?;
        self.taffy.layout(*node_id).ok().cloned()
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn constraints_to_style(c: &Constraints) -> Style {
    Style {
        display: match c.display {
            ProtoDisplay::Block => Display::Block,
            ProtoDisplay::Flex => Display::Flex,
            ProtoDisplay::Grid => Display::Grid,
            ProtoDisplay::None => Display::None,
        },
        flex_direction: match c.flex_direction {
            ProtoFlexDirection::Row => FlexDirection::Row,
            ProtoFlexDirection::RowReverse => FlexDirection::RowReverse,
            ProtoFlexDirection::Column => FlexDirection::Column,
            ProtoFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
        },
        flex_wrap: match c.flex_wrap {
            ProtoFlexWrap::NoWrap => FlexWrap::NoWrap,
            ProtoFlexWrap::Wrap => FlexWrap::Wrap,
            ProtoFlexWrap::WrapReverse => FlexWrap::WrapReverse,
        },
        justify_content: Some(match c.justify_content {
            ProtoJustifyContent::Start => JustifyContent::Start,
            ProtoJustifyContent::End => JustifyContent::End,
            ProtoJustifyContent::Center => JustifyContent::Center,
            ProtoJustifyContent::SpaceBetween => JustifyContent::SpaceBetween,
            ProtoJustifyContent::SpaceAround => JustifyContent::SpaceAround,
            ProtoJustifyContent::SpaceEvenly => JustifyContent::SpaceEvenly,
        }),
        align_items: Some(match c.align_items {
            ProtoAlignItems::Start => AlignItems::Start,
            ProtoAlignItems::End => AlignItems::End,
            ProtoAlignItems::Center => AlignItems::Center,
            ProtoAlignItems::Baseline => AlignItems::Baseline,
            ProtoAlignItems::Stretch => AlignItems::Stretch,
        }),
        align_self: match c.align_self {
            ProtoAlignSelf::Auto => None,
            ProtoAlignSelf::Start => Some(AlignSelf::Start),
            ProtoAlignSelf::End => Some(AlignSelf::End),
            ProtoAlignSelf::Center => Some(AlignSelf::Center),
            ProtoAlignSelf::Baseline => Some(AlignSelf::Baseline),
            ProtoAlignSelf::Stretch => Some(AlignSelf::Stretch),
        },
        align_content: Some(match c.align_content {
            ProtoAlignContent::Start => AlignContent::Start,
            ProtoAlignContent::End => AlignContent::End,
            ProtoAlignContent::Center => AlignContent::Center,
            ProtoAlignContent::SpaceBetween => AlignContent::SpaceBetween,
            ProtoAlignContent::SpaceAround => AlignContent::SpaceAround,
            ProtoAlignContent::SpaceEvenly => AlignContent::SpaceEvenly,
            ProtoAlignContent::Stretch => AlignContent::Stretch,
        }),
        flex_grow: c.flex_grow,
        flex_shrink: c.flex_shrink,
        flex_basis: dimension_to_taffy(&c.flex_basis),
        size: Size {
            width: dimension_to_taffy(&c.width),
            height: dimension_to_taffy(&c.height),
        },
        min_size: Size {
            width: dimension_to_taffy(&c.min_width),
            height: dimension_to_taffy(&c.min_height),
        },
        max_size: Size {
            width: dimension_to_taffy(&c.max_width),
            height: dimension_to_taffy(&c.max_height),
        },
        aspect_ratio: c.aspect_ratio,
        margin: Rect {
            left: dimension_to_length_auto(&c.margin.left),
            right: dimension_to_length_auto(&c.margin.right),
            top: dimension_to_length_auto(&c.margin.top),
            bottom: dimension_to_length_auto(&c.margin.bottom),
        },
        padding: Rect {
            left: dimension_to_length_pct(&c.padding.left),
            right: dimension_to_length_pct(&c.padding.right),
            top: dimension_to_length_pct(&c.padding.top),
            bottom: dimension_to_length_pct(&c.padding.bottom),
        },
        gap: Size {
            width: dimension_to_length_pct(&c.gap.width),
            height: dimension_to_length_pct(&c.gap.height),
        },
        overflow: taffy::Point {
            x: match c.overflow.x {
                ProtoOverflowAxis::Visible => Overflow::Visible,
                ProtoOverflowAxis::Hidden => Overflow::Hidden,
                ProtoOverflowAxis::Scroll => Overflow::Scroll,
                ProtoOverflowAxis::Auto => Overflow::Scroll,
            },
            y: match c.overflow.y {
                ProtoOverflowAxis::Visible => Overflow::Visible,
                ProtoOverflowAxis::Hidden => Overflow::Hidden,
                ProtoOverflowAxis::Scroll => Overflow::Scroll,
                ProtoOverflowAxis::Auto => Overflow::Scroll,
            },
        },
        ..Default::default()
    }
}

fn dimension_to_taffy(d: &ProtoDimension) -> Dimension {
    match d {
        ProtoDimension::Auto => Dimension::Auto,
        ProtoDimension::Cells(v) => Dimension::Length(*v),
        ProtoDimension::Percent(v) => Dimension::Percent(*v / 100.0),
        ProtoDimension::Fr(_) => Dimension::Auto,
        ProtoDimension::Vw(v) => Dimension::Percent(*v / 100.0),
        ProtoDimension::Vh(v) => Dimension::Percent(*v / 100.0),
        ProtoDimension::MinContent => Dimension::Auto,
        ProtoDimension::MaxContent => Dimension::Auto,
    }
}

fn dimension_to_length_auto(d: &ProtoDimension) -> LengthPercentageAuto {
    match d {
        ProtoDimension::Auto => LengthPercentageAuto::Auto,
        ProtoDimension::Cells(v) => LengthPercentageAuto::Length(*v),
        ProtoDimension::Percent(v) => LengthPercentageAuto::Percent(*v / 100.0),
        _ => LengthPercentageAuto::Auto,
    }
}

fn dimension_to_length_pct(d: &ProtoDimension) -> LengthPercentage {
    match d {
        ProtoDimension::Cells(v) => LengthPercentage::Length(*v),
        ProtoDimension::Percent(v) => LengthPercentage::Percent(*v / 100.0),
        _ => LengthPercentage::Length(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::region::Region;
    use ultty_protocol::{
        Constraints, Dimension as ProtoDimension, Display as ProtoDisplay,
        FlexDirection as ProtoFlexDirection,
    };

    fn build_scene() -> SceneTree {
        let mut tree = SceneTree::new();

        // Root with two children: a fixed-size and a flex-grow
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(24.0),
            ..Default::default()
        };

        let mut header = Region::new(RegionId::new(1));
        header.parent = Some(RegionId::ROOT);
        header.constraints = Constraints {
            height: ProtoDimension::Cells(3.0),
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(header);

        let mut body = Region::new(RegionId::new(2));
        body.parent = Some(RegionId::ROOT);
        body.constraints = Constraints {
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(body);

        tree
    }

    #[test]
    fn layout_sync_and_compute() {
        let scene = build_scene();
        let mut engine = LayoutEngine::new();
        engine.sync(&scene);
        engine.compute(80.0, 24.0);

        let root_layout = engine.get_layout(RegionId::ROOT).unwrap();
        assert_eq!(root_layout.size.width, 80.0);
        assert_eq!(root_layout.size.height, 24.0);

        let header = engine.get_layout(RegionId::new(1)).unwrap();
        assert_eq!(header.size.height, 3.0);
        assert_eq!(header.size.width, 80.0);

        let body = engine.get_layout(RegionId::new(2)).unwrap();
        assert_eq!(body.size.width, 80.0);
        assert_eq!(body.size.height, 21.0); // 24 - 3
    }

    #[test]
    fn layout_removes_stale_nodes() {
        let mut scene = build_scene();
        let mut engine = LayoutEngine::new();
        engine.sync(&scene);

        assert!(engine.get_layout(RegionId::new(1)).is_some());

        // Remove a region from the scene
        scene.remove(RegionId::new(1));
        engine.sync(&scene);

        assert!(engine.get_layout(RegionId::new(1)).is_none());
    }

    #[test]
    fn layout_empty_tree() {
        let scene = SceneTree::new();
        let mut engine = LayoutEngine::new();
        engine.sync(&scene);
        engine.compute(80.0, 24.0);

        // Root with default constraints gets no explicit size
        let root = engine.get_layout(RegionId::ROOT).unwrap();
        assert!(root.size.width >= 0.0);
        assert!(root.size.height >= 0.0);
    }

    #[test]
    fn layout_row_direction() {
        let mut tree = SceneTree::new();
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Row,
            width: ProtoDimension::Cells(100.0),
            height: ProtoDimension::Cells(10.0),
            ..Default::default()
        };

        let mut left = Region::new(RegionId::new(1));
        left.parent = Some(RegionId::ROOT);
        left.constraints = Constraints {
            width: ProtoDimension::Cells(30.0),
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(left);

        let mut right = Region::new(RegionId::new(2));
        right.parent = Some(RegionId::ROOT);
        right.constraints = Constraints {
            flex_grow: 1.0,
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(right);

        let mut engine = LayoutEngine::new();
        engine.sync(&tree);
        engine.compute(100.0, 10.0);

        let left_layout = engine.get_layout(RegionId::new(1)).unwrap();
        assert_eq!(left_layout.size.width, 30.0);

        let right_layout = engine.get_layout(RegionId::new(2)).unwrap();
        assert_eq!(right_layout.size.width, 70.0); // 100 - 30
    }
}
