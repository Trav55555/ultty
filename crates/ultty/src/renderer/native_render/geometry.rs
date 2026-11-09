//! Native scene scroll adjustment and clipping geometry.

use std::collections::HashMap;

use ultty_protocol::{OverflowAxis, RegionId};

use crate::scene::tree::SceneTree;

pub(super) type ScrollAdjustments = HashMap<RegionId, (f32, f32)>;
pub(super) type ClipRects = HashMap<RegionId, (f32, f32, f32, f32)>;

/// Rebuild the scroll adjustments and clip rects of the tree under `root`
/// (the full-scene root or a surface root), laid out from pixel `origin`.
/// Every clip rect is also held within `bounds` when it is given.
#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild_scroll_geometry(
    scene: &SceneTree,
    root: RegionId,
    scroll_offset: impl Fn(RegionId) -> (f32, f32),
    cell_width: f32,
    cell_height: f32,
    origin: (f32, f32),
    bounds: Option<(f32, f32, f32, f32)>,
    scroll_adjustments: &mut ScrollAdjustments,
    clip_rects: &mut ClipRects,
) {
    scroll_adjustments.clear();
    for region in scene.iter_tree(root) {
        if region.id == RegionId::ROOT {
            continue;
        }

        let adjustment = accumulated_scroll_adjustment(scene, &scroll_offset, region.parent);
        if adjustment != (0.0, 0.0) {
            scroll_adjustments.insert(region.id, adjustment);
        }
    }

    clip_rects.clear();
    for region in scene.iter_tree(root) {
        if region.id == RegionId::ROOT {
            continue;
        }

        let mut clip = inherited_clip_rect(
            scene,
            region.id,
            cell_width,
            cell_height,
            origin,
            scroll_adjustments,
        );
        if let Some(bounds) = bounds {
            clip.0 = clip.0.max(bounds.0);
            clip.1 = clip.1.max(bounds.1);
            clip.2 = clip.2.min(bounds.2);
            clip.3 = clip.3.min(bounds.3);
        }
        clip_rects.insert(region.id, clip);
    }
}

fn accumulated_scroll_adjustment(
    scene: &SceneTree,
    scroll_offset: &impl Fn(RegionId) -> (f32, f32),
    mut ancestor_id: Option<RegionId>,
) -> (f32, f32) {
    let mut adj_x = 0.0f32;
    let mut adj_y = 0.0f32;

    while let Some(id) = ancestor_id {
        let Some(ancestor) = scene.get(id) else {
            break;
        };

        let (sx, sy) = scroll_offset(id);
        if matches!(ancestor.constraints.overflow.x, OverflowAxis::Scroll) {
            adj_x += sx;
        }
        if matches!(ancestor.constraints.overflow.y, OverflowAxis::Scroll) {
            adj_y += sy;
        }
        ancestor_id = ancestor.parent;
    }

    (adj_x, adj_y)
}

fn inherited_clip_rect(
    scene: &SceneTree,
    region_id: RegionId,
    cell_width: f32,
    cell_height: f32,
    origin: (f32, f32),
    scroll_adjustments: &ScrollAdjustments,
) -> (f32, f32, f32, f32) {
    let Some(region) = scene.get(region_id) else {
        return (0.0, 0.0, 0.0, 0.0);
    };

    let mut clip = region_rect(region, cell_width, cell_height, origin, scroll_adjustments);

    let mut ancestor_id = region.parent;
    while let Some(id) = ancestor_id {
        let Some(ancestor) = scene.get(id) else {
            break;
        };

        let clips_x = matches!(
            ancestor.constraints.overflow.x,
            OverflowAxis::Hidden | OverflowAxis::Scroll
        );
        let clips_y = matches!(
            ancestor.constraints.overflow.y,
            OverflowAxis::Hidden | OverflowAxis::Scroll
        );

        if clips_x || clips_y {
            let ancestor_rect = region_rect(
                ancestor,
                cell_width,
                cell_height,
                origin,
                scroll_adjustments,
            );
            if clips_x {
                clip.0 = clip.0.max(ancestor_rect.0);
                clip.2 = clip.2.min(ancestor_rect.2);
            }
            if clips_y {
                clip.1 = clip.1.max(ancestor_rect.1);
                clip.3 = clip.3.min(ancestor_rect.3);
            }
        }

        ancestor_id = ancestor.parent;
    }

    clip
}

fn region_rect(
    region: &crate::scene::region::Region,
    cell_width: f32,
    cell_height: f32,
    (origin_x, origin_y): (f32, f32),
    scroll_adjustments: &ScrollAdjustments,
) -> (f32, f32, f32, f32) {
    let layout = &region.computed;
    let (scroll_x, scroll_y) = scroll_adjustments
        .get(&region.id)
        .copied()
        .unwrap_or((0.0, 0.0));
    let x = ((layout.x - scroll_x) * cell_width + origin_x).round();
    let y = ((layout.y - scroll_y) * cell_height + origin_y).round();
    let width = (layout.width * cell_width).round();
    let height = (layout.height * cell_height).round();

    (x, y, x + width, y + height)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ultty_protocol::{Constraints, Overflow, OverflowAxis, RegionId};

    use super::{rebuild_scroll_geometry, ClipRects, ScrollAdjustments};
    use crate::scene::region::{ComputedLayout, Region};
    use crate::scene::tree::SceneTree;

    fn region(
        id: RegionId,
        parent: RegionId,
        computed: ComputedLayout,
        overflow: Overflow,
    ) -> Region {
        let mut region = Region::new(id);
        region.parent = Some(parent);
        region.computed = computed;
        region.constraints = Constraints {
            overflow,
            ..Default::default()
        };
        region
    }

    #[test]
    fn nested_scroll_and_clip_geometry_uses_ancestor_rules() {
        let scroll_id = RegionId::new(1);
        let child_id = RegionId::new(2);
        let mut scene = SceneTree::new();

        scene.insert(region(
            scroll_id,
            RegionId::ROOT,
            ComputedLayout {
                x: 2.0,
                y: 3.0,
                width: 10.0,
                height: 5.0,
                ..Default::default()
            },
            Overflow {
                x: OverflowAxis::Visible,
                y: OverflowAxis::Scroll,
            },
        ));
        scene.insert(region(
            child_id,
            scroll_id,
            ComputedLayout {
                x: 2.0,
                y: 8.0,
                width: 6.0,
                height: 4.0,
                ..Default::default()
            },
            Overflow::default(),
        ));

        let mut scroll_adjustments = ScrollAdjustments::new();
        let mut clip_rects = ClipRects::new();
        let mut offsets = HashMap::new();
        offsets.insert(scroll_id, (0.0, 2.0));

        rebuild_scroll_geometry(
            &scene,
            RegionId::ROOT,
            |id| offsets.get(&id).copied().unwrap_or((0.0, 0.0)),
            10.0,
            20.0,
            (0.0, 5.0),
            None,
            &mut scroll_adjustments,
            &mut clip_rects,
        );

        assert_eq!(scroll_adjustments.get(&child_id), Some(&(0.0, 2.0)));
        assert_eq!(scroll_adjustments.get(&scroll_id), None);
        assert_eq!(clip_rects.get(&child_id), Some(&(20.0, 125.0, 80.0, 165.0)));
    }

    /// Surfaces are not drawn by the full-scene pass (ADR 0020 slice 3
    /// draws them at their anchor rows): the geometry the draw loop reads is
    /// the same with a surface as without, and holds nothing for it.
    #[test]
    fn surface_regions_are_left_out_of_full_scene_geometry() {
        let build = |with_surface: bool| {
            let mut scene = SceneTree::new();
            scene.insert(region(
                RegionId::new(1),
                RegionId::ROOT,
                ComputedLayout {
                    width: 80.0,
                    height: 24.0,
                    ..Default::default()
                },
                Overflow::default(),
            ));
            if with_surface {
                scene
                    .try_insert_surface(Region::new(RegionId::new(10)), 3)
                    .unwrap();
                scene.insert(region(
                    RegionId::new(11),
                    RegionId::new(10),
                    ComputedLayout {
                        width: 80.0,
                        height: 3.0,
                        ..Default::default()
                    },
                    Overflow::default(),
                ));
            }
            let mut scroll_adjustments = ScrollAdjustments::new();
            let mut clip_rects = ClipRects::new();
            rebuild_scroll_geometry(
                &scene,
                RegionId::ROOT,
                |_| (0.0, 0.0),
                10.0,
                20.0,
                (0.0, 5.0),
                None,
                &mut scroll_adjustments,
                &mut clip_rects,
            );
            let drawn: Vec<RegionId> = scene.iter_depth_first().map(|r| r.id).collect();
            (drawn, scroll_adjustments, clip_rects)
        };

        let (drawn, adjustments, clips) = build(true);
        assert_eq!(drawn, [RegionId::ROOT, RegionId::new(1)]);
        assert!(!clips.contains_key(&RegionId::new(10)));
        assert!(!clips.contains_key(&RegionId::new(11)));
        assert_eq!((drawn, adjustments, clips), build(false));
    }

    /// A surface's regions are placed from its origin and clipped to the
    /// bounds it is drawn in, here a surface whose top row is above the
    /// pane.
    #[test]
    fn surface_geometry_is_offset_to_its_origin_and_held_in_bounds() {
        let mut scene = SceneTree::new();
        let mut root = Region::new(RegionId::new(10));
        root.computed = ComputedLayout {
            width: 8.0,
            height: 3.0,
            ..Default::default()
        };
        scene.try_insert_surface(root, 3).unwrap();
        scene.insert(region(
            RegionId::new(11),
            RegionId::new(10),
            ComputedLayout {
                x: 1.0,
                y: 0.0,
                width: 4.0,
                height: 2.0,
                ..Default::default()
            },
            Overflow::default(),
        ));

        let mut scroll_adjustments = ScrollAdjustments::new();
        let mut clip_rects = ClipRects::new();
        rebuild_scroll_geometry(
            &scene,
            RegionId::new(10),
            |_| (0.0, 0.0),
            10.0,
            20.0,
            (0.0, -15.0),
            Some((0.0, 5.0, 80.0, 45.0)),
            &mut scroll_adjustments,
            &mut clip_rects,
        );

        assert_eq!(
            clip_rects.get(&RegionId::new(10)),
            Some(&(0.0, 5.0, 80.0, 45.0))
        );
        assert_eq!(
            clip_rects.get(&RegionId::new(11)),
            Some(&(10.0, 5.0, 50.0, 25.0))
        );
        assert!(!clip_rects.contains_key(&RegionId::ROOT));
    }
}
