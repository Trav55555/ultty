//! Protocol error types.

use thiserror::Error;

/// Protocol errors.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Invalid magic bytes in frame header.
    #[error("invalid magic bytes: expected 0x5554, got {0:#06x}")]
    InvalidMagic(u16),

    /// Unknown message type.
    #[error("unknown message type: {0:#06x}")]
    UnknownMessageType(u16),

    /// Message too large.
    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge { size: usize, max: usize },

    /// Unexpected end of input.
    #[error("unexpected end of input: need {needed} bytes, have {have}")]
    UnexpectedEof { needed: usize, have: usize },

    /// Invalid UTF-8 in string field.
    #[error("invalid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    /// Invalid enum variant.
    #[error("invalid {enum_name} variant: {value}")]
    InvalidVariant { enum_name: &'static str, value: u8 },

    /// Region not found.
    #[error("region not found: {0:?}")]
    RegionNotFound(crate::RegionId),

    /// Version mismatch.
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u16, got: u16 },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for protocol operations.
pub type Result<T> = std::result::Result<T, ProtocolError>;
