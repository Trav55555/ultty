//! Native protocol frame batching.

use std::io;

use tracing::{debug, info, warn};
use ultty_protocol::{AppMessage, FrameId, LayoutTrigger};

use super::NativeSession;

/// Result of frame-gate handling for one app message.
pub(super) enum FrameMessageOutcome {
    /// The frame gate handled the message and produced the session keepalive result.
    Handled(bool),
    /// The message is outside frame control and should be applied normally.
    Apply(Box<AppMessage>),
}

impl NativeSession {
    /// Handle frame control and queued messages before command application.
    pub(super) fn handle_frame_message(
        &mut self,
        msg: AppMessage,
    ) -> io::Result<FrameMessageOutcome> {
        if self.current_frame.is_some() {
            return self.handle_message_during_frame(msg);
        }

        match msg {
            AppMessage::BeginFrame { frame_id } => {
                self.handle_begin_frame(frame_id);
                Ok(FrameMessageOutcome::Handled(true))
            }
            AppMessage::EndFrame { frame_id } => {
                self.handle_end_frame(frame_id)?;
                Ok(FrameMessageOutcome::Handled(true))
            }
            other => Ok(FrameMessageOutcome::Apply(Box::new(other))),
        }
    }

    fn handle_message_during_frame(&mut self, msg: AppMessage) -> io::Result<FrameMessageOutcome> {
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
                self.queue_error_message(400, message);
                Ok(FrameMessageOutcome::Handled(true))
            }
            AppMessage::Disconnect => {
                info!("Session {} disconnected (explicit)", self.session_id);
                Ok(FrameMessageOutcome::Handled(false))
            }
            AppMessage::ClientHello { .. } => {
                self.queue_error_message(400, "client_hello not valid during frame".to_string());
                Ok(FrameMessageOutcome::Handled(true))
            }
            other => {
                self.pending_frame_ops.push(other);
                Ok(FrameMessageOutcome::Handled(true))
            }
        }
    }

    /// Handle BeginFrame.
    fn handle_begin_frame(&mut self, frame_id: FrameId) {
        self.pending_frame_ops.clear();
        self.current_frame = Some(frame_id);
        debug!("Begin frame {:?}", frame_id);
    }

    /// Handle EndFrame - compute layout and send LayoutComputed.
    fn handle_end_frame(&mut self, frame_id: FrameId) -> io::Result<()> {
        debug!("End frame {:?}", frame_id);

        let expected = self.current_frame;
        if expected != Some(frame_id) {
            let message = format!(
                "end_frame mismatch: expected {:?}, got {:?}",
                expected, frame_id
            );
            warn!(
                "EndFrame mismatch: expected {:?}, got {:?}",
                expected, frame_id
            );
            self.queue_error_message(400, message);
            return Ok(());
        }

        self.current_frame = None;
        let queued_ops = std::mem::take(&mut self.pending_frame_ops);
        for op in queued_ops {
            if !self.apply_message(op)? {
                return Ok(());
            }
        }

        if self.scene_dirty {
            if self.constraints_dirty {
                self.compute_and_send_layout(Some(frame_id), LayoutTrigger::ConstraintChange)?;
                self.constraints_dirty = false;
            }
            self.scene_dirty = false;
        }

        Ok(())
    }
}
