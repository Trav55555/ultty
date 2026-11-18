//! The Kitty images a screen holds, and the one upload in flight.
//!
//! Images are keyed by their internal [`ImageId`], which no client sees, so
//! a Kitty client cannot collide with a sixel. The client's `i=` maps onto
//! it. Placements are not kept here: each is a [`TerminalImage`] in the
//! screen's [`ImageStore`], pinned to its text line like a sixel, so that
//! it scrolls, reflows and clears by the same rules (ADR 0026,
//! "Placements in the grid and scrollback").
//!
//! [`TerminalImage`]: crate::grid::TerminalImage
//! [`ImageStore`]: crate::grid::ImageStore

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::command::Command;
use super::decode::Encoding;
use crate::grid::ImageId;

/// A transmitted image.
#[derive(Debug, Clone)]
pub struct KittyImage {
    /// Internal id; also the id of its texture.
    pub id: ImageId,
    /// The client's `i=`, or one the terminal chose for an `I=`; 0 for an
    /// anonymous image, which lives only as long as its placements.
    pub client_id: u32,
    /// The client's `I=`, or 0.
    pub number: u32,
    /// Pixel size.
    pub width: u32,
    pub height: u32,
    /// RGBA8 pixels, shared with the placements that show them.
    pub rgba: Arc<[u8]>,
    /// The client hinted the image is short-lived (`N=1`): evicted first.
    pub transient: bool,
}

/// A chunked upload in progress (spec, "Remote client").
#[derive(Debug, Clone)]
pub struct Upload {
    /// The command that started it, whose keys describe the image.
    pub start: Command,
    /// How the data encodes the image.
    pub encoding: Encoding,
    /// The base64-decoded bytes so far.
    pub data: Vec<u8>,
}

/// One screen's Kitty images.
#[derive(Debug, Clone, Default)]
pub struct KittyImages {
    /// By internal id; ids grow, so this is oldest first.
    images: BTreeMap<ImageId, KittyImage>,
    by_client: HashMap<u32, ImageId>,
    /// Sum of the images' RGBA bytes.
    bytes: usize,
    /// Where the search for a free client id starts.
    next_client_id: u32,
    /// The upload in flight, if any.
    pub upload: Option<Upload>,
}

impl KittyImages {
    /// Store `image`, replacing the image with the same client id. Returns
    /// the replaced image, whose placements must go too (spec: "the
    /// existing image and all its placements must be deleted").
    pub fn insert(&mut self, image: KittyImage) -> Option<KittyImage> {
        let mut replaced = None;
        if image.client_id != 0 {
            if let Some(old) = self.by_client.get(&image.client_id).copied() {
                replaced = self.remove(old);
            }
            self.by_client.insert(image.client_id, image.id);
        }
        self.bytes += image.rgba.len();
        self.images.insert(image.id, image);
        replaced
    }

    /// Remove an image by internal id.
    pub fn remove(&mut self, id: ImageId) -> Option<KittyImage> {
        let image = self.images.remove(&id)?;
        self.bytes -= image.rgba.len();
        if image.client_id != 0 && self.by_client.get(&image.client_id) == Some(&id) {
            self.by_client.remove(&image.client_id);
        }
        Some(image)
    }

    /// An image by internal id.
    pub fn get(&self, id: ImageId) -> Option<&KittyImage> {
        self.images.get(&id)
    }

    /// The image with client id `client`.
    pub fn by_client(&self, client: u32) -> Option<&KittyImage> {
        self.images.get(self.by_client.get(&client)?)
    }

    /// The newest image with number `number` (spec, "Requesting image ids
    /// from the terminal").
    pub fn newest_numbered(&self, number: u32) -> Option<&KittyImage> {
        self.images
            .values()
            .rev()
            .find(|image| image.number == number)
    }

    /// A client id no image uses, for an image sent with only `I=`.
    pub fn free_client_id(&mut self) -> u32 {
        let mut id = self.next_client_id.max(1);
        while self.by_client.contains_key(&id) {
            id = id.wrapping_add(1).max(1);
        }
        self.next_client_id = id.wrapping_add(1);
        id
    }

    /// Every image, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &KittyImage> {
        self.images.values()
    }

    /// Number of images.
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Whether there are no images.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Total RGBA bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Bytes of the upload in flight, if any.
    pub fn upload_bytes(&self) -> usize {
        self.upload.as_ref().map_or(0, |upload| upload.data.len())
    }

    /// Drop every image and the upload in flight.
    pub fn clear(&mut self) {
        self.images.clear();
        self.by_client.clear();
        self.bytes = 0;
        self.upload = None;
    }

    /// Whether the byte count and the client map agree with the images.
    pub fn is_consistent(&self) -> bool {
        let bytes: usize = self.images.values().map(|image| image.rgba.len()).sum();
        bytes == self.bytes
            && self.by_client.iter().all(|(client, id)| {
                self.images
                    .get(id)
                    .is_some_and(|image| image.client_id == *client)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(id: u32, client_id: u32, number: u32) -> KittyImage {
        KittyImage {
            id: ImageId(id),
            client_id,
            number,
            width: 1,
            height: 1,
            rgba: Arc::from(vec![0u8; 4]),
            transient: false,
        }
    }

    #[test]
    fn a_client_id_names_the_latest_image_sent_with_it() {
        let mut images = KittyImages::default();
        assert!(images.insert(image(1, 7, 0)).is_none());
        let replaced = images.insert(image(2, 7, 0)).expect("replaced");
        assert_eq!(replaced.id, ImageId(1));
        assert_eq!(images.by_client(7).unwrap().id, ImageId(2));
        assert_eq!((images.len(), images.bytes()), (1, 4));
        assert!(images.is_consistent());
    }

    #[test]
    fn numbers_find_the_newest_image() {
        let mut images = KittyImages::default();
        images.insert(image(1, 10, 3));
        images.insert(image(2, 11, 3));
        assert_eq!(images.newest_numbered(3).unwrap().id, ImageId(2));
    }

    #[test]
    fn free_client_ids_skip_used_ones() {
        let mut images = KittyImages::default();
        images.insert(image(1, 1, 0));
        images.insert(image(2, 2, 0));
        assert_eq!(images.free_client_id(), 3);
        assert_eq!(images.free_client_id(), 4);
    }

    #[test]
    fn anonymous_images_are_not_mapped() {
        let mut images = KittyImages::default();
        images.insert(image(1, 0, 0));
        images.insert(image(2, 0, 0));
        assert_eq!(images.len(), 2);
        images.remove(ImageId(1));
        assert_eq!((images.len(), images.bytes()), (1, 4));
        assert!(images.is_consistent());
    }
}
