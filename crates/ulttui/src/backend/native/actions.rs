//! Rendered widget declarations and authoritative application-side action fence.

use std::collections::HashMap;

use ultty_protocol::{
    valid_action_widget_id, AppMessage, RegionId, SemanticActionKind, SemanticActionRequest,
    CAP_SEMANTIC_ACTIONS, REGION_ACTIONS_METADATA_KEY, REGION_WIDGET_ID_METADATA_KEY,
};

use super::{NativeBackend, REGION_ID_METADATA_KEY};
use crate::backend::native_diff::RetainedTree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActionTarget {
    pub(super) widget_id: String,
    pub(super) actions: Vec<SemanticActionKind>,
}

pub(super) fn collect_targets(
    tree: &RetainedTree,
    supports: &mut impl FnMut(&str, SemanticActionKind) -> bool,
) -> HashMap<RegionId, ActionTarget> {
    let widgets = tree.rendered_widgets();
    let mut counts = HashMap::new();
    for widget in &widgets {
        *counts.entry(widget.identity.id.as_str()).or_insert(0) += 1;
    }
    widgets
        .into_iter()
        .filter_map(|widget| {
            let id = &widget.identity.id;
            if counts[id.as_str()] != 1 || !valid_action_widget_id(id) {
                return None;
            }
            let mut actions = Vec::new();
            if widget.enabled {
                if widget.identity.focusable && supports(id, SemanticActionKind::Focus) {
                    actions.push(SemanticActionKind::Focus);
                }
                if supports(id, SemanticActionKind::Activate) {
                    actions.push(SemanticActionKind::Activate);
                }
            }
            Some((
                widget.region,
                ActionTarget {
                    widget_id: id.clone(),
                    actions,
                },
            ))
        })
        .collect()
}

pub(super) fn declare_targets(
    messages: &mut Vec<AppMessage>,
    tree: &RetainedTree,
    before: &HashMap<RegionId, ActionTarget>,
    after: &HashMap<RegionId, ActionTarget>,
) {
    let mut changed: Vec<_> = before.keys().chain(after.keys()).copied().collect();
    changed.sort_by_key(|id| id.0);
    changed.dedup();
    for id in changed {
        if before.get(&id) == after.get(&id) {
            continue;
        }
        // Metadata replaces the whole map. Preserve the reconciliation key.
        let mut metadata = Vec::new();
        if let Some(key) = tree.key_of(id) {
            metadata.push((REGION_ID_METADATA_KEY.to_owned(), key.to_owned()));
        }
        if let Some(target) = after.get(&id) {
            metadata.push((
                REGION_WIDGET_ID_METADATA_KEY.to_owned(),
                target.widget_id.clone(),
            ));
            metadata.push((
                REGION_ACTIONS_METADATA_KEY.to_owned(),
                target
                    .actions
                    .iter()
                    .map(|action| action.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        // A removed region needs no metadata; sending it would target a deleted ID.
        if !messages.iter().any(
            |message| matches!(message, AppMessage::DeleteRegion { id: removed } if *removed == id),
        ) {
            messages.push(AppMessage::SetRegionMetadata { id, metadata });
        }
    }
}

impl NativeBackend {
    /// Take the next terminal action for application-owned dispatch.
    pub fn take_action_request(&mut self) -> Option<SemanticActionRequest> {
        self.pending_actions.pop_front()
    }

    /// Verify the last presented frame and its exact visible unique target.
    pub fn validate_action_request(&self, request: &SemanticActionRequest) -> Result<(), String> {
        if !self.supports_capability(CAP_SEMANTIC_ACTIONS) {
            return Err("semantic actions were not negotiated".into());
        }
        if self.presented_frame != Some(request.frame_id) {
            return Err("application frame changed".into());
        }
        let target = self
            .action_targets
            .get(&request.id)
            .ok_or_else(|| "widget is missing, hidden or ambiguous".to_string())?;
        if target.widget_id != request.widget_id || !target.actions.contains(&request.action) {
            return Err("widget identity or supported action changed".into());
        }
        Ok(())
    }

    /// Acknowledge applied dispatch, or refuse without exposing application data.
    pub fn send_action_result(
        &mut self,
        request_id: u32,
        error: Option<String>,
    ) -> crate::Result<()> {
        if error.as_ref().is_some_and(|error| {
            error.len() > ultty_protocol::MAX_ACTION_ERROR_BYTES
                || error.chars().any(char::is_control)
        }) {
            return Err(ultty_protocol::ProtocolError::InvalidData(
                "invalid action refusal".into(),
            )
            .into());
        }
        self.send_message(&AppMessage::ActionResult { request_id, error })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    use ultty_protocol::{Content, Decode, FrameId, Semantics, CAP_REGION_METADATA, CAP_SEMANTICS};

    use super::*;
    use crate::widgets::{Column, View};

    struct Target {
        widget: &'static str,
        key: &'static str,
        hidden: bool,
        disabled: bool,
    }

    impl View for Target {
        fn widget_id(&self) -> Option<&str> {
            Some(self.widget)
        }
        fn native_key(&self) -> Option<&str> {
            Some(self.key)
        }
        fn is_focusable(&self) -> bool {
            true
        }
        fn content(&self) -> Content {
            Content::None
        }
        fn semantics(&self) -> Semantics {
            Semantics {
                hidden: self.hidden,
                disabled: self.disabled,
                ..Default::default()
            }
        }
    }

    fn target(widget: &'static str, key: &'static str) -> Target {
        Target {
            widget,
            key,
            hidden: false,
            disabled: false,
        }
    }

    fn backend() -> (NativeBackend, UnixStream) {
        let (writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(writer);
        backend.negotiated_capabilities =
            CAP_SEMANTIC_ACTIONS | CAP_REGION_METADATA | CAP_SEMANTICS;
        (backend, reader)
    }

    #[test]
    fn test_semantic_actions_fence_registry_frame_and_explicit_permission() {
        let (mut backend, mut reader) = backend();
        backend
            .render_with_actions(&target("actual", "native-key"), |_, _| true, None)
            .unwrap();
        let id = *backend.action_targets.keys().next().unwrap();
        let mut request = SemanticActionRequest {
            request_id: 1,
            frame_id: backend.presented_frame.unwrap(),
            id,
            widget_id: "actual".into(),
            action: SemanticActionKind::Activate,
        };
        assert!(backend.validate_action_request(&request).is_ok());
        request.widget_id = "native-key".into();
        assert!(backend.validate_action_request(&request).is_err());
        request.widget_id = "actual".into();
        // Same native key and pixels, different app identity: commit a new frame.
        backend
            .render_with_actions(&target("replacement", "native-key"), |_, _| true, None)
            .unwrap();
        assert!(backend.action_targets.contains_key(&id));
        assert!(backend.validate_action_request(&request).is_err());
        request.frame_id = backend.presented_frame.unwrap();
        request.widget_id = "replacement".into();
        assert!(backend.validate_action_request(&request).is_ok());
        // Permission changes alone must also advance the app frame.
        let before = backend.presented_frame;
        backend
            .render_with_actions(
                &target("replacement", "native-key"),
                |_, action| action == SemanticActionKind::Focus,
                None,
            )
            .unwrap();
        assert_ne!(backend.presented_frame, before);
        request.frame_id = backend.presented_frame.unwrap();
        assert!(backend.validate_action_request(&request).is_err());
        request.action = SemanticActionKind::Focus;
        assert!(backend.validate_action_request(&request).is_ok());
        let mut bytes = Vec::new();
        if let Err(error) = reader.read_to_end(&mut bytes) {
            assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        }
        let mut input = &bytes[..];
        let mut metadata = Vec::new();
        while !input.is_empty() {
            if let AppMessage::SetRegionMetadata {
                metadata: values, ..
            } = AppMessage::decode(&mut input).unwrap()
            {
                metadata.push(values);
            }
        }
        assert!(metadata.iter().any(|values| values
            .contains(&(REGION_WIDGET_ID_METADATA_KEY.into(), "actual".into()))
            && values.contains(&(REGION_ID_METADATA_KEY.into(), "native-key".into()))));
        backend.next_frame_id = u32::MAX;
        assert!(backend
            .render_with_actions(&target("other", "native-key"), |_, _| true, None)
            .is_err());
        assert_ne!(backend.presented_frame, Some(FrameId(u32::MAX)));
    }

    #[test]
    fn test_semantic_actions_refuse_duplicates_hidden_and_disabled() {
        let (mut backend, _reader) = backend();
        let duplicate = Column::new()
            .child(target("same", "first"))
            .child(target("same", "second"));
        backend
            .render_with_actions(&duplicate, |_, _| true, None)
            .unwrap();
        assert!(backend.action_targets.is_empty());
        let mut hidden = target("hidden", "hidden");
        hidden.hidden = true;
        let mut disabled = target("disabled", "disabled");
        disabled.disabled = true;
        backend
            .render_with_actions(
                &Column::new().child(hidden).child(disabled),
                |_, _| true,
                None,
            )
            .unwrap();
        assert_eq!(backend.action_targets.len(), 1);
        assert!(backend
            .action_targets
            .values()
            .all(|target| target.widget_id == "disabled" && target.actions.is_empty()));
    }

    #[test]
    fn test_semantic_root_identity_survives_an_anonymous_presentation_wrapper() {
        let (mut backend, _reader) = backend();
        backend
            .render_with_actions(&Column::new(), |_, _| true, Some(("app-root", true)))
            .unwrap();
        assert_eq!(backend.action_targets.len(), 1);
        let (region, target) = backend.action_targets.iter().next().unwrap();
        assert_eq!(target.widget_id, "app-root");
        assert_eq!(
            target.actions,
            [SemanticActionKind::Focus, SemanticActionKind::Activate]
        );
        assert!(backend
            .validate_action_request(&SemanticActionRequest {
                request_id: 1,
                frame_id: backend.presented_frame.unwrap(),
                id: *region,
                widget_id: "app-root".into(),
                action: SemanticActionKind::Activate,
            })
            .is_ok());
    }
}
