//! What the console draws on, in either mode.
//!
//! The transcript ([`crate::app::App`]) decides what to show; a [`Screen`]
//! decides how. It knows three things:
//!
//! - **printed lines**, finished, which go into the terminal's scrollback
//!   and are never touched again;
//! - **blocks** (a tool call, a diff, an approval), each a fixed number of
//!   rows that may change while open and are final once closed: inline
//!   cards in Ultty, a redrawn live region in ANSI mode, printed when
//!   closed;
//! - **the live lines** at the bottom (the prose line being typed and the
//!   status line), shown only once the console is idle at least once, so a
//!   replay as fast as possible draws nothing it would erase at once.

use thiserror::Error;

use crate::style::Line;

/// Identifies a block for the life of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// Drawing failed.
#[derive(Debug, Error)]
pub enum ScreenError {
    /// Writing to the terminal failed.
    #[error("writing to the terminal failed: {0}")]
    Io(#[from] std::io::Error),
    /// The inline console refused a card or lost its output.
    #[error("inline console: {0}")]
    Console(#[from] ulttui::Error),
}

/// The result of drawing.
pub type Result<T> = std::result::Result<T, ScreenError>;

/// A terminal the transcript draws on.
pub trait Screen {
    /// The terminal's width in columns.
    fn width(&self) -> u16;

    /// Print finished lines into the transcript.
    fn print(&mut self, lines: &[Line]) -> Result<()>;

    /// Open block `id`, `rows` high, showing `lines`.
    fn open(&mut self, id: BlockId, rows: u16, lines: &[Line]) -> Result<()>;

    /// Show `lines` in open block `id` instead.
    fn update(&mut self, id: BlockId, lines: &[Line]) -> Result<()>;

    /// Close block `id` with `lines` as its final text.
    fn close(&mut self, id: BlockId, lines: &[Line]) -> Result<()>;

    /// Show `lines` at the bottom, under everything else, until the next
    /// call replaces them.
    fn live(&mut self, lines: &[Line]) -> Result<()>;

    /// Take the live lines away, at the end of the session.
    fn finish(&mut self) -> Result<()>;

    /// Keys the terminal session read on the app's behalf (natively
    /// in-band, where the console reads the tty); empty elsewhere.
    fn take_input(&mut self) -> Vec<u8> {
        Vec::new()
    }
}
