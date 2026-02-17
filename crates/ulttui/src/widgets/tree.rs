//! Tree widget for hierarchical navigation.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style as RStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Color, Constraints, Content, Dimension, Display, FlexDirection, RegionId,
    Style as ProtoStyle, TextContent,
};

/// A node in a tree hierarchy.
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// Display label for this node.
    pub label: String,
    /// Child nodes.
    pub children: Vec<TreeNode>,
    /// Whether this node is expanded.
    pub expanded: bool,
    /// Optional icon shown before the label.
    pub icon: Option<String>,
}

impl TreeNode {
    /// Create a leaf node with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            children: Vec::new(),
            expanded: false,
            icon: None,
        }
    }

    /// Create a node with child nodes.
    pub fn with_children(label: impl Into<String>, children: Vec<TreeNode>) -> Self {
        Self {
            label: label.into(),
            children,
            expanded: false,
            icon: None,
        }
    }

    /// Set an icon displayed before the label.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set whether this node starts expanded.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }
}

/// A selectable tree widget with keyboard navigation.
#[derive(Debug, Clone)]
pub struct Tree {
    root: TreeNode,
    show_root: bool,
    selected: usize,
    id: Option<String>,
}

/// A flattened, visible tree node used during rendering.
#[derive(Debug, Clone, Copy)]
struct FlatNode<'a> {
    label: &'a str,
    icon: Option<&'a str>,
    depth: usize,
    has_children: bool,
    expanded: bool,
    is_last_sibling: bool,
}

impl Tree {
    /// Create a tree rooted at `root`.
    pub fn new(root: TreeNode) -> Self {
        Self {
            root,
            show_root: true,
            selected: 0,
            id: None,
        }
    }

    /// Show or hide the root node in rendering.
    pub fn show_root(mut self, show_root: bool) -> Self {
        self.show_root = show_root;
        self
    }

    /// Set the selected index into the flattened visible list.
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Set widget ID for focus management.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn visible_nodes(&self) -> Vec<FlatNode<'_>> {
        flatten_visible(&self.root, 0, self.show_root)
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.visible_nodes().len();
        if len == 0 {
            self.selected = 0;
            return;
        }

        if delta > 0 {
            self.selected = (self.selected + delta as usize).min(len - 1);
        } else {
            let abs = (-delta) as usize;
            self.selected = self.selected.saturating_sub(abs);
        }
    }

    fn toggle_selected(&mut self) {
        let paths = visible_paths(&self.root, self.show_root);
        let Some(path) = paths.get(self.selected) else {
            return;
        };

        if let Some(node) = node_mut_by_path(&mut self.root, path) {
            if !node.children.is_empty() {
                node.expanded = !node.expanded;
            }
        }

        let len = self.visible_nodes().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

impl View for Tree {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        ProtoStyle::default()
    }

    fn children_count(&self) -> usize {
        self.visible_nodes().len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let flat = self.visible_nodes();
        let Some(node) = flat.get(index).copied() else {
            return Ok(None);
        };

        let theme = Theme::current();
        let row_style = if index == self.selected {
            ProtoStyle::new()
                .bg(theme.primary)
                .fg(contrasting_color(theme.primary, theme.foreground))
                .bold()
        } else {
            ProtoStyle::default()
        };

        let row_id = RegionId::new(*next_id);
        *next_id += 1;

        send_message(&AppMessage::CreateRegion {
            id: row_id,
            parent,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(1.0),
                ..Default::default()
            },
            style: row_style,
        })?;

        let _ = node.is_last_sibling;
        let indent = "  ".repeat(node.depth);
        let chevron = match (node.has_children, node.expanded) {
            (true, true) => "▾ ",
            (true, false) => "▸ ",
            (false, _) => "  ",
        };

        create_text_region(send_message, row_id, next_id, &indent)?;
        create_text_region(send_message, row_id, next_id, chevron)?;

        if let Some(icon) = node.icon {
            create_text_region(send_message, row_id, next_id, &format!("{icon} "))?;
        }

        create_text_region(send_message, row_id, next_id, node.label)?;

        Ok(Some(row_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let selected_bg = to_ratatui_color(theme.primary);
        let selected_fg = to_ratatui_color(contrasting_color(theme.primary, theme.foreground));

        let lines: Vec<Line> = self
            .visible_nodes()
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let _ = node.is_last_sibling;
                let indent = "  ".repeat(node.depth);
                let chevron = match (node.has_children, node.expanded) {
                    (true, true) => "▾ ",
                    (true, false) => "▸ ",
                    (false, _) => "  ",
                };
                let icon = node.icon.map(|v| format!("{v} ")).unwrap_or_default();
                let text = format!("{indent}{chevron}{icon}{}", node.label);

                let style = if index == self.selected {
                    RStyle::default()
                        .fg(selected_fg)
                        .bg(selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    RStyle::default()
                };

                Line::from(vec![Span::styled(text, style)])
            })
            .collect();

        frame.render_widget(Paragraph::new(lines), area);
    }

    fn is_focusable(&self) -> bool {
        self.id.is_some()
    }

    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<FocusAction> {
        use crate::event::Key;

        match key.key {
            Key::Char('j') | Key::Down => {
                self.move_selection(1);
                Some(FocusAction::Consumed)
            }
            Key::Char('k') | Key::Up => {
                self.move_selection(-1);
                Some(FocusAction::Consumed)
            }
            Key::Enter => {
                self.toggle_selected();
                Some(FocusAction::Consumed)
            }
            Key::Tab if key.modifiers.shift => Some(FocusAction::Previous),
            Key::Tab => Some(FocusAction::Next),
            _ => None,
        }
    }
}

fn create_text_region(
    send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
    parent: RegionId,
    next_id: &mut u32,
    text: &str,
) -> anyhow::Result<()> {
    let region_id = RegionId::new(*next_id);
    *next_id += 1;

    send_message(&AppMessage::CreateRegion {
        id: region_id,
        parent,
        insert_index: -1,
        constraints: Constraints::default(),
        style: ProtoStyle::default(),
    })?;
    send_message(&AppMessage::SetContent {
        id: region_id,
        content: Content::Text(TextContent::new(text.to_owned())),
    })?;

    Ok(())
}

fn flatten_visible(node: &TreeNode, depth: usize, show_root: bool) -> Vec<FlatNode<'_>> {
    let mut out = Vec::new();

    if show_root {
        flatten_visible_recursive(node, depth, true, &mut out);
    } else if node.expanded {
        let last = node.children.len().saturating_sub(1);
        for (index, child) in node.children.iter().enumerate() {
            flatten_visible_recursive(child, depth, index == last, &mut out);
        }
    }

    out
}

fn flatten_visible_recursive<'a>(
    node: &'a TreeNode,
    depth: usize,
    is_last_sibling: bool,
    out: &mut Vec<FlatNode<'a>>,
) {
    out.push(FlatNode {
        label: &node.label,
        icon: node.icon.as_deref(),
        depth,
        has_children: !node.children.is_empty(),
        expanded: node.expanded,
        is_last_sibling,
    });

    if node.expanded {
        let last = node.children.len().saturating_sub(1);
        for (index, child) in node.children.iter().enumerate() {
            flatten_visible_recursive(child, depth + 1, index == last, out);
        }
    }
}

fn visible_paths(root: &TreeNode, show_root: bool) -> Vec<Vec<usize>> {
    let mut out = Vec::new();

    if show_root {
        collect_visible_paths(root, &mut Vec::new(), true, &mut out);
    } else if root.expanded {
        for (index, child) in root.children.iter().enumerate() {
            let mut path = vec![index];
            collect_visible_paths(child, &mut path, true, &mut out);
        }
    }

    out
}

fn collect_visible_paths(
    node: &TreeNode,
    path: &mut Vec<usize>,
    include_current: bool,
    out: &mut Vec<Vec<usize>>,
) {
    if include_current {
        out.push(path.clone());
    }

    if node.expanded {
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            collect_visible_paths(child, path, true, out);
            path.pop();
        }
    }
}

fn node_mut_by_path<'a>(node: &'a mut TreeNode, path: &[usize]) -> Option<&'a mut TreeNode> {
    if path.is_empty() {
        return Some(node);
    }

    let mut current = node;
    for &index in path {
        current = current.children.get_mut(index)?;
    }
    Some(current)
}

fn contrasting_color(bg: Color, fallback: Color) -> Color {
    match bg {
        Color::Rgb(r, g, b) => {
            let luma = (0.299 * f64::from(r)) + (0.587 * f64::from(g)) + (0.114 * f64::from(b));
            if luma > 186.0 {
                Color::Rgb(0, 0, 0)
            } else {
                Color::Rgb(255, 255, 255)
            }
        }
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_node_new() {
        let node = TreeNode::new("root");
        assert_eq!(node.label, "root");
        assert!(node.children.is_empty());
        assert!(!node.expanded);
        assert!(node.icon.is_none());
    }

    #[test]
    fn test_tree_node_with_children() {
        let child = TreeNode::new("child");
        let node = TreeNode::with_children("root", vec![child.clone()]);
        assert_eq!(node.label, "root");
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].label, child.label);
    }

    #[test]
    fn test_tree_node_icon() {
        let node = TreeNode::new("root").icon("*");
        assert_eq!(node.icon.as_deref(), Some("*"));
    }

    #[test]
    fn test_tree_default_collapsed() {
        let node = TreeNode::new("root");
        assert!(!node.expanded);
    }

    #[test]
    fn test_flatten_visible_single_node() {
        let root = TreeNode::new("root");
        let flat = flatten_visible(&root, 0, true);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].label, "root");
        assert_eq!(flat[0].depth, 0);
        assert!(flat[0].is_last_sibling);
    }

    #[test]
    fn test_flatten_visible_expanded() {
        let root = TreeNode::with_children("root", vec![TreeNode::new("a"), TreeNode::new("b")])
            .expanded(true);
        let flat = flatten_visible(&root, 0, true);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[1].label, "a");
        assert_eq!(flat[2].label, "b");
    }

    #[test]
    fn test_flatten_visible_collapsed() {
        let root = TreeNode::with_children("root", vec![TreeNode::new("a"), TreeNode::new("b")]);
        let flat = flatten_visible(&root, 0, true);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].label, "root");
    }

    #[test]
    fn test_flatten_visible_nested() {
        let root = TreeNode::with_children(
            "root",
            vec![
                TreeNode::with_children("a", vec![TreeNode::new("a1")]).expanded(true),
                TreeNode::new("b"),
            ],
        )
        .expanded(true);
        let flat = flatten_visible(&root, 0, true);
        assert_eq!(flat.len(), 4);
        assert_eq!(flat[0].depth, 0);
        assert_eq!(flat[1].depth, 1);
        assert_eq!(flat[2].depth, 2);
        assert!(!flat[1].is_last_sibling);
        assert!(flat[2].is_last_sibling);
    }

    #[test]
    fn test_tree_children_count() {
        let root = TreeNode::with_children("root", vec![TreeNode::new("a")]).expanded(true);
        let tree = Tree::new(root);
        assert_eq!(<Tree as View>::children_count(&tree), 2);
    }

    #[test]
    fn test_tree_constraints_are_column() {
        let tree = Tree::new(TreeNode::new("root"));
        let constraints = <Tree as View>::constraints(&tree);
        assert_eq!(constraints.display, Display::Flex);
        assert_eq!(constraints.flex_direction, FlexDirection::Column);
        assert_eq!(constraints.width, Dimension::Percent(100.0));
    }

    #[test]
    fn test_tree_with_id_is_focusable() {
        let tree = Tree::new(TreeNode::new("root")).id("tree");
        assert!(tree.is_focusable());
        assert_eq!(tree.widget_id(), Some("tree"));
    }

    #[test]
    fn test_tree_without_id_not_focusable() {
        let tree = Tree::new(TreeNode::new("root"));
        assert!(!tree.is_focusable());
        assert_eq!(tree.widget_id(), None);
    }

    #[test]
    fn test_tree_selected_builder() {
        let tree = Tree::new(TreeNode::new("root")).selected(3);
        assert_eq!(tree.selected, 3);
    }

    #[test]
    fn test_tree_show_root() {
        let root = TreeNode::with_children("root", vec![TreeNode::new("a")]).expanded(true);
        let tree = Tree::new(root).show_root(false);
        assert_eq!(tree.visible_nodes().len(), 1);
        assert_eq!(tree.visible_nodes()[0].label, "a");
    }

    #[test]
    fn test_tree_enter_toggles_expanded() {
        let mut tree = Tree::new(TreeNode::with_children("root", vec![TreeNode::new("a")]));
        let key = crate::event::KeyEvent {
            key: crate::event::Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        tree.on_key(&key);
        assert!(tree.root.expanded);
    }

    #[test]
    fn test_tree_jk_navigation() {
        let mut tree = Tree::new(
            TreeNode::with_children("root", vec![TreeNode::new("a"), TreeNode::new("b")])
                .expanded(true),
        );
        let down = crate::event::KeyEvent {
            key: crate::event::Key::Char('j'),
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        let up = crate::event::KeyEvent {
            key: crate::event::Key::Char('k'),
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };

        tree.on_key(&down);
        assert_eq!(tree.selected, 1);
        tree.on_key(&up);
        assert_eq!(tree.selected, 0);
    }
}
