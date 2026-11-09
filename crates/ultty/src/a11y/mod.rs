//! Accessibility: the bridge from Ultty's window to assistive technology
//! through AccessKit (ADR 0021 §3).
//!
//! [`tree`] builds the AccessKit tree as a pure function; [`adapter`] owns
//! the platform adapter and decides when that tree is sent.

pub mod adapter;
pub mod tree;

pub use adapter::Adapter;
