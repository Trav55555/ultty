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

/// A virtual placement (`U=1`) used as a prototype for `U+10EEEE` Unicode
/// placeholder cells (ADR 0026 Slice 2).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VirtualPlacement {
    /// Internal id of the stored Kitty image (also its texture id).
    pub image: ImageId,
    /// The client's image id (`i=`, or the terminal-assigned id for `I=`).
    pub client_id: u32,
    /// The client's placement id (`p=`), 0 for none.
    pub placement_id: u32,
    /// Prototype width in cells.
    pub cols: u16,
    /// Prototype height in cells.
    pub rows: u16,
    /// Source rectangle in image pixels: `[x, y, width, height]`.
    pub source: [u32; 4],
    /// Drawn rectangle inside the `cols x rows` prototype, in cells:
    /// `[x, y, width, height]`.
    pub dest: [f32; 4],
    /// Stacking order relative to text (`z=`).
    pub z: i32,
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
    /// Virtual placements (`U=1`) keyed by `(image, placement_id)`.
    virtual_placements: BTreeMap<(ImageId, u32), VirtualPlacement>,
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

    /// Remove an image by internal id and any virtual placements referring to it.
    pub fn remove(&mut self, id: ImageId) -> Option<KittyImage> {
        let image = self.images.remove(&id)?;
        self.bytes -= image.rgba.len();
        if image.client_id != 0 && self.by_client.get(&image.client_id) == Some(&id) {
            self.by_client.remove(&image.client_id);
        }
        self.virtual_placements
            .retain(|&(img_id, _), _| img_id != id);
        Some(image)
    }

    /// Store or replace a virtual placement (`U=1`).
    pub fn insert_virtual_placement(&mut self, placement: VirtualPlacement) {
        self.virtual_placements
            .insert((placement.image, placement.placement_id), placement);
    }

    /// Remove a virtual placement by `(image, placement_id)`.
    pub fn remove_virtual_placement(
        &mut self,
        image: ImageId,
        placement_id: u32,
    ) -> Option<VirtualPlacement> {
        self.virtual_placements.remove(&(image, placement_id))
    }

    /// Retain only virtual placements satisfying `keep`.
    pub fn retain_virtual_placements(&mut self, mut keep: impl FnMut(&VirtualPlacement) -> bool) {
        self.virtual_placements.retain(|_, vp| keep(vp));
    }

    /// Look up a virtual placement by internal image id and placement id.
    pub fn virtual_placement(
        &self,
        image: ImageId,
        placement_id: u32,
    ) -> Option<&VirtualPlacement> {
        self.virtual_placements.get(&(image, placement_id))
    }

    /// Look up the image and virtual placement for a placeholder cell's
    /// `(client_id, placement_id)`. When `placement_id == 0`, returns the
    /// placement with id 0 if present, or any virtual placement for the image.
    pub fn virtual_placement_for(
        &self,
        client_id: u32,
        placement_id: u32,
    ) -> Option<(&KittyImage, &VirtualPlacement)> {
        let image = self.by_client(client_id)?;
        let vp = if placement_id != 0 {
            self.virtual_placements.get(&(image.id, placement_id))?
        } else {
            self.virtual_placements.get(&(image.id, 0)).or_else(|| {
                self.virtual_placements
                    .range((image.id, 0)..=(image.id, u32::MAX))
                    .next()
                    .map(|(_, vp)| vp)
            })?
        };
        Some((image, vp))
    }

    /// Whether any virtual placements exist on this screen.
    pub fn has_virtual_placements(&self) -> bool {
        !self.virtual_placements.is_empty()
    }

    /// Number of virtual placements on this screen.
    pub fn virtual_placements_len(&self) -> usize {
        self.virtual_placements.len()
    }

    /// Every virtual placement on this screen.
    pub fn virtual_placements(&self) -> impl Iterator<Item = &VirtualPlacement> {
        self.virtual_placements.values()
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

    /// Drop every image, virtual placement, and the upload in flight.
    pub fn clear(&mut self) {
        self.images.clear();
        self.by_client.clear();
        self.virtual_placements.clear();
        self.bytes = 0;
        self.upload = None;
    }

    /// Whether the byte count, client map, and virtual placements agree with the images.
    pub fn is_consistent(&self) -> bool {
        let bytes: usize = self.images.values().map(|image| image.rgba.len()).sum();
        bytes == self.bytes
            && self.by_client.iter().all(|(client, id)| {
                self.images
                    .get(id)
                    .is_some_and(|image| image.client_id == *client)
            })
            && self.virtual_placements.iter().all(|(&(img_id, pid), vp)| {
                vp.image == img_id && vp.placement_id == pid && self.images.contains_key(&img_id)
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
