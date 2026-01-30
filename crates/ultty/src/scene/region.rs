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
            dirty: true,
        }
    }

    /// Mark this region as dirty.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
