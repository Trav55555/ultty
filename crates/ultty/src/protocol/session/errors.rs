//! Native protocol error response helpers.

use tracing::warn;
use ultty_protocol::{RegionId, TerminalMessage, MIN_PROTOCOL_VERSION};

use super::NativeSession;
use crate::scene::tree::SceneTreeError;

/// Native protocol error response before wire encoding.
pub(super) struct NativeProtocolError {
    code: u16,
    message: String,
}

impl NativeProtocolError {
    /// Create a bad-request protocol error.
    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: 400,
            message: message.into(),
        }
    }

    /// Create a not-found protocol error.
    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: 404,
            message: message.into(),
        }
    }

    /// Create an unsupported-version protocol error.
    pub(super) fn unsupported_version(version: u16) -> Self {
        Self {
            code: 426,
            message: format!(
                "unsupported protocol version {} (min supported {})",
                version, MIN_PROTOCOL_VERSION
            ),
        }
    }

    /// Create an internal protocol error.
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self {
            code: 500,
            message: message.into(),
        }
    }

    /// Create an error for a command gated by a missing capability.
    pub(super) fn missing_capability(command: &'static str) -> Self {
        Self::bad_request(format!("{} not negotiated", command))
    }

    /// Create a decode recovery error for a skipped corrupted frame.
    pub(super) fn decode_failure(total_len: usize, error: impl std::fmt::Display) -> Self {
        Self::bad_request(format!(
            "decode error: skipped {} byte message: {}",
            total_len, error
        ))
    }

    /// Create an error for a missing region.
    pub(super) fn region_not_found(id: RegionId) -> Self {
        Self::not_found(format!("region {} not found", id.0))
    }

    /// Map a rejected scene-tree edit to a protocol error.
    pub(super) fn scene_edit_rejected(command: &'static str, error: SceneTreeError) -> Self {
        let message = format!("{}: {}", command, error);
        match error {
            SceneTreeError::RegionNotFound(_) | SceneTreeError::ParentNotFound(_) => {
                Self::not_found(message)
            }
            SceneTreeError::DuplicateRegion(_)
            | SceneTreeError::CannotMoveRoot
            | SceneTreeError::CyclicReparent { .. }
            | SceneTreeError::TooManyRegions { .. }
            | SceneTreeError::TooDeep { .. }
            | SceneTreeError::TooManySurfaces { .. } => Self::bad_request(message),
        }
    }

    /// Create an error for a frame that exceeded the pending-operation budget.
    pub(super) fn frame_too_large(ops: usize, bytes: usize) -> Self {
        Self::bad_request(format!(
            "frame exceeded pending limits ({} ops, {} bytes); frame dropped",
            ops, bytes
        ))
    }

    /// Create an error for a layout engine failure.
    pub(super) fn layout_failed(error: impl std::fmt::Display) -> Self {
        Self::internal(format!("layout failed: {}", error))
    }
}

impl NativeSession {
    /// Queue a native protocol error response for the client.
    pub(super) fn queue_protocol_error(&mut self, error: NativeProtocolError) {
        let NativeProtocolError { code, message } = error;
        self.protocol_errors = self.protocol_errors.wrapping_add(1);
        if self.recent_protocol_errors.len() == super::MAX_RECENT_PROTOCOL_ERRORS {
            self.recent_protocol_errors.pop_back();
        }
        self.recent_protocol_errors
            .push_front(format!("{} {}", code, message));

        if let Err(e) = self.send_message(&TerminalMessage::Error {
            code,
            message: message.clone(),
        }) {
            warn!(
                "Session {} failed to queue error message '{}': {}",
                self.session_id, message, e
            );
        }
    }
}
