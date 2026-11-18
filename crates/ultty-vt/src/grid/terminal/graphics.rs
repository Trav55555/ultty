//! Carrying out Kitty graphics commands on the active screen (ADR 0026).

use std::sync::Arc;

use super::Terminal;
use crate::grid::graphics::command::{self, Command, Verb};
use crate::grid::graphics::decode::{self, Encoding};
use crate::grid::graphics::reply::{self, Addressee, Error};
use crate::grid::graphics::store::{KittyImage, Upload};

impl Terminal {
    /// Carry out one Kitty graphics command: the bytes of an
    /// `ESC _ G … ESC \` string after the `G`.
    ///
    /// With the screen's image quota at 0, images are off and commands are
    /// dropped without a reply, so a client's "query, then DA1" probe sees
    /// no support and falls back.
    pub fn handle_graphics(&mut self, string: &[u8]) {
        if self.buffer().image_quota() == 0 {
            return;
        }
        let (command, payload) = command::parse(string);
        if let Some(error) = command.refusal() {
            // Any other graphics command ends an upload (spec: clients must
            // finish one before sending anything else).
            self.buffer_mut().kitty_images_mut().upload = None;
            self.reply_to(addressee(&command), Err(error));
            return;
        }
        match command.verb {
            Verb::Transmit | Verb::TransmitAndPlace | Verb::Query => {
                self.transmit(command, payload);
            }
            Verb::Place | Verb::Delete => {
                self.buffer_mut().kitty_images_mut().upload = None;
            }
            Verb::Animation | Verb::Unknown => {}
        }
    }

    /// Queue the reply `outcome` to `to`, if its ids and quiet mode call
    /// for one.
    fn reply_to(&mut self, to: Addressee, outcome: Result<(), Error>) {
        if let Some(bytes) = reply::encode(to, outcome) {
            self.queue_response(bytes);
        }
    }

    /// One chunk of an upload, or a whole one-chunk upload.
    fn transmit(&mut self, command: Command, payload: &[u8]) {
        let in_flight = self.buffer_mut().kitty_images_mut().upload.take();
        let mut upload = match in_flight {
            Some(upload) if continues(&upload.start, &command) => upload,
            _ => {
                if command.keys.only(b"amq") {
                    // A continuation with nothing to continue: an upload
                    // that was refused or abandoned. kitty answers EILSEQ;
                    // with no id there is no one to answer.
                    return;
                }
                let encoding = Encoding {
                    format: command.format,
                    width: command.width,
                    height: command.height,
                    compressed: command.compression == Some(b'z'),
                    size: command.size,
                };
                if let Err(error) = decode::check_declared(&encoding) {
                    self.reply_to(addressee(&command), Err(error));
                    return;
                }
                Upload {
                    start: command.clone(),
                    encoding,
                    data: Vec::new(),
                }
            }
        };
        // The last chunk's quiet mode wins, as in kitty.
        if command.quiet != 0 {
            upload.start.quiet = command.quiet;
        }
        let to = addressee(&upload.start);
        if let Err(error) = decode::append_base64(&mut upload.data, payload, command.more) {
            self.reply_to(to, Err(error));
            return;
        }
        if command.more {
            self.buffer_mut().kitty_images_mut().upload = Some(upload);
            return;
        }
        let Upload {
            start,
            encoding,
            data,
        } = upload;
        let decoded = decode::decode(&encoding, data);
        if start.verb == Verb::Query {
            // A query stores nothing, whatever the outcome (spec).
            if start.image_id == 0 {
                return;
            }
            self.reply_to(to, decoded.map(drop));
            return;
        }
        let decoded = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                self.reply_to(to, Err(error));
                return;
            }
        };
        let mut to = to;
        let client_id = match (start.image_id, start.image_number) {
            (0, 0) => 0,
            (0, _) => {
                let id = self.buffer_mut().kitty_images_mut().free_client_id();
                // The reply tells the client the id it was given.
                to.image_id = id;
                id
            }
            (id, _) => id,
        };
        let id = self.allocate_image_id();
        let image = KittyImage {
            id,
            client_id,
            number: start.image_number,
            width: decoded.width,
            height: decoded.height,
            rgba: Arc::from(decoded.rgba),
            transient: start.hints & 1 != 0,
        };
        let buffer = self.buffer_mut();
        if let Some(replaced) = buffer.kitty_images_mut().insert(image) {
            buffer.remove_kitty_image(replaced.id);
        }
        if !buffer.enforce_image_limits(&[id]) {
            buffer.remove_kitty_image(id);
            self.reply_to(to, Err(Error::NO_SPACE));
            return;
        }
        self.reply_to(to, Ok(()));
    }
}

/// Whether `command`, sent during the upload `start` began, is its next
/// chunk. As in kitty, a transmit during an upload continues it; but one
/// that describes an image (any key besides `a`, `m`, `q` and the ids) or
/// names another image starts afresh, so an upload a crashed client left
/// unfinished does not swallow the next program's image.
fn continues(start: &Command, command: &Command) -> bool {
    command.keys.only(b"amqiIp")
        && (command.image_id == 0 || command.image_id == start.image_id)
        && (command.image_number == 0 || command.image_number == start.image_number)
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
