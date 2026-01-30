//! Scene tree management.

use std::collections::HashMap;

use ultty_protocol::RegionId;

use super::region::Region;

/// The scene graph tree.
pub struct SceneTree {
    /// All regions by ID.
    regions: HashMap<RegionId, Region>,
    /// Root region ID.
    root: RegionId,
}

impl SceneTree {
    /// Create a new scene tree with a root region.
    pub fn new() -> Self {
        let root_id = RegionId::ROOT;
        let root = Region::new(root_id);
        let mut regions = HashMap::new();
        regions.insert(root_id, root);

        Self {
            regions,
            root: root_id,
        }
    }

    /// Get a region by ID.
    pub fn get(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(&id)
    }

    /// Get a region mutably by ID.
    pub fn get_mut(&mut self, id: RegionId) -> Option<&mut Region> {
        self.regions.get_mut(&id)
    }

    /// Insert a new region.
    pub fn insert(&mut self, region: Region) {
        let id = region.id;
        if let Some(parent_id) = region.parent {
            if let Some(parent) = self.regions.get_mut(&parent_id) {
                parent.children.push(id);
            }
        }
        self.regions.insert(id, region);
    }

    /// Remove a region and its children.
    pub fn remove(&mut self, id: RegionId) -> Option<Region> {
        if id == self.root {
            return None; // Cannot remove root
        }

        if let Some(region) = self.regions.remove(&id) {
            // Remove from parent's children
            if let Some(parent_id) = region.parent {
                if let Some(parent) = self.regions.get_mut(&parent_id) {
                    parent.children.retain(|&child| child != id);
                }
            }

            // Recursively remove children
            for child_id in &region.children {
                self.remove(*child_id);
            }

            Some(region)
        } else {
            None
        }
    }

    /// Get the root region ID.
    pub fn root(&self) -> RegionId {
        self.root
    }

    /// Iterate all regions in depth-first order.
    pub fn iter_depth_first(&self) -> impl Iterator<Item = &Region> {
        DepthFirstIterator::new(self, self.root)
    }
}

impl Default for SceneTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Depth-first iterator over regions.
struct DepthFirstIterator<'a> {
    tree: &'a SceneTree,
    stack: Vec<RegionId>,
}

impl<'a> DepthFirstIterator<'a> {
    fn new(tree: &'a SceneTree, root: RegionId) -> Self {
        Self {
            tree,
            stack: vec![root],
        }
    }
}

impl<'a> Iterator for DepthFirstIterator<'a> {
    type Item = &'a Region;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.stack.pop()?;
        let region = self.tree.get(id)?;

        // Add children in reverse order so they come out in order
        for child_id in region.children.iter().rev() {
            self.stack.push(*child_id);
        }

        Some(region)
    }
}
