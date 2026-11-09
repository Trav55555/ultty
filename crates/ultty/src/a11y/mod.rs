//! Accessibility: the bridge from Ultty's window to assistive technology
//! through AccessKit (ADR 0021 §3).
//!
//! [`tree`] builds the AccessKit tree as a pure function, with [`scene`]
//! for native apps' region trees and [`live`] for what their live regions
//! announce; [`adapter`] owns the platform adapter and decides when that
//! tree is sent.

pub mod adapter;
pub mod live;
pub mod scene;
pub mod tree;

pub use adapter::Adapter;
