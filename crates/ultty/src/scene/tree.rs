//! Scene tree management.

use std::collections::HashMap;

use ultty_protocol::{Content, RegionId, WidgetContent};

use super::region::Region;

/// A search match within the scene tree.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// The region containing the match.
    pub region_id: RegionId,
    /// Index of the span within RichText (None for plain text).
    pub span_index: Option<usize>,
    /// Byte offset of the match within the text.
    pub byte_offset: usize,
    /// Length of the match in bytes.
    pub length: usize,
}

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

    /// Insert a new region (appends to parent's children).
    pub fn insert(&mut self, region: Region) {
        let id = region.id;
        if let Some(parent_id) = region.parent {
            if let Some(parent) = self.regions.get_mut(&parent_id) {
                parent.children.push(id);
            }
        }
        self.regions.insert(id, region);
    }

    /// Insert a new region at a specific index in the parent's children list.
    /// If index is out of bounds, appends to the end.
    pub fn insert_at(&mut self, region: Region, index: usize) {
        let id = region.id;
        if let Some(parent_id) = region.parent {
            if let Some(parent) = self.regions.get_mut(&parent_id) {
                if index <= parent.children.len() {
                    parent.children.insert(index, id);
                } else {
                    parent.children.push(id);
                }
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
                    for (offset, _) in text_lower.match_indices(&query_lower) {
                        matches.push(SearchMatch {
                            region_id: region.id,
                            span_index: None,
                            byte_offset: offset,
                            length: query.len(),
                        });
                    }
                }
                Content::RichText(rich_text) => {
                    for (span_idx, span) in rich_text.spans.iter().enumerate() {
                        let text_lower = span.text.to_lowercase();
                        for (offset, _) in text_lower.match_indices(&query_lower) {
                            matches.push(SearchMatch {
                                region_id: region.id,
                                span_index: Some(span_idx),
                                byte_offset: offset,
                                length: query.len(),
                            });
                        }
                    }
                }
                Content::Widget(widget) => {
                    // Search text-bearing widgets
                    match widget {
                        WidgetContent::Input(input) => {
                            let text_lower = input.value.to_lowercase();
                            for (offset, _) in text_lower.match_indices(&query_lower) {
                                matches.push(SearchMatch {
                                    region_id: region.id,
                                    span_index: None,
                                    byte_offset: offset,
                                    length: query.len(),
                                });
                            }
                        }
                        WidgetContent::Markdown(md) => {
                            let text_lower = md.source.to_lowercase();
                            for (offset, _) in text_lower.match_indices(&query_lower) {
                                matches.push(SearchMatch {
                                    region_id: region.id,
                                    span_index: None,
                                    byte_offset: offset,
                                    length: query.len(),
                                });
                            }
                        }
                        WidgetContent::Code(code) => {
                            let text_lower = code.source.to_lowercase();
                            for (offset, _) in text_lower.match_indices(&query_lower) {
                                matches.push(SearchMatch {
                                    region_id: region.id,
                                    span_index: None,
                                    byte_offset: offset,
                                    length: query.len(),
                                });
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
        assert_eq!(matches[0].byte_offset, 0);
        assert_eq!(matches[1].region_id, RegionId::new(2));
        assert_eq!(matches[1].span_index, Some(1));
        assert_eq!(matches[1].byte_offset, 0);
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
        assert_eq!(matches[0].byte_offset, 0);
        assert_eq!(matches[1].byte_offset, 4);
        assert_eq!(matches[2].byte_offset, 8);
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
            vec![RegionId::new(1), RegionId::new(4), RegionId::new(2), RegionId::new(3)]
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
}
