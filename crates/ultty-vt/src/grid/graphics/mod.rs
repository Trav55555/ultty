//! The Kitty graphics protocol (ADR 0026, slices 1 and 2): commands, replies,
//! decoding, the per-screen image registry, and Unicode placeholders.
//!
//! Only direct transmission (`t=d`) is accepted. Ultty never opens, reads,
//! unlinks or maps a file or memory object named in terminal output; the
//! file, temporary-file and shared-memory media are refused with `EBADF`.

pub mod command;
pub mod decode;
pub mod placeholder;
pub mod reply;
pub mod store;

pub use command::{Command, Verb};
pub use placeholder::{
    diacritic_index, index_to_diacritic, resolve_visible_placeholders, ResolvedPlaceholder,
    DIACRITICS, PLACEHOLDER_CHAR,
};
pub use reply::{Addressee, Error};
pub use store::{KittyImage, KittyImages, VirtualPlacement};
