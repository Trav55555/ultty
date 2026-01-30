use taffy::prelude::*;
use taffy::TaffyError;

/// The Layout Engine.
/// Wraps `Taffy` to provide Flexbox layout capabilities to the TUI.
pub struct LayoutEngine {
    taffy: TaffyTree,
    root_node: NodeId,
}

impl LayoutEngine {
    pub fn new() -> Self {
        let mut taffy = TaffyTree::new();
        
        // Create an initial root node with default style
        let root_node = taffy.new_leaf(Style::default()).unwrap();
        
        Self {
            taffy,
            root_node,
        }
    }

    /// Compute the layout for the given screen dimensions.
    pub fn compute(&mut self, width: u16, height: u16) -> Result<(), TaffyError> {
        let size = Size {
            width: AvailableSpace::Definite(width as f32),
            height: AvailableSpace::Definite(height as f32),
        };

        // We re-compute the layout tree from the root down
        self.taffy.compute_layout(self.root_node, size)
    }

    /// Get the computed layout (x, y, w, h) for the root node.
    pub fn root_layout(&self) -> Result<taffy::Layout, TaffyError> {
        self.taffy.layout(self.root_node).cloned()
    }
}
