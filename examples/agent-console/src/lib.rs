//! An agent console for Ultty (ADR 0031): it renders a stream of agent
//! events and makes no model calls itself.
//!
//! This crate holds the event model ([`event`]), a forgiving JSON Lines
//! reader ([`reader`]), paced replay ([`replay`]) and the fixture generator
//! ([`generate`]); and the console itself: the transcript ([`app`]), drawn
//! on a [`screen::Screen`] that is inline cards in Ultty ([`native`]) or a
//! redrawn live region anywhere else ([`ansi`]), from styled lines
//! ([`style`], [`markdown`]).

pub mod ansi;
pub mod app;
pub mod event;
pub mod generate;
pub mod keys;
pub mod markdown;
pub mod native;
pub mod reader;
pub mod replay;
pub mod screen;
pub mod style;
