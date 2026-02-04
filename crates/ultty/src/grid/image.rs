//! Image storage and Sixel decoding for terminal inline images.

use std::collections::HashMap;

/// Unique identifier for terminal images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// A decoded terminal image.
#[derive(Debug, Clone)]
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
    /// Position where the image starts (row)
    pub start_row: u16,
    /// Width in cells
    pub cell_width: u16,
    /// Height in cells
    pub cell_height: u16,
}

/// Storage for terminal images.
#[derive(Debug, Default)]
pub struct ImageStore {
    images: HashMap<ImageId, TerminalImage>,
    next_id: u32,
}

impl ImageStore {
    /// Create a new empty image store.
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            next_id: 0,
        }
    }

    /// Add an image to the store.
    pub fn add(&mut self, image: TerminalImage) -> ImageId {
        let id = image.id;
        self.images.insert(id, image);
        id
    }

    /// Allocate a new image ID.
    pub fn allocate_id(&mut self) -> ImageId {
        let id = ImageId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Get an image by ID.
    pub fn get(&self, id: ImageId) -> Option<&TerminalImage> {
        self.images.get(&id)
    }

    /// Remove an image.
    pub fn remove(&mut self, id: ImageId) -> Option<TerminalImage> {
        self.images.remove(&id)
    }

    /// Clear all images.
    pub fn clear(&mut self) {
        self.images.clear();
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
                y += 6;
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
                    x += count;
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
                x += 1;
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
    let max_y: u32 = y + 6; // Add final row height

    if max_x == 0 || max_y == 0 {
        return None;
    }

    // Limit dimensions to prevent OOM from crafted Sixel data
    const MAX_SIXEL_DIM: u32 = 16384;
    if max_x > MAX_SIXEL_DIM || max_y > MAX_SIXEL_DIM {
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
                y += 6;
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
                x += 1;
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
        value = value.saturating_mul(10).saturating_add((data[i] - b'0') as u32);
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
    fn sixel_rejects_oversized_dimensions() {
        // Craft data that would create a very tall image via many newlines
        let mut data = Vec::new();
        data.extend_from_slice(b"#0;2;100;0;0");
        data.push(b'~'); // One sixel column
        // 3000 newlines × 6 pixels each = 18000 > 16384 limit
        for _ in 0..3000 {
            data.push(b'-');
        }
        data.push(b'~');
        let result = decode_sixel(&data, 0);
        assert!(result.is_none());
    }
}
