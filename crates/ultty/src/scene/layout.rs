//! Taffy integration for layout computation.

mod constraints;

use std::collections::HashMap;

use taffy::prelude::*;
use ultty_protocol::{ComputedRegionLayout, RegionId};

use super::region::ComputedLayout;
use super::tree::SceneTree;
use constraints::constraints_to_style;
#[cfg(test)]
use constraints::{dimension_to_length_auto, dimension_to_length_pct, dimension_to_taffy};

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
    /// Surface root nodes and their heights in rows, as of the last sync.
    surfaces: Vec<(NodeId, f32)>,
}

/// A surface root's style: its constraints, but always exactly the space
/// it is laid out in (the viewport's width by its rows) at that space's
/// top left, whatever its size and margin constraints say.
fn surface_root_style(region: &super::region::Region) -> Style {
    Style {
        position: Position::Relative,
        inset: Rect::auto(),
        margin: Rect::zero(),
        size: Size::from_percent(1.0, 1.0),
        min_size: Size::auto(),
        max_size: Size::auto(),
        ..constraints_to_style(&region.constraints)
    }
}

/// Lay out the tree under `node` in `width x height` cells of space.
fn compute_root(
    taffy: &mut TaffyTree<()>,
    node: NodeId,
    width: f32,
    height: f32,
) -> Result<(), LayoutEngineError> {
    let size = Size {
        width: AvailableSpace::Definite(width),
        height: AvailableSpace::Definite(height),
    };
    taffy
        .compute_layout(node, size)
        .map_err(|source| LayoutEngineError::Compute { source })
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            taffy: TaffyTree::new(),
            region_to_node: HashMap::new(),
            node_to_region: HashMap::new(),
            surfaces: Vec::new(),
        }
    }

    pub fn sync(&mut self, scene: &SceneTree) -> Result<(), LayoutEngineError> {
        let mut seen = std::collections::HashSet::new();

        for region in scene.iter_all() {
            seen.insert(region.id);
            let style = if scene.surface(region.id).is_some() {
                surface_root_style(region)
            } else {
                constraints_to_style(&region.constraints)
            };

            if let Some(&node_id) = self.region_to_node.get(&region.id) {
                self.taffy.set_style(node_id, style).map_err(|source| {
                    LayoutEngineError::SetStyle {
                        region_id: region.id,
                        source,
                    }
                })?;
            } else {
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

        // Collect every node whose child list differs from the scene.
        let mut changed = Vec::new();
        for region in scene.iter_all() {
            let Some(&parent_node) = self.region_to_node.get(&region.id) else {
                continue;
            };
            let child_nodes: Vec<_> = region
                .children
                .iter()
                .filter_map(|child_id| self.region_to_node.get(child_id).copied())
                .collect();
            let current = self.taffy.children(parent_node).map_err(|source| {
                LayoutEngineError::SetChildren {
                    region_id: region.id,
                    source,
                }
            })?;
            if current != child_nodes {
                changed.push((region.id, parent_node, child_nodes));
            }
        }

        // Taffy's `set_children` unconditionally clears the parent pointer of
        // every child it previously held. If a child moved to a node that was
        // updated earlier in this pass, that would leave it attached to the
        // new parent's child list with no parent pointer, and the next
        // `remove` of the child would leave a dangling key behind (and panic
        // on the following sync). Detach all changed nodes first so the
        // second pass only ever installs parent pointers.
        for (region_id, node, _) in &changed {
            self.taffy.set_children(*node, &[]).map_err(|source| {
                LayoutEngineError::SetChildren {
                    region_id: *region_id,
                    source,
                }
            })?;
        }
        for (region_id, node, child_nodes) in changed {
            self.taffy
                .set_children(node, &child_nodes)
                .map_err(|source| LayoutEngineError::SetChildren { region_id, source })?;
        }

        self.surfaces = scene
            .surfaces()
            .iter()
            .filter_map(|surface| {
                let node = self.region_to_node.get(&surface.id)?;
                Some((*node, f32::from(surface.rows)))
            })
            .collect();

        Ok(())
    }

    /// Number of regions currently tracked by the engine.
    #[cfg(test)]
    fn node_count(&self) -> usize {
        self.region_to_node.len()
    }

    /// Lay out the full-scene tree at `width x height` and each surface,
    /// independently, at `width` by its rows.
    pub fn compute(&mut self, width: f32, height: f32) -> Result<(), LayoutEngineError> {
        if let Some(&root_node) = self.region_to_node.get(&RegionId::ROOT) {
            compute_root(&mut self.taffy, root_node, width, height)?;
        }
        for &(node, rows) in &self.surfaces {
            compute_root(&mut self.taffy, node, width, rows)?;
        }

        Ok(())
    }

    pub fn get_layout(&self, region_id: RegionId) -> Option<taffy::Layout> {
        let node_id = self.region_to_node.get(&region_id)?;
        self.taffy.layout(*node_id).ok().cloned()
    }

    /// Sync, compute, and write computed layouts back to the scene tree.
    ///
    /// Offsets are absolute from the top left of each region's root, the
    /// viewport or a surface. The full-scene tree comes first, then each
    /// surface's tree in creation order.
    pub fn compute_scene_layout(
        &mut self,
        scene: &mut SceneTree,
        width: f32,
        height: f32,
    ) -> Result<Vec<ComputedRegionLayout>, LayoutEngineError> {
        self.sync(scene)?;
        self.compute(width, height)?;

        let mut absolute_offsets: HashMap<RegionId, (f32, f32)> = HashMap::new();

        let mut regions = Vec::new();
        for region in scene.iter_all() {
            if let Some(taffy_layout) = self.get_layout(region.id) {
                let (parent_abs_x, parent_abs_y) = region
                    .parent
                    .and_then(|pid| absolute_offsets.get(&pid))
                    .copied()
                    .unwrap_or((0.0, 0.0));

                let abs_x = parent_abs_x + taffy_layout.location.x;
                let abs_y = parent_abs_y + taffy_layout.location.y;

                absolute_offsets.insert(region.id, (abs_x, abs_y));

                regions.push(ComputedRegionLayout {
                    id: region.id,
                    x: abs_x,
                    y: abs_y,
                    width: taffy_layout.size.width,
                    height: taffy_layout.size.height,
                    content_x: abs_x + taffy_layout.padding.left + taffy_layout.border.left,
                    content_y: abs_y + taffy_layout.padding.top + taffy_layout.border.top,
                    content_width: taffy_layout.size.width
                        - taffy_layout.padding.left
                        - taffy_layout.padding.right
                        - taffy_layout.border.left
                        - taffy_layout.border.right,
                    content_height: taffy_layout.size.height
                        - taffy_layout.padding.top
                        - taffy_layout.padding.bottom
                        - taffy_layout.border.top
                        - taffy_layout.border.bottom,
                    visible: true,
                });
            }
        }

        for layout in &regions {
            if let Some(region) = scene.get_mut(layout.id) {
                region.computed = ComputedLayout {
                    x: layout.x,
                    y: layout.y,
                    width: layout.width,
                    height: layout.height,
                    content_x: layout.content_x,
                    content_y: layout.content_y,
                    content_width: layout.content_width,
                    content_height: layout.content_height,
                };
                region.dirty = false;
            }
        }

        Ok(regions)
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
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
    fn layout_keeps_reparented_child_when_old_parent_is_removed() {
        let mut scene = SceneTree::new();

        let mut parent = Region::new(RegionId::new(1));
        parent.parent = Some(RegionId::ROOT);
        scene.insert(parent);

        let mut child = Region::new(RegionId::new(2));
        child.parent = Some(RegionId::new(1));
        scene.insert(child);

        let mut engine = LayoutEngine::new();
        engine.sync(&scene).unwrap();

        scene.reparent(RegionId::new(2), RegionId::ROOT, 0).unwrap();
        scene.remove(RegionId::new(1));

        engine.sync(&scene).unwrap();

        assert!(engine.get_layout(RegionId::new(1)).is_none());
        assert!(engine.get_layout(RegionId::new(2)).is_some());
    }

    #[test]
    fn layout_tolerates_duplicate_child_references() {
        let mut scene = SceneTree::new();

        let mut first_child = Region::new(RegionId::new(1));
        first_child.parent = Some(RegionId::ROOT);
        scene.insert(first_child);

        let mut second_child = Region::new(RegionId::new(1));
        second_child.parent = Some(RegionId::ROOT);
        scene.insert(second_child);

        let mut engine = LayoutEngine::new();
        engine.sync(&scene).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        assert!(engine.get_layout(RegionId::new(1)).is_some());
    }

    #[test]
    fn layout_removes_stale_parent_and_child_nodes() {
        let mut scene = SceneTree::new();

        let mut parent = Region::new(RegionId::new(1));
        parent.parent = Some(RegionId::ROOT);
        scene.insert(parent);

        let mut child = Region::new(RegionId::new(2));
        child.parent = Some(RegionId::new(1));
        scene.insert(child);

        let mut engine = LayoutEngine::new();
        engine.sync(&scene).unwrap();

        scene.remove(RegionId::new(1));
        engine.sync(&scene).unwrap();

        assert!(engine.get_layout(RegionId::new(1)).is_none());
        assert!(engine.get_layout(RegionId::new(2)).is_none());
    }

    #[test]
    fn layout_reparent_then_delete_across_syncs_does_not_panic() {
        // Regression: set_children in depth-first order used to clear the
        // taffy parent pointer of a child that had already been re-attached
        // to an earlier sibling, leaving a dangling child reference that
        // panicked on the next sync.
        let mut scene = SceneTree::new();

        let mut q = Region::new(RegionId::new(1));
        q.parent = Some(RegionId::ROOT);
        scene.insert(q);

        let mut p = Region::new(RegionId::new(2));
        p.parent = Some(RegionId::ROOT);
        scene.insert(p);

        let mut b = Region::new(RegionId::new(3));
        b.parent = Some(RegionId::new(2));
        scene.insert(b);

        let mut engine = LayoutEngine::new();
        engine.sync(&scene).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        // Move B from P (later sibling) to Q (earlier sibling).
        scene
            .reparent(RegionId::new(3), RegionId::new(1), 0)
            .unwrap();
        engine.sync(&scene).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        // Delete B, then sync again: must not panic and Q must have no children.
        scene.remove(RegionId::new(3));
        engine.sync(&scene).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        assert!(engine.get_layout(RegionId::new(3)).is_none());
        let q_node = engine.region_to_node[&RegionId::new(1)];
        assert!(engine.taffy.children(q_node).unwrap().is_empty());
        assert_eq!(engine.node_count(), 3, "root, Q and P remain");
    }

    #[test]
    fn layout_engine_survives_create_delete_recreate_cycles() {
        let mut scene = build_scene();
        let mut engine = LayoutEngine::new();

        for cycle in 0..5 {
            // Add a nested pair under the body region, lay out, then delete
            // the parent (cascading) and lay out again with the same engine.
            let mut panel = Region::new(RegionId::new(100 + cycle));
            panel.parent = Some(RegionId::new(2));
            panel.constraints = Constraints {
                height: ProtoDimension::Cells(5.0),
                width: ProtoDimension::Percent(100.0),
                ..Default::default()
            };
            scene.insert(panel);

            let mut inner = Region::new(RegionId::new(200 + cycle));
            inner.parent = Some(RegionId::new(100 + cycle));
            inner.constraints = Constraints {
                height: ProtoDimension::Cells(2.0),
                width: ProtoDimension::Percent(100.0),
                ..Default::default()
            };
            scene.insert(inner);

            let layouts = engine.compute_scene_layout(&mut scene, 80.0, 24.0).unwrap();
            let panel_layout = layouts
                .iter()
                .find(|l| l.id == RegionId::new(100 + cycle))
                .expect("panel laid out");
            assert_eq!(panel_layout.height, 5.0);
            assert_eq!(panel_layout.y, 3.0, "panel sits below the 3-row header");
            let inner_layout = layouts
                .iter()
                .find(|l| l.id == RegionId::new(200 + cycle))
                .expect("inner laid out");
            assert_eq!(inner_layout.height, 2.0);

            scene.remove(RegionId::new(100 + cycle));
            let layouts = engine.compute_scene_layout(&mut scene, 80.0, 24.0).unwrap();
            assert!(layouts.iter().all(|l| l.id.0 < 100));
            let body = layouts
                .iter()
                .find(|l| l.id == RegionId::new(2))
                .expect("body laid out");
            assert_eq!(body.height, 21.0);
        }

        // Re-create an id that was deleted earlier, under a different parent.
        let mut revived = Region::new(RegionId::new(100));
        revived.parent = Some(RegionId::new(1));
        revived.constraints = Constraints {
            height: ProtoDimension::Cells(1.0),
            width: ProtoDimension::Cells(10.0),
            ..Default::default()
        };
        scene.insert(revived);
        let layouts = engine.compute_scene_layout(&mut scene, 80.0, 24.0).unwrap();
        let revived_layout = layouts
            .iter()
            .find(|l| l.id == RegionId::new(100))
            .expect("revived laid out");
        assert_eq!(revived_layout.width, 10.0);
        assert_eq!(revived_layout.y, 0.0, "revived region lives in the header");
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

    #[test]
    fn compute_scene_layout_returns_absolute_layouts_and_updates_scene() {
        let mut tree = build_scene();

        let mut nested = Region::new(RegionId::new(3));
        nested.parent = Some(RegionId::new(2));
        nested.constraints = Constraints {
            width: ProtoDimension::Cells(10.0),
            height: ProtoDimension::Cells(5.0),
            ..Default::default()
        };
        tree.insert(nested);

        let mut engine = LayoutEngine::new();
        let layouts = engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();

        let body = layouts
            .iter()
            .find(|layout| layout.id == RegionId::new(2))
            .unwrap();
        assert_eq!(body.y, 3.0);
        assert_eq!(body.height, 21.0);

        let nested = layouts
            .iter()
            .find(|layout| layout.id == RegionId::new(3))
            .unwrap();
        assert_eq!(nested.y, 3.0);
        assert_eq!(nested.width, 10.0);
        assert_eq!(nested.height, 5.0);

        let nested_region = tree.get(RegionId::new(3)).unwrap();
        assert_eq!(nested_region.computed.y, 3.0);
        assert_eq!(nested_region.computed.width, 10.0);
        assert!(!nested_region.dirty);
    }

    #[test]
    fn compute_scene_layout_removes_stale_engine_nodes() {
        let mut tree = build_scene();
        let mut engine = LayoutEngine::new();
        let layouts = engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();
        assert!(layouts.iter().any(|layout| layout.id == RegionId::new(1)));

        tree.remove(RegionId::new(1));
        let layouts = engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();

        assert!(layouts.iter().all(|layout| layout.id != RegionId::new(1)));
        assert!(engine.get_layout(RegionId::new(1)).is_none());
    }

    /// `build_scene` plus surface 10, three rows high, whose own size
    /// constraints ask for something else, holding a growing child.
    fn build_scene_with_surface() -> SceneTree {
        let mut tree = build_scene();
        let mut surface = Region::new(RegionId::new(10));
        surface.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(7.0),
            height: ProtoDimension::Cells(50.0),
            min_height: ProtoDimension::Cells(40.0),
            // A margin would move a root inside its space.
            margin: ultty_protocol::Edges::all(ProtoDimension::Cells(1.0)),
            padding: ultty_protocol::Edges::all(ProtoDimension::Cells(1.0)),
            ..Default::default()
        };
        tree.try_insert_surface(surface, 3).unwrap();
        let mut body = Region::new(RegionId::new(11));
        body.parent = Some(RegionId::new(10));
        body.constraints = Constraints {
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            // Default margins are Auto, which would absorb free space.
            margin: ultty_protocol::Edges::all(ProtoDimension::Cells(0.0)),
            ..Default::default()
        };
        tree.insert(body);
        tree
    }

    fn layout_of(layouts: &[ComputedRegionLayout], id: u32) -> ComputedRegionLayout {
        *layouts
            .iter()
            .find(|layout| layout.id == RegionId::new(id))
            .unwrap()
    }

    #[test]
    fn surfaces_are_laid_out_at_cols_by_rows_apart_from_root() {
        let mut tree = build_scene_with_surface();
        let mut engine = LayoutEngine::new();
        let layouts = engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();

        let ids: Vec<u32> = layouts.iter().map(|layout| layout.id.0).collect();
        assert_eq!(ids, [0, 1, 2, 10, 11], "ROOT tree first, then the surface");

        let surface = layout_of(&layouts, 10);
        assert_eq!(
            (surface.x, surface.y, surface.width, surface.height),
            (0.0, 0.0, 80.0, 3.0),
            "the surface ignores its width and height constraints"
        );
        let body = layout_of(&layouts, 11);
        assert_eq!(
            (body.x, body.y, body.width, body.height),
            (1.0, 1.0, 78.0, 1.0),
            "children are placed from the surface's own origin"
        );

        // The full-scene tree is exactly what it is without the surface.
        let mut plain = build_scene();
        let plain_layouts = LayoutEngine::new()
            .compute_scene_layout(&mut plain, 80.0, 24.0)
            .unwrap();
        assert_eq!(&layouts[..3], &plain_layouts[..]);
    }

    #[test]
    fn surfaces_follow_the_width_and_keep_their_rows_on_resize() {
        let mut tree = build_scene_with_surface();
        let mut engine = LayoutEngine::new();
        engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();
        let layouts = engine.compute_scene_layout(&mut tree, 120.0, 40.0).unwrap();

        let surface = layout_of(&layouts, 10);
        assert_eq!((surface.width, surface.height), (120.0, 3.0));
        assert_eq!(layout_of(&layouts, 11).width, 118.0);
        assert_eq!(tree.get(RegionId::new(11)).unwrap().computed.width, 118.0);
    }

    #[test]
    fn closing_a_surface_drops_its_nodes() {
        let mut tree = build_scene_with_surface();
        let mut engine = LayoutEngine::new();
        engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();
        assert_eq!(engine.node_count(), 5);

        tree.remove(RegionId::new(10));
        let layouts = engine.compute_scene_layout(&mut tree, 80.0, 24.0).unwrap();
        assert_eq!(layouts.len(), 3);
        assert_eq!(engine.node_count(), 3);
        assert!(engine.get_layout(RegionId::new(11)).is_none());
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
            (ProtoJustifyContent::Start, taffy::JustifyContent::START),
            (ProtoJustifyContent::End, taffy::JustifyContent::END),
            (ProtoJustifyContent::Center, taffy::JustifyContent::CENTER),
            (
                ProtoJustifyContent::SpaceBetween,
                taffy::JustifyContent::SPACE_BETWEEN,
            ),
            (
                ProtoJustifyContent::SpaceAround,
                taffy::JustifyContent::SPACE_AROUND,
            ),
            (
                ProtoJustifyContent::SpaceEvenly,
                taffy::JustifyContent::SPACE_EVENLY,
            ),
        ] {
            let c = Constraints {
                justify_content: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.justify_content, expected, "JustifyContent::{proto:?}");
        }
    }

    #[test]
    fn constraints_align_items_variants() {
        for (proto, expected) in [
            (ProtoAlignItems::Start, taffy::AlignItems::START),
            (ProtoAlignItems::End, taffy::AlignItems::END),
            (ProtoAlignItems::Center, taffy::AlignItems::CENTER),
            (ProtoAlignItems::Baseline, taffy::AlignItems::BASELINE),
            (ProtoAlignItems::Stretch, taffy::AlignItems::STRETCH),
        ] {
            let c = Constraints {
                align_items: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.align_items, expected, "AlignItems::{proto:?}");
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
            (ProtoAlignSelf::Start, taffy::AlignSelf::START),
            (ProtoAlignSelf::End, taffy::AlignSelf::END),
            (ProtoAlignSelf::Center, taffy::AlignSelf::CENTER),
            (ProtoAlignSelf::Baseline, taffy::AlignSelf::BASELINE),
            (ProtoAlignSelf::Stretch, taffy::AlignSelf::STRETCH),
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
            (ProtoAlignContent::Start, taffy::AlignContent::START),
            (ProtoAlignContent::End, taffy::AlignContent::END),
            (ProtoAlignContent::Center, taffy::AlignContent::CENTER),
            (
                ProtoAlignContent::SpaceBetween,
                taffy::AlignContent::SPACE_BETWEEN,
            ),
            (
                ProtoAlignContent::SpaceAround,
                taffy::AlignContent::SPACE_AROUND,
            ),
            (
                ProtoAlignContent::SpaceEvenly,
                taffy::AlignContent::SPACE_EVENLY,
            ),
            (ProtoAlignContent::Stretch, taffy::AlignContent::STRETCH),
        ] {
            let c = Constraints {
                align_content: proto,
                ..default_constraints()
            };
            let style = constraints_to_style(&c);
            assert_eq!(style.align_content, expected, "AlignContent::{proto:?}");
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

        assert_eq!(style.margin.top, LengthPercentageAuto::length(1.0));
        assert_eq!(style.margin.right, LengthPercentageAuto::length(2.0));
        assert_eq!(style.margin.bottom, LengthPercentageAuto::length(3.0));
        assert_eq!(style.margin.left, LengthPercentageAuto::auto());

        assert_eq!(style.padding.left, LengthPercentage::percent(0.1));
        assert_eq!(style.padding.right, LengthPercentage::percent(0.1));

        assert_eq!(style.gap.width, LengthPercentage::length(5.0));
        assert_eq!(style.gap.height, LengthPercentage::percent(0.2));
    }

    // ── dimension_to_taffy tests ────────────────────────────────────

    #[test]
    fn dimension_to_taffy_auto_and_cells() {
        assert_eq!(dimension_to_taffy(&ProtoDimension::Auto), Dimension::auto());
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Cells(10.0)),
            Dimension::length(10.0)
        );
    }

    #[test]
    fn dimension_to_taffy_percent_divides_by_100() {
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Percent(50.0)),
            Dimension::percent(0.5)
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Percent(100.0)),
            Dimension::percent(1.0)
        );
    }

    #[test]
    fn dimension_to_taffy_fr_mincontent_maxcontent_fallback_to_auto() {
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Fr(1.0)),
            Dimension::auto()
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::MinContent),
            Dimension::auto()
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::MaxContent),
            Dimension::auto()
        );
    }

    #[test]
    fn dimension_to_taffy_viewport_units() {
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Vw(50.0)),
            Dimension::percent(0.5)
        );
        assert_eq!(
            dimension_to_taffy(&ProtoDimension::Vh(50.0)),
            Dimension::percent(0.5)
        );
    }

    // ── dimension_to_length_auto tests ──────────────────────────────

    #[test]
    fn length_auto_basic_variants() {
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Auto),
            LengthPercentageAuto::auto()
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Cells(5.0)),
            LengthPercentageAuto::length(5.0)
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Percent(25.0)),
            LengthPercentageAuto::percent(0.25)
        );
    }

    #[test]
    fn length_auto_catch_all_returns_auto() {
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Fr(1.0)),
            LengthPercentageAuto::auto()
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Vw(50.0)),
            LengthPercentageAuto::auto()
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::Vh(50.0)),
            LengthPercentageAuto::auto()
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::MinContent),
            LengthPercentageAuto::auto()
        );
        assert_eq!(
            dimension_to_length_auto(&ProtoDimension::MaxContent),
            LengthPercentageAuto::auto()
        );
    }

    // ── dimension_to_length_pct tests ───────────────────────────────

    #[test]
    fn length_pct_cells_and_percent() {
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Cells(5.0)),
            LengthPercentage::length(5.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Percent(50.0)),
            LengthPercentage::percent(0.5)
        );
    }

    #[test]
    fn length_pct_catch_all_returns_zero_length() {
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Auto),
            LengthPercentage::length(0.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Fr(1.0)),
            LengthPercentage::length(0.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Vw(50.0)),
            LengthPercentage::length(0.0)
        );
        assert_eq!(
            dimension_to_length_pct(&ProtoDimension::Vh(50.0)),
            LengthPercentage::length(0.0)
        );
    }

    /// Replicate chat app hierarchy:
    /// Column (flex, column, flex_grow:1, padding:1, gap:1)
    ///   Title (flex, width:Cells(10), height:Cells(1))
    ///   Divider (flex, height:Cells(1))
    ///   ScrollView (flex, column, flex_grow:1, overflow.y:Scroll)
    ///     Column (flex, column, flex_grow:1, gap:0.5)
    ///       RichText (flex, width:Percent(100), height:Cells(1))
    ///   Divider (flex, height:Cells(1))
    ///   Text (flex, height:Cells(1))
    ///   Muted (flex, width:Cells(24), height:Cells(1))
    #[test]
    fn layout_chat_scrollview_hierarchy() {
        let mut tree = SceneTree::new();

        // Root
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(37.0),
            ..Default::default()
        };

        // Top-level Column (id=1)
        let mut col = Region::new(RegionId::new(1));
        col.parent = Some(RegionId::ROOT);
        col.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            padding: Edges::all(ProtoDimension::Cells(1.0)),
            gap: ProtoSize {
                width: ProtoDimension::Auto,
                height: ProtoDimension::Cells(1.0),
            },
            ..Default::default()
        };
        tree.insert(col);

        // Title (id=2)
        let mut title = Region::new(RegionId::new(2));
        title.parent = Some(RegionId::new(1));
        title.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Cells(10.0),
            height: ProtoDimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(title);

        // Divider 1 (id=3)
        let mut div1 = Region::new(RegionId::new(3));
        div1.parent = Some(RegionId::new(1));
        div1.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        };
        tree.insert(div1);

        // ScrollView (id=4)
        let mut scroll = Region::new(RegionId::new(4));
        scroll.parent = Some(RegionId::new(1));
        scroll.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Visible,
                y: ProtoOverflowAxis::Scroll,
            },
            ..Default::default()
        };
        tree.insert(scroll);

        // ScrollView child Column (id=5)
        let mut scroll_col = Region::new(RegionId::new(5));
        scroll_col.parent = Some(RegionId::new(4));
        scroll_col.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            gap: ProtoSize {
                width: ProtoDimension::Auto,
                height: ProtoDimension::Cells(0.5),
            },
            ..Default::default()
        };
        tree.insert(scroll_col);

        // RichText inside ScrollView (id=6)
        let mut richtext = Region::new(RegionId::new(6));
        richtext.parent = Some(RegionId::new(5));
        richtext.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        };
        tree.insert(richtext);

        // Divider 2 (id=7)
        let mut div2 = Region::new(RegionId::new(7));
        div2.parent = Some(RegionId::new(1));
        div2.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        };
        tree.insert(div2);

        // Text input (id=8)
        let mut text_input = Region::new(RegionId::new(8));
        text_input.parent = Some(RegionId::new(1));
        text_input.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        };
        tree.insert(text_input);

        // Muted text (id=9)
        let mut muted = Region::new(RegionId::new(9));
        muted.parent = Some(RegionId::new(1));
        muted.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Cells(24.0),
            height: ProtoDimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(muted);

        // Compute layout
        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 37.0).unwrap();

        for id_val in 1..=9 {
            let layout = engine.get_layout(RegionId::new(id_val)).unwrap();
            assert!(layout.size.width > 0.0, "Region {} has zero width", id_val);
            assert!(
                layout.size.height > 0.0,
                "Region {} has zero height",
                id_val
            );
        }

        // Specifically check the RichText inside ScrollView
        let richtext_layout = engine.get_layout(RegionId::new(6)).unwrap();
        assert!(
            richtext_layout.size.width > 0.0,
            "RichText width is zero: {:?}",
            richtext_layout
        );
        assert_eq!(
            richtext_layout.size.height, 1.0,
            "RichText height should be 1.0: {:?}",
            richtext_layout
        );

        // Check ScrollView has non-zero size
        let scroll_layout = engine.get_layout(RegionId::new(4)).unwrap();
        assert!(
            scroll_layout.size.height > 1.0,
            "ScrollView should have significant height: {:?}",
            scroll_layout
        );
    }

    /// Test that a chat layout with many messages keeps the input pinned.
    ///
    /// Layout:
    ///   Column (flex_grow:1, flex_shrink:1, overflow_y:hidden, 80x30)
    ///     ScrollView (flex_grow:1, flex_shrink:1, min_height:0, overflow_y:scroll)
    ///       Column (100 items, each 3 cells tall = 300 cells total)
    ///     Divider (height:1)
    ///     Input (height:5)
    ///     Divider (height:1)
    ///     StatusBar (height:1)
    #[test]
    fn layout_chat_input_stays_pinned_with_overflow() {
        let mut tree = SceneTree::new();

        // Root: 80x30 viewport
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(30.0),
            ..Default::default()
        };

        // chat_area Column (id=1)
        let mut chat_col = Region::new(RegionId::new(1));
        chat_col.parent = Some(RegionId::ROOT);
        chat_col.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            width: ProtoDimension::Percent(100.0),
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Visible,
                y: ProtoOverflowAxis::Hidden,
            },
            ..Default::default()
        };
        tree.insert(chat_col);

        // ScrollView (id=2) — flex_grow:1, flex_shrink:1, min_height:0, overflow_y:scroll
        let mut scroll = Region::new(RegionId::new(2));
        scroll.parent = Some(RegionId::new(1));
        scroll.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: ProtoDimension::Cells(0.0),
            width: ProtoDimension::Percent(100.0),
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Hidden,
                y: ProtoOverflowAxis::Scroll,
            },
            ..Default::default()
        };
        tree.insert(scroll);

        // ScrollView content: Column with many items (id=3)
        let mut content_col = Region::new(RegionId::new(3));
        content_col.parent = Some(RegionId::new(2));
        content_col.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(content_col);

        // 100 message items, each 3 cells tall (ids 100..199)
        for i in 0..100 {
            let mut msg = Region::new(RegionId::new(100 + i));
            msg.parent = Some(RegionId::new(3));
            msg.constraints = Constraints {
                display: ProtoDisplay::Flex,
                width: ProtoDimension::Percent(100.0),
                height: ProtoDimension::Cells(3.0),
                ..Default::default()
            };
            tree.insert(msg);
        }

        // Divider 1 (id=4)
        let mut div1 = Region::new(RegionId::new(4));
        div1.parent = Some(RegionId::new(1));
        div1.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(div1);

        // Input area (id=5)
        let mut input = Region::new(RegionId::new(5));
        input.parent = Some(RegionId::new(1));
        input.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(5.0),
            ..Default::default()
        };
        tree.insert(input);

        // Divider 2 (id=6)
        let mut div2 = Region::new(RegionId::new(6));
        div2.parent = Some(RegionId::new(1));
        div2.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(div2);

        // Status bar (id=7)
        let mut status = Region::new(RegionId::new(7));
        status.parent = Some(RegionId::new(1));
        status.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(status);

        // Compute layout
        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 30.0).unwrap();

        let chat_layout = engine.get_layout(RegionId::new(1)).unwrap();
        let scroll_layout = engine.get_layout(RegionId::new(2)).unwrap();
        let div1_layout = engine.get_layout(RegionId::new(4)).unwrap();
        let input_layout = engine.get_layout(RegionId::new(5)).unwrap();
        let div2_layout = engine.get_layout(RegionId::new(6)).unwrap();
        let status_layout = engine.get_layout(RegionId::new(7)).unwrap();

        // The chat_area should be 30 cells tall (fills root)
        assert_eq!(chat_layout.size.height, 30.0, "chat_area height");

        // Fixed items total: 1 + 5 + 1 + 1 = 8 cells
        // ScrollView should get the remaining: 30 - 8 = 22 cells
        assert_eq!(
            scroll_layout.size.height, 22.0,
            "ScrollView height (30 - 8 fixed)"
        );

        // Input should be at y = scroll_height + div1 = 22 + 1 = 23
        assert_eq!(div1_layout.location.y, 22.0, "Divider 1 y position");
        assert_eq!(input_layout.location.y, 23.0, "Input y position");
        assert_eq!(div2_layout.location.y, 28.0, "Divider 2 y position");
        assert_eq!(status_layout.location.y, 29.0, "Status bar y position");

        // All items must be within the 30-cell viewport
        let status_bottom = status_layout.location.y + status_layout.size.height;
        assert!(
            status_bottom <= 30.0,
            "Status bar bottom ({}) exceeds viewport (30)",
            status_bottom
        );
    }

    /// Replicate DockLayout structure with center_content wrapping ScrollView + bottom_bar:
    ///
    /// Stack (100% x 100%)
    ///   DockLayout Column (100% x 100%, flex_grow:1, overflow:hidden)
    ///     Header Row (h:1, flex_shrink:0)
    ///     Middle Row (flex_grow:1, flex_shrink:1, min_height:0, align_items:Stretch)
    ///       center_content Column (flex_grow:1, flex_shrink:1, min_height:0, overflow_y:hidden)
    ///         ScrollView (flex_grow:1, flex_shrink:1, min_height:0, overflow_y:scroll)
    ///           messages Column (100 items x 3 cells = 300 cells total)
    ///         bottom_bar Column (flex_shrink:0)
    ///           Divider (h:1)
    ///           Input (h:5)
    ///           Divider (h:1)
    ///           StatusBar (h:1)
    #[test]
    fn layout_docklayout_center_with_scrollview_and_bottombar() {
        let mut tree = SceneTree::new();

        // Stack root: 66x30
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };

        // DockLayout Column (id=1)
        let mut dock = Region::new(RegionId::new(1));
        dock.parent = Some(RegionId::ROOT);
        dock.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Percent(100.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Hidden,
                y: ProtoOverflowAxis::Hidden,
            },
            ..Default::default()
        };
        tree.insert(dock);

        // Header (id=2)
        let mut header = Region::new(RegionId::new(2));
        header.parent = Some(RegionId::new(1));
        header.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Row,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        };
        tree.insert(header);

        // Middle Row (id=3)
        let mut middle = Region::new(RegionId::new(3));
        middle.parent = Some(RegionId::new(1));
        middle.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Row,
            width: ProtoDimension::Percent(100.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: ProtoDimension::Cells(0.0),
            align_items: ProtoAlignItems::Stretch,
            ..Default::default()
        };
        tree.insert(middle);

        // center_content Column (id=4) — matches DockLayout center override
        let mut center = Region::new(RegionId::new(4));
        center.parent = Some(RegionId::new(3));
        center.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Auto,
            height: ProtoDimension::Percent(100.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: ProtoDimension::Cells(0.0),
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Visible,
                y: ProtoOverflowAxis::Hidden,
            },
            ..Default::default()
        };
        tree.insert(center);

        // ScrollView (id=5)
        let mut scroll = Region::new(RegionId::new(5));
        scroll.parent = Some(RegionId::new(4));
        scroll.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: ProtoDimension::Cells(0.0),
            width: ProtoDimension::Percent(100.0),
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Hidden,
                y: ProtoOverflowAxis::Scroll,
            },
            ..Default::default()
        };
        tree.insert(scroll);

        // Messages column (id=6) inside ScrollView
        let mut msgs = Region::new(RegionId::new(6));
        msgs.parent = Some(RegionId::new(5));
        msgs.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(msgs);

        // 100 message items (ids 100..199)
        for i in 0..100 {
            let mut msg = Region::new(RegionId::new(100 + i));
            msg.parent = Some(RegionId::new(6));
            msg.constraints = Constraints {
                display: ProtoDisplay::Flex,
                width: ProtoDimension::Percent(100.0),
                height: ProtoDimension::Cells(3.0),
                ..Default::default()
            };
            tree.insert(msg);
        }

        // bottom_bar Column (id=7)
        let mut bar = Region::new(RegionId::new(7));
        bar.parent = Some(RegionId::new(4));
        bar.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_shrink: 0.0,
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(bar);

        // Divider 1 (id=8)
        let mut d1 = Region::new(RegionId::new(8));
        d1.parent = Some(RegionId::new(7));
        d1.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        };
        tree.insert(d1);

        // Input (id=9)
        let mut input = Region::new(RegionId::new(9));
        input.parent = Some(RegionId::new(7));
        input.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(5.0),
            ..Default::default()
        };
        tree.insert(input);

        // Divider 2 (id=10)
        let mut d2 = Region::new(RegionId::new(10));
        d2.parent = Some(RegionId::new(7));
        d2.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            flex_shrink: 1.0,
            ..Default::default()
        };
        tree.insert(d2);

        // StatusBar (id=11)
        let mut status = Region::new(RegionId::new(11));
        status.parent = Some(RegionId::new(7));
        status.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(status);

        // Compute
        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(66.0, 30.0).unwrap();

        let _dock_layout = engine.get_layout(RegionId::new(1)).unwrap();
        let header_layout = engine.get_layout(RegionId::new(2)).unwrap();
        let middle_layout = engine.get_layout(RegionId::new(3)).unwrap();
        let center_layout = engine.get_layout(RegionId::new(4)).unwrap();
        let scroll_layout = engine.get_layout(RegionId::new(5)).unwrap();
        let bar_layout = engine.get_layout(RegionId::new(7)).unwrap();
        let _input_layout = engine.get_layout(RegionId::new(9)).unwrap();
        let _status_layout = engine.get_layout(RegionId::new(11)).unwrap();

        // Header should be 1 cell tall
        assert_eq!(header_layout.size.height, 1.0, "header height");

        // Middle Row should fill remaining space: 30 - 1 = 29
        assert_eq!(middle_layout.size.height, 29.0, "middle row height");

        // center_content should stretch to middle height
        assert_eq!(center_layout.size.height, 29.0, "center height");

        // bottom_bar should be 8 cells (1+5+1+1)
        assert_eq!(bar_layout.size.height, 8.0, "bottom_bar height");

        // ScrollView should be 29 - 8 = 21
        assert_eq!(scroll_layout.size.height, 21.0, "scroll height");

        // bottom_bar should be within viewport
        let bar_abs_y = bar_layout.location.y;
        assert!(
            bar_abs_y + bar_layout.size.height <= 29.0,
            "bar bottom {} exceeds center {}",
            bar_abs_y + bar_layout.size.height,
            29.0
        );
    }

    #[test]
    fn layout_scrollview_auto_height_children_stay_at_top() {
        let mut tree = SceneTree::new();

        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };

        let mut scroll = Region::new(RegionId::new(1));
        scroll.parent = Some(RegionId::ROOT);
        scroll.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: ProtoDimension::Cells(0.0),
            width: ProtoDimension::Percent(100.0),
            overflow: ProtoOverflow {
                x: ProtoOverflowAxis::Hidden,
                y: ProtoOverflowAxis::Scroll,
            },
            ..Default::default()
        };
        tree.insert(scroll);

        let mut messages = Region::new(RegionId::new(2));
        messages.parent = Some(RegionId::new(1));
        messages.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Percent(100.0),
            padding: Edges {
                top: ProtoDimension::Cells(1.0),
                right: ProtoDimension::Cells(1.0),
                bottom: ProtoDimension::Cells(1.0),
                left: ProtoDimension::Cells(1.0),
            },
            gap: ProtoSize {
                width: ProtoDimension::Cells(0.0),
                height: ProtoDimension::Cells(0.5),
            },
            ..Default::default()
        };
        tree.insert(messages);

        for i in 0..2 {
            let mut msg = Region::new(RegionId::new(10 + i));
            msg.parent = Some(RegionId::new(2));
            msg.constraints = Constraints {
                display: ProtoDisplay::Flex,
                flex_direction: ProtoFlexDirection::Column,
                width: ProtoDimension::Percent(100.0),
                min_height: ProtoDimension::Cells(4.0),
                padding: Edges {
                    top: ProtoDimension::Cells(1.0),
                    right: ProtoDimension::Cells(1.0),
                    bottom: ProtoDimension::Cells(1.0),
                    left: ProtoDimension::Cells(1.0),
                },
                ..Default::default()
            };
            tree.insert(msg);
        }

        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(120.0, 40.0).unwrap();

        let scroll_layout = engine.get_layout(RegionId::new(1)).unwrap();
        let first_message_layout = engine.get_layout(RegionId::new(10)).unwrap();
        let second_message_layout = engine.get_layout(RegionId::new(11)).unwrap();

        assert_eq!(scroll_layout.location.y, 0.0, "scroll starts at root");
        assert_eq!(
            first_message_layout.location.y, 1.0,
            "first message should sit below container padding; first={:?} second={:?}",
            first_message_layout, second_message_layout
        );
        assert!(
            second_message_layout.location.y <= 6.0,
            "second message should follow the first without a viewport-sized gap: {:?}",
            second_message_layout
        );
    }

    // ── Regression tests for documented layout bugs ─────────────────

    /// Regression: DockLayout middle Row needs AlignItems::Stretch AND children
    /// need height:Percent(100) to fill the cross-axis. Taffy's align-items:stretch
    /// alone doesn't give definite cross-axis height to flex container children
    /// that have no intrinsic content.
    #[test]
    fn layout_row_align_items_stretch_fills_cross_axis() {
        let mut tree = SceneTree::new();
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(24.0),
            ..Default::default()
        };

        // Header (fixed)
        let mut header = Region::new(RegionId::new(1));
        header.parent = Some(RegionId::ROOT);
        header.constraints = Constraints {
            display: ProtoDisplay::Flex,
            height: ProtoDimension::Cells(1.0),
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(header);

        // Middle Row with AlignItems::Stretch
        let mut middle = Region::new(RegionId::new(2));
        middle.parent = Some(RegionId::ROOT);
        middle.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Row,
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            align_items: ProtoAlignItems::Stretch,
            ..Default::default()
        };
        tree.insert(middle);

        // Sidebar with explicit height:100% — stretches to row height
        let mut sidebar = Region::new(RegionId::new(3));
        sidebar.parent = Some(RegionId::new(2));
        sidebar.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Cells(20.0),
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(sidebar);

        // Main with explicit height:100% + flex_grow — stretches to row height
        let mut main = Region::new(RegionId::new(4));
        main.parent = Some(RegionId::new(2));
        main.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_grow: 1.0,
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(main);

        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        let middle_layout = engine.get_layout(RegionId::new(2)).unwrap();
        let sidebar_layout = engine.get_layout(RegionId::new(3)).unwrap();
        let main_layout = engine.get_layout(RegionId::new(4)).unwrap();

        assert_eq!(middle_layout.size.height, 23.0, "middle row = 24 - 1");
        assert_eq!(
            sidebar_layout.size.height, 23.0,
            "sidebar with height:100% should fill row height"
        );
        assert_eq!(
            main_layout.size.height, 23.0,
            "main with height:100% should fill row height"
        );

        // Without height:100%, AlignItems::Start children get zero height
        let sidebar = tree.get_mut(RegionId::new(3)).unwrap();
        sidebar.constraints.height = ProtoDimension::Auto;
        let middle = tree.get_mut(RegionId::new(2)).unwrap();
        middle.constraints.align_items = ProtoAlignItems::Start;
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        let sidebar_layout = engine.get_layout(RegionId::new(3)).unwrap();
        assert_eq!(
            sidebar_layout.size.height, 0.0,
            "sidebar with AlignItems::Start and no content should not stretch"
        );
    }

    /// Regression: padding + border reduce the content area.
    #[test]
    fn layout_padding_reduces_available_space() {
        let mut tree = SceneTree::new();
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(24.0),
            padding: Edges {
                top: ProtoDimension::Cells(2.0),
                right: ProtoDimension::Cells(2.0),
                bottom: ProtoDimension::Cells(2.0),
                left: ProtoDimension::Cells(2.0),
            },
            ..Default::default()
        };

        let mut child = Region::new(RegionId::new(1));
        child.parent = Some(RegionId::ROOT);
        child.constraints = Constraints {
            display: ProtoDisplay::Flex,
            width: ProtoDimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        };
        tree.insert(child);

        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 24.0).unwrap();

        let child_layout = engine.get_layout(RegionId::new(1)).unwrap();
        // 80 - 2 - 2 = 76 width, 24 - 2 - 2 = 20 height
        assert_eq!(child_layout.size.width, 76.0, "child width after padding");
        assert_eq!(child_layout.size.height, 20.0, "child height after padding");
        assert_eq!(child_layout.location.x, 2.0, "child x offset from padding");
        assert_eq!(child_layout.location.y, 2.0, "child y offset from padding");
    }

    /// Regression: multiple children with different flex_grow should get proportional space.
    #[test]
    fn layout_flex_grow_proportional_allocation() {
        let mut tree = SceneTree::new();
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(30.0),
            ..Default::default()
        };

        // Child A: flex_grow: 1
        let mut a = Region::new(RegionId::new(1));
        a.parent = Some(RegionId::ROOT);
        a.constraints = Constraints {
            flex_grow: 1.0,
            ..Default::default()
        };
        tree.insert(a);

        // Child B: flex_grow: 2
        let mut b = Region::new(RegionId::new(2));
        b.parent = Some(RegionId::ROOT);
        b.constraints = Constraints {
            flex_grow: 2.0,
            ..Default::default()
        };
        tree.insert(b);

        // Child C: flex_grow: 1
        let mut c = Region::new(RegionId::new(3));
        c.parent = Some(RegionId::ROOT);
        c.constraints = Constraints {
            flex_grow: 1.0,
            ..Default::default()
        };
        tree.insert(c);

        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 30.0).unwrap();

        let a_layout = engine.get_layout(RegionId::new(1)).unwrap();
        let b_layout = engine.get_layout(RegionId::new(2)).unwrap();
        let c_layout = engine.get_layout(RegionId::new(3)).unwrap();

        // Total flex_grow = 4. Taffy rounds to whole cells.
        // A≈7.5→8, B≈15, C≈7.5→7 (or similar rounding).
        let total = a_layout.size.height + b_layout.size.height + c_layout.size.height;
        assert_eq!(total, 30.0, "children should consume all space");
        assert!(
            (b_layout.size.height - 2.0 * a_layout.size.height).abs() <= 2.0,
            "B (grow:2) should be ~2x A (grow:1): B={}, A={}",
            b_layout.size.height,
            a_layout.size.height
        );
        assert!(
            (a_layout.size.height - c_layout.size.height).abs() <= 1.0,
            "A and C (both grow:1) should be roughly equal: A={}, C={}",
            a_layout.size.height,
            c_layout.size.height
        );
    }

    /// Regression: nested flex containers — inner flex_grow works relative to parent.
    #[test]
    fn layout_nested_flex_grow() {
        let mut tree = SceneTree::new();
        let root = tree.get_mut(RegionId::ROOT).unwrap();
        root.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Column,
            width: ProtoDimension::Cells(80.0),
            height: ProtoDimension::Cells(40.0),
            ..Default::default()
        };

        // Fixed header
        let mut header = Region::new(RegionId::new(1));
        header.parent = Some(RegionId::ROOT);
        header.constraints = Constraints {
            height: ProtoDimension::Cells(5.0),
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(header);

        // Body: flex_grow container
        let mut body = Region::new(RegionId::new(2));
        body.parent = Some(RegionId::ROOT);
        body.constraints = Constraints {
            display: ProtoDisplay::Flex,
            flex_direction: ProtoFlexDirection::Row,
            flex_grow: 1.0,
            width: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(body);

        // Body children: sidebar (fixed) + main (flex_grow)
        let mut sidebar = Region::new(RegionId::new(3));
        sidebar.parent = Some(RegionId::new(2));
        sidebar.constraints = Constraints {
            width: ProtoDimension::Cells(20.0),
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(sidebar);

        let mut main = Region::new(RegionId::new(4));
        main.parent = Some(RegionId::new(2));
        main.constraints = Constraints {
            flex_grow: 1.0,
            height: ProtoDimension::Percent(100.0),
            ..Default::default()
        };
        tree.insert(main);

        let mut engine = LayoutEngine::new();
        engine.sync(&tree).unwrap();
        engine.compute(80.0, 40.0).unwrap();

        let header_layout = engine.get_layout(RegionId::new(1)).unwrap();
        let body_layout = engine.get_layout(RegionId::new(2)).unwrap();
        let sidebar_layout = engine.get_layout(RegionId::new(3)).unwrap();
        let main_layout = engine.get_layout(RegionId::new(4)).unwrap();

        assert_eq!(header_layout.size.height, 5.0);
        assert_eq!(body_layout.size.height, 35.0, "body gets remaining 40-5");
        assert_eq!(sidebar_layout.size.width, 20.0);
        assert_eq!(main_layout.size.width, 60.0, "main gets remaining 80-20");
        assert_eq!(main_layout.size.height, 35.0, "main inherits body height");
    }
}
