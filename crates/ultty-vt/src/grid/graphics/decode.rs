//! Decoding transmitted image data under fixed limits (ADR 0026,
//! "Security"). Every input is untrusted: any program that writes to the
//! terminal, a `cat`ed file or an ssh peer controls it. No allocation here
//! grows past the limits below whatever the input says, and every failure
//! is an [`Error`], never a panic.

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;

use super::reply::Error;
use crate::grid::image::{MAX_SIXEL_DIM, MAX_SIXEL_PIXELS};

/// The most bytes one image may transmit: after base64, before zlib.
pub const MAX_TRANSMITTED_BYTES: usize = 32 * 1024 * 1024;

/// The widest or tallest image, in pixels; the same as for sixel.
pub const MAX_IMAGE_DIM: u32 = MAX_SIXEL_DIM;

/// The most pixels in one image (64 MiB of RGBA); the same as for sixel.
pub const MAX_IMAGE_PIXELS: u64 = MAX_SIXEL_PIXELS;

/// The most RGBA bytes one decoded image holds.
pub const MAX_DECODED_BYTES: usize = MAX_IMAGE_PIXELS as usize * 4;

/// Standard base64 with an exact alphabet; padding may be left out, as
/// some clients do, but nothing else is forgiven.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Append the base64 `chunk` to `data`. A chunk that more chunks follow
/// must be a whole number of 4-byte groups (spec, "Remote client").
pub fn append_base64(data: &mut Vec<u8>, chunk: &[u8], more: bool) -> Result<(), Error> {
    if more && !chunk.len().is_multiple_of(4) {
        return Err(Error::BAD_BASE64);
    }
    // A chunk is at most `MAX_GRAPHICS_APC` long, so checking after
    // decoding it bounds the buffer as well.
    BASE64
        .decode_vec(chunk, data)
        .map_err(|_| Error::BAD_BASE64)?;
    if data.len() > MAX_TRANSMITTED_BYTES {
        return Err(Error::TOO_MUCH_DATA);
    }
    Ok(())
}

/// How transmitted bytes encode an image: the `f`, `s`, `v`, `o` and `S`
/// keys of the command that started the upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    /// 24 (RGB), 32 (RGBA) or 100 (PNG).
    pub format: u32,
    /// Pixel width and height, for RGB and RGBA.
    pub width: u32,
    pub height: u32,
    /// zlib-compressed (`o=z`).
    pub compressed: bool,
    /// For compressed PNG, the size of the PNG data (`S`); 0 if unknown.
    pub size: u32,
}

/// A decoded image: tightly packed RGBA8, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Check pixel dimensions against the limits.
fn check_size(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::ZERO_SIZE);
    }
    if width > MAX_IMAGE_DIM
        || height > MAX_IMAGE_DIM
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(Error::TOO_LARGE);
    }
    Ok(())
}

/// Check the dimensions an upload declares before any data arrives, so a
/// client learns at once that it is too large. PNG declares none.
pub fn check_declared(encoding: &Encoding) -> Result<(), Error> {
    match encoding.format {
        24 | 32 => check_size(encoding.width, encoding.height),
        _ => Ok(()),
    }
}

/// Inflate zlib `data` to at most `limit` bytes. Output past the limit is
/// a decompression bomb: refused, with nothing kept.
pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit).map_err(|_| Error::INFLATE)
}

/// Decode a finished upload.
pub fn decode(encoding: &Encoding, data: Vec<u8>) -> Result<Decoded, Error> {
    match encoding.format {
        24 | 32 => decode_raw(encoding, data),
        100 => {
            let png = if encoding.compressed {
                let limit = match encoding.size {
                    0 => MAX_TRANSMITTED_BYTES,
                    size => (size as usize).min(MAX_TRANSMITTED_BYTES),
                };
                inflate(&data, limit)?
            } else {
                data
            };
            decode_png(&png)
        }
        _ => Err(Error::UNKNOWN_FORMAT),
    }
}

/// RGB or RGBA pixels, whose size the command gives.
fn decode_raw(encoding: &Encoding, data: Vec<u8>) -> Result<Decoded, Error> {
    let (width, height) = (encoding.width, encoding.height);
    check_size(width, height)?;
    let channels: usize = if encoding.format == 24 { 3 } else { 4 };
    let expected = width as usize * height as usize * channels;
    let pixels = if encoding.compressed {
        let pixels = inflate(&data, expected)?;
        if pixels.len() != expected {
            return Err(Error::SIZE_MISMATCH);
        }
        pixels
    } else {
        match data.len().cmp(&expected) {
            std::cmp::Ordering::Less => return Err(Error::NOT_ENOUGH_DATA),
            std::cmp::Ordering::Greater => return Err(Error::SIZE_MISMATCH),
            std::cmp::Ordering::Equal => data,
        }
    };
    let rgba = if channels == 4 {
        pixels
    } else {
        let mut rgba = Vec::with_capacity(expected / 3 * 4);
        for rgb in pixels.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 0xff]);
        }
        rgba
    };
    Ok(Decoded {
        width,
        height,
        rgba,
    })
}

/// A PNG image: its header is checked against the limits before the frame
/// is allocated. Palette, low bit depth and 16-bit images become RGBA8;
/// of an APNG, only the default image is decoded.
pub fn decode_png(png: &[u8]) -> Result<Decoded, Error> {
    let mut decoder = png::Decoder::new_with_limits(
        png,
        png::Limits {
            bytes: MAX_DECODED_BYTES,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| Error::PNG)?;
    let (width, height) = (reader.info().width, reader.info().height);
    check_size(width, height)?;
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err(Error::PNG);
    }
    let size = reader.output_buffer_size();
    if size > MAX_DECODED_BYTES {
        return Err(Error::TOO_LARGE);
    }
    let mut buffer = vec![0; size];
    let frame = reader.next_frame(&mut buffer).map_err(|_| Error::PNG)?;
    buffer.truncate(frame.buffer_size());
    let pixels = width as usize * height as usize;
    let rgba = match color {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 0xff])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 0xff]).collect(),
        // EXPAND turns palette images into RGB or RGBA.
        png::ColorType::Indexed => return Err(Error::PNG),
    };
    if rgba.len() != pixels * 4 {
        return Err(Error::PNG);
    }
    Ok(Decoded {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zlib(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }

    fn png_of(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
        out
    }

    fn raw(format: u32, width: u32, height: u32, compressed: bool) -> Encoding {
        Encoding {
            format,
            width,
            height,
            compressed,
            size: 0,
        }
    }

    #[test]
    fn rgb_is_expanded_to_opaque_rgba() {
        let decoded = decode(&raw(24, 2, 1, false), vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(decoded.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn raw_data_must_match_the_declared_size() {
        assert_eq!(
            decode(&raw(32, 1, 1, false), vec![0; 3]),
            Err(Error::NOT_ENOUGH_DATA)
        );
        assert_eq!(
            decode(&raw(32, 1, 1, false), vec![0; 5]),
            Err(Error::SIZE_MISMATCH)
        );
        assert_eq!(decode(&raw(32, 0, 1, false), vec![]), Err(Error::ZERO_SIZE));
        assert_eq!(
            decode(&raw(32, 16385, 1, false), vec![]),
            Err(Error::TOO_LARGE)
        );
        assert_eq!(
            decode(&raw(32, 8192, 4096, false), vec![]),
            Err(Error::TOO_LARGE)
        );
    }

    #[test]
    fn compressed_raw_data_inflates_to_exactly_the_declared_size() {
        let pixels = [9u8; 16];
        let decoded = decode(&raw(32, 2, 2, true), zlib(&pixels)).unwrap();
        assert_eq!(decoded.rgba, pixels);
        assert_eq!(
            decode(&raw(32, 2, 2, true), zlib(&[9; 12])),
            Err(Error::SIZE_MISMATCH)
        );
    }

    #[test]
    fn a_decompression_bomb_stops_at_the_limit() {
        // 64 MiB of zeros compresses to about 64 KiB; declared as one pixel,
        // inflation must stop after four bytes.
        let bomb = zlib(&vec![0; 64 * 1024 * 1024]);
        assert_eq!(
            decode(&raw(32, 1, 1, true), bomb.clone()),
            Err(Error::INFLATE)
        );
        assert_eq!(inflate(&bomb, 1024), Err(Error::INFLATE));
    }

    #[test]
    fn png_colour_types_become_rgba() {
        let rgba = png_of(1, 1, png::ColorType::Rgba, &[1, 2, 3, 4]);
        assert_eq!(decode_png(&rgba).unwrap().rgba, [1, 2, 3, 4]);
        let rgb = png_of(1, 1, png::ColorType::Rgb, &[1, 2, 3]);
        assert_eq!(decode_png(&rgb).unwrap().rgba, [1, 2, 3, 255]);
        let gray = png_of(2, 1, png::ColorType::Grayscale, &[7, 8]);
        assert_eq!(
            decode_png(&gray).unwrap().rgba,
            [7, 7, 7, 255, 8, 8, 8, 255]
        );
        let gray_alpha = png_of(1, 1, png::ColorType::GrayscaleAlpha, &[7, 9]);
        assert_eq!(decode_png(&gray_alpha).unwrap().rgba, [7, 7, 7, 9]);
    }

    #[test]
    fn compressed_png_is_inflated_first() {
        let png = png_of(1, 1, png::ColorType::Rgba, &[1, 2, 3, 4]);
        let encoding = Encoding {
            format: 100,
            width: 0,
            height: 0,
            compressed: true,
            size: png.len() as u32,
        };
        assert_eq!(decode(&encoding, zlib(&png)).unwrap().rgba, [1, 2, 3, 4]);
    }

    #[test]
    fn png_headers_past_the_limits_are_refused_before_decoding() {
        // A valid header claiming 16384 x 16385 pixels, with no data.
        let mut png = png_of(1, 1, png::ColorType::Rgba, &[0; 4]);
        png[16..20].copy_from_slice(&16384u32.to_be_bytes());
        png[20..24].copy_from_slice(&16385u32.to_be_bytes());
        // Fix the IHDR CRC so the decoder reads the header.
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(decode_png(&png), Err(Error::TOO_LARGE));
    }

    #[test]
    fn garbage_is_a_png_error() {
        assert_eq!(decode_png(b"not a png"), Err(Error::PNG));
        assert_eq!(decode_png(&[]), Err(Error::PNG));
    }

    #[test]
    fn base64_chunks_append_and_are_checked() {
        let mut data = Vec::new();
        append_base64(&mut data, b"QUJD", true).unwrap();
        append_base64(&mut data, b"RA", false).unwrap();
        assert_eq!(data, b"ABCD");
        assert_eq!(
            append_base64(&mut data, b"QUJ", true),
            Err(Error::BAD_BASE64)
        );
        assert_eq!(
            append_base64(&mut data, b"Q!JD", false),
            Err(Error::BAD_BASE64)
        );
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
