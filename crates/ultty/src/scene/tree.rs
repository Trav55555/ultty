//! Scene tree management.

use std::collections::HashMap;

use ultty_protocol::{Content, RegionId, WidgetContent, MAX_REGION_COUNT, MAX_SURFACES};

use super::region::Region;

/// Maximum number of regions (including the root) a scene may hold.
///
/// Bounded by the region count a `LayoutComputed` message can carry, so a
/// full layout reply always stays decodable by the client.
pub const MAX_REGIONS: usize = MAX_REGION_COUNT;

/// Maximum depth of a region below its root (a root is depth 0).
pub const MAX_TREE_DEPTH: usize = 256;

/// Reasons a structural edit to the scene tree is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SceneTreeError {
    #[error("region {} already exists", .0 .0)]
    DuplicateRegion(RegionId),
    #[error("region {} not found", .0 .0)]
    RegionNotFound(RegionId),
    #[error("parent region {} not found", .0 .0)]
    ParentNotFound(RegionId),
    #[error("root region cannot be moved")]
    CannotMoveRoot,
    #[error("region {} cannot be reparented under itself or its descendant {}", id.0, new_parent.0)]
    CyclicReparent { id: RegionId, new_parent: RegionId },
    #[error("scene already holds the maximum of {max} regions")]
    TooManyRegions { max: usize },
    #[error("region would exceed the maximum tree depth of {max}")]
    TooDeep { max: usize },
    #[error("scene already holds the maximum of {max} surfaces")]
    TooManySurfaces { max: usize },
}

/// The root of an inline surface (ADR 0020): a region with no parent,
/// laid out on its own at the viewport's width and `rows` cells high.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceRoot {
    pub id: RegionId,
    pub rows: u16,
}

/// A search match within the scene tree.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// The region containing the match.
    pub region_id: RegionId,
    /// Index of the span within RichText (None for plain text).
    pub span_index: Option<usize>,
    /// Character offset of the match within the text.
    pub char_offset: usize,
    /// Length of the match in characters.
    pub length: usize,
}

/// The scene graph tree.
///
/// It has one full-scene root, [`RegionId::ROOT`], and up to
/// [`MAX_SURFACES`] surface roots. Each root heads a tree of its own;
/// [`SceneTree::iter_depth_first`] walks only the full-scene tree, which is
/// what is drawn over the whole pane.
pub struct SceneTree {
    /// All regions by ID, in every tree.
    regions: HashMap<RegionId, Region>,
    /// Root region ID.
    root: RegionId,
    /// Surface roots, in creation order.
    surfaces: Vec<SurfaceRoot>,
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
            surfaces: Vec::new(),
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

    /// Insert a region, appending to its parent's children.
    ///
    /// Lenient variant for internal construction: a missing parent leaves the
    /// region unlinked, and re-inserting an existing id replaces the region
    /// without ever duplicating a child reference. Protocol-driven inserts go
    /// through [`SceneTree::try_insert_at`], which rejects both cases.
    pub fn insert(&mut self, region: Region) {
        self.insert_at(region, usize::MAX);
    }

    /// Insert a region at a specific index in the parent's children list.
    /// If index is out of bounds, appends to the end. See [`SceneTree::insert`].
    pub fn insert_at(&mut self, region: Region, index: usize) {
        let id = region.id;
        if self.regions.contains_key(&id) {
            self.unlink_from_parent(id);
        }
        if let Some(parent_id) = region.parent {
            if let Some(parent) = self.regions.get_mut(&parent_id) {
                Self::link_child(parent, id, index);
            }
        }
        self.regions.insert(id, region);
    }

    /// Insert a new region under an existing parent, enforcing tree limits.
    ///
    /// Rejects duplicate ids, missing parents, more than [`MAX_REGIONS`]
    /// regions, and nesting deeper than [`MAX_TREE_DEPTH`]. On error the tree
    /// is left untouched.
    pub fn try_insert_at(&mut self, region: Region, index: usize) -> Result<(), SceneTreeError> {
        let id = region.id;
        if self.regions.contains_key(&id) {
            return Err(SceneTreeError::DuplicateRegion(id));
        }
        let Some(parent_id) = region.parent else {
            return Err(SceneTreeError::ParentNotFound(RegionId::ROOT));
        };
        let Some(parent_depth) = self.depth_of(parent_id) else {
            return Err(SceneTreeError::ParentNotFound(parent_id));
        };
        if self.regions.len() >= MAX_REGIONS {
            return Err(SceneTreeError::TooManyRegions { max: MAX_REGIONS });
        }
        if parent_depth + 1 > MAX_TREE_DEPTH {
            return Err(SceneTreeError::TooDeep {
                max: MAX_TREE_DEPTH,
            });
        }
        self.insert_at(region, index);
        Ok(())
    }

    /// Add `region` as the root of a new surface `rows` cells high.
    ///
    /// Rejects duplicate ids, more than [`MAX_REGIONS`] regions and more
    /// than [`MAX_SURFACES`] surfaces. Any parent the region names is
    /// dropped: a surface root has none. On error the tree is left
    /// untouched.
    pub fn try_insert_surface(
        &mut self,
        mut region: Region,
        rows: u16,
    ) -> Result<(), SceneTreeError> {
        let id = region.id;
        if self.regions.contains_key(&id) {
            return Err(SceneTreeError::DuplicateRegion(id));
        }
        if self.surfaces.len() >= MAX_SURFACES {
            return Err(SceneTreeError::TooManySurfaces { max: MAX_SURFACES });
        }
        if self.regions.len() >= MAX_REGIONS {
            return Err(SceneTreeError::TooManyRegions { max: MAX_REGIONS });
        }
        region.parent = None;
        self.regions.insert(id, region);
        self.surfaces.push(SurfaceRoot { id, rows });
        Ok(())
    }

    /// Surface roots, in creation order.
    pub fn surfaces(&self) -> &[SurfaceRoot] {
        &self.surfaces
    }

    /// The surface whose root is `id`, if it is one.
    pub fn surface(&self, id: RegionId) -> Option<SurfaceRoot> {
        self.surfaces
            .iter()
            .copied()
            .find(|surface| surface.id == id)
    }

    /// True for the full-scene root and every surface root.
    fn is_root(&self, id: RegionId) -> bool {
        id == self.root || self.surface(id).is_some()
    }

    /// Remove a region and its entire subtree. Removing a surface root
    /// removes the surface; the full-scene root cannot be removed.
    pub fn remove(&mut self, id: RegionId) -> Option<Region> {
        if id == self.root {
            return None; // Cannot remove root
        }

        let region = self.regions.remove(&id)?;
        self.surfaces.retain(|surface| surface.id != id);
        if let Some(parent_id) = region.parent {
            if let Some(parent) = self.regions.get_mut(&parent_id) {
                parent.children.retain(|&child| child != id);
            }
        }

        // Remove descendants with an explicit worklist; deep chains must not
        // recurse on the stack.
        let mut pending: Vec<RegionId> = region.children.clone();
        while let Some(child_id) = pending.pop() {
            if let Some(child) = self.regions.remove(&child_id) {
                pending.extend(child.children);
            }
        }

        Some(region)
    }

    /// Move an existing region under a new parent.
    ///
    /// If `index` is out of bounds, appends to the end of the new parent's
    /// children list. Rejects moving the root, unknown regions or parents,
    /// moving a region under itself or one of its descendants, and moves that
    /// would nest deeper than [`MAX_TREE_DEPTH`]. On error the tree is left
    /// untouched.
    pub fn reparent(
        &mut self,
        id: RegionId,
        new_parent: RegionId,
        index: usize,
    ) -> Result<(), SceneTreeError> {
        if self.is_root(id) {
            return Err(SceneTreeError::CannotMoveRoot);
        }
        if !self.regions.contains_key(&id) {
            return Err(SceneTreeError::RegionNotFound(id));
        }
        let Some(parent_depth) = self.depth_of(new_parent) else {
            return Err(SceneTreeError::ParentNotFound(new_parent));
        };
        if new_parent == id || self.is_descendant(id, new_parent) {
            return Err(SceneTreeError::CyclicReparent { id, new_parent });
        }
        if parent_depth + 1 + self.subtree_height(id) > MAX_TREE_DEPTH {
            return Err(SceneTreeError::TooDeep {
                max: MAX_TREE_DEPTH,
            });
        }

        self.unlink_from_parent(id);
        if let Some(region) = self.regions.get_mut(&id) {
            region.parent = Some(new_parent);
        }
        if let Some(parent) = self.regions.get_mut(&new_parent) {
            Self::link_child(parent, id, index);
        }
        Ok(())
    }

    /// Get the root region ID.
    pub fn root(&self) -> RegionId {
        self.root
    }

    /// Number of regions in the tree, including the root.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// True when only the root region exists.
    pub fn is_empty(&self) -> bool {
        self.regions.len() <= 1
    }

    /// Depth of a region below its root (a root is 0), or `None` if the
    /// region does not exist or is not attached to a root.
    pub fn depth_of(&self, id: RegionId) -> Option<usize> {
        let mut depth = 0;
        let mut current = id;
        loop {
            if self.is_root(current) {
                return Some(depth);
            }
            current = self.regions.get(&current)?.parent?;
            depth += 1;
            if depth > self.regions.len() {
                // Parent links form a cycle; treat as detached.
                return None;
            }
        }
    }

    /// True if `candidate` is a strict descendant of `ancestor`.
    pub fn is_descendant(&self, ancestor: RegionId, candidate: RegionId) -> bool {
        let mut current = candidate;
        let mut steps = 0;
        while let Some(parent) = self.regions.get(&current).and_then(|r| r.parent) {
            if parent == ancestor {
                return true;
            }
            current = parent;
            steps += 1;
            if steps > self.regions.len() {
                return false;
            }
        }
        false
    }

    /// Height of the subtree rooted at `id` (a leaf has height 0).
    fn subtree_height(&self, id: RegionId) -> usize {
        let mut height = 0;
        let mut pending = vec![(id, 0usize)];
        while let Some((current, depth)) = pending.pop() {
            height = height.max(depth);
            if let Some(region) = self.regions.get(&current) {
                pending.extend(region.children.iter().map(|&child| (child, depth + 1)));
            }
        }
        height
    }

    fn unlink_from_parent(&mut self, id: RegionId) {
        let old_parent = self.regions.get(&id).and_then(|region| region.parent);
        if let Some(old_id) = old_parent {
            if let Some(old) = self.regions.get_mut(&old_id) {
                old.children.retain(|&child| child != id);
            }
        }
    }

    fn link_child(parent: &mut Region, id: RegionId, index: usize) {
        if index <= parent.children.len() {
            parent.children.insert(index, id);
        } else {
            parent.children.push(id);
        }
    }

    /// Iterate the full-scene tree under [`RegionId::ROOT`] in depth-first
    /// (drawing) order. Surfaces are not part of it.
    pub fn iter_depth_first(&self) -> impl Iterator<Item = &Region> {
        DepthFirstIterator::new(self, self.root)
    }

    /// Iterate the tree under `root` (the full-scene root or a surface
    /// root) in depth-first order.
    pub fn iter_tree(&self, root: RegionId) -> impl Iterator<Item = &Region> {
        DepthFirstIterator::new(self, root)
    }

    /// Iterate every attached region: the full-scene tree, then each
    /// surface's tree in creation order.
    pub fn iter_all(&self) -> impl Iterator<Item = &Region> {
        self.iter_depth_first().chain(
            self.surfaces
                .iter()
                .flat_map(|surface| self.iter_tree(surface.id)),
        )
    }

    /// Search all text content in the scene tree for a query string (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<SearchMatch> {
        if query.is_empty() {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let mut matches = Vec::new();

        for region in self.iter_depth_first() {
            match &region.content {
                Content::Text(text_content) => {
                    let text_lower = text_content.text.to_lowercase();
                    for (byte_off, _) in text_lower.match_indices(&query_lower) {
                        let char_offset = text_lower[..byte_off].chars().count();
                        let char_length = query_lower.chars().count();
                        matches.push(SearchMatch {
                            region_id: region.id,
                            span_index: None,
                            char_offset,
                            length: char_length,
                        });
                    }
                }
                Content::RichText(rich_text) => {
                    for (span_idx, span) in rich_text.spans.iter().enumerate() {
                        let text_lower = span.text.to_lowercase();
                        for (byte_off, _) in text_lower.match_indices(&query_lower) {
                            let char_offset = text_lower[..byte_off].chars().count();
                            let char_length = query_lower.chars().count();
                            matches.push(SearchMatch {
                                region_id: region.id,
                                span_index: Some(span_idx),
                                char_offset,
                                length: char_length,
                            });
                        }
                    }
                }
                Content::Widget(widget) => {
                    // Search text-bearing widgets
                    match widget {
                        WidgetContent::Input(input) => {
                            // Search whichever text is actually displayed:
                            // - empty value → placeholder is shown
                            // - password with value → masked bullets, skip search
                            // - normal value → search the value
                            if input.password && !input.value.is_empty() {
                                // Don't expose password content through search
                            } else {
                                let search_text = if input.value.is_empty() {
                                    &input.placeholder
                                } else {
                                    &input.value
                                };
                                let text_lower = search_text.to_lowercase();
                                for (byte_off, _) in text_lower.match_indices(&query_lower) {
                                    let char_offset = text_lower[..byte_off].chars().count();
                                    let char_length = query_lower.chars().count();
                                    matches.push(SearchMatch {
                                        region_id: region.id,
                                        span_index: None,
                                        char_offset,
                                        length: char_length,
                                    });
                                }
                            }
                        }
                        WidgetContent::Markdown(md) => {
                            let text_lower = md.source.to_lowercase();
                            for (byte_off, _) in text_lower.match_indices(&query_lower) {
                                let char_offset = text_lower[..byte_off].chars().count();
                                let char_length = query_lower.chars().count();
                                matches.push(SearchMatch {
                                    region_id: region.id,
                                    span_index: None,
                                    char_offset,
                                    length: char_length,
                                });
                            }
                        }
                        WidgetContent::Code(code) => {
                            let text_lower = code.source.to_lowercase();
                            for (byte_off, _) in text_lower.match_indices(&query_lower) {
                                let char_offset = text_lower[..byte_off].chars().count();
                                let char_length = query_lower.chars().count();
                                matches.push(SearchMatch {
                                    region_id: region.id,
                                    span_index: None,
                                    char_offset,
                                    length: char_length,
                                });
                            }
                        }
                        WidgetContent::Progress(progress) => {
                            if let Some(label) = &progress.label {
                                let text_lower = label.to_lowercase();
                                for (byte_off, _) in text_lower.match_indices(&query_lower) {
                                    let char_offset = text_lower[..byte_off].chars().count();
                                    let char_length = query_lower.chars().count();
                                    matches.push(SearchMatch {
                                        region_id: region.id,
                                        span_index: None,
                                        char_offset,
                                        length: char_length,
                                    });
                                }
                            }
                        }
                        WidgetContent::Spinner(spinner) => {
                            if let Some(label) = &spinner.label {
                                let text_lower = label.to_lowercase();
                                for (byte_off, _) in text_lower.match_indices(&query_lower) {
                                    let char_offset = text_lower[..byte_off].chars().count();
                                    let char_length = query_lower.chars().count();
                                    matches.push(SearchMatch {
                                        region_id: region.id,
                                        span_index: None,
                                        char_offset,
                                        length: char_length,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Content::None => {}
            }
        }

        matches
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

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::{Content, Style, TextContent};

    fn make_tree_with_text() -> SceneTree {
        let mut tree = SceneTree::new();

        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        r1.content = Content::Text(TextContent::new("Hello world"));
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::ROOT);
        r2.content = Content::RichText(
            ultty_protocol::RichText::new()
                .span("Foo bar", Style::default())
                .span("hello again", Style::default()),
        );
        tree.insert(r2);

        let mut r3 = Region::new(RegionId::new(3));
        r3.parent = Some(RegionId::ROOT);
        r3.content = Content::Text(TextContent::new("No match here"));
        tree.insert(r3);

        tree
    }

    #[test]
    fn search_finds_plain_text() {
        let tree = make_tree_with_text();
        let matches = tree.search("hello");
        // Should find "Hello" in r1 and "hello" in r2 span 1
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].region_id, RegionId::new(1));
        assert_eq!(matches[0].span_index, None);
        assert_eq!(matches[0].char_offset, 0);
        assert_eq!(matches[1].region_id, RegionId::new(2));
        assert_eq!(matches[1].span_index, Some(1));
        assert_eq!(matches[1].char_offset, 0);
    }

    #[test]
    fn search_case_insensitive() {
        let tree = make_tree_with_text();
        let matches = tree.search("HELLO");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn search_empty_query() {
        let tree = make_tree_with_text();
        let matches = tree.search("");
        assert!(matches.is_empty());
    }

    #[test]
    fn search_no_matches() {
        let tree = make_tree_with_text();
        let matches = tree.search("zzzzz");
        assert!(matches.is_empty());
    }

    #[test]
    fn search_multiple_in_same_text() {
        let mut tree = SceneTree::new();
        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        r1.content = Content::Text(TextContent::new("aba aba aba"));
        tree.insert(r1);

        let matches = tree.search("aba");
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].char_offset, 0);
        assert_eq!(matches[1].char_offset, 4);
        assert_eq!(matches[2].char_offset, 8);
    }

    #[test]
    fn new_tree_has_root() {
        let tree = SceneTree::new();
        assert!(tree.get(RegionId::ROOT).is_some());
        assert_eq!(tree.root(), RegionId::ROOT);
    }

    #[test]
    fn insert_and_get() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(10));
        r.parent = Some(RegionId::ROOT);
        tree.insert(r);

        assert!(tree.get(RegionId::new(10)).is_some());
        let root = tree.get(RegionId::ROOT).unwrap();
        assert_eq!(root.children, vec![RegionId::new(10)]);
    }

    #[test]
    fn insert_at_specific_index() {
        let mut tree = SceneTree::new();
        for i in 1..=3 {
            let mut r = Region::new(RegionId::new(i));
            r.parent = Some(RegionId::ROOT);
            tree.insert(r);
        }

        // Insert at index 1 (between first and second)
        let mut r4 = Region::new(RegionId::new(4));
        r4.parent = Some(RegionId::ROOT);
        tree.insert_at(r4, 1);

        let root = tree.get(RegionId::ROOT).unwrap();
        assert_eq!(
            root.children,
            vec![
                RegionId::new(1),
                RegionId::new(4),
                RegionId::new(2),
                RegionId::new(3)
            ]
        );
    }

    #[test]
    fn insert_at_out_of_bounds_appends() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        tree.insert_at(r, 999);

        let root = tree.get(RegionId::ROOT).unwrap();
        assert_eq!(root.children, vec![RegionId::new(1)]);
    }

    #[test]
    fn remove_region() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        tree.insert(r);

        assert!(tree.get(RegionId::new(1)).is_some());
        let removed = tree.remove(RegionId::new(1));
        assert!(removed.is_some());
        assert!(tree.get(RegionId::new(1)).is_none());

        let root = tree.get(RegionId::ROOT).unwrap();
        assert!(root.children.is_empty());
    }

    #[test]
    fn remove_cascades_to_children() {
        let mut tree = SceneTree::new();
        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::new(1));
        tree.insert(r2);

        tree.remove(RegionId::new(1));
        assert!(tree.get(RegionId::new(1)).is_none());
        assert!(tree.get(RegionId::new(2)).is_none());
    }

    #[test]
    fn cannot_remove_root() {
        let mut tree = SceneTree::new();
        assert!(tree.remove(RegionId::ROOT).is_none());
        assert!(tree.get(RegionId::ROOT).is_some());
    }

    #[test]
    fn depth_first_iteration_order() {
        let mut tree = SceneTree::new();

        let mut a = Region::new(RegionId::new(1));
        a.parent = Some(RegionId::ROOT);
        tree.insert(a);

        let mut b = Region::new(RegionId::new(2));
        b.parent = Some(RegionId::ROOT);
        tree.insert(b);

        let mut a1 = Region::new(RegionId::new(3));
        a1.parent = Some(RegionId::new(1));
        tree.insert(a1);

        let ids: Vec<u32> = tree.iter_depth_first().map(|r| r.id.0).collect();
        // Root(0) -> A(1) -> A1(3) -> B(2)
        assert_eq!(ids, vec![0, 1, 3, 2]);
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let mut tree = SceneTree::new();
        assert!(tree.remove(RegionId::new(999)).is_none());
    }

    #[test]
    fn reparent_region() {
        let mut tree = SceneTree::new();

        // Create two parent regions under root
        let mut p1 = Region::new(RegionId::new(1));
        p1.parent = Some(RegionId::ROOT);
        tree.insert(p1);

        let mut p2 = Region::new(RegionId::new(2));
        p2.parent = Some(RegionId::ROOT);
        tree.insert(p2);

        // Create a child under p1
        let mut child = Region::new(RegionId::new(3));
        child.parent = Some(RegionId::new(1));
        tree.insert(child);

        // Verify initial state
        assert_eq!(
            tree.get(RegionId::new(1)).unwrap().children,
            vec![RegionId::new(3)]
        );
        assert!(tree.get(RegionId::new(2)).unwrap().children.is_empty());

        tree.reparent(RegionId::new(3), RegionId::new(2), 0)
            .unwrap();

        assert!(tree.get(RegionId::new(1)).unwrap().children.is_empty());
        assert_eq!(
            tree.get(RegionId::new(2)).unwrap().children,
            vec![RegionId::new(3)]
        );
        assert_eq!(
            tree.get(RegionId::new(3)).unwrap().parent,
            Some(RegionId::new(2))
        );
    }

    #[test]
    fn reparent_region_at_specific_index() {
        let mut tree = SceneTree::new();

        for id in [1, 2, 3] {
            let mut region = Region::new(RegionId::new(id));
            region.parent = Some(RegionId::ROOT);
            tree.insert(region);
        }

        let mut child = Region::new(RegionId::new(4));
        child.parent = Some(RegionId::new(1));
        tree.insert(child);

        tree.reparent(RegionId::new(4), RegionId::ROOT, 1).unwrap();

        assert_eq!(
            tree.get(RegionId::ROOT).unwrap().children,
            vec![
                RegionId::new(1),
                RegionId::new(4),
                RegionId::new(2),
                RegionId::new(3)
            ]
        );
        assert!(tree.get(RegionId::new(1)).unwrap().children.is_empty());
    }

    #[test]
    fn reparent_rejects_root_missing_region_and_missing_parent() {
        let mut tree = SceneTree::new();
        let mut region = Region::new(RegionId::new(1));
        region.parent = Some(RegionId::ROOT);
        tree.insert(region);

        assert_eq!(
            tree.reparent(RegionId::ROOT, RegionId::new(1), 0),
            Err(SceneTreeError::CannotMoveRoot)
        );
        assert_eq!(
            tree.reparent(RegionId::new(99), RegionId::ROOT, 0),
            Err(SceneTreeError::RegionNotFound(RegionId::new(99)))
        );
        assert_eq!(
            tree.reparent(RegionId::new(1), RegionId::new(99), 0),
            Err(SceneTreeError::ParentNotFound(RegionId::new(99)))
        );

        assert_eq!(
            tree.get(RegionId::ROOT).unwrap().children,
            vec![RegionId::new(1)]
        );
        assert_eq!(
            tree.get(RegionId::new(1)).unwrap().parent,
            Some(RegionId::ROOT)
        );
    }

    fn chain(tree: &mut SceneTree, first_id: u32, len: u32) {
        let mut parent = RegionId::ROOT;
        for i in 0..len {
            let id = RegionId::new(first_id + i);
            let mut r = Region::new(id);
            r.parent = Some(parent);
            tree.try_insert_at(r, usize::MAX).unwrap();
            parent = id;
        }
    }

    #[test]
    fn remove_deep_chain_is_iterative() {
        let mut tree = SceneTree::new();
        // Deeper than MAX_TREE_DEPTH would allow via try_insert_at, so build
        // it with the lenient insert to exercise remove on its own.
        let mut parent = RegionId::ROOT;
        for i in 1..=200 {
            let id = RegionId::new(i);
            let mut r = Region::new(id);
            r.parent = Some(parent);
            tree.insert(r);
            parent = id;
        }
        assert_eq!(tree.len(), 201);

        assert!(tree.remove(RegionId::new(1)).is_some());
        assert_eq!(tree.len(), 1);
        assert!(tree.get(RegionId::ROOT).unwrap().children.is_empty());
    }

    #[test]
    fn remove_very_deep_chain_does_not_overflow_stack() {
        let mut tree = SceneTree::new();
        let mut parent = RegionId::ROOT;
        for i in 1..=100_000u32 {
            let id = RegionId::new(i);
            let mut r = Region::new(id);
            r.parent = Some(parent);
            tree.insert(r);
            parent = id;
        }
        assert!(tree.remove(RegionId::new(1)).is_some());
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn try_insert_rejects_chain_deeper_than_limit() {
        let mut tree = SceneTree::new();
        chain(&mut tree, 1, MAX_TREE_DEPTH as u32);
        assert_eq!(
            tree.depth_of(RegionId::new(MAX_TREE_DEPTH as u32)),
            Some(MAX_TREE_DEPTH)
        );

        let mut too_deep = Region::new(RegionId::new(100_000));
        too_deep.parent = Some(RegionId::new(MAX_TREE_DEPTH as u32));
        assert_eq!(
            tree.try_insert_at(too_deep, usize::MAX),
            Err(SceneTreeError::TooDeep {
                max: MAX_TREE_DEPTH
            })
        );
        assert!(tree.get(RegionId::new(100_000)).is_none());
    }

    #[test]
    fn try_insert_rejects_more_than_max_regions() {
        let mut tree = SceneTree::new();
        for i in 1..MAX_REGIONS as u32 {
            let mut r = Region::new(RegionId::new(i));
            r.parent = Some(RegionId::ROOT);
            tree.try_insert_at(r, usize::MAX).unwrap();
        }
        assert_eq!(tree.len(), MAX_REGIONS);

        let mut extra = Region::new(RegionId::new(MAX_REGIONS as u32));
        extra.parent = Some(RegionId::ROOT);
        assert_eq!(
            tree.try_insert_at(extra, usize::MAX),
            Err(SceneTreeError::TooManyRegions { max: MAX_REGIONS })
        );
        assert_eq!(tree.len(), MAX_REGIONS);
    }

    #[test]
    fn try_insert_rejects_duplicate_id_and_missing_parent() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        tree.try_insert_at(r, usize::MAX).unwrap();

        let mut dup = Region::new(RegionId::new(1));
        dup.parent = Some(RegionId::ROOT);
        assert_eq!(
            tree.try_insert_at(dup, usize::MAX),
            Err(SceneTreeError::DuplicateRegion(RegionId::new(1)))
        );
        assert_eq!(
            tree.get(RegionId::ROOT).unwrap().children,
            vec![RegionId::new(1)]
        );

        let mut orphan = Region::new(RegionId::new(2));
        orphan.parent = Some(RegionId::new(999));
        assert_eq!(
            tree.try_insert_at(orphan, usize::MAX),
            Err(SceneTreeError::ParentNotFound(RegionId::new(999)))
        );
        assert!(tree.get(RegionId::new(2)).is_none());

        let no_parent = Region::new(RegionId::new(3));
        assert!(tree.try_insert_at(no_parent, usize::MAX).is_err());
    }

    #[test]
    fn insert_duplicate_id_does_not_duplicate_child_reference() {
        let mut tree = SceneTree::new();
        let mut first = Region::new(RegionId::new(1));
        first.parent = Some(RegionId::ROOT);
        tree.insert(first);

        let mut again = Region::new(RegionId::new(1));
        again.parent = Some(RegionId::ROOT);
        again.content = Content::Text(TextContent::new("replaced"));
        tree.insert(again);

        assert_eq!(
            tree.get(RegionId::ROOT).unwrap().children,
            vec![RegionId::new(1)]
        );
        match &tree.get(RegionId::new(1)).unwrap().content {
            Content::Text(tc) => assert_eq!(tc.text, "replaced"),
            _ => panic!("expected replaced text"),
        }
    }

    #[test]
    fn reparent_rejects_self_and_descendant_as_new_parent() {
        let mut tree = SceneTree::new();
        chain(&mut tree, 1, 3); // ROOT -> 1 -> 2 -> 3

        assert_eq!(
            tree.reparent(RegionId::new(1), RegionId::new(1), 0),
            Err(SceneTreeError::CyclicReparent {
                id: RegionId::new(1),
                new_parent: RegionId::new(1)
            })
        );
        assert_eq!(
            tree.reparent(RegionId::new(1), RegionId::new(3), 0),
            Err(SceneTreeError::CyclicReparent {
                id: RegionId::new(1),
                new_parent: RegionId::new(3)
            })
        );
        // Tree unchanged.
        assert_eq!(
            tree.get(RegionId::new(1)).unwrap().parent,
            Some(RegionId::ROOT)
        );
        assert_eq!(
            tree.get(RegionId::new(3)).unwrap().children,
            Vec::<RegionId>::new()
        );
        assert_eq!(tree.iter_depth_first().count(), 4);
    }

    #[test]
    fn reparent_rejects_moves_that_exceed_depth_limit() {
        let mut tree = SceneTree::new();
        chain(&mut tree, 1, MAX_TREE_DEPTH as u32); // deepest at MAX_TREE_DEPTH
        let mut leaf = Region::new(RegionId::new(5000));
        leaf.parent = Some(RegionId::ROOT);
        tree.try_insert_at(leaf, usize::MAX).unwrap();

        assert_eq!(
            tree.reparent(RegionId::new(5000), RegionId::new(MAX_TREE_DEPTH as u32), 0),
            Err(SceneTreeError::TooDeep {
                max: MAX_TREE_DEPTH
            })
        );
        assert_eq!(
            tree.get(RegionId::new(5000)).unwrap().parent,
            Some(RegionId::ROOT)
        );
    }

    #[test]
    fn depth_and_descendant_helpers() {
        let mut tree = SceneTree::new();
        chain(&mut tree, 1, 3);
        assert_eq!(tree.depth_of(RegionId::ROOT), Some(0));
        assert_eq!(tree.depth_of(RegionId::new(3)), Some(3));
        assert_eq!(tree.depth_of(RegionId::new(42)), None);
        assert!(tree.is_descendant(RegionId::ROOT, RegionId::new(3)));
        assert!(tree.is_descendant(RegionId::new(1), RegionId::new(3)));
        assert!(!tree.is_descendant(RegionId::new(3), RegionId::new(1)));
        assert!(!tree.is_descendant(RegionId::new(1), RegionId::new(1)));
        assert!(!tree.is_empty());
        assert!(SceneTree::new().is_empty());
    }

    #[test]
    fn get_mut_modifies_region() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        tree.insert(r);

        tree.get_mut(RegionId::new(1)).unwrap().content =
            Content::Text(TextContent::new("modified"));

        match &tree.get(RegionId::new(1)).unwrap().content {
            Content::Text(tc) => assert_eq!(tc.text, "modified"),
            _ => panic!("expected Text content"),
        }
    }

    #[test]
    fn default_tree_matches_new() {
        let tree1 = SceneTree::new();
        let tree2 = SceneTree::default();
        assert_eq!(tree1.root(), tree2.root());
        assert!(tree1.get(RegionId::ROOT).is_some());
        assert!(tree2.get(RegionId::ROOT).is_some());
    }

    #[test]
    fn depth_first_single_node() {
        let tree = SceneTree::new();
        let ids: Vec<u32> = tree.iter_depth_first().map(|r| r.id.0).collect();
        assert_eq!(ids, vec![0]); // Just root
    }

    #[test]
    fn depth_first_deep_chain() {
        let mut tree = SceneTree::new();

        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::new(1));
        tree.insert(r2);

        let mut r3 = Region::new(RegionId::new(3));
        r3.parent = Some(RegionId::new(2));
        tree.insert(r3);

        let ids: Vec<u32> = tree.iter_depth_first().map(|r| r.id.0).collect();
        // Root(0) -> 1 -> 2 -> 3
        assert_eq!(ids, vec![0, 1, 2, 3]);
    }

    #[test]
    fn insert_without_parent_in_tree() {
        // Region with a parent ID that doesn't exist in the tree
        let mut tree = SceneTree::new();
        let mut orphan = Region::new(RegionId::new(99));
        orphan.parent = Some(RegionId::new(999)); // non-existent parent
        tree.insert(orphan);

        // The region is inserted but not linked to any parent's children
        assert!(tree.get(RegionId::new(99)).is_some());
        assert!(tree.get(RegionId::ROOT).unwrap().children.is_empty());
    }

    #[test]
    fn insert_no_parent_field() {
        // Region with parent = None (like an additional root)
        let mut tree = SceneTree::new();
        let free = Region::new(RegionId::new(50));
        tree.insert(free);

        assert!(tree.get(RegionId::new(50)).is_some());
        // Root should not have it as a child
        assert!(tree.get(RegionId::ROOT).unwrap().children.is_empty());
    }

    #[test]
    fn search_match_fields() {
        let m = SearchMatch {
            region_id: RegionId::new(5),
            span_index: Some(2),
            char_offset: 10,
            length: 3,
        };
        assert_eq!(m.region_id, RegionId::new(5));
        assert_eq!(m.span_index, Some(2));
        assert_eq!(m.char_offset, 10);
        assert_eq!(m.length, 3);
    }

    // ── Widget search tests ─────────────────────────────────────────

    use ultty_protocol::{
        CodeWidget, ImageScale, ImageWidget, InputWidget, MarkdownWidget, ProgressWidget,
        SpinnerWidget,
    };

    #[test]
    fn search_finds_match_in_input_value() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: "hello world".into(),
            placeholder: String::new(),
            multiline: false,
            password: false,
        }));
        tree.insert(r);

        let matches = tree.search("hello");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(1));
        assert_eq!(matches[0].char_offset, 0);
        assert_eq!(matches[0].length, 5);
    }

    #[test]
    fn search_finds_match_in_markdown_source() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Markdown(MarkdownWidget {
            source: "# Title\nSome markdown content".into(),
        }));
        tree.insert(r);

        let matches = tree.search("markdown");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(1));
    }

    #[test]
    fn search_finds_match_in_code_source() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Code(CodeWidget {
            source: "fn main() { println!(\"hello\"); }".into(),
            language: "rust".into(),
            line_numbers: true,
        }));
        tree.insert(r);

        let matches = tree.search("main");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(1));
        assert_eq!(matches[0].char_offset, 3);
    }

    #[test]
    fn search_is_case_insensitive_across_widget_types() {
        let mut tree = SceneTree::new();

        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        r1.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: "HELLO".into(),
            placeholder: String::new(),
            multiline: false,
            password: false,
        }));
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::ROOT);
        r2.content = Content::Widget(WidgetContent::Markdown(MarkdownWidget {
            source: "Hello World".into(),
        }));
        tree.insert(r2);

        let mut r3 = Region::new(RegionId::new(3));
        r3.parent = Some(RegionId::ROOT);
        r3.content = Content::Widget(WidgetContent::Code(CodeWidget {
            source: "hello()".into(),
            language: String::new(),
            line_numbers: false,
        }));
        tree.insert(r3);

        let matches = tree.search("hello");
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn search_returns_correct_region_id_and_offset_for_widgets() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(42));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Code(CodeWidget {
            source: "abc def ghi".into(),
            language: String::new(),
            line_numbers: false,
        }));
        tree.insert(r);

        let matches = tree.search("def");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(42));
        assert_eq!(matches[0].char_offset, 4);
        assert_eq!(matches[0].length, 3);
    }

    #[test]
    fn search_finds_multiple_matches_in_same_widget() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Markdown(MarkdownWidget {
            source: "foo bar foo baz foo".into(),
        }));
        tree.insert(r);

        let matches = tree.search("foo");
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].char_offset, 0);
        assert_eq!(matches[1].char_offset, 8);
        assert_eq!(matches[2].char_offset, 16);
    }

    #[test]
    fn search_non_text_widgets_return_no_matches() {
        let mut tree = SceneTree::new();

        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        r1.content = Content::Widget(WidgetContent::Progress(ProgressWidget {
            value: 0.5,
            label: None,
            animated: false,
            indeterminate: false,
        }));
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::ROOT);
        r2.content = Content::Widget(WidgetContent::Spinner(SpinnerWidget {
            label: None,
            style: Default::default(),
        }));
        tree.insert(r2);

        let mut r3 = Region::new(RegionId::new(3));
        r3.parent = Some(RegionId::ROOT);
        r3.content = Content::Widget(WidgetContent::Image(ImageWidget {
            data: vec![0u8; 16],
            width: 2,
            height: 2,
            scale: ImageScale::Fit,
        }));
        tree.insert(r3);

        let matches = tree.search("test");
        assert!(matches.is_empty());
    }

    #[test]
    fn search_finds_match_in_progress_label() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Progress(ProgressWidget {
            value: 0.75,
            label: Some("Loading data...".into()),
            animated: false,
            indeterminate: false,
        }));
        tree.insert(r);

        let matches = tree.search("loading");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(1));
        assert_eq!(matches[0].char_offset, 0);
        assert_eq!(matches[0].length, 7);
    }

    #[test]
    fn search_finds_match_in_spinner_label() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Spinner(SpinnerWidget {
            label: Some("Processing request".into()),
            style: Default::default(),
        }));
        tree.insert(r);

        let matches = tree.search("processing");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(1));
        assert_eq!(matches[0].char_offset, 0);
    }

    #[test]
    fn search_skips_progress_spinner_with_no_label() {
        let mut tree = SceneTree::new();

        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        r1.content = Content::Widget(WidgetContent::Progress(ProgressWidget {
            value: 0.5,
            label: None,
            animated: false,
            indeterminate: false,
        }));
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::ROOT);
        r2.content = Content::Widget(WidgetContent::Spinner(SpinnerWidget {
            label: None,
            style: Default::default(),
        }));
        tree.insert(r2);

        // With labels
        let mut r3 = Region::new(RegionId::new(3));
        r3.parent = Some(RegionId::ROOT);
        r3.content = Content::Widget(WidgetContent::Progress(ProgressWidget {
            value: 0.5,
            label: Some("target".into()),
            animated: false,
            indeterminate: false,
        }));
        tree.insert(r3);

        let matches = tree.search("target");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].region_id, RegionId::new(3));
    }

    #[test]
    fn search_mixed_text_and_widget_regions() {
        let mut tree = SceneTree::new();

        let mut r1 = Region::new(RegionId::new(1));
        r1.parent = Some(RegionId::ROOT);
        r1.content = Content::Text(TextContent::new("find me here"));
        tree.insert(r1);

        let mut r2 = Region::new(RegionId::new(2));
        r2.parent = Some(RegionId::ROOT);
        r2.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: "find me too".into(),
            placeholder: String::new(),
            multiline: false,
            password: false,
        }));
        tree.insert(r2);

        let mut r3 = Region::new(RegionId::new(3));
        r3.parent = Some(RegionId::ROOT);
        r3.content = Content::Widget(WidgetContent::Code(CodeWidget {
            source: "// find me in code".into(),
            language: String::new(),
            line_numbers: false,
        }));
        tree.insert(r3);

        let mut r4 = Region::new(RegionId::new(4));
        r4.parent = Some(RegionId::ROOT);
        r4.content = Content::None;
        tree.insert(r4);

        let matches = tree.search("find me");
        assert_eq!(matches.len(), 3);
        let region_ids: Vec<u32> = matches.iter().map(|m| m.region_id.0).collect();
        assert!(region_ids.contains(&1));
        assert!(region_ids.contains(&2));
        assert!(region_ids.contains(&3));
    }

    // ── UTF-8 char offset tests ─────────────────────────────────────

    #[test]
    fn search_emoji_text_returns_char_offset() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        // "🎉 hello" — emoji is 4 bytes but 1 char, space is 1 byte/char
        r.content = Content::Text(TextContent::new("🎉 hello"));
        tree.insert(r);

        let matches = tree.search("hello");
        assert_eq!(matches.len(), 1);
        // char_offset should be 2 (emoji + space), not 5 (4 bytes + 1 byte)
        assert_eq!(matches[0].char_offset, 2);
        assert_eq!(matches[0].length, 5);
    }

    #[test]
    fn search_cjk_text_returns_char_offset() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        // "你好world" — each CJK char is 3 bytes but 1 char
        r.content = Content::Text(TextContent::new("你好world"));
        tree.insert(r);

        let matches = tree.search("world");
        assert_eq!(matches.len(), 1);
        // char_offset should be 2, not 6
        assert_eq!(matches[0].char_offset, 2);
        assert_eq!(matches[0].length, 5);
    }

    #[test]
    fn search_mixed_ascii_multibyte_offset() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        // "abc🎉def" — search for "def" should be at char_offset 4 (a,b,c,🎉)
        r.content = Content::Text(TextContent::new("abc🎉def"));
        tree.insert(r);

        let matches = tree.search("def");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].char_offset, 4);
        assert_eq!(matches[0].length, 3);
    }

    // ── Input placeholder search tests ──────────────────────────────

    #[test]
    fn search_finds_placeholder_when_value_empty() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: String::new(),
            placeholder: "type here".into(),
            multiline: false,
            password: false,
        }));
        tree.insert(r);

        let matches = tree.search("type");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].char_offset, 0);
        assert_eq!(matches[0].length, 4);
    }

    #[test]
    fn search_finds_value_when_non_empty() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: "actual text".into(),
            placeholder: "placeholder".into(),
            multiline: false,
            password: false,
        }));
        tree.insert(r);

        let matches = tree.search("actual");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].char_offset, 0);
    }

    #[test]
    fn search_does_not_find_placeholder_when_value_non_empty() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: "something".into(),
            placeholder: "hidden placeholder".into(),
            multiline: false,
            password: false,
        }));
        tree.insert(r);

        let matches = tree.search("hidden");
        assert!(matches.is_empty());
    }

    // ── Password input search tests ─────────────────────────────────

    #[test]
    fn search_does_not_expose_password_value() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: "secret123".into(),
            placeholder: "enter password".into(),
            multiline: false,
            password: true,
        }));
        tree.insert(r);

        // Should NOT find the password value
        let matches = tree.search("secret");
        assert!(matches.is_empty());
    }

    #[test]
    fn search_password_empty_value_finds_placeholder() {
        let mut tree = SceneTree::new();
        let mut r = Region::new(RegionId::new(1));
        r.parent = Some(RegionId::ROOT);
        r.content = Content::Widget(WidgetContent::Input(InputWidget {
            value: String::new(),
            placeholder: "enter password".into(),
            multiline: false,
            password: true,
        }));
        tree.insert(r);

        // Placeholder IS displayed when value is empty, so it should be searchable
        let matches = tree.search("enter");
        assert_eq!(matches.len(), 1);
    }

    fn child_of(id: u32, parent: u32) -> Region {
        let mut region = Region::new(RegionId::new(id));
        region.parent = Some(RegionId::new(parent));
        region
    }

    fn ids<'a>(regions: impl Iterator<Item = &'a Region>) -> Vec<u32> {
        regions.map(|region| region.id.0).collect()
    }

    /// ROOT with child 1, and surface 10 (3 rows) with children 11 and 12,
    /// 12 under 11.
    fn tree_with_surface() -> SceneTree {
        let mut tree = SceneTree::new();
        tree.try_insert_at(child_of(1, 0), usize::MAX).unwrap();
        tree.try_insert_surface(Region::new(RegionId::new(10)), 3)
            .unwrap();
        tree.try_insert_at(child_of(11, 10), usize::MAX).unwrap();
        tree.try_insert_at(child_of(12, 11), usize::MAX).unwrap();
        tree
    }

    #[test]
    fn a_surface_is_a_second_root_outside_the_full_scene_walk() {
        let tree = tree_with_surface();
        assert_eq!(
            tree.surfaces(),
            &[SurfaceRoot {
                id: RegionId::new(10),
                rows: 3
            }]
        );
        assert_eq!(tree.get(RegionId::new(10)).unwrap().parent, None);
        assert_eq!(tree.depth_of(RegionId::new(10)), Some(0));
        assert_eq!(tree.depth_of(RegionId::new(12)), Some(2));
        assert_eq!(ids(tree.iter_depth_first()), [0, 1]);
        assert_eq!(ids(tree.iter_tree(RegionId::new(10))), [10, 11, 12]);
        assert_eq!(ids(tree.iter_all()), [0, 1, 10, 11, 12]);
        assert_eq!(tree.len(), 5);
    }

    #[test]
    fn a_surface_root_drops_any_parent_it_names() {
        let mut tree = SceneTree::new();
        tree.try_insert_surface(child_of(5, 0), 1).unwrap();
        assert_eq!(tree.get(RegionId::new(5)).unwrap().parent, None);
        assert!(tree.get(RegionId::ROOT).unwrap().children.is_empty());
    }

    #[test]
    fn removing_a_surface_root_removes_its_tree_and_the_surface() {
        let mut tree = tree_with_surface();
        assert!(tree.remove(RegionId::new(10)).is_some());
        assert!(tree.surfaces().is_empty());
        for id in [10, 11, 12] {
            assert!(tree.get(RegionId::new(id)).is_none(), "region {id}");
        }
        assert_eq!(ids(tree.iter_all()), [0, 1]);
    }

    #[test]
    fn removing_a_region_inside_a_surface_keeps_the_surface() {
        let mut tree = tree_with_surface();
        tree.remove(RegionId::new(11));
        assert_eq!(tree.surfaces().len(), 1);
        assert_eq!(ids(tree.iter_tree(RegionId::new(10))), [10]);
    }

    #[test]
    fn surface_roots_cannot_move_but_regions_can_move_between_trees() {
        let mut tree = tree_with_surface();
        assert_eq!(
            tree.reparent(RegionId::new(10), RegionId::ROOT, 0),
            Err(SceneTreeError::CannotMoveRoot)
        );
        tree.reparent(RegionId::new(1), RegionId::new(10), usize::MAX)
            .unwrap();
        assert_eq!(ids(tree.iter_depth_first()), [0]);
        assert_eq!(ids(tree.iter_tree(RegionId::new(10))), [10, 11, 12, 1]);
        tree.reparent(RegionId::new(11), RegionId::ROOT, usize::MAX)
            .unwrap();
        assert_eq!(ids(tree.iter_depth_first()), [0, 11, 12]);
    }

    #[test]
    fn try_insert_surface_rejects_duplicates_and_a_seventeenth_surface() {
        let mut tree = SceneTree::new();
        tree.try_insert_at(child_of(1, 0), usize::MAX).unwrap();
        assert_eq!(
            tree.try_insert_surface(Region::new(RegionId::new(1)), 2),
            Err(SceneTreeError::DuplicateRegion(RegionId::new(1)))
        );
        assert_eq!(
            tree.try_insert_surface(Region::new(RegionId::ROOT), 2),
            Err(SceneTreeError::DuplicateRegion(RegionId::ROOT))
        );
        for id in 0..MAX_SURFACES as u32 {
            tree.try_insert_surface(Region::new(RegionId::new(100 + id)), 1)
                .unwrap();
        }
        assert_eq!(
            tree.try_insert_surface(Region::new(RegionId::new(999)), 1),
            Err(SceneTreeError::TooManySurfaces { max: MAX_SURFACES })
        );
        assert!(tree.get(RegionId::new(999)).is_none());
        tree.remove(RegionId::new(100));
        tree.try_insert_surface(Region::new(RegionId::new(999)), 1)
            .unwrap();
    }

    #[test]
    fn surface_regions_count_toward_the_region_limit() {
        let mut tree = SceneTree::new();
        tree.try_insert_surface(Region::new(RegionId::new(1)), 1)
            .unwrap();
        for i in 2..MAX_REGIONS as u32 {
            tree.try_insert_at(child_of(i, 1), usize::MAX).unwrap();
        }
        assert_eq!(tree.len(), MAX_REGIONS);
        assert_eq!(
            tree.try_insert_surface(Region::new(RegionId::new(MAX_REGIONS as u32)), 1),
            Err(SceneTreeError::TooManyRegions { max: MAX_REGIONS })
        );
        assert_eq!(
            tree.try_insert_at(child_of(MAX_REGIONS as u32, 0), usize::MAX),
            Err(SceneTreeError::TooManyRegions { max: MAX_REGIONS })
        );
    }

    #[test]
    fn depth_is_limited_below_a_surface_root() {
        let mut tree = SceneTree::new();
        tree.try_insert_surface(Region::new(RegionId::new(1)), 1)
            .unwrap();
        for i in 0..MAX_TREE_DEPTH as u32 {
            tree.try_insert_at(child_of(2 + i, 1 + i), usize::MAX)
                .unwrap();
        }
        let deepest = 1 + MAX_TREE_DEPTH as u32;
        assert_eq!(tree.depth_of(RegionId::new(deepest)), Some(MAX_TREE_DEPTH));
        assert_eq!(
            tree.try_insert_at(child_of(deepest + 1, deepest), usize::MAX),
            Err(SceneTreeError::TooDeep {
                max: MAX_TREE_DEPTH
            })
        );
    }
}
