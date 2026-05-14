//! Native protocol error response helpers.

use tracing::warn;
use ultty_protocol::{RegionId, TerminalMessage, MIN_PROTOCOL_VERSION};

use super::NativeSession;

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

    /// Create an error for a failed reparent where either endpoint is missing.
    pub(super) fn region_or_parent_not_found(id: RegionId, parent: RegionId) -> Self {
        Self::not_found(format!("region {} or parent {} not found", id.0, parent.0))
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
