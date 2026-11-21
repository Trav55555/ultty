//! An agent console for Ultty (ADR 0031): it renders a stream of agent
//! events and makes no model calls itself.
//!
//! This crate holds the event model ([`event`]), a forgiving JSON Lines
//! reader ([`reader`]), paced replay ([`replay`]) and the fixture generator
//! ([`generate`]). Rendering comes in a later slice.

pub mod event;
pub mod generate;
pub mod reader;
pub mod replay;
