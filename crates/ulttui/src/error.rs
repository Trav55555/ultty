//! The error type for ulttui's public API.

/// What can go wrong running an app or drawing its view.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing the terminal or the native protocol socket failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A native protocol message could not be encoded or decoded.
    #[error(transparent)]
    Protocol(#[from] ultty_protocol::ProtocolError),
    /// The in-band probe of the terminal failed.
    #[error(transparent)]
    Client(#[from] ultty_client::Error),
    /// The terminal sent bytes that are not a protocol frame, so the stream
    /// cannot be followed any further.
    #[error("invalid frame magic: {0:#06x}")]
    BadFrameMagic(u16),
    /// Native mode needs a socket or in-band link and has neither.
    #[error("not connected to the terminal")]
    NotConnected,
    /// The backend was used before `init`.
    #[error("backend not initialized")]
    NotInitialized,
    /// Native mode is not available here: no `ULTTY_SOCKET`, or a terminal
    /// that does not answer the in-band probe.
    #[error("native mode unavailable: {0}")]
    NativeUnavailable(&'static str),
    /// The terminal closed, refused, or never finished the native handshake.
    #[error("native handshake failed: {0}")]
    Handshake(String),
    /// An inline card was asked for with a height out of range
    /// (see [`crate::inline::Console::open`]).
    #[error("invalid inline card: {0}")]
    InvalidCard(String),
    /// An error injected into a [`crate::backend::MockBackend`] by a test.
    #[error("{0}")]
    Mock(String),
}

/// `Result` with ulttui's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Apps keep using `anyhow` or `Box<dyn Error>` in `main`; `?` needs
    /// these bounds to convert.
    #[test]
    fn error_converts_into_boxed_errors() {
        fn assert_bounds<E: std::error::Error + Send + Sync + 'static>() {}
        assert_bounds::<Error>();

        let io = Error::from(std::io::Error::other("gone"));
        let boxed: Box<dyn std::error::Error + Send + Sync> = io.into();
        assert_eq!(boxed.to_string(), "gone");
    }
}
