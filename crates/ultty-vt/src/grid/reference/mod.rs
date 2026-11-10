//! A frozen copy of the grid storage, for the differential oracle (ADR 0022,
//! migration slice 2).
//!
//! ADR 0022 replaces the storage under `Grid`, `Row`, `Buffer` and
//! `CompactRow` without changing what a terminal does. This module keeps the
//! storage as it was before that work started (`grid.rs`, `row.rs`,
//! `buffer.rs`, `history.rs` are copies, test modules dropped), and
//! [`oracle`] drives it and the live storage with the same streams and
//! compares them after every step.
//!
//! Only storage is copied. `terminal.rs` and `term_handler.rs` are compiled a
//! second time inside this module with `#[path]`: they name storage through
//! `super::`, which here resolves to the copies, so both terminals run the
//! same handler and terminal code over different storage. Their unit tests
//! run against the copies too. Value types that are not storage (`Cell`,
//! `Dimensions`, `CursorPosition`, `CursorShape`, the palette, images, the
//! cluster tables) are the live ones.
//!
//! When a slice changes an API that `terminal.rs` or `term_handler.rs` uses,
//! give the copy the same API over its old storage; never change what the
//! copy stores or does. The module goes away in slice 6.

#![allow(
    dead_code,
    reason = "the copies keep the whole storage API; the oracle uses part of it"
)]

mod buffer;
mod grid;
mod history;
mod row;
// Compiled a second time on purpose: see the module docs.
#[allow(
    clippy::duplicate_mod,
    reason = "the same code over the frozen storage"
)]
#[path = "../term_handler.rs"]
mod term_handler;
#[allow(
    clippy::duplicate_mod,
    reason = "the same code over the frozen storage"
)]
#[path = "../terminal.rs"]
mod terminal;

// Paths `terminal.rs`, `term_handler.rs` and the copies reach through `super::`.
use super::{cell, cluster, image};
pub use super::{
    decode_sixel, format_color_spec, Cell, CellFlags, CursorPosition, CursorShape, Dimensions,
    ImageStore, Palette, TerminalImage, ThemeColors, MAX_COMBINING_PER_CELL,
};
pub use buffer::{Anchor, AnchorRefusal, Buffer};
pub use grid::{Cursor, Grid};
pub use row::{AnchorTag, Row, RowMarks};
pub use term_handler::TerminalHandler;
pub use terminal::{
    MouseButton, MouseEncoding, MouseMode, MouseModifiers, SelectionMode, SelectionPoint, Terminal,
};
