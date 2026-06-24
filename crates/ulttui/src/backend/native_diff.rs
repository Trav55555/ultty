//! Native protocol frame diffing.
//!
//! Ulttui view trees are rebuilt in immediate-mode style each render pass, but
//! native protocol regions are retained by the terminal. This optimizer maps the
//! fresh frame's traversal-order `RegionId`s onto the previous retained
//! snapshot. Until stable native keys are wired through the protocol, sibling
//! insertion/reorder changes are treated as content/style updates to existing
//! traversal slots rather than semantic moves.

use std::collections::HashMap;

use ultty_protocol::{AppMessage, Constraints, Content, RegionId, RichText, Style};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RegionSnapshot {
    pub(super) parent: RegionId,
    pub(super) constraints: Constraints,
    pub(super) style: Style,
    pub(super) content: Content,
    pub(super) scroll: Option<(f32, f32, bool)>,
}

pub(super) fn optimize_frame_messages(
    previous_regions: &HashMap<RegionId, RegionSnapshot>,
    frame_messages: Vec<AppMessage>,
    allow_append_content: bool,
) -> (Vec<AppMessage>, HashMap<RegionId, RegionSnapshot>) {
    let mut outgoing = Vec::new();
    let mut next_regions: HashMap<RegionId, RegionSnapshot> = HashMap::new();

    for msg in frame_messages {
        match msg {
            AppMessage::CreateRegion {
                id,
                parent,
                insert_index,
                constraints,
                style,
            } => {
                let next_snapshot = RegionSnapshot {
                    parent,
                    constraints: constraints.clone(),
                    style: style.clone(),
                    content: Content::None,
                    scroll: None,
                };
                next_regions.insert(id, next_snapshot);

                if let Some(prev) = previous_regions.get(&id) {
                    if prev.parent != parent {
                        outgoing.push(AppMessage::ReparentRegion {
                            id,
                            new_parent: parent,
                            insert_index,
                        });
                    }
                    if prev.constraints != constraints {
                        outgoing.push(AppMessage::SetConstraints { id, constraints });
                    }
                    if prev.style != style {
                        outgoing.push(AppMessage::SetStyle { id, style });
                    }
                } else {
                    outgoing.push(AppMessage::CreateRegion {
                        id,
                        parent,
                        insert_index,
                        constraints,
                        style,
                    });
                }
            }
            AppMessage::SetContent { id, content } => {
                if let Some(next) = next_regions.get_mut(&id) {
                    next.content = content.clone();
                } else if let Some(prev) = previous_regions.get(&id) {
                    next_regions.insert(id, prev.clone());
                    if let Some(next) = next_regions.get_mut(&id) {
                        next.content = content.clone();
                    }
                }

                if let Some(prev) = previous_regions.get(&id) {
                    if allow_append_content {
                        if let Some(delta) = append_delta(&prev.content, &content) {
                            if !delta.is_empty() {
                                outgoing.push(AppMessage::AppendContent { id, text: delta });
                            }
                        } else if prev.content != content {
                            outgoing.push(AppMessage::SetContent { id, content });
                        }
                    } else if prev.content != content {
                        outgoing.push(AppMessage::SetContent { id, content });
                    }
                } else {
                    outgoing.push(AppMessage::SetContent { id, content });
                }
            }
            AppMessage::Scroll { id, x, y, animate } => {
                if let Some(next) = next_regions.get_mut(&id) {
                    next.scroll = Some((x, y, animate));
                }

                let changed = previous_regions
                    .get(&id)
                    .map(|prev| prev.scroll != Some((x, y, animate)))
                    .unwrap_or(true);
                if changed {
                    outgoing.push(AppMessage::Scroll { id, x, y, animate });
                }
            }
            other => outgoing.push(other),
        }
    }

    for (id, next) in &next_regions {
        if let Some(prev) = previous_regions.get(id) {
            if prev.content != Content::None && next.content == Content::None {
                outgoing.push(AppMessage::SetContent {
                    id: *id,
                    content: Content::None,
                });
            }
        }
    }

    for id in previous_regions.keys() {
        if !next_regions.contains_key(id) {
            outgoing.push(AppMessage::DeleteRegion { id: *id });
        }
    }

    (outgoing, next_regions)
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
    use super::{optimize_frame_messages, RegionSnapshot};
    use std::collections::HashMap;
    use ultty_protocol::{
        AppMessage, Constraints, Content, RegionId, RichText, Style, TextContent,
    };

    fn snapshot_with_text(text: &str) -> RegionSnapshot {
        RegionSnapshot {
            parent: RegionId::ROOT,
            constraints: Constraints::default(),
            style: Style::default(),
            content: Content::Text(TextContent::new(text)),
            scroll: None,
        }
    }

    fn create_region(id: RegionId) -> AppMessage {
        AppMessage::CreateRegion {
            id,
            parent: RegionId::ROOT,
            insert_index: -1,
            constraints: Constraints::default(),
            style: Style::default(),
        }
    }

    fn set_text(id: RegionId, text: &str) -> AppMessage {
        AppMessage::SetContent {
            id,
            content: Content::Text(TextContent::new(text)),
        }
    }

    #[test]
    fn optimize_converts_text_growth_to_append_content() {
        let id = RegionId::new(1);
        let mut previous = HashMap::new();
        previous.insert(id, snapshot_with_text("hello"));

        let frame = vec![
            AppMessage::CreateRegion {
                id,
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent {
                id,
                content: Content::Text(TextContent::new("hello world")),
            },
        ];

        let (outgoing, next) = optimize_frame_messages(&previous, frame, true);

        assert!(outgoing.iter().any(|m| {
            matches!(m, AppMessage::AppendContent { id: mid, text } if *mid == id && text == " world")
        }));
        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::CreateRegion { id: mid, .. } if *mid == id)));
        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::SetContent { id: mid, .. } if *mid == id)));
        assert_eq!(
            next.get(&id).map(|r| &r.content),
            Some(&Content::Text(TextContent::new("hello world")))
        );
    }

    #[test]
    fn optimize_converts_rich_text_last_span_growth_to_append_content() {
        let id = RegionId::new(1);
        let mut previous = HashMap::new();
        previous.insert(
            id,
            RegionSnapshot {
                parent: RegionId::ROOT,
                constraints: Constraints::default(),
                style: Style::default(),
                content: Content::RichText(RichText::new().text("hello")),
                scroll: None,
            },
        );

        let frame = vec![
            AppMessage::CreateRegion {
                id,
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent {
                id,
                content: Content::RichText(RichText::new().text("hello world")),
            },
        ];

        let (outgoing, next) = optimize_frame_messages(&previous, frame, true);

        assert!(outgoing.iter().any(|m| {
            matches!(m, AppMessage::AppendContent { id: mid, text } if *mid == id && text == " world")
        }));
        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::SetContent { id: mid, .. } if *mid == id)));
        assert_eq!(
            next.get(&id).map(|r| &r.content),
            Some(&Content::RichText(RichText::new().text("hello world")))
        );
    }

    #[test]
    fn optimize_documents_traversal_order_identity_on_sibling_insert() {
        let id_1 = RegionId::new(1);
        let id_2 = RegionId::new(2);
        let id_3 = RegionId::new(3);

        let mut previous = HashMap::new();
        previous.insert(id_1, snapshot_with_text("A"));
        previous.insert(id_2, snapshot_with_text("B"));

        // Semantic children changed from [A, B] to [X, A, B]. Because region
        // IDs are traversal-order slots today, the optimizer mutates slot 1 to
        // X and slot 2 to A, then creates slot 3 for B. A future stable-keyed
        // native identity layer can preserve semantic A/B identity instead.
        let frame = vec![
            create_region(id_1),
            set_text(id_1, "X"),
            create_region(id_2),
            set_text(id_2, "A"),
            create_region(id_3),
            set_text(id_3, "B"),
        ];

        let (outgoing, next) = optimize_frame_messages(&previous, frame, false);

        assert!(outgoing.iter().any(|m| {
            matches!(m, AppMessage::SetContent { id, content: Content::Text(text) } if *id == id_1 && text.text == "X")
        }));
        assert!(outgoing.iter().any(|m| {
            matches!(m, AppMessage::SetContent { id, content: Content::Text(text) } if *id == id_2 && text.text == "A")
        }));
        assert!(outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::CreateRegion { id, .. } if *id == id_3)));
        assert_eq!(
            next.get(&id_3).map(|r| &r.content),
            Some(&Content::Text(TextContent::new("B")))
        );
    }

    #[test]
    fn optimize_emits_delete_for_removed_region() {
        let removed = RegionId::new(9);
        let mut previous = HashMap::new();
        previous.insert(removed, snapshot_with_text("gone"));

        let (outgoing, _next) = optimize_frame_messages(&previous, Vec::new(), true);

        assert!(outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::DeleteRegion { id } if *id == removed)));
    }

    #[test]
    fn optimize_clears_content_when_region_becomes_container() {
        let id = RegionId::new(3);
        let mut previous = HashMap::new();
        previous.insert(id, snapshot_with_text("stale"));

        let frame = vec![AppMessage::CreateRegion {
            id,
            parent: RegionId::ROOT,
            insert_index: -1,
            constraints: Constraints::default(),
            style: Style::default(),
        }];

        let (outgoing, _next) = optimize_frame_messages(&previous, frame, true);

        assert!(outgoing.iter().any(|m| {
            matches!(
                m,
                AppMessage::SetContent {
                    id: mid,
                    content: Content::None
                } if *mid == id
            )
        }));
    }

    #[test]
    fn optimize_falls_back_to_set_content_when_append_not_negotiated() {
        let id = RegionId::new(1);
        let mut previous = HashMap::new();
        previous.insert(id, snapshot_with_text("alpha"));

        let frame = vec![
            AppMessage::CreateRegion {
                id,
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent {
                id,
                content: Content::Text(TextContent::new("alpha beta")),
            },
        ];

        let (outgoing, _next) = optimize_frame_messages(&previous, frame, false);

        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::AppendContent { id: mid, .. } if *mid == id)));
        assert!(outgoing.iter().any(|m| {
            matches!(
                m,
                AppMessage::SetContent {
                    id: mid,
                    content: Content::Text(_)
                } if *mid == id
            )
        }));
    }
}
