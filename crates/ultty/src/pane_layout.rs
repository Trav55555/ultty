//! How a tab's panes are arranged: a tree of splits.
//!
//! A leaf is one pane (a terminal). A split lays its children side by side
//! (`Horizontal`) or stacked (`Vertical`), each child taking a share of the
//! split's space given by its ratio; ratios within a split sum to 1.
//! Splitting a pane in the direction of the split it already sits in adds a
//! sibling to that split; splitting it the other way nests a new split in
//! its place. Geometry (pane rectangles, dividers, neighbours) comes from
//! the tree, so rendering, grid sizing, divider dragging and focus moves
//! all agree.

use crate::terminal_manager::{SplitDirection, TerminalId};

/// A rectangle in pixels: x, y, width, height.
pub type Rect = (f32, f32, f32, f32);

/// Smallest share a pane keeps when a divider is dragged.
const MIN_RATIO: f32 = 0.1;

#[derive(Debug, Clone, PartialEq)]
pub enum PaneNode {
    Leaf(TerminalId),
    Split {
        direction: SplitDirection,
        /// Children and their shares of this split; the shares sum to 1.
        children: Vec<(PaneNode, f32)>,
    },
}

/// A divider between two neighbouring children of one split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divider {
    /// The split it belongs to, as child indices from the root.
    path: DividerPath,
    /// It sits between child `index` and child `index + 1`.
    index: usize,
    pub direction: SplitDirection,
    /// Where it is drawn: a zero-width (side by side) or zero-height
    /// (stacked) line along the boundary.
    pub line: Rect,
    /// The extent of its split along the split direction, in pixels.
    split_size: f32,
}

/// Child indices from the root to a split; at most 16 levels deep.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DividerPath {
    len: u8,
    steps: [u8; 16],
}

impl DividerPath {
    fn root() -> Self {
        Self {
            len: 0,
            steps: [0; 16],
        }
    }

    fn child(mut self, index: usize) -> Self {
        if (self.len as usize) < self.steps.len() {
            self.steps[self.len as usize] = index as u8;
            self.len += 1;
        }
        self
    }

    fn steps(&self) -> &[u8] {
        &self.steps[..self.len as usize]
    }
}

/// A direction to move focus in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneLayout {
    root: PaneNode,
}

impl PaneLayout {
    /// A layout holding one pane.
    pub fn new(pane: TerminalId) -> Self {
        Self {
            root: PaneNode::Leaf(pane),
        }
    }

    /// Build from a tree, e.g. one restored from a saved session.
    pub fn from_root(root: PaneNode) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &PaneNode {
        &self.root
    }

    /// Swap two existing leaves, preserving split weights and process identities.
    pub fn swap(&mut self, first: TerminalId, second: TerminalId) -> bool {
        let panes = self.panes();
        if !panes.contains(&first) || !panes.contains(&second) {
            return false;
        }
        fn replace(node: &mut PaneNode, first: TerminalId, second: TerminalId) {
            match node {
                PaneNode::Leaf(id) if *id == first => *id = second,
                PaneNode::Leaf(id) if *id == second => *id = first,
                PaneNode::Leaf(_) => {}
                PaneNode::Split { children, .. } => {
                    for (child, _) in children {
                        replace(child, first, second);
                    }
                }
            }
        }
        replace(&mut self.root, first, second);
        true
    }

    /// Panes in order: left to right, top to bottom, depth first.
    pub fn panes(&self) -> Vec<TerminalId> {
        let mut panes = Vec::new();
        collect_leaves(&self.root, &mut panes);
        panes
    }

    /// The root split's direction, or `Horizontal` for a single pane.
    pub fn root_direction(&self) -> SplitDirection {
        match &self.root {
            PaneNode::Split { direction, .. } => *direction,
            PaneNode::Leaf(_) => SplitDirection::default(),
        }
    }

    /// Put `new` next to `target`, side by side or stacked. Returns false
    /// when `target` is not in the layout.
    pub fn split(
        &mut self,
        target: TerminalId,
        direction: SplitDirection,
        new: TerminalId,
    ) -> bool {
        split_node(&mut self.root, target, direction, new)
    }

    /// Remove a pane; its share goes to its neighbour, and a split left with
    /// one child is replaced by that child. The last pane cannot be removed.
    pub fn remove(&mut self, pane: TerminalId) -> bool {
        if self.root == PaneNode::Leaf(pane) {
            return false;
        }
        let removed = remove_leaf(&mut self.root, pane);
        if removed {
            collapse(&mut self.root);
        }
        removed
    }

    /// Each pane's rectangle within `area`.
    pub fn rects(&self, area: Rect) -> Vec<(TerminalId, Rect)> {
        let mut rects = Vec::new();
        layout_node(&self.root, area, &mut rects);
        rects
    }

    /// Every divider within `area`.
    pub fn dividers(&self, area: Rect) -> Vec<Divider> {
        let mut dividers = Vec::new();
        collect_dividers(&self.root, area, DividerPath::root(), &mut dividers);
        dividers
    }

    /// The divider within `tolerance` pixels of (x, y), if any.
    pub fn divider_at(&self, area: Rect, x: f32, y: f32, tolerance: f32) -> Option<Divider> {
        self.dividers(area).into_iter().find(|divider| {
            let (lx, ly, lw, lh) = divider.line;
            match divider.direction {
                SplitDirection::Horizontal => {
                    (x - lx).abs() <= tolerance && y >= ly && y <= ly + lh
                }
                SplitDirection::Vertical => (y - ly).abs() <= tolerance && x >= lx && x <= lx + lw,
            }
        })
    }

    /// Move `divider` by `delta` pixels along its split, keeping each side
    /// at least `MIN_RATIO` of the split.
    pub fn drag(&mut self, divider: &Divider, delta: f32) {
        if divider.split_size <= 0.0 {
            return;
        }
        let Some(PaneNode::Split { children, .. }) =
            node_at_mut(&mut self.root, divider.path.steps())
        else {
            return;
        };
        let i = divider.index;
        if i + 1 >= children.len() {
            return;
        }
        let pair = children[i].1 + children[i + 1].1;
        let shift = delta / divider.split_size;
        let left = (children[i].1 + shift)
            .clamp(MIN_RATIO.min(pair / 2.0), pair - MIN_RATIO.min(pair / 2.0));
        children[i].1 = left;
        children[i + 1].1 = pair - left;
    }

    /// The pane to move focus to from `from` in `direction`: the nearest pane
    /// on that side that overlaps it across the other axis.
    pub fn neighbor(
        &self,
        area: Rect,
        from: TerminalId,
        direction: FocusDirection,
    ) -> Option<TerminalId> {
        let rects = self.rects(area);
        let (_, (fx, fy, fw, fh)) = *rects.iter().find(|(id, _)| *id == from)?;
        const EPS: f32 = 0.5;
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|(id, (x, y, w, h))| {
                let overlaps_y = *y < fy + fh - EPS && y + h > fy + EPS;
                let overlaps_x = *x < fx + fw - EPS && x + w > fx + EPS;
                let gap = match direction {
                    FocusDirection::Right if overlaps_y && *x >= fx + fw - EPS => x - (fx + fw),
                    FocusDirection::Left if overlaps_y && x + w <= fx + EPS => fx - (x + w),
                    FocusDirection::Down if overlaps_x && *y >= fy + fh - EPS => y - (fy + fh),
                    FocusDirection::Up if overlaps_x && y + h <= fy + EPS => fy - (y + h),
                    _ => return None,
                };
                // Nearest first; among equals, the one most aligned with `from`.
                let offset = match direction {
                    FocusDirection::Left | FocusDirection::Right => (y - fy).abs(),
                    FocusDirection::Up | FocusDirection::Down => (x - fx).abs(),
                };
                Some((gap, offset, *id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)))
            .map(|(_, _, id)| id)
    }
}

fn collect_leaves(node: &PaneNode, out: &mut Vec<TerminalId>) {
    match node {
        PaneNode::Leaf(id) => out.push(*id),
        PaneNode::Split { children, .. } => {
            for (child, _) in children {
                collect_leaves(child, out);
            }
        }
    }
}

fn split_node(
    node: &mut PaneNode,
    target: TerminalId,
    direction: SplitDirection,
    new: TerminalId,
) -> bool {
    match node {
        PaneNode::Leaf(id) if *id == target => {
            *node = PaneNode::Split {
                direction,
                children: vec![(PaneNode::Leaf(target), 0.5), (PaneNode::Leaf(new), 0.5)],
            };
            true
        }
        PaneNode::Leaf(_) => false,
        PaneNode::Split {
            direction: own,
            children,
        } => {
            // Same direction: the new pane joins this split right after the
            // target and takes half of the target's share.
            if *own == direction {
                if let Some(index) = children
                    .iter()
                    .position(|(child, _)| *child == PaneNode::Leaf(target))
                {
                    let half = children[index].1 / 2.0;
                    children[index].1 = half;
                    children.insert(index + 1, (PaneNode::Leaf(new), half));
                    return true;
                }
            }
            children
                .iter_mut()
                .any(|(child, _)| split_node(child, target, direction, new))
        }
    }
}

fn remove_leaf(node: &mut PaneNode, pane: TerminalId) -> bool {
    let PaneNode::Split { children, .. } = node else {
        return false;
    };
    if let Some(index) = children
        .iter()
        .position(|(child, _)| *child == PaneNode::Leaf(pane))
    {
        let (_, share) = children.remove(index);
        // The freed share goes to the neighbour that was next to it.
        let heir = if index > 0 { index - 1 } else { 0 };
        if let Some((_, ratio)) = children.get_mut(heir) {
            *ratio += share;
        }
        return true;
    }
    children
        .iter_mut()
        .any(|(child, _)| remove_leaf(child, pane))
}

/// Replace every split left with a single child by that child.
fn collapse(node: &mut PaneNode) {
    if let PaneNode::Split { children, .. } = node {
        for (child, _) in children.iter_mut() {
            collapse(child);
        }
        if children.len() == 1 {
            let (only, _) = children.pop().expect("one child");
            *node = only;
        }
    }
}

fn layout_node(node: &PaneNode, (x, y, w, h): Rect, out: &mut Vec<(TerminalId, Rect)>) {
    match node {
        PaneNode::Leaf(id) => out.push((*id, (x, y, w, h))),
        PaneNode::Split {
            direction,
            children,
        } => {
            let mut offset = 0.0;
            for (child, ratio) in children {
                let rect = match direction {
                    SplitDirection::Horizontal => (x + offset, y, w * ratio, h),
                    SplitDirection::Vertical => (x, y + offset, w, h * ratio),
                };
                offset += match direction {
                    SplitDirection::Horizontal => w * ratio,
                    SplitDirection::Vertical => h * ratio,
                };
                layout_node(child, rect, out);
            }
        }
    }
}

fn collect_dividers(
    node: &PaneNode,
    (x, y, w, h): Rect,
    path: DividerPath,
    out: &mut Vec<Divider>,
) {
    let PaneNode::Split {
        direction,
        children,
    } = node
    else {
        return;
    };
    let size = match direction {
        SplitDirection::Horizontal => w,
        SplitDirection::Vertical => h,
    };
    let mut offset = 0.0;
    for (index, (child, ratio)) in children.iter().enumerate() {
        let extent = size * ratio;
        let rect = match direction {
            SplitDirection::Horizontal => (x + offset, y, extent, h),
            SplitDirection::Vertical => (x, y + offset, w, extent),
        };
        collect_dividers(child, rect, path.child(index), out);
        offset += extent;
        if index + 1 < children.len() {
            let line = match direction {
                SplitDirection::Horizontal => (x + offset, y, 0.0, h),
                SplitDirection::Vertical => (x, y + offset, w, 0.0),
            };
            out.push(Divider {
                path,
                index,
                direction: *direction,
                line,
                split_size: size,
            });
        }
    }
}

fn node_at_mut<'a>(node: &'a mut PaneNode, steps: &[u8]) -> Option<&'a mut PaneNode> {
    let Some((first, rest)) = steps.split_first() else {
        return Some(node);
    };
    match node {
        PaneNode::Split { children, .. } => {
            let (child, _) = children.get_mut(*first as usize)?;
            node_at_mut(child, rest)
        }
        PaneNode::Leaf(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(n: usize) -> TerminalId {
        TerminalId::new(n)
    }

    const AREA: Rect = (0.0, 0.0, 800.0, 600.0);

    #[test]
    fn splitting_the_same_way_adds_a_sibling() {
        let mut layout = PaneLayout::new(t(0));
        assert!(layout.split(t(0), SplitDirection::Horizontal, t(1)));
        assert!(layout.split(t(1), SplitDirection::Horizontal, t(2)));
        assert_eq!(layout.panes(), [t(0), t(1), t(2)]);
        let rects = layout.rects(AREA);
        assert_eq!(rects[0].1, (0.0, 0.0, 400.0, 600.0));
        assert_eq!(rects[1].1, (400.0, 0.0, 200.0, 600.0));
        assert_eq!(rects[2].1, (600.0, 0.0, 200.0, 600.0));
    }

    #[test]
    fn splitting_the_other_way_nests() {
        // [0 | 1], then split 1 down: [0 | (1 / 2)].
        let mut layout = PaneLayout::new(t(0));
        layout.split(t(0), SplitDirection::Horizontal, t(1));
        layout.split(t(1), SplitDirection::Vertical, t(2));
        let rects = layout.rects(AREA);
        assert_eq!(rects[0], (t(0), (0.0, 0.0, 400.0, 600.0)));
        assert_eq!(rects[1], (t(1), (400.0, 0.0, 400.0, 300.0)));
        assert_eq!(rects[2], (t(2), (400.0, 300.0, 400.0, 300.0)));
        assert_eq!(layout.root_direction(), SplitDirection::Horizontal);
    }

    #[test]
    fn removing_gives_the_space_back_and_collapses() {
        let mut layout = PaneLayout::new(t(0));
        layout.split(t(0), SplitDirection::Horizontal, t(1));
        layout.split(t(1), SplitDirection::Vertical, t(2));
        assert!(layout.remove(t(2)));
        // The nested split had one child left and is gone.
        assert_eq!(layout.rects(AREA)[1], (t(1), (400.0, 0.0, 400.0, 600.0)));
        assert!(layout.remove(t(0)));
        assert_eq!(layout.root(), &PaneNode::Leaf(t(1)));
        assert!(!layout.remove(t(1)), "the last pane stays");
        assert!(!layout.remove(t(9)), "unknown panes are ignored");
    }

    #[test]
    fn dividers_sit_between_children_and_drag_within_bounds() {
        let mut layout = PaneLayout::new(t(0));
        layout.split(t(0), SplitDirection::Horizontal, t(1));
        layout.split(t(1), SplitDirection::Vertical, t(2));
        let dividers = layout.dividers(AREA);
        assert_eq!(dividers.len(), 2);
        let vertical_line = layout.divider_at(AREA, 401.0, 100.0, 4.0).unwrap();
        assert_eq!(vertical_line.direction, SplitDirection::Horizontal);
        let nested = layout.divider_at(AREA, 600.0, 299.0, 4.0).unwrap();
        assert_eq!(nested.direction, SplitDirection::Vertical);
        assert!(layout.divider_at(AREA, 200.0, 300.0, 4.0).is_none());

        layout.drag(&vertical_line, 200.0);
        assert_eq!(layout.rects(AREA)[0].1, (0.0, 0.0, 600.0, 600.0));
        // Dragging far keeps a minimum share on the other side.
        let line = layout.divider_at(AREA, 600.0, 100.0, 4.0).unwrap();
        layout.drag(&line, 10_000.0);
        assert!(layout.rects(AREA)[1].1 .2 >= 800.0 * MIN_RATIO - 0.01);
    }

    #[test]
    fn focus_moves_to_the_nearest_pane_on_that_side() {
        // [0 | (1 / 2)]
        let mut layout = PaneLayout::new(t(0));
        layout.split(t(0), SplitDirection::Horizontal, t(1));
        layout.split(t(1), SplitDirection::Vertical, t(2));
        assert_eq!(
            layout.neighbor(AREA, t(0), FocusDirection::Right),
            Some(t(1))
        );
        assert_eq!(layout.neighbor(AREA, t(2), FocusDirection::Up), Some(t(1)));
        assert_eq!(
            layout.neighbor(AREA, t(2), FocusDirection::Left),
            Some(t(0))
        );
        assert_eq!(
            layout.neighbor(AREA, t(1), FocusDirection::Down),
            Some(t(2))
        );
        assert_eq!(layout.neighbor(AREA, t(0), FocusDirection::Left), None);
        assert_eq!(layout.neighbor(AREA, t(1), FocusDirection::Up), None);
    }
}
