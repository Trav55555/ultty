//! Carrying out Kitty graphics commands on the active screen (ADR 0026).

use super::Terminal;
use crate::grid::graphics::{command, reply, Addressee, Command, Error};

impl Terminal {
    /// Carry out one Kitty graphics command: the bytes of an
    /// `ESC _ G … ESC \` string after the `G`.
    pub fn handle_graphics(&mut self, string: &[u8]) {
        let (command, _payload) = command::parse(string);
        if let Some(error) = command.refusal() {
            self.reply_to(&command, Err(error));
        }
    }

    /// Queue the reply to `command`, if its ids and quiet mode call for one.
    fn reply_to(&mut self, command: &Command, outcome: Result<(), Error>) {
        if let Some(bytes) = reply::encode(addressee(command), outcome) {
            self.queue_response(bytes);
        }
    }
}

/// The ids a reply to `command` echoes.
fn addressee(command: &Command) -> Addressee {
    Addressee {
        image_id: command.image_id,
        image_number: command.image_number,
        placement_id: command.placement_id,
        quiet: command.quiet,
    }
}
