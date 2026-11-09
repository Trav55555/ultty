//! Native scenes in the accessibility tree (ADR 0021 §1, §3, §6): a
//! session's region tree, each region a node with the role, name and state
//! of its effective semantics ([`effective_semantics`]).
//!
//! [`SceneSnapshot::capture`] copies one region tree (a full-scene `ROOT`
//! or an inline surface) out of a session's scene; [`super::tree::build`]
//! places it in the window's tree through an [`Emitter`].
//!
//! Text is not given as `TextRun`s: AccessKit exposes runs only under a
//! `Label`, `Document`, `Terminal` or text input, and runs inside the
//! pane's `Terminal` would join its document. A region's text is instead
//! its name when it is one line (a `Text` region is then a `Label`, whose
//! value is its name), else one `Label` child per line, which Orca reads
//! line by line in browse mode and flat review.
//!
//! Apps are not trusted: a pane shows at most [`MAX_PANE_NODES`] nodes of
//! native scenes, and since node ids are hashes of names the app partly
//! chooses, a node whose id is already in the tree is left out with its
//! subtree rather than handed to AccessKit, which panics on a duplicate.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use accesskit::{Live as AkLive, Node, NodeId, Rect, Role as AkRole, Toggled as AkToggled};
use ultty_protocol::{Content, Expanded, Live, RegionId, Role, Semantics, Toggled, WidgetContent};

use super::live::{Announcement, LiveRegions};
use super::tree::{region_announcement_node_id, region_line_node_id, region_node_id};
use crate::scene::semantics::{capped, effective_semantics};
use crate::scene::tree::SceneTree;

/// The most lines of a region's text given as `Label` children: the last
/// ones, which in a transcript are the newest. Bounds every rebuild of a
/// long log to a few hundred nodes.
pub const MAX_TEXT_LINES: usize = 200;

/// The most nodes the native scenes of one pane add to the tree, regions
/// and text lines together; past it the rest is left out and a last
/// `Label` says so. Bounds the copy and the rebuild for any app.
pub const MAX_PANE_NODES: usize = 5000;

/// Where a region tree is drawn: its layout's cell (0, 0) at window pixel
/// (`x`, `y`), cells `cell_width` by `cell_height` pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub cell_width: f64,
    pub cell_height: f64,
}

/// One region tree of a native session, as the accessibility tree shows
/// it: hidden regions and their subtrees already left out, and at most
/// [`MAX_PANE_NODES`] nodes' worth of it.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSnapshot {
    session: u64,
    /// The tree's root region, first in `regions`.
    root: u32,
    /// Depth first, each before its children.
    regions: Vec<RegionSnapshot>,
    /// The session's focused region, when it is in this tree.
    focus: Option<u32>,
    /// Whether regions or lines were left out to stay within the budget.
    truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct RegionSnapshot {
    id: u32,
    /// Children shown, in order. Some may be missing from the snapshot
    /// when it was truncated.
    children: Vec<u32>,
    semantics: Semantics,
    /// The `Input` widget's flags, for the text input role.
    input: Option<InputKind>,
    /// The lines of its text that its name does not already say, with
    /// their line numbers in the text: the last non-empty ones.
    lines: Vec<(usize, String)>,
    /// What its live queue has announced, newest last; empty until
    /// [`SceneSnapshot::set_announcements`].
    announcements: Vec<Announcement>,
    bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct InputKind {
    multiline: bool,
    password: bool,
}

impl SceneSnapshot {
    /// Copy the tree under `root` of `session`'s `scene`, drawn at `at`.
    /// `focused` is the session's focused region and `scroll` gives a
    /// region's scroll offset in cells; a region's bounds are moved by the
    /// offsets of the regions above it, as it is drawn.
    ///
    /// Copies at most [`MAX_PANE_NODES`] regions and lines, and of each
    /// region's text only the lines it shows.
    pub fn capture(
        scene: &SceneTree,
        root: RegionId,
        session: u64,
        focused: Option<RegionId>,
        scroll: impl Fn(RegionId) -> (f32, f32),
        at: Placement,
    ) -> Self {
        let mut regions = Vec::new();
        let mut focus = None;
        let mut budget = MAX_PANE_NODES;
        let mut truncated = false;
        // (region, scroll of the regions above it), children pushed in
        // reverse so they come out in order.
        let mut stack = Vec::new();
        if scene
            .get(root)
            .is_some_and(|region| !region.semantics().hidden)
        {
            stack.push((root, (0.0_f32, 0.0_f32)));
        }
        while let Some((id, (scroll_x, scroll_y))) = stack.pop() {
            let Some(region) = scene.get(id) else {
                continue;
            };
            if budget == 0 {
                truncated = true;
                break;
            }
            budget -= 1;
            let children: Vec<u32> = region
                .children
                .iter()
                .filter(|&&child| {
                    scene
                        .get(child)
                        .is_some_and(|child| !child.semantics().hidden)
                })
                .map(|child| child.0)
                .collect();
            let (own_x, own_y) = scroll(id);
            let below = (scroll_x + own_x, scroll_y + own_y);
            stack.extend(
                children
                    .iter()
                    .rev()
                    .map(|&child| (RegionId::new(child), below)),
            );
            if Some(id) == focused {
                focus = Some(id.0);
            }
            let semantics = effective_semantics(region);
            let lines = match content_text(&region.content) {
                Some(text) => {
                    let lines = shown_lines(&text, semantics.label.as_deref(), budget);
                    truncated |= lines.len() == budget && budget < MAX_TEXT_LINES;
                    lines
                }
                None => Vec::new(),
            };
            budget -= lines.len();
            let layout = region.computed;
            let x0 = at.x + f64::from(layout.x - scroll_x) * at.cell_width;
            let y0 = at.y + f64::from(layout.y - scroll_y) * at.cell_height;
            regions.push(RegionSnapshot {
                id: id.0,
                children,
                semantics,
                input: input_kind(&region.content),
                lines,
                announcements: Vec::new(),
                bounds: Rect {
                    x0,
                    y0,
                    x1: x0 + f64::from(layout.width) * at.cell_width,
                    y1: y0 + f64::from(layout.height) * at.cell_height,
                },
            });
        }
        Self {
            session,
            root: root.0,
            regions,
            focus,
            truncated,
        }
    }

    /// Copy each region's kept announcements out of `live`.
    pub fn set_announcements(&mut self, live: &LiveRegions) {
        for region in &mut self.regions {
            region.announcements = live
                .announcements(self.session, region.id)
                .cloned()
                .collect();
        }
    }

    /// Whether the tree has nothing to show: its root is hidden.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The root region's node.
    pub fn root_node_id(&self) -> NodeId {
        region_node_id(self.session, self.root)
    }

    /// The focused region's node, when it is in this tree.
    pub fn focus_node_id(&self) -> Option<NodeId> {
        self.focus
            .map(|region| region_node_id(self.session, region))
    }

    /// Add a node for every region, and the `Label`s of their text lines,
    /// to `out`, spending one of `budget` per node; the root is
    /// `described_by` that node too, if given. Returns the root's id, or
    /// `None` when nothing was added: an empty tree, no budget left, or a
    /// root whose id is taken.
    pub(super) fn emit(
        &self,
        out: &mut Emitter,
        budget: &mut usize,
        described_by: Option<NodeId>,
    ) -> Option<NodeId> {
        if self.truncated {
            out.truncated = true;
        }
        let by_id: HashMap<u32, usize> = self
            .regions
            .iter()
            .enumerate()
            .map(|(index, region)| (region.id, index))
            .collect();
        let root = self.emit_region(0, &by_id, out, budget, false)?;
        if let Some(target) = described_by {
            // The root is pushed after its subtree, so it is the last node.
            if let Some((_, node)) = out.nodes.last_mut() {
                node.push_described_by(target);
            }
        }
        Some(root)
    }

    /// Add region `index` and its subtree. `in_live` says whether a region
    /// above it is live: AccessKit's liveness is inherited, so every node
    /// under a live region that is not live itself is marked `Off`, else
    /// each new or renamed one (a line, a growing child) would be spoken
    /// besides, and faster than, the region's announcements.
    fn emit_region(
        &self,
        index: usize,
        by_id: &HashMap<u32, usize>,
        out: &mut Emitter,
        budget: &mut usize,
        in_live: bool,
    ) -> Option<NodeId> {
        let region = self.regions.get(index)?;
        if *budget == 0 {
            out.truncated = true;
            return None;
        }
        let id = region_node_id(self.session, region.id);
        if !out.claim(id) {
            return None;
        }
        *budget -= 1;
        let mut node = region_node(region);
        let live = region.semantics.live != Live::Off;
        if in_live && !live {
            node.set_live(AkLive::Off);
        }
        // The announcements are added last but counted first, so the text
        // and children cannot take all the budget and hold them back to
        // burst out in a later tree.
        let reserved = region.announcements.len().min(*budget);
        *budget -= reserved;
        // The text comes before any child regions, as it is drawn.
        let mut children = Vec::with_capacity(region.lines.len() + region.children.len());
        for (line, text) in &region.lines {
            if *budget == 0 {
                out.truncated = true;
                break;
            }
            let line_id = region_line_node_id(self.session, region.id, *line);
            if out.claim(line_id) {
                *budget -= 1;
                let mut label = Node::new(AkRole::Label);
                label.set_value(text.as_str());
                if live || in_live {
                    label.set_live(AkLive::Off);
                }
                out.nodes.push((line_id, label));
                children.push(line_id);
            }
        }
        for child in &region.children {
            if let Some(&child) = by_id.get(child) {
                if let Some(child) = self.emit_region(child, by_id, out, budget, live || in_live) {
                    children.push(child);
                }
            }
        }
        // Then its announcements, newest last; adding one is what makes
        // the platform speak it.
        *budget += reserved;
        for announcement in &region.announcements {
            if *budget == 0 {
                out.truncated = true;
                break;
            }
            let id = region_announcement_node_id(self.session, region.id, announcement.serial);
            if out.claim(id) {
                *budget -= 1;
                let mut label = Node::new(AkRole::Label);
                label.set_value(announcement.text.as_str());
                label.set_live(match announcement.live {
                    Live::Assertive => AkLive::Assertive,
                    Live::Polite | Live::Off => AkLive::Polite,
                });
                out.nodes.push((id, label));
                children.push(id);
            }
        }
        node.set_children(children);

        let semantics = &region.semantics;
        for (kind, target) in [
            (RelationKind::LabelledBy, semantics.labelled_by),
            (RelationKind::DescribedBy, semantics.described_by),
            (RelationKind::Controls, semantics.controls),
        ] {
            if let Some(target) = target {
                out.relations.push(Relation {
                    node: out.nodes.len(),
                    kind,
                    target: region_node_id(self.session, target.0),
                });
            }
        }
        out.nodes.push((id, node));
        Some(id)
    }
}

/// The nodes of one tree update as they are built, each id at most once.
#[derive(Debug, Default)]
pub(super) struct Emitter {
    pub(super) nodes: Vec<(NodeId, Node)>,
    relations: Vec<Relation>,
    /// Every id pushed or reserved.
    taken: HashSet<NodeId>,
    /// Native nodes left out because their id was taken.
    collisions: usize,
    /// Whether native nodes were left out for the budget since this was
    /// last cleared.
    pub(super) truncated: bool,
}

/// Whether a collision was logged; once per run is enough.
static COLLISION_LOGGED: AtomicBool = AtomicBool::new(false);

impl Emitter {
    /// Push one of Ultty's own nodes (window, tabs, panes, rows), whose
    /// ids are distinct by construction.
    pub(super) fn push_fixed(&mut self, id: NodeId, node: Node) {
        self.taken.insert(id);
        self.nodes.push((id, node));
    }

    /// Keep `id` for one of Ultty's own nodes pushed later, so no native
    /// node takes it first.
    pub(super) fn reserve(&mut self, id: NodeId) {
        self.taken.insert(id);
    }

    /// Whether `id` is in the tree or reserved.
    pub(super) fn contains(&self, id: NodeId) -> bool {
        self.taken.contains(&id)
    }

    /// Take `id` for a native node; false, and the node must be left
    /// out, when it is already taken.
    fn claim(&mut self, id: NodeId) -> bool {
        let fresh = self.taken.insert(id);
        if !fresh {
            self.collisions += 1;
        }
        fresh
    }

    /// The nodes, with each relationship whose target was built added to
    /// its node; a target that does not exist, is hidden or is not shown
    /// is dropped (spec §10.5).
    pub(super) fn finish(mut self) -> Vec<(NodeId, Node)> {
        if self.collisions > 0 && !COLLISION_LOGGED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: "ultty::a11y",
                count = self.collisions,
                "native accessibility nodes left out: their ids collide"
            );
        }
        for relation in &self.relations {
            if !self.taken.contains(&relation.target) {
                continue;
            }
            let node = &mut self.nodes[relation.node].1;
            match relation.kind {
                RelationKind::LabelledBy => node.push_labelled_by(relation.target),
                RelationKind::DescribedBy => node.push_described_by(relation.target),
                RelationKind::Controls => node.push_controlled(relation.target),
            }
        }
        self.nodes
    }
}

/// A relationship of a region node to another region, kept until every
/// node is built, so that a target not in the tree can be dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Relation {
    /// Index of the node in the emitter's node list.
    node: usize,
    kind: RelationKind,
    target: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RelationKind {
    LabelledBy,
    DescribedBy,
    Controls,
}

/// A region content's text: borrowed, except rich text, whose spans are
/// joined (a region holds at most 4 MiB).
fn content_text(content: &Content) -> Option<Cow<'_, str>> {
    match content {
        Content::Text(text) => Some(Cow::Borrowed(&text.text)),
        Content::RichText(rich) => Some(Cow::Owned(
            rich.spans.iter().map(|span| span.text.as_str()).collect(),
        )),
        Content::Widget(WidgetContent::Markdown(markdown)) => Some(Cow::Borrowed(&markdown.source)),
        Content::Widget(WidgetContent::Code(code)) => Some(Cow::Borrowed(&code.source)),
        _ => None,
    }
}

/// An `Input` widget's flags.
fn input_kind(content: &Content) -> Option<InputKind> {
    match content {
        Content::Widget(WidgetContent::Input(input)) => Some(InputKind {
            multiline: input.multiline,
            password: input.password,
        }),
        _ => None,
    }
}

/// The lines of `text` to give as `Label`s under a region named `label`:
/// none when the name says the whole text (as far as a [`capped`] name
/// can); else the last non-empty lines,
/// at most [`MAX_TEXT_LINES`] and `budget`, with their line numbers, in
/// order. Copies only those lines.
fn shown_lines(text: &str, label: Option<&str>, budget: usize) -> Vec<(usize, String)> {
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.trim().is_empty() || label.is_some_and(|label| *label == *capped(line)) {
        return Vec::new();
    }
    // As many as `str::lines` gives: a last line without its newline counts.
    let count = text.bytes().filter(|&b| b == b'\n').count() + usize::from(!text.ends_with('\n'));
    let mut lines: Vec<(usize, String)> = text
        .lines()
        .rev()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(MAX_TEXT_LINES.min(budget))
        .map(|(from_end, line)| (count - 1 - from_end, capped(line).into_owned()))
        .collect();
    lines.reverse();
    lines
}

/// The node for `region`, its children not yet set.
fn region_node(region: &RegionSnapshot) -> Node {
    let semantics = &region.semantics;
    let label = semantics.label.as_deref();
    let role = match semantics.role {
        // A one-line text is a label, named by its value; one with a value
        // of its own is a paragraph, which can carry both.
        Role::Text if region.lines.is_empty() && semantics.value_text.is_none() => AkRole::Label,
        Role::Text => AkRole::Paragraph,
        Role::Auto | Role::Group => AkRole::Group,
        Role::Heading => AkRole::Heading,
        Role::Log => AkRole::Log,
        Role::Status => AkRole::Status,
        Role::Alert => AkRole::Alert,
        Role::Dialog => AkRole::Dialog,
        Role::Input => match region.input {
            Some(InputKind { password: true, .. }) => AkRole::PasswordInput,
            Some(InputKind {
                multiline: true, ..
            }) => AkRole::MultilineTextInput,
            _ => AkRole::TextInput,
        },
        Role::Button => AkRole::Button,
        Role::Checkbox => AkRole::CheckBox,
        Role::Switch => AkRole::Switch,
        Role::Progress => AkRole::ProgressIndicator,
        Role::List => AkRole::List,
        Role::ListItem => AkRole::ListItem,
        Role::TabList => AkRole::TabList,
        Role::Tab => AkRole::Tab,
        Role::Tree => AkRole::Tree,
        Role::TreeItem => AkRole::TreeItem,
        Role::Image => AkRole::Image,
        Role::Link => AkRole::Link,
    };
    let mut node = Node::new(role);
    node.set_bounds(region.bounds);
    if role == AkRole::Label {
        // AccessKit names a label by its value.
        if let Some(label) = label {
            node.set_value(capped(label));
        }
    } else {
        if let Some(label) = label {
            node.set_label(capped(label));
        }
        if let Some(value) = &semantics.value_text {
            node.set_value(capped(value));
        }
    }
    if let Some(description) = &semantics.description {
        node.set_description(capped(description));
    }
    set_state(&mut node, semantics);
    node
}

/// The state fields: value and range, toggled, expanded, level, position
/// in set, the flags and liveness. Out-of-range numbers are clamped as
/// spec §10.5 says.
fn set_state(node: &mut Node, semantics: &Semantics) {
    if let Some(value) = semantics.value {
        let finite = [value.now, value.min, value.max]
            .iter()
            .all(|v| v.is_finite());
        if finite && value.min <= value.max {
            node.set_min_numeric_value(f64::from(value.min));
            node.set_max_numeric_value(f64::from(value.max));
            node.set_numeric_value(f64::from(value.now.clamp(value.min, value.max)));
        } else if value.now.is_finite() {
            // No known range.
            node.set_numeric_value(f64::from(value.now));
        }
    }
    match semantics.toggled {
        Toggled::Unset => {}
        Toggled::False => node.set_toggled(AkToggled::False),
        Toggled::True => node.set_toggled(AkToggled::True),
        Toggled::Mixed => node.set_toggled(AkToggled::Mixed),
    }
    match semantics.expanded {
        Expanded::Unset => {}
        Expanded::Collapsed => node.set_expanded(false),
        Expanded::Expanded => node.set_expanded(true),
    }
    if semantics.level > 0 {
        node.set_level(usize::from(semantics.level));
    }
    if let Some(position) = semantics.position.filter(|position| position.size > 0) {
        let index = position.index.clamp(1, position.size);
        node.set_position_in_set(index as usize);
        node.set_size_of_set(position.size as usize);
    }
    if semantics.busy {
        node.set_busy();
    }
    if semantics.disabled {
        node.set_disabled();
    }
    if semantics.read_only {
        node.set_read_only();
    }
    if semantics.modal {
        node.set_modal();
    }
    match semantics.live {
        Live::Off => {}
        Live::Polite => node.set_live(AkLive::Polite),
        Live::Assertive => node.set_live(AkLive::Assertive),
    }
    if semantics.atomic {
        node.set_live_atomic();
    }
}

#[cfg(test)]
mod tests {
    use accesskit::TreeUpdate;
    use ultty_protocol::{
        ButtonWidget, InputWidget, ProgressWidget, SemanticValue, SetPosition, SpinnerWidget,
        TextContent,
    };

    use super::*;
    use crate::a11y::tree::{
        build, pane_node_id, row_node_id, GridSnapshot, PaneSnapshot, Snapshot, SurfaceSnapshot,
        TabSnapshot,
    };
    use crate::grid::{Dimensions, Terminal, TerminalHandler};
    use crate::protocol::ansi::AnsiParser;
    use crate::scene::region::{ComputedLayout, Region};

    const AT: Placement = Placement {
        x: 0.0,
        y: 40.0,
        cell_width: 10.0,
        cell_height: 20.0,
    };

    /// A scene built region by region, as a session holds it.
    struct Scene {
        tree: SceneTree,
        focused: Option<RegionId>,
    }

    impl Scene {
        fn new() -> Self {
            Self {
                tree: SceneTree::new(),
                focused: None,
            }
        }

        /// Add region `id` under `parent` with `content`, laid out at cell
        /// (1, `id`), 8 by 1, and return it to set semantics on.
        fn add(&mut self, id: u32, parent: u32, content: Content) -> &mut Region {
            let mut region = Region::new(RegionId::new(id));
            region.parent = Some(RegionId::new(parent));
            region.content = content;
            region.computed = ComputedLayout {
                x: 1.0,
                y: id as f32,
                width: 8.0,
                height: 1.0,
                ..ComputedLayout::default()
            };
            self.tree.insert(region);
            self.tree.get_mut(RegionId::new(id)).expect("just added")
        }

        fn surface(&mut self, id: u32) {
            self.tree
                .try_insert_surface(Region::new(RegionId::new(id)), 2)
                .expect("a surface");
        }

        fn capture(&self, session: u64, root: u32) -> SceneSnapshot {
            SceneSnapshot::capture(
                &self.tree,
                RegionId::new(root),
                session,
                self.focused,
                |_| (0.0, 0.0),
                AT,
            )
        }
    }

    fn text(text: &str) -> Content {
        Content::Text(TextContent::new(text))
    }

    fn widget(widget: WidgetContent) -> Content {
        Content::Widget(widget)
    }

    fn semantics(role: Role) -> Semantics {
        Semantics {
            role,
            ..Semantics::default()
        }
    }

    fn pane(scenes: Vec<SceneSnapshot>, grid: Option<GridSnapshot>) -> Snapshot {
        Snapshot {
            tabs: vec![TabSnapshot {
                id: 1,
                title: "app".to_owned(),
                selected: true,
            }],
            panes: vec![PaneSnapshot {
                id: 1,
                title: "app".to_owned(),
                bounds: Rect {
                    x0: 0.0,
                    y0: 40.0,
                    x1: 200.0,
                    y1: 140.0,
                },
                grid,
                scenes,
                surfaces: Vec::new(),
            }],
        }
    }

    /// The tree of a full-scene app, session 7, showing `scene`'s ROOT.
    fn full_scene(scene: &Scene) -> TreeUpdate {
        build("Ultty", &pane(vec![scene.capture(7, 0)], None))
    }

    fn screen(cols: u16, rows: u16, bytes: &str) -> GridSnapshot {
        let mut terminal = Terminal::new(Dimensions::new(cols, rows));
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(bytes.as_bytes(), |action| handler.handle(action));
        GridSnapshot::capture(terminal.grid(), AT.cell_width, AT.cell_height)
    }

    fn find(update: &TreeUpdate, id: NodeId) -> Option<&Node> {
        update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .map(|(_, node)| node)
    }

    fn node(update: &TreeUpdate, id: NodeId) -> &Node {
        find(update, id).unwrap_or_else(|| panic!("no node {id:?}"))
    }

    fn region(update: &TreeUpdate, id: u32) -> &Node {
        node(update, region_node_id(7, id))
    }

    /// Feed `update` to AccessKit's consumer, which panics on a malformed
    /// tree (an orphan, a duplicate child, a missing focus).
    fn consume(update: TreeUpdate) -> accesskit_consumer::Tree {
        accesskit_consumer::Tree::new(update, true)
    }

    #[test]
    fn explicit_roles_map_to_the_adr_table() {
        let table = [
            (Role::Group, AkRole::Group),
            (Role::Heading, AkRole::Heading),
            (Role::Log, AkRole::Log),
            (Role::Status, AkRole::Status),
            (Role::Alert, AkRole::Alert),
            (Role::Dialog, AkRole::Dialog),
            (Role::Input, AkRole::TextInput),
            (Role::Button, AkRole::Button),
            (Role::Checkbox, AkRole::CheckBox),
            (Role::Switch, AkRole::Switch),
            (Role::Progress, AkRole::ProgressIndicator),
            (Role::List, AkRole::List),
            (Role::ListItem, AkRole::ListItem),
            (Role::TabList, AkRole::TabList),
            (Role::Tab, AkRole::Tab),
            (Role::Tree, AkRole::Tree),
            (Role::TreeItem, AkRole::TreeItem),
            (Role::Image, AkRole::Image),
            (Role::Link, AkRole::Link),
        ];
        let mut scene = Scene::new();
        for (id, (role, _)) in (1..).zip(table) {
            scene
                .add(id, 0, Content::None)
                .set_semantics(semantics(role));
        }
        let update = full_scene(&scene);
        for (id, (role, expected)) in (1..).zip(table) {
            assert_eq!(region(&update, id).role(), expected, "{role:?}");
        }
        consume(update);
    }

    #[test]
    fn derived_roles_come_from_content() {
        let mut scene = Scene::new();
        scene.add(1, 0, Content::None);
        scene.add(2, 0, text("Build ok\n"));
        scene.add(3, 0, text("compiling\n\nfinished"));
        scene.add(4, 0, widget(WidgetContent::Button(ButtonWidget::new("+1"))));
        scene.add(
            5,
            0,
            widget(WidgetContent::Input(InputWidget {
                value: "cargo test".to_owned(),
                placeholder: "Command".to_owned(),
                ..InputWidget::default()
            })),
        );
        scene.add(
            6,
            0,
            widget(WidgetContent::Input(InputWidget {
                value: "hunter2".to_owned(),
                password: true,
                ..InputWidget::default()
            })),
        );
        scene.add(
            7,
            0,
            widget(WidgetContent::Input(InputWidget {
                multiline: true,
                ..InputWidget::default()
            })),
        );
        scene.add(
            8,
            0,
            widget(WidgetContent::Progress(ProgressWidget {
                value: 0.25,
                label: Some("Download".to_owned()),
                animated: false,
                indeterminate: false,
            })),
        );
        scene.add(
            9,
            0,
            widget(WidgetContent::Spinner(SpinnerWidget::default())),
        );
        scene.add(10, 0, text("a\nb")).appended = true;
        let update = full_scene(&scene);

        assert_eq!(region(&update, 1).role(), AkRole::Group);
        assert_eq!(region(&update, 1).label(), None);

        // One line of text is a label named by its value.
        let label = region(&update, 2);
        assert_eq!(
            (label.role(), label.value()),
            (AkRole::Label, Some("Build ok"))
        );
        assert!(label.children().is_empty());

        // Several lines are a paragraph of one label per non-empty line.
        let paragraph = region(&update, 3);
        assert_eq!(paragraph.role(), AkRole::Paragraph);
        assert_eq!(
            paragraph.children(),
            &[region_line_node_id(7, 3, 0), region_line_node_id(7, 3, 2)]
        );
        assert_eq!(
            node(&update, region_line_node_id(7, 3, 2)).value(),
            Some("finished")
        );

        let button = region(&update, 4);
        assert_eq!(
            (button.role(), button.label()),
            (AkRole::Button, Some("+1"))
        );

        let input = region(&update, 5);
        assert_eq!(input.role(), AkRole::TextInput);
        assert_eq!(input.value(), Some("cargo test"));
        assert_eq!(input.description(), Some("Command"));
        assert_eq!(input.label(), None);

        let password = region(&update, 6);
        assert_eq!(password.role(), AkRole::PasswordInput);
        assert_eq!(password.value(), None, "never the password");
        assert_eq!(region(&update, 7).role(), AkRole::MultilineTextInput);

        let progress = region(&update, 8);
        assert_eq!(progress.role(), AkRole::ProgressIndicator);
        assert_eq!(progress.label(), Some("Download"));
        assert_eq!(
            (
                progress.numeric_value(),
                progress.min_numeric_value(),
                progress.max_numeric_value()
            ),
            (Some(0.25), Some(0.0), Some(1.0))
        );

        let spinner = region(&update, 9);
        assert_eq!(spinner.role(), AkRole::Status);
        assert!(spinner.is_busy());

        let log = region(&update, 10);
        assert_eq!(log.role(), AkRole::Log);
        assert_eq!(log.children().len(), 2);
        assert_eq!(log.live(), None, "liveness is never inferred");

        // What a screen reader is told: the label's name is its text.
        let tree = consume(update);
        let state = tree.state();
        let label = state
            .node_by_tree_local_id(region_node_id(7, 2), accesskit::TreeId::ROOT)
            .expect("the label");
        assert_eq!(label.value().as_deref(), Some("Build ok"));
        assert!(label.label_comes_from_value());
    }

    #[test]
    fn an_explicit_label_names_text_whose_lines_are_its_children() {
        let mut scene = Scene::new();
        scene.add(1, 0, text("Hello")).set_semantics(Semantics {
            label: Some("Greeting".to_owned()),
            ..semantics(Role::Text)
        });
        scene.add(2, 0, text("Title")).set_semantics(Semantics {
            level: 2,
            ..semantics(Role::Heading)
        });
        let update = full_scene(&scene);

        let greeting = region(&update, 1);
        assert_eq!(greeting.role(), AkRole::Paragraph);
        assert_eq!(greeting.label(), Some("Greeting"));
        assert_eq!(greeting.children(), &[region_line_node_id(7, 1, 0)]);
        // A heading is named by its one line; the line is not repeated.
        let heading = region(&update, 2);
        assert_eq!(heading.role(), AkRole::Heading);
        assert_eq!(heading.label(), Some("Title"));
        assert_eq!(heading.level(), Some(2));
        assert!(heading.children().is_empty());
    }

    #[test]
    fn a_long_text_keeps_its_last_lines() {
        let lines: Vec<String> = (0..MAX_TEXT_LINES + 50)
            .map(|n| format!("line {n}"))
            .collect();
        let mut scene = Scene::new();
        scene.add(1, 0, text(&lines.join("\n")));
        let update = full_scene(&scene);

        let children = region(&update, 1).children();
        assert_eq!(children.len(), MAX_TEXT_LINES);
        assert_eq!(children[0], region_line_node_id(7, 1, 50));
        assert_eq!(
            node(&update, *children.last().expect("lines")).value(),
            Some(format!("line {}", MAX_TEXT_LINES + 49).as_str())
        );
    }

    #[test]
    fn values_and_positions_are_clamped() {
        let mut scene = Scene::new();
        let value = |now, min, max| Semantics {
            value: Some(SemanticValue { now, min, max }),
            ..semantics(Role::Progress)
        };
        let position = |index, size| Semantics {
            position: Some(SetPosition { index, size }),
            ..semantics(Role::ListItem)
        };
        scene
            .add(1, 0, Content::None)
            .set_semantics(value(5.0, 0.0, 2.0));
        scene
            .add(2, 0, Content::None)
            .set_semantics(value(-1.0, 0.0, 2.0));
        scene
            .add(3, 0, Content::None)
            .set_semantics(value(5.0, 3.0, 1.0));
        scene.add(4, 0, Content::None).set_semantics(position(0, 3));
        scene.add(5, 0, Content::None).set_semantics(position(9, 3));
        scene.add(6, 0, Content::None).set_semantics(position(2, 0));
        let update = full_scene(&scene);

        let numbers = |id| {
            let node = region(&update, id);
            (
                node.numeric_value(),
                node.min_numeric_value(),
                node.max_numeric_value(),
            )
        };
        assert_eq!(numbers(1), (Some(2.0), Some(0.0), Some(2.0)));
        assert_eq!(numbers(2), (Some(0.0), Some(0.0), Some(2.0)));
        // min > max: no known range, the value as sent.
        assert_eq!(numbers(3), (Some(5.0), None, None));

        let place = |id| {
            let node = region(&update, id);
            (node.position_in_set(), node.size_of_set())
        };
        assert_eq!(place(4), (Some(1), Some(3)));
        assert_eq!(place(5), (Some(3), Some(3)));
        assert_eq!(place(6), (None, None), "an empty set has no position");
    }

    #[test]
    fn state_and_flags_are_exposed() {
        let mut scene = Scene::new();
        scene.add(1, 0, Content::None).set_semantics(Semantics {
            label: Some("Wrap".to_owned()),
            toggled: Toggled::True,
            disabled: true,
            ..semantics(Role::Checkbox)
        });
        scene.add(2, 0, Content::None).set_semantics(Semantics {
            toggled: Toggled::Mixed,
            read_only: true,
            ..semantics(Role::Switch)
        });
        scene.add(3, 0, Content::None).set_semantics(Semantics {
            expanded: Expanded::Collapsed,
            level: 3,
            position: Some(SetPosition { index: 2, size: 5 }),
            ..semantics(Role::TreeItem)
        });
        scene.add(4, 0, Content::None).set_semantics(Semantics {
            expanded: Expanded::Expanded,
            ..semantics(Role::TreeItem)
        });
        scene.add(5, 0, Content::None).set_semantics(Semantics {
            modal: true,
            description: Some("Unsaved changes".to_owned()),
            ..semantics(Role::Dialog)
        });
        scene.add(6, 0, text("thinking")).set_semantics(Semantics {
            live: Live::Polite,
            busy: true,
            atomic: true,
            ..semantics(Role::Log)
        });
        scene.add(7, 0, Content::None).set_semantics(Semantics {
            live: Live::Assertive,
            value_text: Some("3 errors".to_owned()),
            ..semantics(Role::Alert)
        });
        let update = full_scene(&scene);

        let checkbox = region(&update, 1);
        assert_eq!(checkbox.label(), Some("Wrap"));
        assert_eq!(checkbox.toggled(), Some(AkToggled::True));
        assert!(checkbox.is_disabled());
        let switch = region(&update, 2);
        assert_eq!(switch.toggled(), Some(AkToggled::Mixed));
        assert!(switch.is_read_only());
        let item = region(&update, 3);
        assert_eq!(item.is_expanded(), Some(false));
        assert_eq!(item.level(), Some(3));
        assert_eq!(
            (item.position_in_set(), item.size_of_set()),
            (Some(2), Some(5))
        );
        assert_eq!(region(&update, 4).is_expanded(), Some(true));
        let dialog = region(&update, 5);
        assert!(dialog.is_modal());
        assert_eq!(dialog.description(), Some("Unsaved changes"));
        let log = region(&update, 6);
        assert_eq!(log.live(), Some(AkLive::Polite));
        assert!(log.is_busy() && log.is_live_atomic());
        let alert = region(&update, 7);
        assert_eq!(alert.live(), Some(AkLive::Assertive));
        assert_eq!(alert.value(), Some("3 errors"));
        consume(update);
    }

    #[test]
    fn a_hidden_region_and_its_subtree_are_left_out() {
        let mut scene = Scene::new();
        scene.add(1, 0, Content::None).set_semantics(Semantics {
            hidden: true,
            ..Semantics::default()
        });
        scene.add(2, 1, text("under the divider"));
        scene.add(3, 0, text("shown"));
        scene.focused = Some(RegionId::new(2));
        let update = full_scene(&scene);

        assert!(find(&update, region_node_id(7, 1)).is_none());
        assert!(find(&update, region_node_id(7, 2)).is_none());
        assert_eq!(region(&update, 0).children(), &[region_node_id(7, 3)]);
        // Focus on a hidden region falls back to the app's root.
        assert_eq!(update.focus, region_node_id(7, 0));
        consume(update);
    }

    #[test]
    fn relationships_resolve_and_dangling_ones_are_dropped() {
        let mut scene = Scene::new();
        scene.add(1, 0, text("Name"));
        scene.add(2, 0, text("Your full name"));
        scene.add(3, 0, Content::None).set_semantics(Semantics {
            hidden: true,
            ..Semantics::default()
        });
        scene
            .add(4, 0, widget(WidgetContent::Input(InputWidget::default())))
            .set_semantics(Semantics {
                labelled_by: Some(RegionId::new(1)),
                described_by: Some(RegionId::new(2)),
                controls: Some(RegionId::new(99)),
                ..semantics(Role::Input)
            });
        scene.add(5, 0, Content::None).set_semantics(Semantics {
            labelled_by: Some(RegionId::new(3)),
            controls: Some(RegionId::new(4)),
            ..semantics(Role::Button)
        });
        let update = full_scene(&scene);

        let input = region(&update, 4);
        assert_eq!(input.labelled_by(), &[region_node_id(7, 1)]);
        assert_eq!(input.described_by(), &[region_node_id(7, 2)]);
        assert!(input.controls().is_empty(), "region 99 does not exist");
        let button = region(&update, 5);
        assert!(button.labelled_by().is_empty(), "region 3 is hidden");
        assert_eq!(button.controls(), &[region_node_id(7, 4)]);

        // AccessKit names the input by the label it points at.
        let tree = consume(update);
        let state = tree.state();
        let input = state
            .node_by_tree_local_id(region_node_id(7, 4), accesskit::TreeId::ROOT)
            .expect("the input");
        assert_eq!(input.label().as_deref(), Some("Name"));
    }

    #[test]
    fn focus_is_on_the_apps_focused_region_or_its_root() {
        let mut scene = Scene::new();
        scene.add(1, 0, widget(WidgetContent::Button(ButtonWidget::new("+1"))));
        scene.add(2, 0, widget(WidgetContent::Button(ButtonWidget::new("-1"))));
        scene.focused = Some(RegionId::new(2));
        let update = full_scene(&scene);

        // The pane is a group holding the app's tree, not a terminal.
        let pane = node(&update, pane_node_id(1));
        assert_eq!(pane.role(), AkRole::Group);
        assert_eq!(pane.label(), Some("app"));
        assert_eq!(pane.children(), &[region_node_id(7, 0)]);
        assert_eq!(update.focus, region_node_id(7, 2));
        let tree = consume(update);
        let state = tree.state();
        let focus = state.focus().expect("a focused node");
        assert_eq!(focus.role(), AkRole::Button);
        assert_eq!(focus.label().as_deref(), Some("-1"));

        // After Blur, the app's root.
        scene.focused = None;
        assert_eq!(full_scene(&scene).focus, region_node_id(7, 0));
    }

    #[test]
    fn bounds_follow_layout_placement_and_scroll() {
        let mut scene = Scene::new();
        scene.add(1, 0, Content::None);
        scene.add(2, 1, Content::None);
        let snapshot = SceneSnapshot::capture(
            &scene.tree,
            RegionId::ROOT,
            7,
            None,
            |id| {
                if id == RegionId::new(1) {
                    (0.0, 1.0)
                } else {
                    (0.0, 0.0)
                }
            },
            AT,
        );
        let update = build("Ultty", &pane(vec![snapshot], None));

        // Region 1 at cells (1, 1), 8 by 1, drawn from pixel (0, 40).
        assert_eq!(
            region(&update, 1).bounds(),
            Some(Rect {
                x0: 10.0,
                y0: 60.0,
                x1: 90.0,
                y1: 80.0
            })
        );
        // Region 2, at row 2, is scrolled up a row by region 1.
        assert_eq!(region(&update, 2).bounds().map(|b| b.y0), Some(60.0));
    }

    #[test]
    fn an_inline_surface_follows_its_anchor_row_and_is_described_by_it() {
        let mut scene = Scene::new();
        scene.surface(5);
        scene.add(6, 5, text("Running tests"));
        scene.surface(8);
        scene.add(9, 8, text("earlier"));
        let mut snapshot = pane(Vec::new(), Some(screen(10, 3, "$ x\r\nok\r\n")));
        snapshot.panes[0].surfaces = vec![
            SurfaceSnapshot {
                row: 1,
                scene: scene.capture(7, 5),
            },
            SurfaceSnapshot {
                row: -1,
                scene: scene.capture(7, 8),
            },
        ];
        let update = build("Ultty", &snapshot);

        let terminal = node(&update, pane_node_id(1));
        assert_eq!(terminal.role(), AkRole::Terminal);
        assert_eq!(
            terminal.children(),
            &[
                // Its anchor row is above the screen: first, undescribed.
                region_node_id(7, 8),
                row_node_id(1, 0),
                row_node_id(1, 1),
                region_node_id(7, 5),
                row_node_id(1, 2),
            ]
        );
        let surface = region(&update, 5);
        assert_eq!(surface.role(), AkRole::Group);
        assert_eq!(surface.described_by(), &[row_node_id(1, 1)]);
        assert!(region(&update, 8).described_by().is_empty());
        assert_eq!(surface.children(), &[region_node_id(7, 6)]);
        // Focus stays on the terminal, and its document is still the grid.
        assert_eq!(update.focus, pane_node_id(1));
        let tree = consume(update);
        let state = tree.state();
        let focus = state.focus().expect("a focused node");
        assert_eq!(focus.document_range().text(), "$ x\nok\n");
    }

    #[test]
    fn node_ids_are_stable_and_unique_across_sessions() {
        let mut first = Scene::new();
        first.add(1, 0, text("one"));
        first.add(2, 0, text("two\nlines"));
        let mut second = Scene::new();
        second.add(1, 0, text("one"));
        second.add(2, 0, text("two\nlines"));
        let snapshot = pane(vec![first.capture(3, 0), second.capture(4, 0)], None);
        let update = build("Ultty", &snapshot);

        let mut ids: Vec<NodeId> = update.nodes.iter().map(|(id, _)| *id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "every node id is distinct");
        assert_ne!(region_node_id(3, 1), region_node_id(4, 1));
        assert_eq!(region_node_id(3, 1), region_node_id(3, 1));

        let again = build("Ultty", &snapshot);
        assert_eq!(update.nodes, again.nodes);
        // With no focus, the app drawn last has it.
        assert_eq!(update.focus, region_node_id(4, 0));
        consume(update);
    }

    fn assert_ids_distinct(update: &TreeUpdate) {
        let mut ids: Vec<NodeId> = update.nodes.iter().map(|(id, _)| *id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "a node id appears twice");
    }

    #[test]
    fn colliding_ids_leave_the_later_subtree_out_instead_of_panicking() {
        // Two snapshots of the same session collide on every id, as two
        // apps whose names hash alike would.
        let mut scene = Scene::new();
        scene.add(1, 0, text("one\ntwo"));
        scene.add(2, 1, widget(WidgetContent::Button(ButtonWidget::new("Go"))));
        scene.focused = Some(RegionId::new(2));
        let update = build(
            "Ultty",
            &pane(vec![scene.capture(7, 0), scene.capture(7, 0)], None),
        );
        assert_ids_distinct(&update);
        assert_eq!(
            node(&update, pane_node_id(1)).children(),
            &[region_node_id(7, 0)],
            "the second root collided and was left out"
        );
        let tree = consume(update);
        assert_eq!(
            tree.state().focus().map(|focus| focus.role()),
            Some(AkRole::Button)
        );

        // A surface listed twice, over a grid.
        let mut surface = Scene::new();
        surface.surface(5);
        surface.add(6, 5, text("x"));
        let mut snapshot = pane(Vec::new(), Some(screen(10, 2, "$ ")));
        snapshot.panes[0].surfaces = vec![
            SurfaceSnapshot {
                row: 0,
                scene: surface.capture(7, 5),
            },
            SurfaceSnapshot {
                row: 1,
                scene: surface.capture(7, 5),
            },
        ];
        let update = build("Ultty", &snapshot);
        assert_ids_distinct(&update);
        consume(update);
    }

    #[test]
    fn a_native_node_cannot_take_an_id_already_in_the_tree() {
        // As if a region's id hashed to a pane's or a row's.
        let mut scene = Scene::new();
        scene.add(1, 0, Content::None);
        scene.add(2, 1, text("a\nb"));
        scene.add(3, 1, Content::None);
        let mut out = Emitter::default();
        out.reserve(region_node_id(7, 2));
        let mut budget = MAX_PANE_NODES;
        let root = scene.capture(7, 0).emit(&mut out, &mut budget, None);
        assert_eq!(root, Some(region_node_id(7, 0)));
        let nodes = out.finish();
        let find = |id| nodes.iter().find(|(node_id, _)| *node_id == id);
        assert!(find(region_node_id(7, 2)).is_none(), "taken, left out");
        assert!(
            find(region_line_node_id(7, 2, 0)).is_none(),
            "with its subtree"
        );
        let parent = &find(region_node_id(7, 1)).expect("region 1").1;
        assert_eq!(parent.children(), &[region_node_id(7, 3)]);
    }

    #[test]
    fn a_huge_scene_is_cut_to_the_budget_quickly() {
        let mut scene = Scene::new();
        for id in 1..=10_000 {
            scene.add(id, 0, Content::None);
        }
        let big: String = (0..200_000).map(|n| format!("line {n}\n")).collect();
        assert!(big.len() > 2 << 20, "a multi-MiB text");
        scene.add(10_001, 0, text(&big));

        let started = std::time::Instant::now();
        let update = build("Ultty", &pane(vec![scene.capture(7, 0)], None));
        let elapsed = started.elapsed();

        // The window, tab list, tab, panel and pane, then the scene.
        assert!(
            update.nodes.len() <= MAX_PANE_NODES + 6,
            "{} nodes",
            update.nodes.len()
        );
        let pane_node = node(&update, pane_node_id(1));
        let note = *pane_node.children().last().expect("children");
        assert_eq!(node(&update, note).value(), Some("… truncated"));
        assert!(elapsed.as_secs_f64() < 2.0, "took {elapsed:?}");
        consume(update);

        // The text alone: only its last lines are copied, numbered.
        let mut scene = Scene::new();
        scene.add(1, 0, text(&big));
        let update = build("Ultty", &pane(vec![scene.capture(7, 0)], None));
        let children = region(&update, 1).children();
        assert_eq!(children.len(), MAX_TEXT_LINES);
        assert_eq!(
            node(&update, region_line_node_id(7, 1, 199_999)).value(),
            Some("line 199999")
        );
        assert!(find(&update, region_line_node_id(7, 1, 0)).is_none());
    }

    #[test]
    fn names_and_lines_are_capped() {
        use crate::scene::semantics::MAX_NAME_CHARS;

        let line = "x".repeat(4 << 20);
        let mut scene = Scene::new();
        scene.add(1, 0, text(&line));
        scene.add(2, 0, text(&format!("short\n{line}")));
        scene.add(3, 0, Content::None).set_semantics(Semantics {
            description: Some("d".repeat(MAX_NAME_CHARS + 1)),
            ..semantics(Role::Group)
        });
        let update = full_scene(&scene);

        let expected = format!("{}…", "x".repeat(MAX_NAME_CHARS));
        assert_eq!(region(&update, 1).value(), Some(expected.as_str()));
        assert_eq!(
            node(&update, region_line_node_id(7, 2, 1)).value(),
            Some(expected.as_str())
        );
        assert_eq!(
            region(&update, 3).description().map(|d| d.chars().count()),
            Some(MAX_NAME_CHARS + 1),
            "cut to the cap, then the ellipsis"
        );
        consume(update);
    }

    #[test]
    fn an_app_set_value_on_text_is_kept() {
        let mut scene = Scene::new();
        scene.add(1, 0, text("Volume")).set_semantics(Semantics {
            value_text: Some("80%".to_owned()),
            ..semantics(Role::Text)
        });
        let update = full_scene(&scene);
        let paragraph = region(&update, 1);
        assert_eq!(paragraph.role(), AkRole::Paragraph);
        assert_eq!(paragraph.label(), Some("Volume"));
        assert_eq!(paragraph.value(), Some("80%"));
    }

    /// Counts what AccessKit reports between two trees.
    #[derive(Default)]
    struct Changes {
        added: usize,
        updated: usize,
        removed: usize,
    }

    impl accesskit_consumer::TreeChangeHandler for Changes {
        fn node_added(&mut self, _: &accesskit_consumer::NodeRef) {
            self.added += 1;
        }
        fn node_updated(
            &mut self,
            _: &accesskit_consumer::NodeRef,
            _: &accesskit_consumer::NodeRef,
        ) {
            self.updated += 1;
        }
        fn focus_moved(
            &mut self,
            _: Option<&accesskit_consumer::NodeRef>,
            _: Option<&accesskit_consumer::NodeRef>,
        ) {
        }
        fn node_removed(&mut self, _: &accesskit_consumer::NodeRef) {
            self.removed += 1;
        }
    }

    #[test]
    fn a_second_tree_after_moves_hides_and_a_closed_surface_updates_cleanly() {
        let mut scene = Scene::new();
        scene.surface(5);
        scene.add(6, 5, text("first"));
        scene.add(7, 5, text("second"));
        scene.surface(8);
        scene.add(9, 8, text("third"));
        let grid = screen(10, 3, "$ ");
        let snapshot = |scene: &Scene, surfaces: &[(i64, u32)]| {
            let mut snapshot = pane(Vec::new(), Some(grid.clone()));
            snapshot.panes[0].surfaces = surfaces
                .iter()
                .map(|&(row, root)| SurfaceSnapshot {
                    row,
                    scene: scene.capture(7, root),
                })
                .collect();
            snapshot
        };
        let mut tree = consume(build("Ultty", &snapshot(&scene, &[(0, 5), (1, 8)])));

        // Move 7 under surface 8, hide 9, close surface 5.
        scene
            .tree
            .reparent(RegionId::new(7), RegionId::new(8), usize::MAX)
            .expect("reparent");
        scene
            .tree
            .get_mut(RegionId::new(9))
            .expect("region 9")
            .set_semantics(Semantics {
                hidden: true,
                ..Semantics::default()
            });
        scene.tree.remove(RegionId::new(5));
        let second = build("Ultty", &snapshot(&scene, &[(1, 8)]));
        assert_ids_distinct(&second);

        let mut changes = Changes::default();
        tree.update_and_process_changes(second, &mut changes);
        assert!(changes.removed >= 2, "surface 5 and region 9 left");
        let state = tree.state();
        let moved = state
            .node_by_tree_local_id(region_node_id(7, 7), accesskit::TreeId::ROOT)
            .expect("region 7, moved");
        assert_eq!(
            moved.parent_id(),
            Some(
                state
                    .node_by_tree_local_id(region_node_id(7, 8), accesskit::TreeId::ROOT)
                    .expect("surface 8")
                    .id()
            )
        );
        assert!(state
            .node_by_tree_local_id(region_node_id(7, 5), accesskit::TreeId::ROOT)
            .is_none());
    }

    /// A polite log, region 1 of session 7, with `text` and an open stream
    /// when `streaming`, shown with what `live` announced.
    fn live_log(text: &str, streaming: bool, live: &LiveRegions) -> TreeUpdate {
        let mut scene = Scene::new();
        let log = scene.add(1, 0, self::text(text));
        log.appended = true;
        log.streaming = streaming;
        log.set_semantics(Semantics {
            label: Some("Transcript".to_owned()),
            live: Live::Polite,
            ..Semantics::default()
        });
        let mut snapshot = pane(vec![scene.capture(7, 0)], None);
        snapshot.set_announcements(live);
        build("Ultty", &snapshot)
    }

    /// Records how many nodes AccessKit reports as added, and the live ones.
    #[derive(Default)]
    struct Added {
        /// What the platform adapters would announce: live nodes added,
        /// and live nodes whose name changed.
        live: Vec<(String, AkLive)>,
        added: usize,
    }

    /// What AT-SPI names a node: a label's name is its value.
    fn name(node: &accesskit_consumer::NodeRef) -> String {
        node.label().or_else(|| node.value()).unwrap_or_default()
    }

    impl accesskit_consumer::TreeChangeHandler for Added {
        fn node_added(&mut self, node: &accesskit_consumer::NodeRef) {
            self.added += 1;
            if node.live() != AkLive::Off {
                self.live.push((name(node), node.live()));
            }
        }
        fn node_updated(
            &mut self,
            old: &accesskit_consumer::NodeRef,
            new: &accesskit_consumer::NodeRef,
        ) {
            if new.live() != AkLive::Off && name(old) != name(new) {
                self.live.push((name(new), new.live()));
            }
        }
        fn focus_moved(
            &mut self,
            _: Option<&accesskit_consumer::NodeRef>,
            _: Option<&accesskit_consumer::NodeRef>,
        ) {
        }
        fn node_removed(&mut self, _: &accesskit_consumer::NodeRef) {}
    }

    #[test]
    fn a_flushed_queue_is_added_as_a_live_label_and_kept_ones_are_not_added_again() {
        use crate::a11y::live::{FLUSH_SPACING, MAX_ANNOUNCEMENTS};
        use crate::a11y::tree::region_announcement_node_id;

        let t0 = std::time::Instant::now();
        let mut live = LiveRegions::default();
        let mut transcript = String::new();
        let mut tree = consume(live_log("", true, &live));
        for n in 0..10u32 {
            let now = t0 + FLUSH_SPACING * n;
            let line = format!("line {n}\n");
            transcript.push_str(&line);
            live.push(7, 1, Live::Polite, &line, now);
            assert!(live.flush(now));
            let update = live_log(&transcript, true, &live);
            assert_ids_distinct(&update);

            // The newest announcement is the region's last child.
            let log = region(&update, 1);
            assert!(log.is_busy(), "streaming");
            let newest = region_announcement_node_id(7, 1, u64::from(n));
            assert_eq!(log.children().last(), Some(&newest));
            let label = node(&update, newest);
            assert_eq!(label.role(), AkRole::Label);
            assert_eq!(label.value(), Some(format!("line {n}").as_str()));
            assert_eq!(label.live(), Some(AkLive::Polite));
            // At most eight are kept, the newest.
            let kept: Vec<u64> = (0..=u64::from(n))
                .filter(|&serial| {
                    log.children()
                        .contains(&region_announcement_node_id(7, 1, serial))
                })
                .collect();
            assert_eq!(kept.len(), (n as usize + 1).min(MAX_ANNOUNCEMENTS));
            assert_eq!(kept.last(), Some(&u64::from(n)));

            // What the platform adapter announces: the new node only.
            let mut added = Added::default();
            tree.update_and_process_changes(update, &mut added);
            assert_eq!(
                added.live,
                [(format!("line {n}"), AkLive::Polite)],
                "update {n}"
            );
        }

        // The stream ends: busy clears, and nothing is added or announced.
        let update = live_log(&transcript, false, &live);
        assert!(!region(&update, 1).is_busy());
        let mut added = Added::default();
        tree.update_and_process_changes(update, &mut added);
        assert_eq!(added.added, 0);
        assert!(added.live.is_empty());
    }

    #[test]
    fn a_live_regions_text_lines_are_not_live_themselves() {
        let live = LiveRegions::default();
        let update = live_log("one\ntwo\n", false, &live);
        let log = region(&update, 1);
        assert_eq!(log.live(), Some(AkLive::Polite));
        assert_eq!(log.label(), Some("Transcript"));
        for line in [0, 1] {
            assert_eq!(
                node(&update, region_line_node_id(7, 1, line)).live(),
                Some(AkLive::Off),
                "a new line would be announced besides the announcement"
            );
        }
        // A region that is not live has nothing to switch off.
        let mut scene = Scene::new();
        scene.add(1, 0, text("one\ntwo"));
        let update = full_scene(&scene);
        assert_eq!(node(&update, region_line_node_id(7, 1, 0)).live(), None);
    }

    #[test]
    fn assertive_announcements_stay_assertive() {
        let t0 = std::time::Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Assertive, "error: boom\n", t0);
        assert!(live.flush(t0));
        let update = live_log("error: boom\n", false, &live);
        let id = crate::a11y::tree::region_announcement_node_id(7, 1, 0);
        assert_eq!(node(&update, id).live(), Some(AkLive::Assertive));
        consume(update);
    }

    #[test]
    fn an_announcement_whose_id_is_taken_is_left_out() {
        let t0 = std::time::Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "hi\n", t0);
        assert!(live.flush(t0));
        let mut scene = Scene::new();
        scene.add(1, 0, text("hi\n")).set_semantics(Semantics {
            live: Live::Polite,
            ..Semantics::default()
        });
        let mut snapshot = scene.capture(7, 0);
        snapshot.set_announcements(&live);
        let mut out = Emitter::default();
        out.reserve(crate::a11y::tree::region_announcement_node_id(7, 1, 0));
        let mut budget = MAX_PANE_NODES;
        snapshot.emit(&mut out, &mut budget, None);
        let nodes = out.finish();
        let log = &nodes
            .iter()
            .find(|(id, _)| *id == region_node_id(7, 1))
            .expect("region 1")
            .1;
        assert!(log.children().is_empty(), "{:?}", log.children());
    }

    #[test]
    fn nothing_under_a_live_region_is_live_but_its_announcements() {
        let scene_with = |child_text: &str| {
            let mut scene = Scene::new();
            let log = scene.add(1, 0, Content::None);
            log.set_semantics(Semantics {
                label: Some("Transcript".to_owned()),
                live: Live::Polite,
                ..Semantics::default()
            });
            // A child the app streams into without marking it live.
            scene.add(2, 1, text(child_text)).appended = true;
            scene.add(3, 2, text(child_text)).appended = true;
            let mut snapshot = pane(vec![scene.capture(7, 0)], None);
            snapshot.set_announcements(&LiveRegions::default());
            build("Ultty", &snapshot)
        };
        let update = scene_with("Thinking");
        for id in [2, 3] {
            assert_eq!(region(&update, id).live(), Some(AkLive::Off), "region {id}");
        }
        let mut tree = consume(update);
        for text in ["Thinking about", "Thinking about it\nand more", "x\ny\nz"] {
            let update = scene_with(text);
            let mut added = Added::default();
            tree.update_and_process_changes(update, &mut added);
            assert!(added.live.is_empty(), "{text:?} announced {:?}", added.live);
        }
        let state = tree.state();
        let line = state
            .node_by_tree_local_id(region_line_node_id(7, 3, 2), accesskit::TreeId::ROOT)
            .expect("a line of region 3");
        assert_eq!(line.live(), AkLive::Off);
    }

    #[test]
    fn announcements_are_not_crowded_out_by_the_node_budget() {
        let t0 = std::time::Instant::now();
        let mut live = LiveRegions::default();
        live.push(7, 1, Live::Polite, "kept\n", t0);
        assert!(live.flush(t0));
        let mut scene = Scene::new();
        scene.add(1, 0, Content::None).set_semantics(Semantics {
            live: Live::Polite,
            ..Semantics::default()
        });
        for id in 2..(MAX_PANE_NODES as u32 + 100) {
            scene.add(id, 1, Content::None);
        }
        let mut snapshot = pane(vec![scene.capture(7, 0)], None);
        snapshot.set_announcements(&live);
        let update = build("Ultty", &snapshot);
        let announcement = crate::a11y::tree::region_announcement_node_id(7, 1, 0);
        assert_eq!(region(&update, 1).children().last(), Some(&announcement));
        assert_eq!(node(&update, announcement).value(), Some("kept"));
        consume(update);
    }
}
