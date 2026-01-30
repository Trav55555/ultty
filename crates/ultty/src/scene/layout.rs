//! Taffy integration for layout computation.

use taffy::prelude::*;
use ultty_protocol::RegionId;

use super::tree::SceneTree;

/// Layout engine wrapping Taffy.
pub struct LayoutEngine {
    taffy: TaffyTree,
    /// Mapping from RegionId to Taffy NodeId.
    region_to_node: std::collections::HashMap<RegionId, NodeId>,
}

impl LayoutEngine {
    /// Create a new layout engine.
    pub fn new() -> Self {
        Self {
            taffy: TaffyTree::new(),
            region_to_node: std::collections::HashMap::new(),
        }
    }

    /// Synchronize the Taffy tree with the scene tree.
    pub fn sync(&mut self, _scene: &SceneTree) {
        // TODO: Walk scene tree and create/update Taffy nodes
    }

    /// Compute layout for the given viewport size.
    pub fn compute(&mut self, width: f32, height: f32) {
        if let Some(&root_node) = self.region_to_node.get(&RegionId::ROOT) {
            let size = Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            };
            let _ = self.taffy.compute_layout(root_node, size);
        }
    }

    /// Get computed layout for a region.
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
