//! The AccessKit tree Ultty exposes, built without any platform types.

use accesskit::{Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};

/// The window's node, the root of the tree.
pub const WINDOW_ID: NodeId = NodeId(0);

/// A full tree of one `Window` node named `title`.
///
/// What a screen reader gets until the window's contents are exposed: the
/// window is named, and focus rests on it.
pub fn window_only(title: &str) -> TreeUpdate {
    let mut window = Node::new(Role::Window);
    window.set_label(title);
    TreeUpdate {
        nodes: vec![(WINDOW_ID, window)],
        tree: Some(TreeInfo::new(WINDOW_ID)),
        tree_id: TreeId::ROOT,
        focus: WINDOW_ID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_only_is_one_focused_window_node_named_after_the_title() {
        let update = window_only("vim ~/notes");

        assert_eq!(update.nodes.len(), 1);
        let (id, node) = &update.nodes[0];
        assert_eq!(*id, WINDOW_ID);
        assert_eq!(node.role(), Role::Window);
        assert_eq!(node.label(), Some("vim ~/notes"));
        assert!(node.children().is_empty());
        assert_eq!(update.tree.as_ref().map(|tree| tree.root), Some(WINDOW_ID));
        assert_eq!(update.tree_id, TreeId::ROOT);
        assert_eq!(update.focus, WINDOW_ID);
    }
}
