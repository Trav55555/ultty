//! Local authenticated automation and reusable runtime control semantics.

pub mod auth;
pub mod common;
pub mod events;
pub mod mcp;
pub mod paths;
mod server;
pub mod wait;

pub use server::{format_response, ControlServer};
