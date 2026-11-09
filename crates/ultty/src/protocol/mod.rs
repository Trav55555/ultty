//! Protocol handling for both native and ANSI.

/// The VT/xterm parser, re-exported from [`ultty_vt`].
pub use ultty_vt::ansi;
pub mod pairing;
pub mod server;
pub mod session;

pub use server::ProtocolServer;
pub use session::NativeSession;
