//! Bounded, frame-fenced semantic action requests.

use std::collections::HashMap;
use std::io;

use ultty_protocol::{
    declared_actions, valid_action_widget_id, Display, RegionId, SemanticActionKind,
    SemanticActionRequest, TerminalMessage, CAP_SEMANTIC_ACTIONS, MAX_ACTION_ERROR_BYTES,
    REGION_WIDGET_ID_METADATA_KEY,
};

use super::NativeSession;
use crate::scene::geometry::rebuild_scroll_geometry;

/// Maximum unanswered and not-yet-collected action results per native session.
pub const MAX_PENDING_SEMANTIC_ACTIONS: usize = 128;

impl NativeSession {
    /// Request an action against the exact last committed app frame and scene generation.
    pub fn request_semantic_action(
        &mut self,
        expected_generation: u64,
        id: RegionId,
        widget_id: &str,
        action: SemanticActionKind,
    ) -> io::Result<u32> {
        if !self.is_confirmed() || !self.has_capability(CAP_SEMANTIC_ACTIONS) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "semantic actions not negotiated",
            ));
        }
        if expected_generation != self.shown_generation() || self.current_frame.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stale or uncommitted scene",
            ));
        }
        let frame_id = self.semantic_committed_frame.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "no committed semantic frame")
        })?;
        if !valid_action_widget_id(widget_id)
            || !self.semantic_target_is_valid(id, widget_id, action)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "action target is unavailable or ambiguous",
            ));
        }
        if self.semantic_pending.len() + self.semantic_results.len() >= MAX_PENDING_SEMANTIC_ACTIONS
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "semantic action queue is full",
            ));
        }
        let request_id = self
            .semantic_next_request
            .ok_or_else(|| io::Error::other("semantic action IDs exhausted"))?;
        self.semantic_next_request = request_id.checked_add(1);
        self.send_message(&TerminalMessage::InvokeAction(SemanticActionRequest {
            request_id,
            frame_id,
            id,
            widget_id: widget_id.to_owned(),
            action,
        }))?;
        self.semantic_pending.insert(request_id);
        Ok(request_id)
    }

    fn semantic_target_is_valid(
        &self,
        id: RegionId,
        widget_id: &str,
        action: SemanticActionKind,
    ) -> bool {
        if self.scene.is_in_surface(id) {
            return false;
        }
        let Some(region) = self.scene.get(id) else {
            return false;
        };
        if !declared_actions(&region.metadata).is_some_and(|(declared_id, mask)| {
            declared_id == widget_id && mask & action.mask() != 0
        }) || self
            .scene
            .iter_all()
            .filter(|candidate| {
                candidate
                    .metadata
                    .iter()
                    .any(|(key, value)| key == REGION_WIDGET_ID_METADATA_KEY && value == widget_id)
            })
            .count()
            != 1
        {
            return false;
        }
        self.is_interactive_target(id)
    }

    /// Whether a committed full-scene target and its ancestors are visible,
    /// enabled and intersect the clipped viewport.
    pub fn is_interactive_target(&self, id: RegionId) -> bool {
        if !self.is_full_scene() || self.scene.is_in_surface(id) {
            return false;
        }
        let mut node = Some(id);
        while let Some(current) = node {
            let Some(region) = self.scene.get(current) else {
                return false;
            };
            if region.semantics().hidden
                || region.semantics().disabled
                || region.constraints.display == Display::None
                || !region.style.opacity.is_finite()
                || region.style.opacity <= 0.0
            {
                return false;
            }
            node = region.parent;
        }
        if id == RegionId::ROOT {
            return self.viewport_width.is_finite()
                && self.viewport_width > 0.0
                && self.viewport_height.is_finite()
                && self.viewport_height > 0.0;
        }
        let mut adjustments = HashMap::new();
        let mut clips = HashMap::new();
        rebuild_scroll_geometry(
            &self.scene,
            RegionId::ROOT,
            |region| self.scroll_offset(region),
            1.0,
            1.0,
            (0.0, 0.0),
            Some((0.0, 0.0, self.viewport_width, self.viewport_height)),
            &mut adjustments,
            &mut clips,
        );
        clips.get(&id).is_some_and(|&(x0, y0, x1, y1)| {
            [x0, y0, x1, y1].iter().all(|value| value.is_finite()) && x1 > x0 && y1 > y0
        })
    }

    pub(super) fn accept_semantic_result(&mut self, request_id: u32, error: Option<String>) {
        if error
            .as_ref()
            .is_some_and(|message| message.len() > MAX_ACTION_ERROR_BYTES)
        {
            return;
        }
        if self.semantic_pending.remove(&request_id) {
            self.semantic_results.push_back((request_id, error));
        }
    }

    /// Cancel an unanswered request. Its ID will never be reused by this session.
    pub fn cancel_semantic_action(&mut self, request_id: u32) {
        self.semantic_pending.remove(&request_id);
        self.semantic_results.retain(|(id, _)| *id != request_id);
    }

    /// Drain only acknowledgements for requests this session issued.
    pub fn take_semantic_action_results(&mut self) -> Vec<(u32, Option<String>)> {
        self.semantic_results.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_manager::TabId;
    use ultty_protocol::{
        AppMessage, Constraints, Dimension, FrameId, Style, CAP_LAYOUT_FLEXBOX,
        CAP_REGION_METADATA, CAP_SEMANTICS, PROTOCOL_VERSION, REGION_ACTIONS_METADATA_KEY,
    };

    fn ready_session() -> NativeSession {
        let (stream, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut session = NativeSession::new(stream, 7, TabId::new(1)).unwrap();
        session.set_viewport(80.0, 24.0);
        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_LAYOUT_FLEXBOX
                    | CAP_REGION_METADATA
                    | CAP_SEMANTICS
                    | CAP_SEMANTIC_ACTIONS,
            })
            .unwrap();
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        session
            .handle_message(AppMessage::CreateRegion {
                id: RegionId::new(1),
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints {
                    width: Dimension::Cells(10.0),
                    height: Dimension::Cells(2.0),
                    ..Constraints::default()
                },
                style: Style::default(),
            })
            .unwrap();
        session
            .handle_message(AppMessage::SetRegionMetadata {
                id: RegionId::new(1),
                metadata: vec![
                    (REGION_WIDGET_ID_METADATA_KEY.into(), "increment".into()),
                    (REGION_ACTIONS_METADATA_KEY.into(), "focus,activate".into()),
                ],
            })
            .unwrap();
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        session.take_sent_messages();
        session
    }

    #[test]
    fn action_requires_committed_frame_and_correlates_exact_acknowledgement() {
        let mut session = ready_session();
        let generation = session.shown_generation();
        let id = session
            .request_semantic_action(
                generation,
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate,
            )
            .unwrap();
        assert!(
            matches!(session.take_sent_messages().as_slice(), [TerminalMessage::InvokeAction(request)] if request.frame_id == FrameId(1) && request.request_id == id)
        );
        session
            .handle_message(AppMessage::ActionResult {
                request_id: id + 1,
                error: None,
            })
            .unwrap();
        assert!(session.take_semantic_action_results().is_empty());
        session
            .handle_message(AppMessage::ActionResult {
                request_id: id,
                error: None,
            })
            .unwrap();
        session
            .handle_message(AppMessage::ActionResult {
                request_id: id,
                error: None,
            })
            .unwrap();
        assert_eq!(session.take_semantic_action_results(), vec![(id, None)]);
        assert!(session
            .request_semantic_action(
                generation + 1,
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate
            )
            .is_err());
        assert!(session
            .request_semantic_action(
                generation,
                RegionId::new(1),
                "other",
                SemanticActionKind::Activate
            )
            .is_err());
    }

    #[test]
    fn uncommitted_stale_reused_and_unframed_scenes_refuse_actions() {
        let mut session = ready_session();
        let generation = session.shown_generation();
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        assert!(session
            .request_semantic_action(
                generation,
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate
            )
            .is_err());
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        session
            .handle_message(AppMessage::SetRegionMetadata {
                id: RegionId::new(1),
                metadata: vec![],
            })
            .unwrap();
        assert!(session
            .request_semantic_action(
                session.shown_generation(),
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate
            )
            .is_err());
        assert!(!session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(2)
            })
            .unwrap());
    }

    #[test]
    fn malformed_action_declarations_are_rejected() {
        for declaration in ["activate,unknown", "activate,activate", "activate,"] {
            let mut session = ready_session();
            session
                .handle_message(AppMessage::BeginFrame {
                    frame_id: FrameId(2),
                })
                .unwrap();
            session
                .handle_message(AppMessage::SetRegionMetadata {
                    id: RegionId::new(1),
                    metadata: vec![
                        (REGION_WIDGET_ID_METADATA_KEY.into(), "increment".into()),
                        (REGION_ACTIONS_METADATA_KEY.into(), declaration.into()),
                    ],
                })
                .unwrap();
            session
                .handle_message(AppMessage::EndFrame {
                    frame_id: FrameId(2),
                })
                .unwrap();
            assert!(
                session
                    .request_semantic_action(
                        session.shown_generation(),
                        RegionId::new(1),
                        "increment",
                        SemanticActionKind::Activate
                    )
                    .is_err(),
                "accepted {declaration}"
            );
        }
    }

    #[test]
    fn hidden_and_duplicate_widget_targets_are_rejected() {
        let mut session = ready_session();
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        session
            .handle_message(AppMessage::CreateRegion {
                id: RegionId::new(2),
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints {
                    width: Dimension::Cells(10.0),
                    height: Dimension::Cells(2.0),
                    ..Constraints::default()
                },
                style: Style::default(),
            })
            .unwrap();
        session
            .handle_message(AppMessage::SetRegionMetadata {
                id: RegionId::new(2),
                metadata: vec![(REGION_WIDGET_ID_METADATA_KEY.into(), "increment".into())],
            })
            .unwrap();
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        assert!(session
            .request_semantic_action(
                session.shown_generation(),
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate
            )
            .is_err());
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(3),
            })
            .unwrap();
        session
            .handle_message(AppMessage::DeleteRegion {
                id: RegionId::new(2),
            })
            .unwrap();
        session
            .handle_message(AppMessage::SetSemantics {
                id: RegionId::new(1),
                semantics: ultty_protocol::Semantics {
                    hidden: true,
                    ..Default::default()
                },
            })
            .unwrap();
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(3),
            })
            .unwrap();
        assert!(session
            .request_semantic_action(
                session.shown_generation(),
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate
            )
            .is_err());
    }

    #[test]
    fn semantic_pending_and_uncollected_results_share_a_hard_limit() {
        let mut session = ready_session();
        let generation = session.shown_generation();
        for _ in 0..MAX_PENDING_SEMANTIC_ACTIONS {
            session
                .request_semantic_action(
                    generation,
                    RegionId::new(1),
                    "increment",
                    SemanticActionKind::Activate,
                )
                .unwrap();
        }
        assert_eq!(
            session
                .request_semantic_action(
                    generation,
                    RegionId::new(1),
                    "increment",
                    SemanticActionKind::Activate
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        session
            .handle_message(AppMessage::ActionResult {
                request_id: 1,
                error: None,
            })
            .unwrap();
        assert_eq!(
            session
                .request_semantic_action(
                    generation,
                    RegionId::new(1),
                    "increment",
                    SemanticActionKind::Activate
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(session.take_semantic_action_results(), vec![(1, None)]);
        assert!(session
            .request_semantic_action(
                generation,
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate
            )
            .is_ok());
    }

    #[test]
    fn cancellation_never_reuses_id_and_late_ack_cannot_complete_another() {
        let mut session = ready_session();
        let generation = session.shown_generation();
        let first = session
            .request_semantic_action(
                generation,
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate,
            )
            .unwrap();
        session.cancel_semantic_action(first);
        let second = session
            .request_semantic_action(
                generation,
                RegionId::new(1),
                "increment",
                SemanticActionKind::Activate,
            )
            .unwrap();
        assert_ne!(first, second);
        session
            .handle_message(AppMessage::ActionResult {
                request_id: first,
                error: None,
            })
            .unwrap();
        assert!(session.take_semantic_action_results().is_empty());
        session
            .handle_message(AppMessage::ActionResult {
                request_id: second,
                error: Some("refused".into()),
            })
            .unwrap();
        assert_eq!(
            session.take_semantic_action_results(),
            vec![(second, Some("refused".into()))]
        );
    }
}
