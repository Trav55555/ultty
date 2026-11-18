//! The Kitty graphics protocol (ADR 0026, slice 1): commands, replies,
//! decoding and the per-screen image registry.
//!
//! Only direct transmission (`t=d`) is accepted. Ultty never opens, reads,
//! unlinks or maps a file or memory object named in terminal output; the
//! file, temporary-file and shared-memory media are refused with `EBADF`.

pub mod command;
pub mod decode;
pub mod reply;
pub mod store;

pub use command::{Command, Verb};
pub use reply::{Addressee, Error};
pub use store::{KittyImage, KittyImages};
