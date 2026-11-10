//! Native protocol frame diffing with keyed reconciliation.
//!
//! Ulttui view trees are rebuilt in immediate-mode style each render pass, but
//! native protocol regions are retained by the terminal. Each frame the walk
//! emits messages with provisional, traversal-order ids; this module maps
//! them onto stable retained ids and emits only the difference.
//!
//! Children that expose a [`crate::widgets::View::native_key`] are matched to
//! the previous frame's sibling with the same key, so inserting, removing or
//! reordering keyed siblings reuses their regions instead of rewriting every
//! traversal slot. Unkeyed children fall back to positional matching among
//! the remaining unkeyed siblings of the same parent.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use ultty_protocol::{
    AppMessage, Constraints, Content, Live, RegionId, RichText, Semantics, Style,
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RegionSnapshot {
    pub(super) parent: RegionId,
    pub(super) key: Option<String>,
    pub(super) constraints: Constraints,
    pub(super) style: Style,
    pub(super) content: Content,
    pub(super) scroll: Option<(f32, f32, bool)>,
    /// What the frame sent with `SetSemantics`; `None` leaves the region's
    /// semantics to the terminal.
    pub(super) semantics: Option<Semantics>,
}

/// Regions the terminal currently retains: stable ids plus sibling order.
#[derive(Debug)]
pub(crate) struct RetainedTree {
    regions: HashMap<RegionId, RegionSnapshot>,
    /// Ordered children per parent; parents include the terminal's root.
    children: HashMap<RegionId, Vec<RegionId>>,
    /// Next never-used stable id. Ids are never recycled, so a region id
    /// observed by the terminal always refers to the same logical region.
    next_id: u32,
}

impl Default for RetainedTree {
    fn default() -> Self {
        Self::new()
    }
}

impl RetainedTree {
    pub(crate) fn new() -> Self {
        Self {
            regions: HashMap::new(),
            children: HashMap::new(),
            next_id: 1,
        }
    }

    /// Make sure `id` (for example the terminal-owned root) is never handed
    /// out as a stable id.
    pub(crate) fn reserve(&mut self, id: RegionId) {
        if id.0 >= self.next_id {
            self.next_id = id.0.wrapping_add(1);
        }
    }

    /// The next stable id this tree would hand out; every id it holds or
    /// reserved is below it.
    pub(crate) fn next_id(&self) -> u32 {
        self.next_id
    }

    /// The native key of retained region `id`, if it has one.
    pub(super) fn key_of(&self, id: RegionId) -> Option<&str> {
        self.regions.get(&id)?.key.as_deref()
    }

    /// The retained region with native key `key`; the oldest one if
    /// several share it.
    pub(super) fn region_with_key(&self, key: &str) -> Option<RegionId> {
        self.regions
            .iter()
            .filter(|(_, region)| region.key.as_deref() == Some(key))
            .map(|(id, _)| *id)
            .min_by_key(|id| id.0)
    }

    pub(super) fn children_of(&self, parent: RegionId) -> &[RegionId] {
        self.children.get(&parent).map(Vec::as_slice).unwrap_or(&[])
    }

    fn allocate(&mut self) -> RegionId {
        let id = RegionId::new(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

thread_local! {
    /// Native keys recorded by the view walk, keyed by provisional region id.
    /// `None` while no native frame is being collected, so walks done for
    /// tests or the mock backend record nothing.
    static KEY_RECORDER: RefCell<Option<HashMap<RegionId, String>>> = const { RefCell::new(None) };
}

/// Record the native key of a region created during the current frame walk.
pub(crate) fn record_native_key(id: RegionId, key: &str) {
    KEY_RECORDER.with(|recorder| {
        if let Some(keys) = recorder.borrow_mut().as_mut() {
            keys.insert(id, key.to_string());
        }
    });
}

pub(crate) fn begin_native_key_recording() {
    KEY_RECORDER.with(|recorder| *recorder.borrow_mut() = Some(HashMap::new()));
}

pub(crate) fn take_native_keys() -> HashMap<RegionId, String> {
    KEY_RECORDER.with(|recorder| recorder.borrow_mut().take().unwrap_or_default())
}

struct ProvisionalNode {
    key: Option<String>,
    constraints: Constraints,
    style: Style,
    content: Content,
    scroll: Option<(f32, f32, bool)>,
    semantics: Option<Semantics>,
}

/// What the terminal agreed to, as far as the diff cares.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrameCapabilities {
    /// Text growth may go as `AppendContent` (`CAP_APPEND_CONTENT`).
    pub(crate) append_content: bool,
    /// `SetSemantics` may be sent (`CAP_SEMANTICS`); without it the
    /// frame's semantics are dropped.
    pub(crate) semantics: bool,
}

/// Result of matching one frame's provisional regions onto retained ids.
struct Reconciliation {
    /// Provisional id -> stable id for every region in the frame.
    mapping: HashMap<RegionId, RegionId>,
    /// Stable ids that existed in the previous frame and survive this one.
    retained: HashSet<RegionId>,
    /// Parents whose retained children changed relative order, with the new
    /// full child order.
    reordered: Vec<(RegionId, Vec<RegionId>)>,
    next: RetainedTree,
}

pub(crate) fn optimize_frame_messages(
    previous: &RetainedTree,
    frame_messages: Vec<AppMessage>,
    native_keys: &HashMap<RegionId, String>,
    capabilities: FrameCapabilities,
) -> (Vec<AppMessage>, RetainedTree) {
    let (nodes, order) = parse_provisional_tree(&frame_messages, native_keys);
    let mut reconciliation = Reconciliation {
        mapping: HashMap::new(),
        retained: HashSet::new(),
        reordered: Vec::new(),
        next: RetainedTree {
            regions: HashMap::new(),
            children: HashMap::new(),
            next_id: previous.next_id,
        },
    };

    // Parents that are not provisional regions belong to the terminal (the
    // session root); their ids are already stable.
    let mut top_parents: Vec<RegionId> = order
        .keys()
        .filter(|parent| !nodes.contains_key(parent))
        .copied()
        .collect();
    top_parents.sort_by_key(|id| id.0);
    for parent in top_parents {
        reconcile_children(
            parent,
            parent,
            &nodes,
            &order,
            previous,
            &mut reconciliation,
        );
    }

    let Reconciliation {
        mapping,
        retained,
        reordered,
        next,
    } = reconciliation;

    let mut outgoing = Vec::new();

    // Deletes go first so every later insert index refers to a sibling list
    // without stale entries.
    let mut removed: Vec<RegionId> = previous
        .regions
        .keys()
        .filter(|id| !retained.contains(id))
        .copied()
        .collect();
    removed.sort_by_key(|id| id.0);
    outgoing.extend(
        removed
            .into_iter()
            .map(|id| AppMessage::DeleteRegion { id }),
    );

    for msg in frame_messages {
        match msg {
            AppMessage::CreateRegion {
                id,
                parent,
                constraints,
                style,
                ..
            } => {
                let stable = mapping[&id];
                let stable_parent = mapping.get(&parent).copied().unwrap_or(parent);
                if let Some(prev) = previous.regions.get(&stable) {
                    if prev.constraints != constraints {
                        outgoing.push(AppMessage::SetConstraints {
                            id: stable,
                            constraints,
                        });
                    }
                    if prev.style != style {
                        outgoing.push(AppMessage::SetStyle { id: stable, style });
                    }
                } else {
                    let insert_index = next
                        .children_of(stable_parent)
                        .iter()
                        .position(|child| *child == stable)
                        .map(|index| index as i32)
                        .unwrap_or(-1);
                    outgoing.push(AppMessage::CreateRegion {
                        id: stable,
                        parent: stable_parent,
                        insert_index,
                        constraints,
                        style,
                    });
                }
            }
            AppMessage::SetContent { id, content } => {
                let stable = mapping[&id];
                let live = next.regions.get(&stable).is_some_and(|region| {
                    region
                        .semantics
                        .as_ref()
                        .is_some_and(|semantics| semantics.live != Live::Off)
                });
                match previous.regions.get(&stable) {
                    Some(prev) if live && capabilities.append_content => {
                        outgoing.extend(live_content_update(stable, &prev.content, content));
                    }
                    Some(prev) => {
                        let delta = capabilities
                            .append_content
                            .then(|| append_delta(&prev.content, &content))
                            .flatten();
                        match delta {
                            Some(delta) if delta.is_empty() => {}
                            Some(delta) => outgoing.push(AppMessage::AppendContent {
                                id: stable,
                                text: delta,
                            }),
                            None if prev.content != content => {
                                outgoing.push(AppMessage::SetContent {
                                    id: stable,
                                    content,
                                });
                            }
                            None => {}
                        }
                    }
                    None => outgoing.push(AppMessage::SetContent {
                        id: stable,
                        content,
                    }),
                }
            }
            AppMessage::Scroll { id, x, y, animate } => {
                let stable = mapping[&id];
                let changed = previous
                    .regions
                    .get(&stable)
                    .map(|prev| prev.scroll != Some((x, y, animate)))
                    .unwrap_or(true);
                if changed {
                    outgoing.push(AppMessage::Scroll {
                        id: stable,
                        x,
                        y,
                        animate,
                    });
                }
            }
            AppMessage::SetSemantics { id, semantics } => {
                let stable = mapping[&id];
                let unchanged = previous
                    .regions
                    .get(&stable)
                    .is_some_and(|prev| prev.semantics.as_ref() == Some(&semantics));
                if capabilities.semantics && !unchanged {
                    outgoing.push(AppMessage::SetSemantics {
                        id: stable,
                        semantics,
                    });
                }
            }
            other => outgoing.push(other),
        }
    }

    // A region whose view no longer says more than its content goes back
    // to what the terminal derives.
    if capabilities.semantics {
        let mut reset: Vec<RegionId> = next
            .regions
            .iter()
            .filter(|(id, snapshot)| {
                snapshot.semantics.is_none()
                    && previous
                        .regions
                        .get(id)
                        .is_some_and(|prev| prev.semantics.is_some())
            })
            .map(|(id, _)| *id)
            .collect();
        reset.sort_by_key(|id| id.0);
        outgoing.extend(reset.into_iter().map(|id| AppMessage::SetSemantics {
            id,
            semantics: Semantics::default(),
        }));
    }

    let mut cleared: Vec<RegionId> = next
        .regions
        .iter()
        .filter(|(id, snapshot)| {
            snapshot.content == Content::None
                && previous
                    .regions
                    .get(id)
                    .is_some_and(|prev| prev.content != Content::None)
        })
        .map(|(id, _)| *id)
        .collect();
    cleared.sort_by_key(|id| id.0);
    outgoing.extend(cleared.into_iter().map(|id| AppMessage::SetContent {
        id,
        content: Content::None,
    }));

    for (parent, children) in reordered {
        for (index, child) in children.into_iter().enumerate() {
            outgoing.push(AppMessage::ReparentRegion {
                id: child,
                new_parent: parent,
                insert_index: index as i32,
            });
        }
    }

    (outgoing, next)
}

type ProvisionalOrder = HashMap<RegionId, Vec<RegionId>>;

fn parse_provisional_tree(
    frame_messages: &[AppMessage],
    native_keys: &HashMap<RegionId, String>,
) -> (HashMap<RegionId, ProvisionalNode>, ProvisionalOrder) {
    let mut nodes = HashMap::new();
    let mut order: ProvisionalOrder = HashMap::new();
    for msg in frame_messages {
        match msg {
            AppMessage::CreateRegion {
                id,
                parent,
                constraints,
                style,
                ..
            } => {
                nodes.insert(
                    *id,
                    ProvisionalNode {
                        key: native_keys.get(id).cloned(),
                        constraints: constraints.clone(),
                        style: style.clone(),
                        content: Content::None,
                        scroll: None,
                        semantics: None,
                    },
                );
                order.entry(*parent).or_default().push(*id);
            }
            AppMessage::SetSemantics { id, semantics } => {
                if let Some(node) = nodes.get_mut(id) {
                    node.semantics = Some(semantics.clone());
                }
            }
            AppMessage::SetContent { id, content } => {
                if let Some(node) = nodes.get_mut(id) {
                    node.content = content.clone();
                }
            }
            AppMessage::Scroll { id, x, y, animate } => {
                if let Some(node) = nodes.get_mut(id) {
                    node.scroll = Some((*x, *y, *animate));
                }
            }
            _ => {}
        }
    }
    (nodes, order)
}

/// Match the provisional children of `provisional_parent` onto the retained
/// children of `stable_parent`, then recurse.
fn reconcile_children(
    provisional_parent: RegionId,
    stable_parent: RegionId,
    nodes: &HashMap<RegionId, ProvisionalNode>,
    order: &ProvisionalOrder,
    previous: &RetainedTree,
    out: &mut Reconciliation,
) {
    let Some(provisional_children) = order.get(&provisional_parent) else {
        return;
    };
    let prev_children = previous.children_of(stable_parent);

    let mut prev_by_key: HashMap<&str, RegionId> = prev_children
        .iter()
        .filter_map(|id| {
            previous
                .regions
                .get(id)
                .and_then(|region| region.key.as_deref())
                .map(|key| (key, *id))
        })
        .collect();

    // Pass 1: keyed children claim the previous sibling with the same key.
    let mut effective_keys: Vec<Option<&str>> = Vec::with_capacity(provisional_children.len());
    let mut assigned: Vec<Option<RegionId>> = vec![None; provisional_children.len()];
    let mut claimed: HashSet<RegionId> = HashSet::new();
    let mut seen_keys: HashSet<&str> = HashSet::new();
    for (index, provisional) in provisional_children.iter().enumerate() {
        let key = nodes[provisional].key.as_deref().filter(|key| {
            let fresh = seen_keys.insert(key);
            if !fresh {
                tracing::warn!(
                    "duplicate native key {key:?} among siblings; treating the later one as unkeyed"
                );
            }
            fresh
        });
        if let Some(prev_id) = key.and_then(|key| prev_by_key.remove(key)) {
            assigned[index] = Some(prev_id);
            claimed.insert(prev_id);
        }
        effective_keys.push(key);
    }

    // Pass 2: unkeyed children take the remaining unkeyed previous siblings
    // in order (traversal-slot identity, as before keys existed).
    let prev_unkeyed: Vec<RegionId> = prev_children
        .iter()
        .copied()
        .filter(|id| {
            !claimed.contains(id) && previous.regions.get(id).is_some_and(|r| r.key.is_none())
        })
        .collect();
    let mut prev_unkeyed = prev_unkeyed.into_iter();
    for index in 0..provisional_children.len() {
        if assigned[index].is_none() && effective_keys[index].is_none() {
            if let Some(prev_id) = prev_unkeyed.next() {
                assigned[index] = Some(prev_id);
                claimed.insert(prev_id);
            }
        }
    }

    let mut new_order = Vec::with_capacity(provisional_children.len());
    for (index, provisional) in provisional_children.iter().enumerate() {
        let stable = match assigned[index] {
            Some(id) => {
                out.retained.insert(id);
                id
            }
            None => out.next.allocate(),
        };
        out.mapping.insert(*provisional, stable);
        let node = &nodes[provisional];
        out.next.regions.insert(
            stable,
            RegionSnapshot {
                parent: stable_parent,
                key: effective_keys[index].map(str::to_string),
                constraints: node.constraints.clone(),
                style: node.style.clone(),
                content: node.content.clone(),
                scroll: node.scroll,
                semantics: node.semantics.clone(),
            },
        );
        new_order.push(stable);
    }

    let prev_retained: Vec<RegionId> = prev_children
        .iter()
        .copied()
        .filter(|id| claimed.contains(id))
        .collect();
    let new_retained: Vec<RegionId> = new_order
        .iter()
        .copied()
        .filter(|id| claimed.contains(id))
        .collect();
    if prev_retained != new_retained {
        out.reordered.push((stable_parent, new_order.clone()));
    }
    out.next.children.insert(stable_parent, new_order);

    for provisional in provisional_children {
        let stable = out.mapping[provisional];
        reconcile_children(*provisional, stable, nodes, order, previous, out);
    }
}

/// The messages that take a live region from `previous` to `next`
/// content.
///
/// The terminal announces only text that arrives with `AppendContent`
/// (ADR 0021 §3), so text a live region grows by is always appended, even
/// when its styling changed too, as when a streamed markdown span turns
/// bold: the new text is appended, then a `SetContent` in the same frame
/// puts the styled result in place. When text in the last line changed as
/// well (a markdown marker that became styling), what follows the
/// unchanged part is appended again; a change before the last line is
/// set, not announced.
fn live_content_update(id: RegionId, previous: &Content, next: Content) -> Vec<AppMessage> {
    if *previous == next {
        return Vec::new();
    }
    let (Some(before), Some(after)) = (plain_text(previous), plain_text(&next)) else {
        return vec![AppMessage::SetContent { id, content: next }];
    };
    let kept = common_prefix_len(&before, &after);
    if before[kept..].contains('\n') || kept == after.len() {
        return vec![AppMessage::SetContent { id, content: next }];
    }
    let text = after[kept..].to_string();
    let appended = with_appended(previous, &text);
    let mut messages = vec![AppMessage::AppendContent { id, text }];
    if appended != next {
        messages.push(AppMessage::SetContent { id, content: next });
    }
    messages
}

/// The text of text content, spans joined.
fn plain_text(content: &Content) -> Option<Cow<'_, str>> {
    match content {
        Content::None => Some(Cow::Borrowed("")),
        Content::Text(text) => Some(Cow::Borrowed(&text.text)),
        Content::RichText(rich) => Some(Cow::Owned(
            rich.spans.iter().map(|span| span.text.as_str()).collect(),
        )),
        Content::Widget(_) => None,
    }
}

/// Length in bytes of the longest common prefix of `a` and `b`.
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((index, _), _)| index)
}

/// `content` after the terminal appends `text` to it, as `AppendContent`
/// does.
fn with_appended(content: &Content, text: &str) -> Content {
    let mut content = content.clone();
    match &mut content {
        Content::Text(existing) => existing.text.push_str(text),
        Content::RichText(rich) => match rich.spans.last_mut() {
            Some(last) => last.text.push_str(text),
            None => rich.spans.push(ultty_protocol::Span {
                text: text.to_string(),
                style: Style::new(),
            }),
        },
        Content::None => content = Content::Text(ultty_protocol::TextContent::new(text)),
        Content::Widget(_) => {}
    }
    content
}

fn append_delta(previous: &Content, next: &Content) -> Option<String> {
    match (previous, next) {
        (Content::Text(prev), Content::Text(next))
            if prev.wrap == next.wrap
                && prev.align == next.align
                && prev.overflow == next.overflow
                && next.text.starts_with(&prev.text)
                && next.text.len() > prev.text.len() =>
        {
            Some(next.text[prev.text.len()..].to_string())
        }
        (Content::RichText(prev), Content::RichText(next)) => rich_text_append_delta(prev, next),
        _ => None,
    }
}

fn rich_text_append_delta(previous: &RichText, next: &RichText) -> Option<String> {
    if previous.spans.is_empty()
        || previous.spans.len() != next.spans.len()
        || previous.spans.len() > next.spans.len()
    {
        return None;
    }

    let last_index = previous.spans.len() - 1;
    for (prev, next) in previous.spans[..last_index]
        .iter()
        .zip(&next.spans[..last_index])
    {
        if prev != next {
            return None;
        }
    }

    let prev_last = &previous.spans[last_index];
    let next_last = &next.spans[last_index];
    if prev_last.style == next_last.style
        && next_last.text.starts_with(&prev_last.text)
        && next_last.text.len() > prev_last.text.len()
    {
        Some(next_last.text[prev_last.text.len()..].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{optimize_frame_messages, FrameCapabilities, RetainedTree};
    use std::collections::HashMap;
    use ultty_protocol::{
        AppMessage, Constraints, Content, RegionId, RichText, Style, TextContent,
    };
    use ultty_protocol::{Live, Role, Semantics};

    /// One region in a test frame: (provisional id, parent, native key, text).
    type Spec<'a> = (u32, RegionId, Option<&'a str>, Option<&'a str>);

    fn frame(specs: &[Spec<'_>]) -> (Vec<AppMessage>, HashMap<RegionId, String>) {
        let mut messages = Vec::new();
        let mut keys = HashMap::new();
        for (id, parent, key, text) in specs {
            let id = RegionId::new(*id);
            messages.push(AppMessage::CreateRegion {
                id,
                parent: *parent,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            });
            if let Some(text) = text {
                messages.push(AppMessage::SetContent {
                    id,
                    content: Content::Text(TextContent::new(*text)),
                });
            }
            if let Some(key) = key {
                keys.insert(id, key.to_string());
            }
        }
        (messages, keys)
    }

    fn run(previous: &RetainedTree, specs: &[Spec<'_>]) -> (Vec<AppMessage>, RetainedTree) {
        let (messages, keys) = frame(specs);
        optimize_frame_messages(previous, messages, &keys, FrameCapabilities::default())
    }

    fn count(messages: &[AppMessage], pred: impl Fn(&AppMessage) -> bool) -> usize {
        messages.iter().filter(|m| pred(m)).count()
    }

    fn id_for_key(tree: &RetainedTree, key: &str) -> RegionId {
        tree.regions
            .iter()
            .find(|(_, r)| r.key.as_deref() == Some(key))
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("no region with key {key}"))
    }

    const ROOT: RegionId = RegionId::ROOT;
    const LIST: RegionId = RegionId::new(1);

    fn keyed_list(items: &[&str]) -> Vec<Spec<'static>> {
        let mut specs: Vec<Spec<'static>> = vec![(1, ROOT, None, None)];
        for (i, item) in items.iter().enumerate() {
            let leaked: &'static str = Box::leak(item.to_string().into_boxed_str());
            specs.push((2 + i as u32, LIST, Some(leaked), Some(leaked)));
        }
        specs
    }

    #[test]
    fn first_frame_creates_everything_with_stable_ids() {
        let (outgoing, tree) = run(&RetainedTree::new(), &keyed_list(&["a", "b", "c"]));
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::CreateRegion { .. })),
            4
        );
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::SetContent { .. })),
            3
        );
        assert_eq!(tree.children_of(ROOT).len(), 1);
        assert_eq!(tree.children_of(tree.children_of(ROOT)[0]).len(), 3);
    }

    #[test]
    fn keyed_prepend_reuses_existing_regions_and_sends_only_the_new_one() {
        let (_, tree1) = run(&RetainedTree::new(), &keyed_list(&["a", "b", "c"]));
        let before: Vec<RegionId> = ["a", "b", "c"]
            .iter()
            .map(|k| id_for_key(&tree1, k))
            .collect();

        let (outgoing, tree2) = run(&tree1, &keyed_list(&["x", "a", "b", "c"]));

        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::SetContent { .. })),
            1
        );
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::DeleteRegion { .. })),
            0
        );
        assert_eq!(
            count(&outgoing, |m| matches!(
                m,
                AppMessage::ReparentRegion { .. }
            )),
            0
        );
        let creates: Vec<&AppMessage> = outgoing
            .iter()
            .filter(|m| matches!(m, AppMessage::CreateRegion { .. }))
            .collect();
        assert_eq!(creates.len(), 1);
        assert!(matches!(
            creates[0],
            AppMessage::CreateRegion {
                insert_index: 0,
                ..
            }
        ));

        let after: Vec<RegionId> = ["a", "b", "c"]
            .iter()
            .map(|k| id_for_key(&tree2, k))
            .collect();
        assert_eq!(before, after, "keyed siblings keep their region ids");
        let list_id = tree2.children_of(ROOT)[0];
        assert_eq!(tree2.children_of(list_id)[1..], after[..]);
    }

    #[test]
    fn keyed_removal_deletes_only_that_region() {
        let (_, tree1) = run(&RetainedTree::new(), &keyed_list(&["a", "b", "c"]));
        let b = id_for_key(&tree1, "b");

        let (outgoing, tree2) = run(&tree1, &keyed_list(&["a", "c"]));

        assert_eq!(outgoing, vec![AppMessage::DeleteRegion { id: b }]);
        assert_eq!(tree2.regions.len(), 3);
    }

    #[test]
    fn keyed_reorder_emits_reparent_instead_of_content_rewrites() {
        let (_, tree1) = run(&RetainedTree::new(), &keyed_list(&["a", "b", "c"]));
        let list_id = tree1.children_of(ROOT)[0];
        let (a, b, c) = (
            id_for_key(&tree1, "a"),
            id_for_key(&tree1, "b"),
            id_for_key(&tree1, "c"),
        );

        let (outgoing, tree2) = run(&tree1, &keyed_list(&["c", "a", "b"]));

        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::SetContent { .. })),
            0
        );
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::CreateRegion { .. })),
            0
        );
        assert_eq!(
            outgoing,
            vec![
                AppMessage::ReparentRegion {
                    id: c,
                    new_parent: list_id,
                    insert_index: 0
                },
                AppMessage::ReparentRegion {
                    id: a,
                    new_parent: list_id,
                    insert_index: 1
                },
                AppMessage::ReparentRegion {
                    id: b,
                    new_parent: list_id,
                    insert_index: 2
                },
            ]
        );
        assert_eq!(tree2.children_of(list_id), &[c, a, b]);
    }

    #[test]
    fn unkeyed_children_fall_back_to_positional_slots() {
        let unkeyed = |items: &[&'static str]| -> Vec<Spec<'static>> {
            let mut specs: Vec<Spec<'static>> = vec![(1, ROOT, None, None)];
            for (i, item) in items.iter().enumerate() {
                specs.push((2 + i as u32, LIST, None, Some(item)));
            }
            specs
        };
        let (_, tree1) = run(&RetainedTree::new(), &unkeyed(&["A", "B"]));

        let (outgoing, _) = run(&tree1, &unkeyed(&["X", "A", "B"]));

        // Slots 1 and 2 are rewritten to X and A, slot 3 is created for B.
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::SetContent { .. })),
            3
        );
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::CreateRegion { .. })),
            1
        );
        assert_eq!(
            count(&outgoing, |m| matches!(m, AppMessage::DeleteRegion { .. })),
            0
        );
    }

    #[test]
    fn stable_ids_are_never_recycled() {
        let (_, tree1) = run(&RetainedTree::new(), &keyed_list(&["a"]));
        let a = id_for_key(&tree1, "a");
        let (_, tree2) = run(&tree1, &keyed_list(&["b"]));
        let b = id_for_key(&tree2, "b");
        let (_, tree3) = run(&tree2, &keyed_list(&["a"]));
        let a_again = id_for_key(&tree3, "a");

        assert!(b.0 > a.0);
        assert!(a_again.0 > b.0, "a removed and re-added gets a fresh id");
    }

    #[test]
    fn reserve_skips_terminal_owned_ids() {
        let mut tree = RetainedTree::new();
        tree.reserve(RegionId::new(7));
        assert_eq!(tree.allocate(), RegionId::new(8));
    }

    fn tree_with_text(id: RegionId, text: &str) -> RetainedTree {
        tree_with_content(id, Content::Text(TextContent::new(text)))
    }

    fn tree_with_content(id: RegionId, content: Content) -> RetainedTree {
        let mut tree = RetainedTree::new();
        tree.regions.insert(
            id,
            super::RegionSnapshot {
                parent: ROOT,
                key: None,
                constraints: Constraints::default(),
                style: Style::default(),
                content,
                scroll: None,
                semantics: None,
            },
        );
        tree.children.insert(ROOT, vec![id]);
        tree.reserve(id);
        tree
    }

    fn frame_with_content(id: RegionId, content: Content) -> Vec<AppMessage> {
        vec![
            AppMessage::CreateRegion {
                id,
                parent: ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent { id, content },
        ]
    }

    #[test]
    fn optimize_converts_text_growth_to_append_content() {
        let id = RegionId::new(1);
        let previous = tree_with_text(id, "hello");
        let frame = frame_with_content(id, Content::Text(TextContent::new("hello world")));

        let (outgoing, next) =
            optimize_frame_messages(&previous, frame, &HashMap::new(), APPEND_ONLY);

        assert_eq!(
            outgoing,
            vec![AppMessage::AppendContent {
                id,
                text: " world".to_string()
            }]
        );
        assert_eq!(
            next.regions.get(&id).map(|r| &r.content),
            Some(&Content::Text(TextContent::new("hello world")))
        );
    }

    #[test]
    fn optimize_converts_rich_text_last_span_growth_to_append_content() {
        let id = RegionId::new(1);
        let previous = tree_with_content(id, Content::RichText(RichText::new().text("hello")));
        let frame = frame_with_content(id, Content::RichText(RichText::new().text("hello world")));

        let (outgoing, next) =
            optimize_frame_messages(&previous, frame, &HashMap::new(), APPEND_ONLY);

        assert_eq!(
            outgoing,
            vec![AppMessage::AppendContent {
                id,
                text: " world".to_string()
            }]
        );
        assert_eq!(
            next.regions.get(&id).map(|r| &r.content),
            Some(&Content::RichText(RichText::new().text("hello world")))
        );
    }

    #[test]
    fn optimize_emits_delete_for_removed_region() {
        let removed = RegionId::new(9);
        let previous = tree_with_text(removed, "gone");

        let (outgoing, next) =
            optimize_frame_messages(&previous, Vec::new(), &HashMap::new(), APPEND_ONLY);

        assert_eq!(outgoing, vec![AppMessage::DeleteRegion { id: removed }]);
        assert!(next.regions.is_empty());
    }

    #[test]
    fn optimize_clears_content_when_region_becomes_container() {
        let id = RegionId::new(3);
        let previous = tree_with_text(id, "stale");
        let frame = vec![AppMessage::CreateRegion {
            id,
            parent: ROOT,
            insert_index: -1,
            constraints: Constraints::default(),
            style: Style::default(),
        }];

        let (outgoing, _next) =
            optimize_frame_messages(&previous, frame, &HashMap::new(), APPEND_ONLY);

        assert_eq!(
            outgoing,
            vec![AppMessage::SetContent {
                id,
                content: Content::None
            }]
        );
    }

    #[test]
    fn optimize_falls_back_to_set_content_when_append_not_negotiated() {
        let id = RegionId::new(1);
        let previous = tree_with_text(id, "alpha");
        let frame = frame_with_content(id, Content::Text(TextContent::new("alpha beta")));

        let (outgoing, _next) = optimize_frame_messages(
            &previous,
            frame,
            &HashMap::new(),
            FrameCapabilities::default(),
        );

        assert_eq!(
            outgoing,
            vec![AppMessage::SetContent {
                id,
                content: Content::Text(TextContent::new("alpha beta"))
            }]
        );
    }

    const APPEND_ONLY: FrameCapabilities = FrameCapabilities {
        append_content: true,
        semantics: false,
    };

    const ALL: FrameCapabilities = FrameCapabilities {
        append_content: true,
        semantics: true,
    };

    fn frame_with_semantics(
        id: RegionId,
        content: Content,
        semantics: Option<Semantics>,
    ) -> Vec<AppMessage> {
        let mut frame = frame_with_content(id, content);
        if let Some(semantics) = semantics {
            frame.insert(1, AppMessage::SetSemantics { id, semantics });
        }
        frame
    }

    fn labelled(label: &str) -> Semantics {
        Semantics {
            role: Role::Button,
            label: Some(label.to_string()),
            ..Semantics::default()
        }
    }

    fn semantics_sent(messages: &[AppMessage]) -> Vec<(RegionId, Semantics)> {
        messages
            .iter()
            .filter_map(|message| match message {
                AppMessage::SetSemantics { id, semantics } => Some((*id, semantics.clone())),
                _ => None,
            })
            .collect()
    }

    fn text(text: &str) -> Content {
        Content::Text(TextContent::new(text))
    }

    #[test]
    fn semantics_are_sent_with_a_new_region_after_it_is_created() {
        let id = RegionId::new(1);
        let frame = frame_with_semantics(id, text("x"), Some(labelled("Close")));

        let (outgoing, next) =
            optimize_frame_messages(&RetainedTree::new(), frame, &HashMap::new(), ALL);

        let create = outgoing
            .iter()
            .position(|m| matches!(m, AppMessage::CreateRegion { .. }))
            .unwrap();
        let set = outgoing
            .iter()
            .position(|m| matches!(m, AppMessage::SetSemantics { .. }))
            .unwrap();
        assert!(create < set);
        assert_eq!(semantics_sent(&outgoing), vec![(id, labelled("Close"))]);
        assert_eq!(next.regions[&id].semantics, Some(labelled("Close")));
    }

    #[test]
    fn unchanged_semantics_are_not_sent_again_and_changed_ones_are() {
        let id = RegionId::new(1);
        let first = frame_with_semantics(id, text("x"), Some(labelled("Close")));
        let (_, tree) = optimize_frame_messages(&RetainedTree::new(), first, &HashMap::new(), ALL);

        let same = frame_with_semantics(id, text("x"), Some(labelled("Close")));
        let (outgoing, tree) = optimize_frame_messages(&tree, same, &HashMap::new(), ALL);
        assert_eq!(outgoing, Vec::new(), "an unchanged frame sends nothing");

        let renamed = frame_with_semantics(id, text("x"), Some(labelled("Dismiss")));
        let (outgoing, tree) = optimize_frame_messages(&tree, renamed, &HashMap::new(), ALL);
        assert_eq!(semantics_sent(&outgoing), vec![(id, labelled("Dismiss"))]);

        // Back to what the content says: the terminal derives it again.
        let plain = frame_with_semantics(id, text("x"), None);
        let (outgoing, tree) = optimize_frame_messages(&tree, plain, &HashMap::new(), ALL);
        assert_eq!(semantics_sent(&outgoing), vec![(id, Semantics::default())]);
        assert_eq!(tree.regions[&id].semantics, None);

        let plain = frame_with_semantics(id, text("x"), None);
        let (outgoing, _) = optimize_frame_messages(&tree, plain, &HashMap::new(), ALL);
        assert_eq!(outgoing, Vec::new());
    }

    #[test]
    fn semantics_are_not_sent_without_the_capability() {
        let id = RegionId::new(1);
        let append_only = FrameCapabilities {
            append_content: true,
            semantics: false,
        };
        let frame = frame_with_semantics(id, text("x"), Some(labelled("Close")));
        let (outgoing, tree) =
            optimize_frame_messages(&RetainedTree::new(), frame, &HashMap::new(), append_only);
        assert_eq!(semantics_sent(&outgoing), Vec::new());

        let plain = frame_with_semantics(id, text("x"), None);
        let (outgoing, _) = optimize_frame_messages(&tree, plain, &HashMap::new(), append_only);
        assert_eq!(outgoing, Vec::new(), "no reset either");
    }

    fn live_log() -> Option<Semantics> {
        Some(Semantics {
            role: Role::Log,
            live: Live::Polite,
            ..Semantics::default()
        })
    }

    fn live_step(previous: Content, next: Content) -> Vec<AppMessage> {
        let id = RegionId::new(1);
        let first = frame_with_semantics(id, previous, live_log());
        let (_, tree) = optimize_frame_messages(&RetainedTree::new(), first, &HashMap::new(), ALL);
        let frame = frame_with_semantics(id, next, live_log());
        optimize_frame_messages(&tree, frame, &HashMap::new(), ALL).0
    }

    #[test]
    fn a_live_region_appends_new_text_even_when_its_styling_changes() {
        let id = RegionId::new(1);
        let bold = Style::new().bold();
        let before = Content::RichText(RichText::new().text("Hello "));
        let after = Content::RichText(RichText::new().text("Hello ").span("world", bold));

        assert_eq!(
            live_step(before, after.clone()),
            vec![
                AppMessage::AppendContent {
                    id,
                    text: "world".to_string()
                },
                AppMessage::SetContent { id, content: after }
            ]
        );
    }

    #[test]
    fn a_live_region_reappends_a_last_line_that_changed() {
        let id = RegionId::new(1);
        let bold = Style::new().bold();
        let before = Content::RichText(RichText::new().text("Done.\nA **bo"));
        let after = Content::RichText(
            RichText::new()
                .text("Done.\nA ")
                .span("bold", bold)
                .text(" word"),
        );

        assert_eq!(
            live_step(before, after.clone()),
            vec![
                AppMessage::AppendContent {
                    id,
                    text: "bold word".to_string()
                },
                AppMessage::SetContent { id, content: after }
            ]
        );
    }

    #[test]
    fn a_live_region_does_not_reannounce_rewritten_earlier_lines() {
        let id = RegionId::new(1);
        let after = text("Changed\nsecond and more");
        assert_eq!(
            live_step(text("First\nsecond"), after.clone()),
            vec![AppMessage::SetContent { id, content: after }]
        );
        let shrunk = text("First\nsec");
        assert_eq!(
            live_step(text("First\nsecond"), shrunk.clone()),
            vec![AppMessage::SetContent {
                id,
                content: shrunk
            }]
        );
        assert_eq!(live_step(text("same"), text("same")), Vec::new());
    }

    #[test]
    fn a_live_region_growing_plainly_sends_only_the_append() {
        let id = RegionId::new(1);
        assert_eq!(
            live_step(text("Hello"), text("Hello world")),
            vec![AppMessage::AppendContent {
                id,
                text: " world".to_string()
            }]
        );
        assert_eq!(
            live_step(text("héllo"), text("héllo wörld")),
            vec![AppMessage::AppendContent {
                id,
                text: " wörld".to_string()
            }]
        );
    }

    #[test]
    fn a_region_that_is_not_live_keeps_the_strict_append_rule() {
        let id = RegionId::new(1);
        let previous = tree_with_content(id, Content::RichText(RichText::new().text("Hello ")));
        let after = Content::RichText(
            RichText::new()
                .text("Hello ")
                .span("world", Style::new().bold()),
        );
        let (outgoing, _) = optimize_frame_messages(
            &previous,
            frame_with_content(id, after.clone()),
            &HashMap::new(),
            ALL,
        );
        assert_eq!(
            outgoing,
            vec![AppMessage::SetContent { id, content: after }]
        );
    }
}
