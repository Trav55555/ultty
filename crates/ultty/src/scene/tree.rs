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
}
