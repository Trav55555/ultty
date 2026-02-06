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

#[derive(Debug)]
pub enum LayoutEngineError {
    SetStyle {
        region_id: RegionId,
        source: taffy::TaffyError,
    },
    NewLeaf {
        region_id: RegionId,
        source: taffy::TaffyError,
    },
    Remove {
        region_id: RegionId,
        source: taffy::TaffyError,
    },
    SetChildren {
        region_id: RegionId,
        source: taffy::TaffyError,
    },
    Compute {
        source: taffy::TaffyError,
    },
}

impl std::fmt::Display for LayoutEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetStyle { region_id, source } => {
                write!(f, "set_style failed for region {}: {}", region_id.0, source)
            }
            Self::NewLeaf { region_id, source } => {
                write!(f, "new_leaf failed for region {}: {}", region_id.0, source)
            }
            Self::Remove { region_id, source } => {
                write!(f, "remove failed for region {}: {}", region_id.0, source)
            }
            Self::SetChildren { region_id, source } => {
                write!(
                    f,
                    "set_children failed for region {}: {}",
                    region_id.0, source
                )
            }
            Self::Compute { source } => write!(f, "compute_layout failed: {}", source),
        }
    }
}

impl std::error::Error for LayoutEngineError {}

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

    pub fn sync(&mut self, scene: &SceneTree) -> Result<(), LayoutEngineError> {
        let mut seen = std::collections::HashSet::new();

        for region in scene.iter_depth_first() {
            seen.insert(region.id);

            if let Some(&node_id) = self.region_to_node.get(&region.id) {
                let style = constraints_to_style(&region.constraints);
                self.taffy.set_style(node_id, style).map_err(|source| {
                    LayoutEngineError::SetStyle {
                        region_id: region.id,
                        source,
                    }
                })?;
            } else {
                let style = constraints_to_style(&region.constraints);
                let node_id =
                    self.taffy
                        .new_leaf(style)
                        .map_err(|source| LayoutEngineError::NewLeaf {
                            region_id: region.id,
                            source,
                        })?;
                self.region_to_node.insert(region.id, node_id);
                self.node_to_region.insert(node_id, region.id);
            }
        }

        let stale: Vec<_> = self
            .region_to_node
            .iter()
            .filter(|(id, _)| !seen.contains(id))
            .map(|(id, node)| (*id, *node))
            .collect();

        for (region_id, node_id) in stale {
            self.taffy
                .remove(node_id)
                .map_err(|source| LayoutEngineError::Remove { region_id, source })?;
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

                self.taffy
                    .set_children(parent_node, &child_nodes)
                    .map_err(|source| LayoutEngineError::SetChildren {
                        region_id: region.id,
                        source,
                    })?;
            }
        }

        Ok(())
    }

    pub fn compute(&mut self, width: f32, height: f32) -> Result<(), LayoutEngineError> {
        if let Some(&root_node) = self.region_to_node.get(&RegionId::ROOT) {
            let size = Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            };
            self.taffy
                .compute_layout(root_node, size)
                .map_err(|source| LayoutEngineError::Compute { source })?;
        }

        Ok(())
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
        engine.sync(&scene).unwrap();
        engine.compute(80.0, 24.0).unwrap();

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
        engine.sync(&scene).unwrap();

        assert!(engine.get_layout(RegionId::new(1)).is_some());

        // Remove a region from the scene
        scene.remove(RegionId::new(1));
        engine.sync(&scene).unwrap();

        assert!(engine.get_layout(RegionId::new(1)).is_none());
    }

    #[test]
    fn layout_empty_tree() {
        let scene = SceneTree::new();
        let mut engine = LayoutEngine::new();
        engine.sync(&scene).unwrap();
        engine.compute(80.0, 24.0).unwrap();

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
        engine.sync(&tree).unwrap();
        engine.compute(100.0, 10.0).unwrap();

        let left_layout = engine.get_layout(RegionId::new(1)).unwrap();
        assert_eq!(left_layout.size.width, 30.0);

        let right_layout = engine.get_layout(RegionId::new(2)).unwrap();
        assert_eq!(right_layout.size.width, 70.0); // 100 - 30
    }

    // ── constraints_to_style tests ──────────────────────────────────

    use ultty_protocol::{
        AlignContent as ProtoAlignContent, AlignItems as ProtoAlignItems,
        AlignSelf as ProtoAlignSelf, Edges, FlexWrap as ProtoFlexWrap,
        JustifyContent as ProtoJustifyContent, Overflow as ProtoOverflow,
        OverflowAxis as ProtoOverflowAxis, Size as ProtoSize,
    };

    fn default_constraints() -> Constraints {
        Constraints::default()
    }

    #[test]
    fn constraints_display_variants() {
        for (proto, expected) in [
            (ProtoDisplay::Block, taffy::Display::Block),
            (ProtoDisplay::Flex, taffy::Display::Flex),
            (ProtoDisplay::Grid, taffy::Display::Grid),
            (ProtoDisplay::None, taffy::Display::None),
        ] {
            let c = Constraints {
                display: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.display, expected, "Display::{proto:?}");
        }
    }

    #[test]
    fn constraints_flex_direction_variants() {
        for (proto, expected) in [
            (ProtoFlexDirection::Row, taffy::FlexDirection::Row),
            (
                ProtoFlexDirection::RowReverse,
                taffy::FlexDirection::RowReverse,
            ),
            (ProtoFlexDirection::Column, taffy::FlexDirection::Column),
            (
                ProtoFlexDirection::ColumnReverse,
                taffy::FlexDirection::ColumnReverse,
            ),
        ] {
            let c = Constraints {
                flex_direction: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.flex_direction, expected, "FlexDirection::{proto:?}");
        }
    }

    #[test]
    fn constraints_flex_wrap_variants() {
        for (proto, expected) in [
            (ProtoFlexWrap::NoWrap, taffy::FlexWrap::NoWrap),
            (ProtoFlexWrap::Wrap, taffy::FlexWrap::Wrap),
            (ProtoFlexWrap::WrapReverse, taffy::FlexWrap::WrapReverse),
        ] {
            let c = Constraints {
                flex_wrap: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.flex_wrap, expected, "FlexWrap::{proto:?}");
        }
    }

    #[test]
    fn constraints_justify_content_variants() {
        for (proto, expected) in [
            (ProtoJustifyContent::Start, taffy::JustifyContent::Start),
            (ProtoJustifyContent::End, taffy::JustifyContent::End),
            (ProtoJustifyContent::Center, taffy::JustifyContent::Center),
            (
                ProtoJustifyContent::SpaceBetween,
                taffy::JustifyContent::SpaceBetween,
            ),
            (
                ProtoJustifyContent::SpaceAround,
                taffy::JustifyContent::SpaceAround,
            ),
            (
                ProtoJustifyContent::SpaceEvenly,
                taffy::JustifyContent::SpaceEvenly,
            ),
        ] {
            let c = Constraints {
                justify_content: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(
                style.justify_content,
                Some(expected),
                "JustifyContent::{proto:?}"
            );
        }
    }

    #[test]
    fn constraints_align_items_variants() {
        for (proto, expected) in [
            (ProtoAlignItems::Start, taffy::AlignItems::Start),
            (ProtoAlignItems::End, taffy::AlignItems::End),
            (ProtoAlignItems::Center, taffy::AlignItems::Center),
            (ProtoAlignItems::Baseline, taffy::AlignItems::Baseline),
            (ProtoAlignItems::Stretch, taffy::AlignItems::Stretch),
        ] {
            let c = Constraints {
                align_items: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.align_items, Some(expected), "AlignItems::{proto:?}");
        }
    }

    #[test]
    fn constraints_align_self_auto_maps_to_none() {
        let c = Constraints {
            align_self: ProtoAlignSelf::Auto,
            ..default_constraints()
        };
        let style = constraints_to_style(&c);
        assert_eq!(style.align_self, None);
    }

    #[test]
    fn constraints_align_self_non_auto_maps_to_some() {
        for (proto, expected) in [
            (ProtoAlignSelf::Start, taffy::AlignSelf::Start),
            (ProtoAlignSelf::End, taffy::AlignSelf::End),
            (ProtoAlignSelf::Center, taffy::AlignSelf::Center),
            (ProtoAlignSelf::Baseline, taffy::AlignSelf::Baseline),
            (ProtoAlignSelf::Stretch, taffy::AlignSelf::Stretch),
        ] {
            let c = Constraints {
                align_self: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.align_self, Some(expected), "AlignSelf::{proto:?}");
        }
    }

    #[test]
    fn constraints_align_content_variants() {
        for (proto, expected) in [
            (ProtoAlignContent::Start, taffy::AlignContent::Start),
            (ProtoAlignContent::End, taffy::AlignContent::End),
            (ProtoAlignContent::Center, taffy::AlignContent::Center),
            (
                ProtoAlignContent::SpaceBetween,
                taffy::AlignContent::SpaceBetween,
            ),
            (
                ProtoAlignContent::SpaceAround,
                taffy::AlignContent::SpaceAround,
            ),
            (
                ProtoAlignContent::SpaceEvenly,
                taffy::AlignContent::SpaceEvenly,
            ),
            (ProtoAlignContent::Stretch, taffy::AlignContent::Stretch),
        ] {
            let c = Constraints {
                align_content: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(
                style.align_content,
                Some(expected),
                "AlignContent::{proto:?}"
            );
        }
    }

    #[test]
    fn constraints_overflow_axis_mapping() {
        for (proto, expected) in [
            (ProtoOverflowAxis::Visible, taffy::Overflow::Visible),
            (ProtoOverflowAxis::Hidden, taffy::Overflow::Hidden),
            (ProtoOverflowAxis::Scroll, taffy::Overflow::Scroll),
            (ProtoOverflowAxis::Auto, taffy::Overflow::Scroll),
        ] {
            let c = Constraints {
                overflow: ProtoOverflow { x: proto, y: proto },
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.overflow.x, expected, "Overflow x for {proto:?}");
            assert_eq!(style.overflow.y, expected, "Overflow y for {proto:?}");
        }
    }

    #[test]
    fn constraints_flex_grow_shrink_aspect_passthrough() {
        let c = Constraints {
            flex_grow: 2.5,
            flex_shrink: 0.5,
            aspect_ratio: Some(1.5),
            ..default_constraints()
        };
        let style = constraints_to_style(&c);
        assert_eq!(style.flex_grow, 2.5);
        assert_eq!(style.flex_shrink, 0.5);
        assert_eq!(style.aspect_ratio, Some(1.5));
    }

    #[test]
    fn constraints_margin_padding_gap_use_dimension_helpers() {
        let c = Constraints {
            margin: Edges::new(
                ProtoDimension::Cells(1.0),
                ProtoDimension::Cells(2.0),
                ProtoDimension::Cells(3.0),
                ProtoDimension::Auto,
            ),
            padding: Edges::all(ProtoDimension::Percent(10.0)),
            gap: ProtoSize {
                width: ProtoDimension::Cells(5.0),
                height: ProtoDimension::Percent(20.0),
            },
            ..default_constraints()
        };
        let style = constraints_to_style(&c);

        assert_eq!(style.margin.top, LengthPercentageAuto::Length(1.0));
        assert_eq!(style.margin.right, LengthPercentageAuto::Length(2.0));
        assert_eq!(style.margin.bottom, LengthPercentageAuto::Length(3.0));
        assert_eq!(style.margin.left, LengthPercentageAuto::Auto);

        assert_eq!(style.padding.left, LengthPercentage::Percent(0.1));
        assert_eq!(style.padding.right, LengthPercentage::Percent(0.1));

        assert_eq!(style.gap.width, LengthPercentage::Length(5.0));
        assert_eq!(style.gap.height, LengthPercentage::Percent(0.2));
    }

    // ── dimension_to_taffy tests ────────────────────────────────────

    #[test]
    fn dimension_to_taffy_auto_and_cells() {
        assert_eq!(dimension_to_taffy(&ProtoDimension::Auto), Dimension::Auto);
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Cells(10.0)),
            Dimension::Length(10.0)
        );
    }

    #[test]
    fn dimension_to_taffy_percent_divides_by_100() {
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Percent(50.0)),
            Dimension::Percent(0.5)
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Percent(100.0)),
            Dimension::Percent(1.0)
        );
    }

    #[test]
    fn dimension_to_taffy_fr_mincontent_maxcontent_fallback_to_auto() {
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Fr(1.0)),
            Dimension::Auto
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::MinContent),
            Dimension::Auto
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::MaxContent),
            Dimension::Auto
        );
    }

    #[test]
    fn dimension_to_taffy_viewport_units() {
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Vw(50.0)),
            Dimension::Percent(0.5)
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Vh(50.0)),
            Dimension::Percent(0.5)
        );
    }

    // ── dimension_to_length_auto tests ──────────────────────────────

    #[test]
    fn length_auto_basic_variants() {
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Auto),
            LengthPercentageAuto::Auto
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Cells(5.0)),
            LengthPercentageAuto::Length(5.0)
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Percent(25.0)),
            LengthPercentageAuto::Percent(0.25)
        );
    }

    #[test]
    fn length_auto_catch_all_returns_auto() {
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Fr(1.0)),
            LengthPercentageAuto::Auto
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Vw(50.0)),
            LengthPercentageAuto::Auto
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Vh(50.0)),
            LengthPercentageAuto::Auto
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::MinContent),
            LengthPercentageAuto::Auto
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::MaxContent),
            LengthPercentageAuto::Auto
        );
    }

    // ── dimension_to_length_pct tests ───────────────────────────────

    #[test]
    fn length_pct_cells_and_percent() {
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Cells(5.0)),
            LengthPercentage::Length(5.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Percent(50.0)),
            LengthPercentage::Percent(0.5)
        );
    }

    #[test]
    fn length_pct_catch_all_returns_zero_length() {
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Auto),
            LengthPercentage::Length(0.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Fr(1.0)),
            LengthPercentage::Length(0.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Vw(50.0)),
            LengthPercentage::Length(0.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Vh(50.0)),
            LengthPercentage::Length(0.0)
        );
    }
}
