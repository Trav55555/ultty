//! Image storage and Sixel decoding for terminal inline images.

use std::collections::{HashMap, VecDeque};

/// Maximum width or height in pixels of a decoded Sixel image.
pub const MAX_SIXEL_DIM: u32 = 16384;

/// Maximum number of pixels in a decoded Sixel image (64 MiB of RGBA).
pub const MAX_SIXEL_PIXELS: u64 = 16 * 1024 * 1024;

/// Maximum total RGBA bytes held by an [`ImageStore`] before old images are evicted.
pub const MAX_IMAGE_STORE_BYTES: usize = 256 * 1024 * 1024;

/// Maximum number of images held by an [`ImageStore`].
pub const MAX_IMAGE_STORE_COUNT: usize = 1024;

/// Unique identifier for terminal images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ImageId(pub u32);

/// A decoded terminal image.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalImage {
    /// Unique identifier
    pub id: ImageId,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// RGBA pixel data (row-major, 4 bytes per pixel)
    pub data: Vec<u8>,
    /// Position where the image starts (column)
    pub start_col: u16,
    /// The text line the image's top row sits on, numbered as
    /// `evicted + absolute index` (ADR 0020): it scrolls with the text,
    /// into scrollback, and follows reflow.
    pub line: usize,
    /// Width in cells
    pub cell_width: u16,
    /// Height in cells
    pub cell_height: u16,
}

/// Storage for terminal images.
///
/// Bounded by [`MAX_IMAGE_STORE_BYTES`] and [`MAX_IMAGE_STORE_COUNT`]; the
/// oldest images are evicted first.
#[derive(Debug, Default, Clone)]
pub struct ImageStore {
    images: HashMap<ImageId, TerminalImage>,
    /// Insertion order (front = oldest) for eviction
    order: VecDeque<ImageId>,
    /// Sum of `data.len()` over all stored images
    total_bytes: usize,
}

impl ImageStore {
    /// Create a new empty image store.
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            order: VecDeque::new(),
            total_bytes: 0,
        }
    }

    /// Add an image to the store, evicting the oldest images if the byte or
    /// count limits would be exceeded.
    pub fn add(&mut self, image: TerminalImage) -> ImageId {
        let id = image.id;
        if let Some(previous) = self.images.remove(&id) {
            self.total_bytes -= previous.data.len();
            self.order.retain(|&existing| existing != id);
        }
        self.total_bytes += image.data.len();
        self.images.insert(id, image);
        self.order.push_back(id);

        while (self.total_bytes > MAX_IMAGE_STORE_BYTES
            || self.images.len() > MAX_IMAGE_STORE_COUNT)
            && self.order.len() > 1
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.images.remove(&oldest) {
                self.total_bytes -= evicted.data.len();
            }
        }
        id
    }

    /// Total RGBA bytes currently stored.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Get an image by ID.
    pub fn get(&self, id: ImageId) -> Option<&TerminalImage> {
        self.images.get(&id)
    }

    /// Remove an image.
    pub fn remove(&mut self, id: ImageId) -> Option<TerminalImage> {
        let removed = self.images.remove(&id)?;
        self.total_bytes -= removed.data.len();
        self.order.retain(|&existing| existing != id);
        Some(removed)
    }

    /// Let `update` change each image in place; the images it returns
    /// `false` for are removed.
    pub fn retain_mut(&mut self, mut update: impl FnMut(&mut TerminalImage) -> bool) {
        let mut freed = 0;
        self.images.retain(|_, image| {
            let keep = update(image);
            if !keep {
                freed += image.data.len();
            }
            keep
        });
        self.total_bytes -= freed;
        let images = &self.images;
        self.order.retain(|id| images.contains_key(id));
    }

    /// Remove and return every image, oldest first.
    pub fn take_all(&mut self) -> Vec<TerminalImage> {
        let order = std::mem::take(&mut self.order);
        let mut images = std::mem::take(&mut self.images);
        self.total_bytes = 0;
        order.iter().filter_map(|id| images.remove(id)).collect()
    }

    /// Clear all images.
    pub fn clear(&mut self) {
        self.images.clear();
        self.order.clear();
        self.total_bytes = 0;
    }

    /// Get all images.
    pub fn images(&self) -> impl Iterator<Item = &TerminalImage> {
        self.images.values()
    }

    /// Number of stored images.
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Check if store is empty.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

/// Sixel color palette.
#[derive(Debug, Clone)]
struct SixelPalette {
    colors: [u32; 256],
}

impl Default for SixelPalette {
    fn default() -> Self {
        // Initialize with VGA-like default palette
        let mut colors = [0xFF000000u32; 256]; // Default to black with full alpha

        // Standard 16 colors
        colors[0] = 0xFF000000; // Black
        colors[1] = 0xFF0000AA; // Blue
        colors[2] = 0xFF00AA00; // Green
        colors[3] = 0xFF00AAAA; // Cyan
        colors[4] = 0xFFAA0000; // Red
        colors[5] = 0xFFAA00AA; // Magenta
        colors[6] = 0xFFAA5500; // Brown/Yellow
        colors[7] = 0xFFAAAAAA; // Light Gray
        colors[8] = 0xFF555555; // Dark Gray
        colors[9] = 0xFF5555FF; // Bright Blue
        colors[10] = 0xFF55FF55; // Bright Green
        colors[11] = 0xFF55FFFF; // Bright Cyan
        colors[12] = 0xFFFF5555; // Bright Red
        colors[13] = 0xFFFF55FF; // Bright Magenta
        colors[14] = 0xFFFFFF55; // Bright Yellow
        colors[15] = 0xFFFFFFFF; // White

        Self { colors }
    }
}

impl SixelPalette {
    fn set_rgb(&mut self, index: u8, r: u8, g: u8, b: u8) {
        // RGBA format (little-endian becomes ABGR in memory)
        self.colors[index as usize] =
            0xFF000000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
    }

    fn set_hls(&mut self, index: u8, h: u16, l: u16, s: u16) {
        // Convert HLS to RGB
        // H is 0-360, L and S are 0-100
        let (r, g, b) = hls_to_rgb(h, l, s);
        self.set_rgb(index, r, g, b);
    }

    fn get(&self, index: u8) -> u32 {
        self.colors[index as usize]
    }
}

/// Convert HLS (Hue, Lightness, Saturation) to RGB.
/// H is 0-360, L and S are 0-100.
fn hls_to_rgb(h: u16, l: u16, s: u16) -> (u8, u8, u8) {
    let l = l as f32 / 100.0;
    let s = s as f32 / 100.0;

    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }

    let h = (h % 360) as f32 / 60.0;

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = l - c / 2.0;

    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// Decode Sixel data into RGBA pixels.
///
/// Sixel is a graphics format where each character represents a vertical strip
/// of 6 pixels. The character value (63-126) has 6 bits, each controlling
/// one pixel in the strip.
pub fn decode_sixel(data: &[u8], background_mode: u16) -> Option<(Vec<u8>, u32, u32)> {
    let mut palette = SixelPalette::default();
    let mut current_color: u8 = 0;

    // First pass: determine image dimensions
    let mut max_x: u32 = 0;

    let mut x: u32 = 0;
    let mut y: u32 = 0;

    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        match c {
            // Graphics newline (-)
            b'-' => {
                max_x = max_x.max(x);
                x = 0;
                y = y.saturating_add(6);
            }
            // Carriage return ($)
            b'$' => {
                max_x = max_x.max(x);
                x = 0;
            }
            // Repeat introducer (!)
            b'!' => {
                let (count, next_i) = parse_number(&data[i + 1..]);
                i = i + 1 + next_i;
                if i < data.len() && data[i] >= 63 && data[i] <= 126 {
                    // Saturate: anything past MAX_SIXEL_DIM is rejected below
                    x = x.saturating_add(count);
                }
            }
            // Color introducer (#)
            b'#' => {
                let (params, next_i) = parse_color_params(&data[i + 1..]);
                i = i + 1 + next_i - 1; // -1 because we'll increment at end
                if params.len() == 1 {
                    // Just selecting a color
                    current_color = params[0] as u8;
                }
                // Color definition is handled in second pass
            }
            // Sixel data character (63-126)
            63..=126 => {
                x = x.saturating_add(1);
            }
            // Raster attributes (")
            b'"' => {
                // Skip raster attributes in dimension calculation
                while i < data.len() && data[i] != b'#' && data[i] != b'-' && data[i] != b'$' {
                    if data[i] >= 63 && data[i] <= 126 {
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    max_x = max_x.max(x);
    let max_y: u32 = y.saturating_add(6); // Add final row height

    if max_x == 0 || max_y == 0 {
        return None;
    }

    // Limit dimensions to prevent OOM from crafted Sixel data
    if max_x > MAX_SIXEL_DIM
        || max_y > MAX_SIXEL_DIM
        || u64::from(max_x) * u64::from(max_y) > MAX_SIXEL_PIXELS
    {
        return None;
    }

    // Allocate pixel buffer (RGBA)
    let mut pixels = vec![0u8; (max_x * max_y * 4) as usize];

    // Set background based on mode
    if background_mode == 0 || background_mode == 2 {
        // Transparent or set to background color
        // Default is transparent (already 0)
    } else {
        // Keep existing - for terminal this means transparent
    }

    // Second pass: decode pixels
    x = 0;
    y = 0;
    i = 0;

    while i < data.len() {
        let c = data[i];
        match c {
            // Graphics newline
            b'-' => {
                x = 0;
                y = y.saturating_add(6);
            }
            // Carriage return
            b'$' => {
                x = 0;
            }
            // Repeat introducer
            b'!' => {
                let (count, next_i) = parse_number(&data[i + 1..]);
                i = i + 1 + next_i;
                if i < data.len() && data[i] >= 63 && data[i] <= 126 {
                    let sixel = data[i] - 63;
                    let color = palette.get(current_color);
                    // Clamp to the remaining width: pixels past it are
                    // invisible and would only burn time.
                    let count = count.min(max_x.saturating_sub(x));
                    for _ in 0..count {
                        draw_sixel(&mut pixels, max_x, max_y, x, y, sixel, color);
                        x += 1;
                    }
                }
            }
            // Color introducer
            b'#' => {
                let (params, next_i) = parse_color_params(&data[i + 1..]);
                i = i + 1 + next_i - 1;

                match params.len() {
                    1 => {
                        // Select color
                        current_color = params[0] as u8;
                    }
                    5 => {
                        // Define color: #Pc;Pu;Px;Py;Pz
                        let index = params[0] as u8;
                        let color_space = params[1];
                        match color_space {
                            1 => {
                                // HLS
                                palette.set_hls(index, params[2], params[3], params[4]);
                            }
                            _ => {
                                // RGB (percentages 0-100)
                                let r = ((params[2] as u32 * 255) / 100) as u8;
                                let g = ((params[3] as u32 * 255) / 100) as u8;
                                let b = ((params[4] as u32 * 255) / 100) as u8;
                                palette.set_rgb(index, r, g, b);
                            }
                        }
                        current_color = index;
                    }
                    _ => {}
                }
            }
            // Raster attributes: "Pan;Pad;Ph;Pv
            b'"' => {
                // Skip to next control character or sixel
                let mut j = i + 1;
                while j < data.len() {
                    let next = data[j];
                    if next == b'#'
                        || next == b'-'
                        || next == b'$'
                        || next == b'!'
                        || (63..=126).contains(&next)
                    {
                        break;
                    }
                    j += 1;
                }
                i = j - 1;
            }
            // Sixel data character
            63..=126 => {
                let sixel = c - 63;
                let color = palette.get(current_color);
                draw_sixel(&mut pixels, max_x, max_y, x, y, sixel, color);
                x = x.saturating_add(1);
            }
            _ => {}
        }
        i += 1;
    }

    Some((pixels, max_x, max_y))
}

/// Draw a single sixel (6 vertical pixels) at the given position.
fn draw_sixel(pixels: &mut [u8], width: u32, height: u32, x: u32, y: u32, sixel: u8, color: u32) {
    for bit in 0..6 {
        if (sixel & (1 << bit)) != 0 {
            let py = y + bit;
            if x < width && py < height {
                let offset = ((py * width + x) * 4) as usize;
                if offset + 3 < pixels.len() {
                    // RGBA
                    pixels[offset] = (color & 0xFF) as u8; // R
                    pixels[offset + 1] = ((color >> 8) & 0xFF) as u8; // G
                    pixels[offset + 2] = ((color >> 16) & 0xFF) as u8; // B
                    pixels[offset + 3] = ((color >> 24) & 0xFF) as u8; // A
                }
            }
        }
    }
}

/// Parse a decimal number from the data, returning the value and bytes consumed.
fn parse_number(data: &[u8]) -> (u32, usize) {
    let mut value: u32 = 0;
    let mut i = 0;

    while i < data.len() && data[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((data[i] - b'0') as u32);
        i += 1;
    }

    (value.max(1), i) // Minimum 1 for repeat count
}

/// Parse color parameters (semicolon-separated numbers).
fn parse_color_params(data: &[u8]) -> (Vec<u16>, usize) {
    let mut params = Vec::new();
    let mut current: u16 = 0;
    let mut has_value = false;
    let mut i = 0;

    while i < data.len() {
        let c = data[i];
        match c {
            b'0'..=b'9' => {
                current = current.saturating_mul(10).saturating_add((c - b'0') as u16);
                has_value = true;
            }
            b';' => {
                params.push(current);
                current = 0;
                has_value = false;
            }
            _ => {
                break;
            }
        }
        i += 1;
    }

    if has_value || !params.is_empty() {
        params.push(current);
    }

    (params, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hls_to_rgb() {
        // Pure red at full saturation and medium lightness
        let (r, g, b) = hls_to_rgb(0, 50, 100);
        assert_eq!(r, 255);
        assert!(g < 10);
        assert!(b < 10);

        // Pure green
        let (r, g, b) = hls_to_rgb(120, 50, 100);
        assert!(r < 10);
        assert_eq!(g, 255);
        assert!(b < 10);

        // Pure blue
        let (r, g, b) = hls_to_rgb(240, 50, 100);
        assert!(r < 10);
        assert!(g < 10);
        assert_eq!(b, 255);

        // White (full lightness)
        let (r, g, b) = hls_to_rgb(0, 100, 0);
        assert_eq!(r, 255);
        assert_eq!(g, 255);
        assert_eq!(b, 255);

        // Black (zero lightness)
        let (r, g, b) = hls_to_rgb(0, 0, 0);
        assert_eq!(r, 0);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_parse_number() {
        assert_eq!(parse_number(b"123abc"), (123, 3));
        assert_eq!(parse_number(b"1"), (1, 1));
        assert_eq!(parse_number(b"abc"), (1, 0)); // minimum 1
    }

    #[test]
    fn test_parse_color_params() {
        let (params, len) = parse_color_params(b"1;2;100;50;50#");
        assert_eq!(params, vec![1, 2, 100, 50, 50]);
        assert_eq!(len, 13);

        let (params, _) = parse_color_params(b"5");
        assert_eq!(params, vec![5]);
    }

    #[test]
    fn test_simple_sixel() {
        // A simple 1x6 red pixel column
        // #0;2;100;0;0 = define color 0 as RGB(100%, 0%, 0%) = red
        // ? = sixel char 63, which is 0b000000 (no pixels)
        // ~ = sixel char 126, which is 0b111111 (all 6 pixels)
        let data = b"#0;2;100;0;0~";
        let result = decode_sixel(data, 0);
        assert!(result.is_some());

        let (pixels, width, height) = result.unwrap();
        assert_eq!(width, 1);
        assert_eq!(height, 6);

        // Check that all 6 pixels are red
        for y in 0..6 {
            let offset = y * 4;
            assert_eq!(pixels[offset], 255, "R at y={}", y); // R
            assert_eq!(pixels[offset + 1], 0, "G at y={}", y); // G
            assert_eq!(pixels[offset + 2], 0, "B at y={}", y); // B
            assert_eq!(pixels[offset + 3], 255, "A at y={}", y); // A
        }
    }

    #[test]
    fn test_sixel_with_repeat() {
        // 4 red columns: !4~ means repeat ~ 4 times
        let data = b"#0;2;100;0;0!4~";
        let result = decode_sixel(data, 0);
        assert!(result.is_some());

        let (_, width, height) = result.unwrap();
        assert_eq!(width, 4);
        assert_eq!(height, 6);
    }

    #[test]
    fn parse_number_saturates_on_overflow() {
        // A very long digit string should saturate to u32::MAX, not wrap
        let data = b"99999999999999999999";
        let (value, consumed) = parse_number(data);
        assert_eq!(consumed, data.len());
        assert_eq!(value, u32::MAX);
    }

    #[test]
    fn parse_color_params_saturates_on_overflow() {
        let data = b"99999;99999";
        let (params, _) = parse_color_params(data);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], u16::MAX);
        assert_eq!(params[1], u16::MAX);
    }

    #[test]
    fn sixel_huge_repeat_counts_do_not_overflow() {
        // Two repeats that together exceed u32::MAX, then a modest one.
        let data = b"!2147483648~!2147483648~!100~";
        let start = std::time::Instant::now();
        let result = decode_sixel(data, 0);
        assert!(result.is_none(), "oversized width must be rejected");
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn sixel_repeat_is_clamped_to_image_width() {
        // First row is 2 columns wide; a later repeat of 1000 on a row must
        // not walk past the buffer or take noticeable time.
        let data = b"~~$!1000~";
        let (pixels, width, height) = decode_sixel(data, 0).unwrap();
        assert_eq!(width, 1000);
        assert_eq!(height, 6);
        assert_eq!(pixels.len(), 1000 * 6 * 4);
    }

    #[test]
    fn sixel_rejects_too_many_pixels() {
        // 16384 wide via repeat, then enough rows to exceed MAX_SIXEL_PIXELS
        // while each dimension alone is still within MAX_SIXEL_DIM.
        let mut data = Vec::new();
        data.extend_from_slice(b"!16384~");
        let rows_needed = (MAX_SIXEL_PIXELS / 16384 / 6) as usize + 2;
        data.extend(std::iter::repeat_n(b'-', rows_needed));
        data.push(b'~');
        assert!(decode_sixel(&data, 0).is_none());
    }

    fn image_of(id: u32, bytes: usize) -> TerminalImage {
        TerminalImage {
            id: ImageId(id),
            width: 1,
            height: 1,
            data: vec![0; bytes],
            start_col: 0,
            line: 0,
            cell_width: 1,
            cell_height: 1,
        }
    }

    #[test]
    fn image_store_evicts_oldest_when_over_byte_limit() {
        let mut store = ImageStore::new();
        let chunk = MAX_IMAGE_STORE_BYTES / 4;
        for id in 0..6 {
            store.add(image_of(id, chunk));
        }
        assert_eq!(store.len(), 4);
        assert_eq!(store.total_bytes(), chunk * 4);
        assert!(store.get(ImageId(0)).is_none());
        assert!(store.get(ImageId(1)).is_none());
        assert!(store.get(ImageId(5)).is_some());
    }

    #[test]
    fn image_store_evicts_oldest_when_over_count_limit() {
        let mut store = ImageStore::new();
        for id in 0..(MAX_IMAGE_STORE_COUNT as u32 + 5) {
            store.add(image_of(id, 4));
        }
        assert_eq!(store.len(), MAX_IMAGE_STORE_COUNT);
        assert!(store.get(ImageId(0)).is_none());
        assert!(store
            .get(ImageId(MAX_IMAGE_STORE_COUNT as u32 + 4))
            .is_some());
    }

    #[test]
    fn image_store_remove_and_clear_track_bytes() {
        let mut store = ImageStore::new();
        store.add(image_of(1, 16));
        store.add(image_of(2, 32));
        assert_eq!(store.total_bytes(), 48);
        store.remove(ImageId(1));
        assert_eq!(store.total_bytes(), 32);
        store.clear();
        assert_eq!(store.total_bytes(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn sixel_rejects_oversized_dimensions() {
        // Craft data that would create a very tall image via many newlines
        let mut data = Vec::new();
        data.extend_from_slice(b"#0;2;100;0;0");
        data.push(b'~'); // One sixel column
                         // 3000 newlines × 6 pixels each = 18000 > 16384 limit
        data.extend(std::iter::repeat_n(b'-', 3000));
        data.push(b'~');
        let result = decode_sixel(&data, 0);
        assert!(result.is_none());
    }
}
