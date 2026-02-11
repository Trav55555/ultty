//! Region node in the scene graph.

use ultty_protocol::{Constraints, Content, RegionId, Style};

/// A region in the scene graph.
pub struct Region {
    /// Unique identifier.
    pub id: RegionId,
    /// Parent region (None for root).
    pub parent: Option<RegionId>,
    /// Child regions.
    pub children: Vec<RegionId>,
    /// Layout constraints.
    pub constraints: Constraints,
    /// Computed layout (from Taffy).
    pub computed: ComputedLayout,
    /// Content.
    pub content: Content,
    /// Style.
    pub style: Style,
    /// Arbitrary metadata as key-value pairs.
    pub metadata: Vec<(String, String)>,
    /// Dirty flag for rendering.
    pub dirty: bool,
}

/// Computed layout after Taffy processing.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComputedLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Region {
    /// Create a new region.
    pub fn new(id: RegionId) -> Self {
        Self {
            id,
            parent: None,
            children: Vec::new(),
            constraints: Constraints::default(),
            computed: ComputedLayout::default(),
            content: Content::None,
            style: Style::default(),
            metadata: Vec::new(),
            dirty: true,
        }
    }

    /// Mark this region as dirty.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::{Color, Content, Style, TextContent};

    #[test]
    fn new_region_has_correct_defaults() {
        let id = RegionId::new(42);
        let region = Region::new(id);

        assert_eq!(region.id, id);
        assert_eq!(region.parent, None);
        assert!(region.children.is_empty());
        assert_eq!(region.content, Content::None);
        assert_eq!(region.style, Style::default());
        assert!(region.metadata.is_empty());
        assert!(region.dirty, "newly created region should be dirty");
    }

    #[test]
    fn region_with_root_id() {
        let region = Region::new(RegionId::ROOT);
        assert_eq!(region.id, RegionId::ROOT);
        assert_eq!(region.id.0, 0);
    }

    #[test]
    fn computed_layout_default_is_zero() {
        let layout = ComputedLayout::default();
        assert_eq!(layout.x, 0.0);
        assert_eq!(layout.y, 0.0);
        assert_eq!(layout.width, 0.0);
        assert_eq!(layout.height, 0.0);
    }

    #[test]
    fn computed_layout_fields() {
        let layout = ComputedLayout {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        assert_eq!(layout.x, 10.0);
        assert_eq!(layout.y, 20.0);
        assert_eq!(layout.width, 100.0);
        assert_eq!(layout.height, 50.0);
    }

    #[test]
    fn computed_layout_clone_and_copy() {
        let layout = ComputedLayout {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        let cloned = layout;
        assert_eq!(cloned.x, layout.x);
        assert_eq!(cloned.width, layout.width);
    }

    #[test]
    fn set_region_content_text() {
        let mut region = Region::new(RegionId::new(1));
        region.content = Content::Text(TextContent::new("hello"));
        match &region.content {
            Content::Text(tc) => assert_eq!(tc.text, "hello"),
            _ => panic!("expected Text content"),
        }
    }

    #[test]
    fn set_region_content_none() {
        let region = Region::new(RegionId::new(1));
        assert_eq!(region.content, Content::None);
    }

    #[test]
    fn set_region_style() {
        let mut region = Region::new(RegionId::new(1));
        let style = Style::new().bold().fg(Color::RED);
        region.style = style.clone();
        assert!(region.style.bold);
        assert_eq!(region.style.fg, Some(Color::RED));
    }

    #[test]
    fn mark_dirty() {
        let mut region = Region::new(RegionId::new(1));
        region.dirty = false;
        region.mark_dirty();
        assert!(region.dirty);
    }

    #[test]
    fn parent_child_fields() {
        let mut region = Region::new(RegionId::new(5));
        region.parent = Some(RegionId::ROOT);
        region.children.push(RegionId::new(10));
        region.children.push(RegionId::new(11));

        assert_eq!(region.parent, Some(RegionId::ROOT));
        assert_eq!(region.children.len(), 2);
        assert_eq!(region.children[0], RegionId::new(10));
        assert_eq!(region.children[1], RegionId::new(11));
    }

    #[test]
    fn computed_layout_on_region() {
        let mut region = Region::new(RegionId::new(1));
        region.computed = ComputedLayout {
            x: 5.0,
            y: 10.0,
            width: 200.0,
            height: 100.0,
        };
        assert_eq!(region.computed.x, 5.0);
        assert_eq!(region.computed.y, 10.0);
        assert_eq!(region.computed.width, 200.0);
        assert_eq!(region.computed.height, 100.0);
    }
}
