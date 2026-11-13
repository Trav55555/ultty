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
use std::ops::Range;

use ultty_protocol::{
    AppMessage, Constraints, Content, Live, RegionId, RichText, Semantics, Style,
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RegionSnapshot {
    pub(super) parent: RegionId,
    pub(super) key: Option<String>,
    pub(super) metadata: Vec<(String, String)>,
    pub(super) widget: Option<WidgetIdentity>,
    pub(super) constraints: Constraints,
    pub(super) style: Style,
    pub(super) content: Content,
    pub(super) scroll: Option<(f32, f32, bool)>,
    /// What the frame sent with `SetSemantics`; `None` leaves the region's
    /// semantics to the terminal.
    pub(super) semantics: Option<Semantics>,
    /// The text of a live region announced so far (sent as
    /// `AppendContent`), in the form it has now; empty when not live.
    pub(super) announced: String,
}

/// Regions the terminal currently retains: stable ids plus sibling order.
#[derive(Debug)]
pub(crate) struct RetainedTree {
    pub(super) regions: HashMap<RegionId, RegionSnapshot>,
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

/// Application identity captured independently of native reconciliation keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WidgetIdentity {
    pub(crate) id: String,
    pub(crate) focusable: bool,
}

/// A rendered widget after inherited visibility and enabled state are resolved.
pub(super) struct RenderedWidget<'a> {
    pub(super) region: RegionId,
    pub(super) identity: &'a WidgetIdentity,
    pub(super) enabled: bool,
}

impl RetainedTree {
    pub(super) fn rendered_widgets(&self) -> Vec<RenderedWidget<'_>> {
        let mut widgets = Vec::new();
        for (id, region) in &self.regions {
            let Some(identity) = &region.widget else {
                continue;
            };
            let mut cursor = Some(region);
            let mut visible = true;
            let mut enabled = true;
            while let Some(ancestor) = cursor {
                if ancestor.constraints.display == ultty_protocol::Display::None
                    || ancestor
                        .semantics
                        .as_ref()
                        .is_some_and(|semantics| semantics.hidden)
                {
                    visible = false;
                    break;
                }
                enabled &= !ancestor
                    .semantics
                    .as_ref()
                    .is_some_and(|semantics| semantics.disabled);
                cursor = self.regions.get(&ancestor.parent);
            }
            if visible {
                widgets.push(RenderedWidget {
                    region: *id,
                    identity,
                    enabled,
                });
            }
        }
        widgets
    }
}

/// What the view walk says about its regions besides the messages, keyed
/// by provisional region id.
#[derive(Debug, Default)]
pub(crate) struct FrameNotes {
    /// Native keys ([`crate::widgets::View::native_key`]).
    pub(crate) keys: HashMap<RegionId, String>,
    pub(crate) metadata: HashMap<RegionId, Vec<(String, String)>>,
    pub(crate) widgets: HashMap<RegionId, WidgetIdentity>,
    /// Live regions whose text is markdown prose still arriving: an inline
    /// opener at the end of it may yet turn into styling, so it is not
    /// announced yet ([`held_back`]).
    pub(crate) arriving_prose: HashSet<RegionId>,
    /// Live regions with bytes of their text drawn but not said: a task
    /// list item's checkbox, which its semantics give as `toggled`.
    pub(crate) unspoken: HashMap<RegionId, Range<usize>>,
}

thread_local! {
    /// Notes recorded by the view walk. `None` while no native frame is
    /// being collected, so walks done for tests or the mock backend record
    /// nothing.
    static NOTES_RECORDER: RefCell<Option<FrameNotes>> = const { RefCell::new(None) };
}

/// Record the native key of a region created during the current frame walk.
pub(crate) fn record_native_key(id: RegionId, key: &str) {
    NOTES_RECORDER.with(|recorder| {
        if let Some(notes) = recorder.borrow_mut().as_mut() {
            notes.keys.insert(id, key.to_string());
        }
    });
}

/// Record extra native region metadata for the current frame.
pub(crate) fn record_native_metadata(id: RegionId, metadata: Vec<(String, String)>) {
    NOTES_RECORDER.with(|recorder| {
        if let Some(notes) = recorder.borrow_mut().as_mut() {
            if !metadata.is_empty() {
                notes.metadata.insert(id, metadata);
            }
        }
    });
}

/// Record actual widget identity, without changing native reconciliation keys.
pub(crate) fn record_widget(id: RegionId, widget_id: &str, focusable: bool) {
    NOTES_RECORDER.with(|recorder| {
        if let Some(notes) = recorder.borrow_mut().as_mut() {
            notes.widgets.insert(
                id,
                WidgetIdentity {
                    id: widget_id.to_owned(),
                    focusable,
                },
            );
        }
    });
}

/// Record that region `id` of the current frame walk is markdown prose
/// still arriving.
pub(crate) fn record_arriving_prose(id: RegionId) {
    NOTES_RECORDER.with(|recorder| {
        if let Some(notes) = recorder.borrow_mut().as_mut() {
            notes.arriving_prose.insert(id);
        }
    });
}

/// Record that bytes `range` of region `id`'s text are drawn but not
/// announced.
pub(crate) fn record_unspoken(id: RegionId, range: Range<usize>) {
    NOTES_RECORDER.with(|recorder| {
        if let Some(notes) = recorder.borrow_mut().as_mut() {
            notes.unspoken.insert(id, range);
        }
    });
}

pub(crate) fn begin_frame_notes() {
    NOTES_RECORDER.with(|recorder| *recorder.borrow_mut() = Some(FrameNotes::default()));
}

pub(crate) fn take_frame_notes() -> FrameNotes {
    NOTES_RECORDER.with(|recorder| recorder.borrow_mut().take().unwrap_or_default())
}

struct ProvisionalNode {
    key: Option<String>,
    metadata: Vec<(String, String)>,
    widget: Option<WidgetIdentity>,
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
    /// An input's caret may be sent (`CAP_INPUT_CURSOR`); without it the
    /// frame's inputs go without one.
    pub(crate) input_cursor: bool,
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

/// Take the caret out of every input in `messages`, for a terminal that
/// cannot decode one.
fn drop_input_cursors(messages: &mut [AppMessage]) {
    for message in messages {
        if let AppMessage::SetContent {
            content: Content::Widget(ultty_protocol::WidgetContent::Input(input)),
            ..
        } = message
        {
            input.cursor = None;
        }
    }
}

pub(crate) fn optimize_frame_messages(
    previous: &RetainedTree,
    frame_messages: Vec<AppMessage>,
    notes: &FrameNotes,
    capabilities: FrameCapabilities,
) -> (Vec<AppMessage>, RetainedTree) {
    let mut frame_messages = frame_messages;
    if !capabilities.input_cursor {
        drop_input_cursors(&mut frame_messages);
    }
    let (nodes, order) = parse_provisional_tree(&frame_messages, notes);
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
        mut next,
    } = reconciliation;

    let mut outgoing = Vec::new();
    // Sent after every content change, so text a region gets in the frame
    // it stops being live is still announced.
    let mut semantics_changes = Vec::new();

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
                // Live in this frame or the last: text that arrives in the
                // frame a region stops being live is still announced.
                let live_now = is_live(next.regions.get(&stable));
                let live = live_now || is_live(previous.regions.get(&stable));
                let prev = previous.regions.get(&stable);
                match prev {
                    _ if live && capabilities.append_content && plain_text(&content).is_some() => {
                        let unspoken = notes.unspoken.get(&id).cloned().unwrap_or_default();
                        // A region that turns live has said what it shows
                        // already; only what it grows by is news.
                        let (before, announced) =
                            prev.map_or((&Content::None, String::new()), |prev| {
                                let announced = if is_live(Some(prev)) {
                                    prev.announced.clone()
                                } else {
                                    let shown = plain_text(&prev.content).unwrap_or_default();
                                    spoken_part(&shown, unspoken.clone()).into_owned()
                                };
                                (&prev.content, announced)
                            });
                        // Held back only while it stays live: the frame it
                        // stops is the last chance to say it.
                        let hold = live_now && notes.arriving_prose.contains(&id);
                        let (messages, announced) = live_content_update(
                            stable, before, &announced, content, hold, unspoken,
                        );
                        outgoing.extend(messages);
                        if let Some(region) = next.regions.get_mut(&stable) {
                            region.announced = announced;
                        }
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
                    // Turning live goes before the region's content, so
                    // what it grows by in this frame is announced; any
                    // other change after, so text that arrives as it stops
                    // being live still is.
                    let queue = if semantics.live != Live::Off {
                        &mut outgoing
                    } else {
                        &mut semantics_changes
                    };
                    queue.push(AppMessage::SetSemantics {
                        id: stable,
                        semantics,
                    });
                }
            }
            other => outgoing.push(other),
        }
    }

    outgoing.append(&mut semantics_changes);
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
    notes: &FrameNotes,
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
                        key: notes.keys.get(id).cloned(),
                        metadata: notes.metadata.get(id).cloned().unwrap_or_default(),
                        widget: notes.widgets.get(id).cloned(),
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
                metadata: node.metadata.clone(),
                widget: node.widget.clone(),
                constraints: node.constraints.clone(),
                style: node.style.clone(),
                content: node.content.clone(),
                scroll: node.scroll,
                semantics: node.semantics.clone(),
                announced: String::new(),
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
/// content, given the text it has `announced` so far (in its current
/// form); and what it has announced after them.
///
/// The terminal announces only text that arrives with `AppendContent`
/// (ADR 0021 §3), so what the region shows past what it has announced is
/// appended, then a `SetContent` in the same frame puts the styled result
/// in place when appending alone does not give it (a span turned bold, a
/// line rewritten). Where to resume is [`resume_at`]: text is never
/// skipped, and repeated rather than dropped when it cannot be told apart.
/// With `hold_back`, the end of the text from an unresolved inline
/// markdown opener waits ([`held_back`]). Bytes `unspoken` of the text
/// are never said.
fn live_content_update(
    id: RegionId,
    previous: &Content,
    announced: &str,
    next: Content,
    hold_back: bool,
    unspoken: Range<usize>,
) -> (Vec<AppMessage>, String) {
    let after = plain_text(&next).unwrap_or_default();
    let spoken = spoken_part(&after, unspoken);
    let speakable = if hold_back {
        held_back(&spoken)
    } else {
        &spoken
    };
    let start = resume_at(announced, speakable);
    let mut messages = Vec::new();
    let mut shown = previous.clone();
    if start < speakable.len() {
        let text = speakable[start..].to_string();
        shown = with_appended(previous, &text);
        messages.push(AppMessage::AppendContent { id, text });
    }
    // Everything up to `start` had been said, in some form, and the rest
    // just was.
    let announced = speakable.to_string();
    if shown != next {
        messages.push(AppMessage::SetContent { id, content: next });
    }
    (messages, announced)
}

/// `text` without bytes `unspoken`, or all of it when they are not a
/// range of it.
fn spoken_part(text: &str, unspoken: Range<usize>) -> Cow<'_, str> {
    match (text.get(..unspoken.start), text.get(unspoken.end..)) {
        (Some(""), Some(after)) => Cow::Borrowed(after),
        (Some(before), Some(after)) if unspoken.start <= unspoken.end => {
            Cow::Owned(format!("{before}{after}"))
        }
        _ => Cow::Borrowed(text),
    }
}

/// Where in `speakable` the text not yet announced starts, given that
/// `announced` was, in the form it then had.
///
/// Growth resumes where `announced` ends. Letter case is ignored, so a
/// line set in capitals once complete (a `# Title`) is not news. When an
/// earlier line was rewritten, the announced part of the last announced
/// line is looked for at the start of the same line now, and resumed
/// after. Text that only got shorter has nothing new. Otherwise it
/// resumes at the start of the word where the texts
/// part, so a word may be spoken twice but none is skipped.
fn resume_at(announced: &str, speakable: &str) -> usize {
    if let Some(end) = prefix_end(announced, speakable) {
        return end;
    }
    if prefix_end(speakable, announced).is_some() {
        // Only shorter: nothing new.
        return speakable.len();
    }
    let line = announced.matches('\n').count();
    let old_line_start = announced.rfind('\n').map_or(0, |index| index + 1);
    let same_line_start = if line == 0 {
        Some(0)
    } else {
        speakable
            .match_indices('\n')
            .nth(line - 1)
            .map(|(index, _)| index + 1)
    };
    if let Some(start) = same_line_start {
        if let Some(end) = prefix_end(&announced[old_line_start..], &speakable[start..]) {
            return start + end;
        }
    }
    let common = common_prefix_end(announced, speakable);
    speakable[..common]
        .rfind(char::is_whitespace)
        .map_or(0, |index| {
            index + speakable[index..].chars().next().map_or(1, char::len_utf8)
        })
}

/// Whether two characters are the same but for letter case.
fn same_letter(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Where `prefix` ends in `text`, if `text` starts with it (ignoring
/// letter case).
fn prefix_end(prefix: &str, text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for a in prefix.chars() {
        let (_, b) = chars.next()?;
        if !same_letter(a, b) {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(index, _)| index))
}

/// Where in `text` its longest common prefix with `other` ends (ignoring
/// letter case).
fn common_prefix_end(other: &str, text: &str) -> usize {
    let mut end = 0;
    for (a, (index, b)) in other.chars().zip(text.char_indices()) {
        if !same_letter(a, b) {
            return index;
        }
        end = index + b.len_utf8();
    }
    end
}

/// How far back a lone `*`, `_` or `` ` `` holds the text after it. Past
/// that it is taken as a literal character (`2 * 3`), so it delays speech
/// by at most this many bytes.
const HOLD_BACK_BYTES: usize = 80;

/// `text` without the end of its last line from the first inline markdown
/// opener still shown as characters: until it resolves into styling it is
/// drawn literally, and announcing it would speak `**bo` and then `bold`,
/// or a link's address. `[` holds until the line ends (for a paragraph,
/// until the block is finished) unless its `]` is already followed by
/// something that cannot start a link target (`x[i] = 0`); so do `**`,
/// `__` and `~~`. A lone `*`, `_` (not inside a word) or `` ` `` holds
/// only within [`HOLD_BACK_BYTES`] of the end: a code span that closed
/// would no longer show its backticks. A last line of only `=` or `-` is
/// held whole: under a line of text it is a setext underline that turns
/// the block into a heading.
fn held_back(text: &str) -> &str {
    let line_start = text.rfind('\n').map_or(0, |index| index + 1);
    let window_start = text.len().saturating_sub(HOLD_BACK_BYTES);
    let line = &text[line_start..];
    let underline = line.trim();
    if !underline.is_empty()
        && (underline.bytes().all(|b| b == b'=') || underline.bytes().all(|b| b == b'-'))
    {
        return &text[..line_start.saturating_sub(1)];
    }
    let mut previous: Option<char> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        let next = chars.peek().map(|(_, next)| *next);
        let at = line_start + offset;
        let opener = match c {
            '[' => !cannot_be_link(&line[offset..]),
            '`' => at >= window_start,
            '*' | '_' | '~' if next == Some(c) => {
                c != '_' || !previous.is_some_and(char::is_alphanumeric)
            }
            '*' => at >= window_start,
            '_' => at >= window_start && !previous.is_some_and(char::is_alphanumeric),
            _ => false,
        };
        if opener {
            return &text[..at];
        }
        previous = Some(c);
    }
    text
}

/// Whether `rest`, which starts with `[`, already shows its `]` followed by
/// a character that cannot open a link target, so it stays literal.
fn cannot_be_link(rest: &str) -> bool {
    let Some(close) = rest.find(']') else {
        return false;
    };
    rest[close + 1..]
        .chars()
        .next()
        .is_some_and(|after| after != '(' && after != '[')
}

/// Whether the frame said `region` is live.
fn is_live(region: Option<&RegionSnapshot>) -> bool {
    region
        .and_then(|region| region.semantics.as_ref())
        .is_some_and(|semantics| semantics.live != Live::Off)
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
    use super::{optimize_frame_messages, FrameCapabilities, FrameNotes, RetainedTree};
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
        optimize_frame_messages(
            previous,
            messages,
            &FrameNotes {
                keys,
                ..FrameNotes::default()
            },
            FrameCapabilities::default(),
        )
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
                metadata: Vec::new(),
                widget: None,
                constraints: Constraints::default(),
                style: Style::default(),
                content,
                scroll: None,
                semantics: None,
                announced: String::new(),
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
            optimize_frame_messages(&previous, frame, &FrameNotes::default(), APPEND_ONLY);

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
            optimize_frame_messages(&previous, frame, &FrameNotes::default(), APPEND_ONLY);

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
            optimize_frame_messages(&previous, Vec::new(), &FrameNotes::default(), APPEND_ONLY);

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
            optimize_frame_messages(&previous, frame, &FrameNotes::default(), APPEND_ONLY);

        assert_eq!(
            outgoing,
            vec![AppMessage::SetContent {
                id,
                content: Content::None
            }]
        );
    }

    #[test]
    fn an_inputs_caret_is_sent_only_to_a_terminal_that_negotiated_it() {
        use ultty_protocol::{InputWidget, WidgetContent};

        let id = RegionId::new(1);
        let input = |cursor| {
            Content::Widget(WidgetContent::Input(InputWidget {
                value: "héllo".into(),
                cursor,
                ..InputWidget::default()
            }))
        };
        let sent_cursor = |capabilities| {
            let frame = frame_with_content(id, input(Some(2)));
            let (outgoing, _next) = optimize_frame_messages(
                &RetainedTree::new(),
                frame,
                &FrameNotes::default(),
                capabilities,
            );
            outgoing
                .iter()
                .filter_map(|message| match message {
                    AppMessage::SetContent {
                        content: Content::Widget(ultty_protocol::WidgetContent::Input(input)),
                        ..
                    } => Some(input.cursor),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(sent_cursor(ALL), [Some(2)]);
        assert_eq!(sent_cursor(APPEND_ONLY), [None]);
    }

    #[test]
    fn optimize_falls_back_to_set_content_when_append_not_negotiated() {
        let id = RegionId::new(1);
        let previous = tree_with_text(id, "alpha");
        let frame = frame_with_content(id, Content::Text(TextContent::new("alpha beta")));

        let (outgoing, _next) = optimize_frame_messages(
            &previous,
            frame,
            &FrameNotes::default(),
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
        input_cursor: false,
    };

    const ALL: FrameCapabilities = FrameCapabilities {
        append_content: true,
        semantics: true,
        input_cursor: true,
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
            optimize_frame_messages(&RetainedTree::new(), frame, &FrameNotes::default(), ALL);

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
        let (_, tree) =
            optimize_frame_messages(&RetainedTree::new(), first, &FrameNotes::default(), ALL);

        let same = frame_with_semantics(id, text("x"), Some(labelled("Close")));
        let (outgoing, tree) = optimize_frame_messages(&tree, same, &FrameNotes::default(), ALL);
        assert_eq!(outgoing, Vec::new(), "an unchanged frame sends nothing");

        let renamed = frame_with_semantics(id, text("x"), Some(labelled("Dismiss")));
        let (outgoing, tree) = optimize_frame_messages(&tree, renamed, &FrameNotes::default(), ALL);
        assert_eq!(semantics_sent(&outgoing), vec![(id, labelled("Dismiss"))]);

        // Back to what the content says: the terminal derives it again.
        let plain = frame_with_semantics(id, text("x"), None);
        let (outgoing, tree) = optimize_frame_messages(&tree, plain, &FrameNotes::default(), ALL);
        assert_eq!(semantics_sent(&outgoing), vec![(id, Semantics::default())]);
        assert_eq!(tree.regions[&id].semantics, None);

        let plain = frame_with_semantics(id, text("x"), None);
        let (outgoing, _) = optimize_frame_messages(&tree, plain, &FrameNotes::default(), ALL);
        assert_eq!(outgoing, Vec::new());
    }

    #[test]
    fn semantics_are_not_sent_without_the_capability() {
        let id = RegionId::new(1);
        let append_only = FrameCapabilities {
            append_content: true,
            semantics: false,
            input_cursor: false,
        };
        let frame = frame_with_semantics(id, text("x"), Some(labelled("Close")));
        let (outgoing, tree) = optimize_frame_messages(
            &RetainedTree::new(),
            frame,
            &FrameNotes::default(),
            append_only,
        );
        assert_eq!(semantics_sent(&outgoing), Vec::new());

        let plain = frame_with_semantics(id, text("x"), None);
        let (outgoing, _) =
            optimize_frame_messages(&tree, plain, &FrameNotes::default(), append_only);
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
        let (_, tree) =
            optimize_frame_messages(&RetainedTree::new(), first, &FrameNotes::default(), ALL);
        let frame = frame_with_semantics(id, next, live_log());
        optimize_frame_messages(&tree, frame, &FrameNotes::default(), ALL).0
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
    fn a_marker_held_back_on_the_last_line_is_spoken_once_resolved() {
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
    fn a_rewrite_is_set_once_and_only_text_past_what_was_announced_is_appended() {
        let id = RegionId::new(1);
        // An earlier line rewritten while new text arrives: the new text
        // past the old length is still announced.
        let after = text("Changed\nsecond and more");
        assert_eq!(
            live_step(text("First\nsecond"), after.clone()),
            vec![
                AppMessage::AppendContent {
                    id,
                    text: " and more".to_string()
                },
                AppMessage::SetContent { id, content: after }
            ]
        );
        // A one-line restyle (setext heading, `Title` set as `TITLE`) is
        // not announced again.
        let upper = text("TITLE");
        assert_eq!(
            live_step(text("Title"), upper.clone()),
            vec![AppMessage::SetContent { id, content: upper }]
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
        // Nothing in common: all of it, repeated rather than dropped.
        assert_eq!(
            live_step(text("aé"), text("ééé")),
            vec![
                AppMessage::AppendContent {
                    id,
                    text: "ééé".to_string()
                },
                AppMessage::SetContent {
                    id,
                    content: text("ééé")
                }
            ]
        );
    }

    #[test]
    fn a_region_created_live_announces_its_first_text_after_turning_live() {
        let id = RegionId::new(1);
        let frame = frame_with_semantics(id, text("Thinking about it"), live_log());
        let (outgoing, tree) =
            optimize_frame_messages(&RetainedTree::new(), frame, &FrameNotes::default(), ALL);
        assert!(matches!(outgoing[0], AppMessage::CreateRegion { .. }));
        assert!(matches!(outgoing[1], AppMessage::SetSemantics { .. }));
        assert_eq!(
            outgoing[2..],
            [AppMessage::AppendContent {
                id,
                text: "Thinking about it".to_string()
            }]
        );
        assert_eq!(tree.regions[&id].announced, "Thinking about it");
    }

    #[test]
    fn unresolved_inline_markdown_is_held_back_until_it_resolves() {
        let id = RegionId::new(1);
        let bold = Style::new().bold();
        for (open, resolved) in [
            (
                "A **bo",
                RichText::new()
                    .text("A ")
                    .span("bold", bold.clone())
                    .text(" move"),
            ),
            (
                "Run `fo",
                RichText::new()
                    .text("Run ")
                    .span("foo", bold.clone())
                    .text(" now"),
            ),
            (
                "See [the docs](http://exa",
                RichText::new()
                    .text("See ")
                    .span("the docs", bold.clone())
                    .text(" now"),
            ),
        ] {
            let first = frame_with_semantics(id, text(open), live_log());
            let (outgoing, tree) =
                optimize_frame_messages(&RetainedTree::new(), first, &prose(), ALL);
            let spoken: Vec<&AppMessage> = outgoing
                .iter()
                .filter(|m| matches!(m, AppMessage::AppendContent { .. }))
                .collect();
            let prefix = &open[..open.find(['*', '`', '[']).unwrap()];
            assert_eq!(
                spoken,
                [&AppMessage::AppendContent {
                    id,
                    text: prefix.to_string()
                }],
                "{open:?}"
            );

            let resolved = Content::RichText(resolved);
            let second = frame_with_semantics(id, resolved.clone(), live_log());
            let (outgoing, _) = optimize_frame_messages(&tree, second, &prose(), ALL);
            let whole = plain_text_of(&resolved);
            assert_eq!(
                outgoing,
                vec![
                    AppMessage::AppendContent {
                        id,
                        text: whole[prefix.len()..].to_string()
                    },
                    AppMessage::SetContent {
                        id,
                        content: resolved
                    }
                ],
                "{open:?}: nothing is spoken twice"
            );
        }
        // A literal opener long past is not held back.
        let literal = format!("2 * 3 is six, {}", "and so on ".repeat(10));
        let (outgoing, _) = optimize_frame_messages(
            &RetainedTree::new(),
            frame_with_semantics(id, text(&literal), live_log()),
            &prose(),
            ALL,
        );
        assert!(outgoing.contains(&AppMessage::AppendContent {
            id,
            text: literal.clone()
        }));
        // snake_case is a word, not an opener.
        assert_eq!(super::held_back("a snake_case"), "a snake_case");
        // A bracket that can no longer become a link does not hold the line.
        assert_eq!(
            super::held_back("Use array[0] to get the first"),
            "Use array[0] to get the first"
        );
        assert_eq!(super::held_back("see [1] and [the"), "see [1] and ");
        assert_eq!(super::held_back("see [the docs]"), "see ");
        assert_eq!(super::held_back("see [the docs](http"), "see ");
        // A lone backtick far back is literal; a recent one still holds.
        let tick = format!("Type ` then {}", "and so on ".repeat(10));
        assert_eq!(super::held_back(&tick), tick);
        assert_eq!(super::held_back("run `cargo"), "run ");
        // In the frame the region stops being live, everything goes.
        let first = frame_with_semantics(id, text("A **bo"), live_log());
        let (_, tree) = optimize_frame_messages(&RetainedTree::new(), first, &prose(), ALL);
        let last = frame_with_semantics(id, text("A **bo"), None);
        let (outgoing, _) = optimize_frame_messages(&tree, last, &prose(), ALL);
        assert_eq!(
            outgoing[0],
            AppMessage::AppendContent {
                id,
                text: "**bo".to_string()
            }
        );
    }

    #[test]
    fn a_link_address_and_a_setext_underline_are_never_spoken() {
        let id = RegionId::new(1);
        let bold = Style::new().bold();
        let frames = [
            text("See [te"),
            text("See [text](ht"),
            text("See [text](http://x \"ti"),
            Content::RichText(RichText::new().text("See ").span("text", bold).text(" now")),
            text("See text now\nTitle"),
            text("See text now\nTitle\n="),
            text("See text now\nTitle\n====="),
            text("See text now\nTITLE"),
        ];
        let mut tree = RetainedTree::new();
        let mut said = Vec::new();
        for content in frames {
            let frame = frame_with_semantics(id, content, live_log());
            let (outgoing, next) = optimize_frame_messages(&tree, frame, &prose(), ALL);
            tree = next;
            said.extend(outgoing.into_iter().filter_map(|m| match m {
                AppMessage::AppendContent { text, .. } => Some(text),
                _ => None,
            }));
        }
        assert_eq!(said, ["See ", "text now", "\nTitle"]);
        assert_eq!(super::held_back("Title\n====="), "Title");
        assert_eq!(super::held_back("Title\n-- "), "Title");
        assert_eq!(super::held_back("a\n= b"), "a\n= b");
        assert_eq!(super::held_back("=-="), "=-=");
    }

    #[test]
    fn an_unspoken_prefix_is_drawn_but_not_announced() {
        let id = RegionId::new(1);
        let notes = || FrameNotes {
            unspoken: [(id, 0..4)].into(),
            ..FrameNotes::default()
        };
        let first = frame_with_semantics(id, text("[ ] buy"), live_log());
        let (outgoing, tree) = optimize_frame_messages(&RetainedTree::new(), first, &notes(), ALL);
        assert_eq!(
            outgoing[2..],
            [
                AppMessage::AppendContent {
                    id,
                    text: "buy".to_string()
                },
                AppMessage::SetContent {
                    id,
                    content: text("[ ] buy")
                }
            ]
        );
        let second = frame_with_semantics(id, text("[ ] buy milk"), live_log());
        let (outgoing, _) = optimize_frame_messages(&tree, second, &notes(), ALL);
        assert_eq!(
            outgoing,
            [AppMessage::AppendContent {
                id,
                text: " milk".to_string()
            }]
        );
    }

    fn plain_text_of(content: &Content) -> String {
        super::plain_text(content).unwrap().into_owned()
    }

    /// Notes for a frame whose region 1 is markdown prose still arriving.
    fn prose() -> FrameNotes {
        FrameNotes {
            arriving_prose: [RegionId::new(1)].into(),
            ..FrameNotes::default()
        }
    }

    #[test]
    fn only_arriving_markdown_prose_is_held_back() {
        let id = RegionId::new(1);
        // A plain live log says brackets and stars as they come.
        let line = "[INFO] a * b `x";
        let (outgoing, _) = optimize_frame_messages(
            &RetainedTree::new(),
            frame_with_semantics(id, text(line), live_log()),
            &FrameNotes::default(),
            ALL,
        );
        assert!(outgoing.contains(&AppMessage::AppendContent {
            id,
            text: line.to_string()
        }));
    }

    #[test]
    fn resuming_never_skips_text() {
        use super::resume_at;
        // Growth.
        assert_eq!(resume_at("Hello", "Hello world"), 5);
        // Case only (a heading set in capitals): nothing new.
        assert_eq!(resume_at("Title", "TITLE"), 5);
        // An earlier line rewritten: resume on the same line.
        let after = "Changed\nsecond and more";
        assert_eq!(&after[resume_at("First\nsecond", after)..], " and more");
        // Announced text that shrank (a literal opener that resolved):
        // resume at the start of the word where they part.
        let after = "Start this is bold";
        assert_eq!(&after[resume_at("Start *", after)..], "this is bold");
        let after = "See the docs for";
        assert_eq!(
            &after[resume_at("See [the docs](https://example.com/a", after)..],
            "the docs for"
        );
        // Never past the end.
        assert_eq!(resume_at("abcdef", "abc"), 3);
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
    fn text_that_arrives_as_a_region_stops_being_live_is_still_appended_first() {
        let id = RegionId::new(1);
        let first = frame_with_semantics(id, text("Hello"), live_log());
        let (_, tree) =
            optimize_frame_messages(&RetainedTree::new(), first, &FrameNotes::default(), ALL);

        let last = frame_with_semantics(id, text("Hello world"), None);
        let (outgoing, _) = optimize_frame_messages(&tree, last, &FrameNotes::default(), ALL);
        assert_eq!(
            outgoing,
            vec![
                AppMessage::AppendContent {
                    id,
                    text: " world".to_string()
                },
                AppMessage::SetSemantics {
                    id,
                    semantics: Semantics::default()
                }
            ]
        );
    }

    #[test]
    fn a_live_container_never_gets_its_children_text_appended() {
        let id = RegionId::new(1);
        let child = RegionId::new(2);
        let frame = |line: &str| {
            vec![
                AppMessage::CreateRegion {
                    id,
                    parent: ROOT,
                    insert_index: -1,
                    constraints: Constraints::default(),
                    style: Style::default(),
                },
                AppMessage::SetSemantics {
                    id,
                    semantics: live_log().unwrap(),
                },
                AppMessage::CreateRegion {
                    id: child,
                    parent: id,
                    insert_index: -1,
                    constraints: Constraints::default(),
                    style: Style::default(),
                },
                AppMessage::SetContent {
                    id: child,
                    content: text(line),
                },
            ]
        };
        let (_, tree) = optimize_frame_messages(
            &RetainedTree::new(),
            frame("Hi"),
            &FrameNotes::default(),
            ALL,
        );
        let (outgoing, _) =
            optimize_frame_messages(&tree, frame("Hi there"), &FrameNotes::default(), ALL);
        assert!(
            !outgoing.iter().any(
                |m| matches!(m, AppMessage::AppendContent { id: target, .. } if *target == id)
            ),
            "{outgoing:?}"
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
            &FrameNotes::default(),
            ALL,
        );
        assert_eq!(
            outgoing,
            vec![AppMessage::SetContent { id, content: after }]
        );
    }
}
