//! Carrying out Kitty graphics commands on the active screen (ADR 0026).

use std::collections::HashSet;
use std::sync::Arc;

use super::Terminal;
use crate::grid::graphics::command::{self, Command, Verb};
use crate::grid::graphics::decode::{self, Encoding};
use crate::grid::graphics::reply::{self, Addressee, Error};
use crate::grid::graphics::store::{KittyImage, Upload, VirtualPlacement};
use crate::grid::{ImageId, KittyPlacement, TerminalImage};

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
            Verb::Place => {
                self.buffer_mut().kitty_images_mut().upload = None;
                self.place_sent(&command);
            }
            Verb::Delete => self.delete(&command),
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
        // The image shows from the cursor where the last chunk arrived
        // (spec, "Remote client").
        let outcome = if start.verb == Verb::TransmitAndPlace {
            self.place(id, &start)
        } else {
            Ok(())
        };
        self.reply_to(to, outcome);
    }

    /// `a=p`: place an image sent earlier, named by `i` or `I`.
    fn place_sent(&mut self, command: &Command) {
        let images = self.buffer().kitty_images();
        let found = match (command.image_id, command.image_number) {
            // kitty only logs a put with neither; there is no one to reply to.
            (0, 0) => return,
            (0, number) => images.newest_numbered(number),
            (id, _) => images.by_client(id),
        };
        let mut to = addressee(command);
        let outcome = match found.map(|image| (image.id, image.client_id)) {
            Some((id, client_id)) => {
                to.image_id = client_id;
                self.place(id, command)
            }
            None => Err(Error::NOT_FOUND),
        };
        self.reply_to(to, outcome);
    }

    /// Place stored image `image` at the cursor as `command` says, and move
    /// the cursor past it unless `C=1`.
    fn place(&mut self, image: ImageId, command: &Command) -> Result<(), Error> {
        let stored = self
            .buffer()
            .kitty_images()
            .get(image)
            .ok_or(Error::NOT_FOUND)?;
        let (width, height, client_id) = (stored.width, stored.height, stored.client_id);
        let rgba = Arc::clone(&stored.rgba);
        let (cell_width, cell_height) = self
            .cell_pixel_size()
            .filter(|&(w, h)| w > 0 && h > 0)
            .map_or((8, 16), |(w, h)| (u32::from(w), u32::from(h)));
        let layout = Layout::new(command, width, height, cell_width, cell_height)
            .ok_or(Error::EMPTY_SOURCE)?;
        // An image without an id cannot be named again, so its placements
        // get no id either (spec).
        let placement_id = if client_id == 0 {
            0
        } else {
            command.placement_id
        };
        if command.virtual_placement {
            if client_id == 0 {
                return Ok(());
            }
            let buffer = self.buffer_mut();
            let previous = buffer
                .kitty_images_mut()
                .remove_virtual_placement(image, placement_id);
            buffer
                .kitty_images_mut()
                .insert_virtual_placement(VirtualPlacement {
                    image,
                    client_id,
                    placement_id,
                    cols: layout.cols,
                    rows: layout.rows,
                    source: layout.source,
                    dest: layout.dest,
                    z: command.z,
                });
            if !buffer.enforce_image_limits(&[image]) {
                buffer
                    .kitty_images_mut()
                    .remove_virtual_placement(image, placement_id);
                if let Some(prev) = previous {
                    buffer.kitty_images_mut().insert_virtual_placement(prev);
                }
                return Err(Error::NO_SPACE);
            }
            return Ok(());
        }
        let cursor = self.grid().cursor().position;
        let line = self.buffer().line_of_screen_row(cursor.row);
        let id = self.allocate_image_id();
        let buffer = self.buffer_mut();
        if placement_id != 0 {
            // The same image and placement id replaces the old placement.
            buffer.images_mut().retain_mut(|placed| {
                placed
                    .kitty
                    .as_ref()
                    .is_none_or(|kitty| kitty.image != image || kitty.placement_id != placement_id)
            });
        }
        buffer.images_mut().add(TerminalImage {
            id,
            width,
            height,
            data: rgba,
            start_col: cursor.col,
            line,
            cell_width: layout.cols,
            cell_height: layout.rows,
            z: command.z,
            kitty: Some(KittyPlacement {
                image,
                client_id,
                placement_id,
                source: layout.source,
                dest: layout.dest,
            }),
        });
        if !buffer.enforce_image_limits(&[id, image]) {
            buffer.images_mut().remove(id);
            return Err(Error::NO_SPACE);
        }
        if command.cursor_policy != 1 {
            self.move_cursor_past_image(layout.cols, layout.rows);
        }
        Ok(())
    }

    /// Move the cursor right by `cols` and down by `rows - 1`, as kitty
    /// does after a placement: past the last column it goes to the start
    /// of the next line, and past the bottom margin the screen scrolls.
    fn move_cursor_past_image(&mut self, cols: u16, rows: u16) {
        let start_col = self.grid().cursor().position.col;
        for _ in 1..rows {
            self.buffer_mut().newline();
        }
        let dims = self.dimensions();
        let col = u32::from(start_col) + u32::from(cols);
        if col >= u32::from(dims.cols) {
            self.buffer_mut().newline();
            self.grid_mut().cursor_mut().position.col = 0;
        } else {
            self.grid_mut().cursor_mut().position.col = col as u16;
        }
    }

    /// `a=d`: delete placements, and with an uppercase `d` the images left
    /// without one. Kitty deletes leave sixel images alone. Any delete
    /// ends an upload in flight (spec).
    fn delete(&mut self, command: &Command) {
        let rows = self.dimensions().rows;
        let cursor = self.grid().cursor().position;
        let buffer = self.buffer_mut();
        buffer.kitty_images_mut().upload = None;
        let what = command.delete.to_ascii_lowercase();
        let free = command.delete.is_ascii_uppercase();
        let named = match what {
            b'i' if command.image_id != 0 => buffer.kitty_images().by_client(command.image_id),
            b'n' if command.image_number != 0 => {
                buffer.kitty_images().newest_numbered(command.image_number)
            }
            _ => None,
        }
        .map(|image| image.id);
        let in_range = |client: u32| client != 0 && command.x <= client && client <= command.y;
        let top = buffer.screen_top_line();
        let hit = |placed: &TerminalImage, kitty: &KittyPlacement| {
            let on_row = |row: u32| {
                let line = top + row as usize;
                placed.line <= line && line < placed.line + usize::from(placed.cell_height)
            };
            let on_col = |col: u32| {
                u32::from(placed.start_col) <= col
                    && col < u32::from(placed.start_col) + u32::from(placed.cell_width)
            };
            // Cells in delete commands count from 1.
            let at = |x: u32, y: u32| x >= 1 && y >= 1 && on_col(x - 1) && on_row(y - 1);
            match what {
                b'a' => {
                    placed.line + usize::from(placed.cell_height) > top
                        && placed.line < top + usize::from(rows)
                }
                b'i' | b'n' => {
                    named == Some(kitty.image)
                        && (command.placement_id == 0 || kitty.placement_id == command.placement_id)
                }
                b'c' => on_col(u32::from(cursor.col)) && on_row(u32::from(cursor.row)),
                b'p' => at(command.x, command.y),
                b'q' => at(command.x, command.y) && placed.z == command.z,
                b'r' => in_range(kitty.client_id),
                b'x' => command.x >= 1 && on_col(command.x - 1),
                b'y' => command.y >= 1 && on_row(command.y - 1),
                b'z' => placed.z == command.z,
                // `f`/`F` deletes animation frames; there are none.
                _ => false,
            }
        };
        let mut touched: HashSet<ImageId> = HashSet::new();
        buffer
            .images_mut()
            .retain_mut(|placed| match &placed.kitty {
                Some(kitty) if hit(placed, kitty) => {
                    touched.insert(kitty.image);
                    false
                }
                _ => true,
            });
        if matches!(what, b'i' | b'n' | b'r') {
            buffer.kitty_images_mut().retain_virtual_placements(|vp| {
                let delete = match what {
                    b'i' | b'n' => {
                        named == Some(vp.image)
                            && (command.placement_id == 0
                                || vp.placement_id == command.placement_id)
                    }
                    b'r' => in_range(vp.client_id),
                    _ => false,
                };
                if delete {
                    touched.insert(vp.image);
                }
                !delete
            });
        }
        if free {
            // Images named by id or number go even if never placed, as do
            // those in an id range (kitty's `handle_delete_command`).
            touched.extend(named);
            if what == b'r' {
                touched.extend(
                    buffer
                        .kitty_images()
                        .iter()
                        .filter(|image| in_range(image.client_id))
                        .map(|image| image.id),
                );
            }
            let placed: HashSet<ImageId> = buffer
                .images()
                .images()
                .filter_map(|placed| placed.kitty.as_ref().map(|kitty| kitty.image))
                .chain(
                    buffer
                        .kitty_images()
                        .virtual_placements()
                        .map(|vp| vp.image),
                )
                .collect();
            for id in touched.difference(&placed) {
                buffer.kitty_images_mut().remove(*id);
            }
        }
        buffer.prune_orphan_kitty_images(&[]);
    }
}

/// Where a placement shows: its span in cells, the part of the image it
/// shows and where inside the span it is drawn.
#[derive(Debug, PartialEq)]
struct Layout {
    cols: u16,
    rows: u16,
    source: [u32; 4],
    dest: [f32; 4],
}

impl Layout {
    /// Lay out `command`'s placement of a `width` x `height` image with
    /// cells of `cell_width` x `cell_height` pixels (kitty's
    /// `update_dest_rect`). Without `c` and `r` the image keeps its pixel
    /// size; with one, the other follows the aspect ratio; with both, it is
    /// scaled to fit and centred (letterboxed). `None` when the source
    /// rectangle holds no pixel.
    fn new(
        command: &Command,
        width: u32,
        height: u32,
        cell_width: u32,
        cell_height: u32,
    ) -> Option<Self> {
        let x = command.x.min(width);
        let y = command.y.min(height);
        let w = match command.w {
            0 => width,
            w => w,
        }
        .min(width - x);
        let h = match command.h {
            0 => height,
            h => h,
        }
        .min(height - y);
        if w == 0 || h == 0 {
            return None;
        }
        let (cw, ch) = (f64::from(cell_width), f64::from(cell_height));
        let off_x = f64::from(command.cell_x.min(cell_width - 1));
        let off_y = f64::from(command.cell_y.min(cell_height - 1));
        let (sw, sh) = (f64::from(w), f64::from(h));
        let cells = |pixels: f64, cell: f64| (pixels / cell).ceil().clamp(1.0, f64::from(u16::MAX));
        let (cols, rows, dx, dy, dw, dh) = match (command.cols, command.rows) {
            (0, 0) => (
                cells(sw + off_x, cw),
                cells(sh + off_y, ch),
                off_x,
                off_y,
                sw,
                sh,
            ),
            (0, r) => {
                let dh = f64::from(r) * ch - off_y;
                let dw = dh * sw / sh;
                (cells(dw + off_x, cw), f64::from(r), off_x, off_y, dw, dh)
            }
            (c, 0) => {
                let dw = f64::from(c) * cw - off_x;
                let dh = dw * sh / sw;
                (f64::from(c), cells(dh + off_y, ch), off_x, off_y, dw, dh)
            }
            (c, r) => {
                let (box_w, box_h) = (f64::from(c) * cw - off_x, f64::from(r) * ch - off_y);
                let scale = (box_w / sw).min(box_h / sh);
                let (dw, dh) = (sw * scale, sh * scale);
                let (dx, dy) = (off_x + (box_w - dw) / 2.0, off_y + (box_h - dh) / 2.0);
                (f64::from(c), f64::from(r), dx, dy, dw, dh)
            }
        };
        let (cols, rows) = (cols.min(f64::from(u16::MAX)), rows.min(f64::from(u16::MAX)));
        Some(Self {
            cols: cols as u16,
            rows: rows as u16,
            source: [x, y, w, h],
            dest: [
                (dx / cw) as f32,
                (dy / ch) as f32,
                ((dw / cw) as f32).min(cols as f32),
                ((dh / ch) as f32).min(rows as f32),
            ],
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(control: &[u8], width: u32, height: u32) -> Layout {
        let (command, _) = command::parse(control);
        Layout::new(&command, width, height, 10, 20).unwrap()
    }

    #[test]
    fn an_image_keeps_its_pixel_size_by_default() {
        let got = layout(b"", 25, 30);
        assert_eq!((got.cols, got.rows), (3, 2));
        assert_eq!(got.source, [0, 0, 25, 30]);
        assert_eq!(got.dest, [0.0, 0.0, 2.5, 1.5]);
    }

    #[test]
    fn offsets_inside_the_first_cell_widen_the_span() {
        let got = layout(b"X=9,Y=15", 10, 20);
        assert_eq!((got.cols, got.rows), (2, 2));
        assert_eq!(got.dest, [0.9, 0.75, 1.0, 1.0]);
    }

    #[test]
    fn one_given_dimension_keeps_the_aspect_ratio() {
        let got = layout(b"c=4", 20, 10);
        assert_eq!((got.cols, got.rows), (4, 1));
        assert_eq!(got.dest, [0.0, 0.0, 4.0, 1.0]);
        let got = layout(b"r=2", 20, 10);
        assert_eq!((got.cols, got.rows), (8, 2));
    }

    #[test]
    fn both_dimensions_letterbox() {
        // A square image in a 4x1 box (40x20 px) is 20x20, centred.
        let got = layout(b"c=4,r=1", 10, 10);
        assert_eq!((got.cols, got.rows), (4, 1));
        assert_eq!(got.dest, [1.0, 0.0, 2.0, 1.0]);
    }

    #[test]
    fn the_source_rectangle_is_cut_to_the_image() {
        let got = layout(b"x=5,y=5,w=100,h=3", 10, 10);
        assert_eq!(got.source, [5, 5, 5, 3]);
        let (command, _) = command::parse(b"x=10");
        assert_eq!(Layout::new(&command, 10, 10, 10, 20), None);
    }
}
