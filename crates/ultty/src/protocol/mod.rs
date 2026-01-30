//! Protocol handling for both native and ANSI.

pub mod ansi;
pub mod connection;
pub mod native;

// Re-exports
pub use connection::Connection;
