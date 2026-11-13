//! Native protocol frame batching.

use std::io;

use tracing::{debug, info, warn};
use ultty_protocol::{AppMessage, FrameId};

use super::errors::NativeProtocolError;
use super::NativeSession;

/// Maximum number of operations a single frame may queue before `EndFrame`.
pub(super) const MAX_PENDING_FRAME_OPS: usize = 10_000;

/// Maximum wire bytes a single frame may queue before `EndFrame` (32 MiB).
pub(super) const MAX_PENDING_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// Result of frame-gate handling for one app message.
pub(super) enum FrameMessageOutcome {
    /// The frame gate handled the message and produced the session keepalive result.
    Handled(bool),
    /// The message is outside frame control and should be applied normally.
    Apply(Box<AppMessage>),
}

impl NativeSession {
    /// Handle frame control and queued messages before command application.
    ///
    /// `wire_len` is the number of socket bytes the message occupied; it is
    /// charged against the pending-frame byte budget while a frame is open.
    pub(super) fn handle_frame_message(
        &mut self,
        msg: AppMessage,
        wire_len: usize,
    ) -> io::Result<FrameMessageOutcome> {
        if self.current_frame.is_some() {
            return self.handle_message_during_frame(msg, wire_len);
        }

        match msg {
            AppMessage::BeginFrame { frame_id } => {
                let accepted = self.handle_begin_frame(frame_id);
                Ok(FrameMessageOutcome::Handled(accepted))
            }
            AppMessage::EndFrame { frame_id } => {
                self.handle_end_frame(frame_id)?;
                Ok(FrameMessageOutcome::Handled(true))
            }
            other => {
                if self.has_capability(ultty_protocol::CAP_SEMANTIC_ACTIONS)
                    && matches!(
                        other,
                        AppMessage::CreateRegion { .. }
                            | AppMessage::SetConstraints { .. }
                            | AppMessage::SetStyle { .. }
                            | AppMessage::SetContent { .. }
                            | AppMessage::AppendContent { .. }
                            | AppMessage::StreamStart { .. }
                            | AppMessage::StreamEnd { .. }
                            | AppMessage::SetRegionMetadata { .. }
                            | AppMessage::SetSemantics { .. }
                            | AppMessage::DeleteRegion { .. }
                            | AppMessage::CreateSurface { .. }
                            | AppMessage::CloseSurface { .. }
                            | AppMessage::ReparentRegion { .. }
                    )
                {
                    // A scene mutation without a frame cannot be correlated
                    // with the app's committed action registry.
                    self.semantic_committed_frame = None;
                }
                Ok(FrameMessageOutcome::Apply(Box::new(other)))
            }
        }
    }

    fn handle_message_during_frame(
        &mut self,
        msg: AppMessage,
        wire_len: usize,
    ) -> io::Result<FrameMessageOutcome> {
        match msg {
            AppMessage::EndFrame { frame_id } => {
                self.handle_end_frame(frame_id)?;
                Ok(FrameMessageOutcome::Handled(true))
            }
            AppMessage::BeginFrame { frame_id } => {
                let message = format!(
                    "begin_frame while frame {:?} in progress (got {:?})",
                    self.current_frame, frame_id
                );
                warn!("Session {} {}", self.session_id, message);
                self.queue_protocol_error(NativeProtocolError::bad_request(message));
                Ok(FrameMessageOutcome::Handled(true))
            }
            AppMessage::Disconnect => {
                info!("Session {} disconnected (explicit)", self.session_id);
                Ok(FrameMessageOutcome::Handled(false))
            }
            AppMessage::ClientHello { .. } => {
                self.queue_protocol_error(NativeProtocolError::bad_request(
                    "client_hello not valid during frame",
                ));
                Ok(FrameMessageOutcome::Handled(true))
            }
            other => {
                let ops = self.pending_frame_ops.len() + 1;
                let bytes = self.pending_frame_bytes.saturating_add(wire_len);
                if ops > MAX_PENDING_FRAME_OPS || bytes > MAX_PENDING_FRAME_BYTES {
                    warn!(
                        "Session {} frame {:?} exceeded pending limits ({} ops, {} bytes), disconnecting",
                        self.session_id, self.current_frame, ops, bytes
                    );
                    self.queue_protocol_error(NativeProtocolError::frame_too_large(ops, bytes));
                    self.discard_pending_frame();
                    return Ok(FrameMessageOutcome::Handled(false));
                }
                self.pending_frame_ops.push(other);
                self.pending_frame_bytes = bytes;
                Ok(FrameMessageOutcome::Handled(true))
            }
        }
    }

    /// Handle BeginFrame.
    fn handle_begin_frame(&mut self, frame_id: FrameId) -> bool {
        if self.has_capability(ultty_protocol::CAP_SEMANTIC_ACTIONS) {
            if frame_id.0 == 0
                || self
                    .semantic_last_frame
                    .is_some_and(|last| frame_id.0 <= last.0)
            {
                self.queue_protocol_error(NativeProtocolError::bad_request(
                    "semantic frame IDs must strictly increase",
                ));
                self.semantic_committed_frame = None;
                return false;
            }
            self.semantic_last_frame = Some(frame_id);
        }
        self.discard_pending_frame();
        self.current_frame = Some(frame_id);
        debug!("Begin frame {:?}", frame_id);
        true
    }

    /// Drop the open frame and everything queued for it.
    fn discard_pending_frame(&mut self) {
        self.current_frame = None;
        self.pending_frame_ops.clear();
        self.pending_frame_bytes = 0;
    }

    /// Handle EndFrame - apply queued operations, compute layout and send LayoutComputed.
    fn handle_end_frame(&mut self, frame_id: FrameId) -> io::Result<()> {
        debug!("End frame {:?}", frame_id);

        let expected = self.current_frame;
        if expected != Some(frame_id) {
            let message = format!(
                "end_frame mismatch: expected {:?}, got {:?}; pending frame discarded",
                expected, frame_id
            );
            warn!("Session {} {}", self.session_id, message);
            self.queue_protocol_error(NativeProtocolError::bad_request(message));
            // Never apply a frame whose bracket does not match: reset so the
            // next BeginFrame starts clean instead of wedging the session.
            self.discard_pending_frame();
            self.semantic_committed_frame = None;
            return Ok(());
        }

        let errors_before = self.protocol_errors;
        self.current_frame = None;
        self.pending_frame_bytes = 0;
        let queued_ops = std::mem::take(&mut self.pending_frame_ops);
        for op in queued_ops {
            if !self.apply_message(op)? {
                return Ok(());
            }
        }

        // Every frame that changed the scene gets a LayoutComputed reply, so a
        // client that waits on it after EndFrame never blocks on a
        // content-only frame.
        if self.scene_dirty {
            let trigger = self.layout_trigger_for_scene_change();
            self.compute_and_send_layout(Some(frame_id), trigger)?;
            self.mark_scene_layout_clean();
        }

        if self.has_capability(ultty_protocol::CAP_SEMANTIC_ACTIONS) {
            self.semantic_committed_frame =
                (self.protocol_errors == errors_before).then_some(frame_id);
        }
        Ok(())
    }
}
