//! Native scenes in the accessibility tree (ADR 0021 §1, §3, §6): a
//! session's region tree, each region a node with the role, name and state
//! of its effective semantics ([`effective_semantics`]).
//!
//! [`SceneSnapshot::capture`] copies one region tree (a full-scene `ROOT`
//! or an inline surface) out of a session's scene; [`super::tree::build`]
//! places it in the window's tree and calls [`SceneSnapshot::push_nodes`].
//!
//! Text is not given as `TextRun`s: AccessKit exposes runs only under a
//! `Label`, `Document`, `Terminal` or text input, and runs inside the
//! pane's `Terminal` would join its document. A region's text is instead
//! its name when it is one line (a `Text` region is then a `Label`, whose
//! value is its name), else one `Label` child per line, which Orca reads
//! line by line in browse mode and flat review.

use accesskit::{Live as AkLive, Node, NodeId, Rect, Role as AkRole, Toggled as AkToggled};
use ultty_protocol::{Content, Expanded, Live, RegionId, Role, Semantics, Toggled, WidgetContent};

use super::tree::{region_line_node_id, region_node_id};
use crate::scene::semantics::effective_semantics;
use crate::scene::tree::SceneTree;

/// The most lines of a region's text given as `Label` children: the last
/// ones, which in a transcript are the newest. Bounds every rebuild of a
/// long log to a few hundred nodes.
pub const MAX_TEXT_LINES: usize = 200;

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
/// it: hidden regions and their subtrees already left out.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSnapshot {
    session: u64,
    /// The tree's root region, first in `regions`.
    root: u32,
    /// Depth first, each before its children.
    regions: Vec<RegionSnapshot>,
    /// The session's focused region, when it is in this tree.
    focus: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
struct RegionSnapshot {
    id: u32,
    /// Children shown, in order.
    children: Vec<u32>,
    semantics: Semantics,
    /// The `Input` widget's flags, for the text input role.
    input: Option<InputKind>,
    /// The text of a text, rich text, markdown or code region.
    text: Option<String>,
    bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct InputKind {
    multiline: bool,
    password: bool,
}

/// A relationship of a region node to another region, kept until every
/// node is built, so that a target not in the tree can be dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Relation {
    /// Index of the node in the update's node list.
    pub(super) node: usize,
    pub(super) kind: RelationKind,
    pub(super) target: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum RelationKind {
    LabelledBy,
    DescribedBy,
    Controls,
}

impl SceneSnapshot {
    /// Copy the tree under `root` of `session`'s `scene`, drawn at `at`.
    /// `focused` is the session's focused region and `scroll` gives a
    /// region's scroll offset in cells; a region's bounds are moved by the
    /// offsets of the regions above it, as it is drawn.
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
            let layout = region.computed;
            let x0 = at.x + f64::from(layout.x - scroll_x) * at.cell_width;
            let y0 = at.y + f64::from(layout.y - scroll_y) * at.cell_height;
            let (text, input) = content_parts(&region.content);
            regions.push(RegionSnapshot {
                id: id.0,
                children,
                semantics: effective_semantics(region),
                input,
                text,
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
    /// to `nodes`, root first; relationships go to `relations` to be
    /// resolved once the whole tree is built.
    pub(super) fn push_nodes(
        &self,
        nodes: &mut Vec<(NodeId, Node)>,
        relations: &mut Vec<Relation>,
    ) {
        for region in &self.regions {
            let id = region_node_id(self.session, region.id);
            let (mut node, lines) = region_node(region);
            let mut children: Vec<NodeId> = region
                .children
                .iter()
                .map(|&child| region_node_id(self.session, child))
                .collect();
            let line_nodes: Vec<(NodeId, Node)> = lines
                .into_iter()
                .map(|(index, line)| {
                    let mut label = Node::new(AkRole::Label);
                    label.set_value(line);
                    (region_line_node_id(self.session, region.id, index), label)
                })
                .collect();
            // The text comes before any child regions, as it is drawn.
            children.splice(0..0, line_nodes.iter().map(|(id, _)| *id));
            node.set_children(children);

            let index = nodes.len();
            let semantics = &region.semantics;
            for (kind, target) in [
                (RelationKind::LabelledBy, semantics.labelled_by),
                (RelationKind::DescribedBy, semantics.described_by),
                (RelationKind::Controls, semantics.controls),
            ] {
                if let Some(target) = target {
                    relations.push(Relation {
                        node: index,
                        kind,
                        target: region_node_id(self.session, target.0),
                    });
                }
            }
            nodes.push((id, node));
            nodes.extend(line_nodes);
        }
    }
}

/// A region content's text, and an input's flags.
fn content_parts(content: &Content) -> (Option<String>, Option<InputKind>) {
    match content {
        Content::Text(text) => (Some(text.text.clone()), None),
        Content::RichText(rich) => (
            Some(rich.spans.iter().map(|span| span.text.as_str()).collect()),
            None,
        ),
        Content::Widget(WidgetContent::Markdown(markdown)) => (Some(markdown.source.clone()), None),
        Content::Widget(WidgetContent::Code(code)) => (Some(code.source.clone()), None),
        Content::Widget(WidgetContent::Input(input)) => (
            None,
            Some(InputKind {
                multiline: input.multiline,
                password: input.password,
            }),
        ),
        _ => (None, None),
    }
}

/// The node for `region`, and its text lines to add as `Label` children,
/// each with its line number in the text.
fn region_node(region: &RegionSnapshot) -> (Node, Vec<(usize, &str)>) {
    let semantics = &region.semantics;
    let label = semantics.label.as_deref();
    // Lines of text not already said by the name.
    let lines: Vec<(usize, &str)> = match region.text.as_deref() {
        Some(text) if !names_the_text(label, text) => text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let lines = lines[lines.len().saturating_sub(MAX_TEXT_LINES)..].to_vec();

    let role = match semantics.role {
        // A one-line text is a label, named by its value.
        Role::Text if lines.is_empty() => AkRole::Label,
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
            node.set_value(label);
        }
    } else {
        if let Some(label) = label {
            node.set_label(label);
        }
        if let Some(value) = &semantics.value_text {
            node.set_value(value.as_str());
        }
    }
    if let Some(description) = &semantics.description {
        node.set_description(description.as_str());
    }
    set_state(&mut node, semantics);
    (node, lines)
}

/// Whether `label` is the whole of `text`: one line, said by the name.
fn names_the_text(label: Option<&str>, text: &str) -> bool {
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.trim().is_empty() || label == Some(line)
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

/// Add each relationship whose target was built to its node; a target
/// that does not exist, is hidden or is not shown is dropped (spec §10.5).
pub(super) fn resolve_relations(nodes: &mut [(NodeId, Node)], relations: &[Relation]) {
    if relations.is_empty() {
        return;
    }
    let built: std::collections::HashSet<NodeId> = nodes.iter().map(|(id, _)| *id).collect();
    for relation in relations {
        if !built.contains(&relation.target) {
            continue;
        }
        let node = &mut nodes[relation.node].1;
        match relation.kind {
            RelationKind::LabelledBy => node.push_labelled_by(relation.target),
            RelationKind::DescribedBy => node.push_described_by(relation.target),
            RelationKind::Controls => node.push_controlled(relation.target),
        }
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
}
