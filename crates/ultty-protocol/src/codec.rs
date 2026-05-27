//! Encoding and decoding for protocol messages.
//!
//! Frame format:
//! - Magic: 2 bytes (0x55 0x54 = "UT")
//! - Type: 2 bytes (u16 little-endian)
//! - Length: 4 bytes (u32 little-endian, payload length)
//! - Payload: variable

mod frame;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::{
    content::{
        ButtonStyle, ButtonWidget, CodeWidget, Content, ImageScale, ImageWidget, InputWidget,
        MarkdownWidget, ProgressWidget, RichText, Span, SpinnerStyle, SpinnerWidget, TextAlign,
        TextContent, TextOverflow, TextWrap, WidgetContent,
    },
    events::{
        FocusEvent, InputEvent, InputType, Key, Modifiers, MouseButton, ResizeEvent, ScrollEvent,
    },
    layout::{
        AlignContent, AlignItems, AlignSelf, Constraints, Dimension, Display, Edges, FlexDirection,
        FlexWrap, GridLine, GridPlacement, JustifyContent, Overflow, OverflowAxis, Size,
        TrackSizingFunction,
    },
    messages::{ComputedRegionLayout, FrameId, LayoutTrigger},
    style::{BorderEdges, BorderStyle, BorderType, Color, Easing, NamedColor, Style, Transition},
    AppMessage, ProtocolError, RegionId, Result, TerminalMessage,
};

use frame::{begin_frame, decode_frame_header, finish_frame};

// ============================================================================
// Constants
// ============================================================================

/// Maximum allowed string length (1 MB) to prevent OOM attacks.
const MAX_STRING_LEN: usize = 1024 * 1024;

/// Maximum allowed region count in a layout message to prevent OOM attacks.
const MAX_REGION_COUNT: usize = 10_000;

/// Maximum allowed payload length (16 MB) to prevent OOM attacks.
const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;

/// Maximum allowed grid track count to prevent OOM attacks.
const MAX_TRACK_COUNT: usize = 1_000;

/// Maximum allowed image data size (64 MB) to prevent OOM attacks.
const MAX_IMAGE_DATA_LEN: usize = 64 * 1024 * 1024;

/// Maximum allowed RichText span count to prevent OOM attacks.
const MAX_SPAN_COUNT: usize = 10_000;

/// Maximum allowed metadata key/value pair count to prevent OOM attacks.
const MAX_METADATA_PAIRS: usize = 1024;

/// Maximum total bytes across all metadata keys and values (1 MB).
const MAX_METADATA_TOTAL_BYTES: usize = 1024 * 1024;

// ============================================================================
// Validation helpers
// ============================================================================

/// Reject NaN and Infinity values in decoded floats.
fn validate_f32(v: f32) -> Result<f32> {
    if v.is_nan() || v.is_infinite() {
        return Err(ProtocolError::InvalidFloat(v));
    }
    Ok(v)
}

// ============================================================================
// Traits
// ============================================================================

/// Trait for encoding messages to bytes.
pub trait Encode {
    /// Encode to a byte buffer.
    fn encode(&self, buf: &mut BytesMut);

    /// Encode to a new Bytes.
    fn encode_to_bytes(&self) -> Bytes {
        let mut buf = BytesMut::new();
        self.encode(&mut buf);
        buf.freeze()
    }
}

/// Trait for decoding messages from bytes.
pub trait Decode: Sized {
    /// Decode from a byte buffer.
    fn decode(buf: &mut impl Buf) -> Result<Self>;
}

// ============================================================================
// Helper functions
// ============================================================================

fn encode_string(s: &str, buf: &mut BytesMut) {
    let bytes = s.as_bytes();
    buf.put_u32_le(bytes.len() as u32);
    buf.put_slice(bytes);
}

fn decode_string(buf: &mut impl Buf) -> Result<String> {
    if buf.remaining() < 4 {
        return Err(ProtocolError::UnexpectedEof {
            needed: 4,
            have: buf.remaining(),
        });
    }
    let len = buf.get_u32_le() as usize;
    if len > MAX_STRING_LEN {
        return Err(ProtocolError::InvalidData(format!(
            "string length {} exceeds maximum {}",
            len, MAX_STRING_LEN
        )));
    }
    if buf.remaining() < len {
        return Err(ProtocolError::UnexpectedEof {
            needed: len,
            have: buf.remaining(),
        });
    }
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    String::from_utf8(bytes).map_err(|e| ProtocolError::InvalidUtf8(e.to_string()))
}

fn encode_option_f32(opt: Option<f32>, buf: &mut BytesMut) {
    match opt {
        Some(v) => {
            buf.put_u8(1);
            buf.put_f32_le(v);
        }
        None => buf.put_u8(0),
    }
}

fn decode_option_f32(buf: &mut impl Buf) -> Result<Option<f32>> {
    if buf.remaining() < 1 {
        return Err(ProtocolError::UnexpectedEof {
            needed: 1,
            have: buf.remaining(),
        });
    }
    match buf.get_u8() {
        0 => Ok(None),
        1 => {
            if buf.remaining() < 4 {
                return Err(ProtocolError::UnexpectedEof {
                    needed: 4,
                    have: buf.remaining(),
                });
            }
            Ok(Some(validate_f32(buf.get_f32_le())?))
        }
        _ => Err(ProtocolError::InvalidData("invalid option tag".into())),
    }
}

fn encode_option_string(opt: &Option<String>, buf: &mut BytesMut) {
    match opt {
        Some(s) => {
            buf.put_u8(1);
            encode_string(s, buf);
        }
        None => buf.put_u8(0),
    }
}

fn decode_option_string(buf: &mut impl Buf) -> Result<Option<String>> {
    if buf.remaining() < 1 {
        return Err(ProtocolError::UnexpectedEof {
            needed: 1,
            have: buf.remaining(),
        });
    }
    match buf.get_u8() {
        0 => Ok(None),
        1 => Ok(Some(decode_string(buf)?)),
        _ => Err(ProtocolError::InvalidData("invalid option tag".into())),
    }
}

// ============================================================================
// Basic Types
// ============================================================================

impl Encode for RegionId {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u32_le(self.0);
    }
}

impl Decode for RegionId {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 4 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 4,
                have: buf.remaining(),
            });
        }
        Ok(RegionId(buf.get_u32_le()))
    }
}

impl Encode for FrameId {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u32_le(self.0);
    }
}

impl Decode for FrameId {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 4 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 4,
                have: buf.remaining(),
            });
        }
        Ok(FrameId(buf.get_u32_le()))
    }
}

// ============================================================================
// Color Types
// ============================================================================

impl Encode for NamedColor {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for NamedColor {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        let val = buf.get_u8();
        match val {
            0 => Ok(NamedColor::Black),
            1 => Ok(NamedColor::Red),
            2 => Ok(NamedColor::Green),
            3 => Ok(NamedColor::Yellow),
            4 => Ok(NamedColor::Blue),
            5 => Ok(NamedColor::Magenta),
            6 => Ok(NamedColor::Cyan),
            7 => Ok(NamedColor::White),
            8 => Ok(NamedColor::BrightBlack),
            9 => Ok(NamedColor::BrightRed),
            10 => Ok(NamedColor::BrightGreen),
            11 => Ok(NamedColor::BrightYellow),
            12 => Ok(NamedColor::BrightBlue),
            13 => Ok(NamedColor::BrightMagenta),
            14 => Ok(NamedColor::BrightCyan),
            15 => Ok(NamedColor::BrightWhite),
            16 => Ok(NamedColor::Foreground),
            17 => Ok(NamedColor::Background),
            _ => Err(ProtocolError::InvalidData(format!(
                "invalid named color: {}",
                val
            ))),
        }
    }
}

impl Encode for Color {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            Color::Named(c) => {
                buf.put_u8(0);
                c.encode(buf);
            }
            Color::Rgb(r, g, b) => {
                buf.put_u8(1);
                buf.put_u8(*r);
                buf.put_u8(*g);
                buf.put_u8(*b);
            }
            Color::Indexed(i) => {
                buf.put_u8(2);
                buf.put_u8(*i);
            }
        }
    }
}

impl Decode for Color {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(Color::Named(NamedColor::decode(buf)?)),
            1 => {
                if buf.remaining() < 3 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 3,
                        have: buf.remaining(),
                    });
                }
                Ok(Color::Rgb(buf.get_u8(), buf.get_u8(), buf.get_u8()))
            }
            2 => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 1,
                        have: buf.remaining(),
                    });
                }
                Ok(Color::Indexed(buf.get_u8()))
            }
            t => Err(ProtocolError::InvalidData(format!(
                "invalid color type: {}",
                t
            ))),
        }
    }
}

fn encode_option_color(opt: &Option<Color>, buf: &mut BytesMut) {
    match opt {
        Some(c) => {
            buf.put_u8(1);
            c.encode(buf);
        }
        None => buf.put_u8(0),
    }
}

fn decode_option_color(buf: &mut impl Buf) -> Result<Option<Color>> {
    if buf.remaining() < 1 {
        return Err(ProtocolError::UnexpectedEof {
            needed: 1,
            have: buf.remaining(),
        });
    }
    match buf.get_u8() {
        0 => Ok(None),
        1 => Ok(Some(Color::decode(buf)?)),
        _ => Err(ProtocolError::InvalidData("invalid option tag".into())),
    }
}

// ============================================================================
// Border Types
// ============================================================================

impl Encode for BorderType {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for BorderType {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(BorderType::None),
            1 => Ok(BorderType::Thin),
            2 => Ok(BorderType::Thick),
            3 => Ok(BorderType::Double),
            4 => Ok(BorderType::Rounded),
            5 => Ok(BorderType::Dashed),
            6 => Ok(BorderType::Ascii),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid border type: {}",
                v
            ))),
        }
    }
}

impl Encode for BorderEdges {
    fn encode(&self, buf: &mut BytesMut) {
        let bits = (self.top as u8)
            | ((self.right as u8) << 1)
            | ((self.bottom as u8) << 2)
            | ((self.left as u8) << 3);
        buf.put_u8(bits);
    }
}

impl Decode for BorderEdges {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        let bits = buf.get_u8();
        Ok(BorderEdges {
            top: bits & 1 != 0,
            right: bits & 2 != 0,
            bottom: bits & 4 != 0,
            left: bits & 8 != 0,
        })
    }
}

impl Encode for BorderStyle {
    fn encode(&self, buf: &mut BytesMut) {
        self.style.encode(buf);
        self.edges.encode(buf);
        buf.put_u8(self.radius);
    }
}

impl Decode for BorderStyle {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(BorderStyle {
            style: BorderType::decode(buf)?,
            edges: BorderEdges::decode(buf)?,
            radius: {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 1,
                        have: buf.remaining(),
                    });
                }
                buf.get_u8()
            },
        })
    }
}

// ============================================================================
// Style
// ============================================================================

impl Encode for Style {
    fn encode(&self, buf: &mut BytesMut) {
        encode_option_color(&self.fg, buf);
        encode_option_color(&self.bg, buf);
        self.border.encode(buf);
        encode_option_color(&self.border_color, buf);
        let flags = (self.bold as u8)
            | ((self.italic as u8) << 1)
            | ((self.underline as u8) << 2)
            | ((self.dim as u8) << 3)
            | ((self.transition.is_some() as u8) << 4);
        buf.put_u8(flags);
        buf.put_f32_le(self.opacity);
        if let Some(ref transition) = self.transition {
            transition.encode(buf);
        }
    }
}

impl Decode for Style {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let fg = decode_option_color(buf)?;
        let bg = decode_option_color(buf)?;
        let border = BorderStyle::decode(buf)?;
        let border_color = decode_option_color(buf)?;
        if buf.remaining() < 5 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 5,
                have: buf.remaining(),
            });
        }
        let flags = buf.get_u8();
        let opacity = validate_f32(buf.get_f32_le())?;
        let transition = if flags & 16 != 0 {
            Some(Transition::decode(buf)?)
        } else {
            None
        };
        Ok(Style {
            fg,
            bg,
            border,
            border_color,
            bold: flags & 1 != 0,
            italic: flags & 2 != 0,
            underline: flags & 4 != 0,
            dim: flags & 8 != 0,
            opacity,
            transition,
        })
    }
}

impl Encode for Transition {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u32_le(self.duration_ms);
        buf.put_u8(self.easing as u8);
        buf.put_u32_le(self.properties);
    }
}

impl Decode for Transition {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 9 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 9,
                have: buf.remaining(),
            });
        }
        let duration_ms = buf.get_u32_le();
        let easing_byte = buf.get_u8();
        let easing = match easing_byte {
            0 => Easing::Linear,
            1 => Easing::EaseIn,
            2 => Easing::EaseOut,
            3 => Easing::EaseInOut,
            _ => Easing::Linear,
        };
        let properties = buf.get_u32_le();
        Ok(Transition {
            duration_ms,
            easing,
            properties,
        })
    }
}

// ============================================================================
// Layout Types
// ============================================================================

impl Encode for Dimension {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            Dimension::Auto => buf.put_u8(0),
            Dimension::Cells(v) => {
                buf.put_u8(1);
                buf.put_f32_le(*v);
            }
            Dimension::Percent(v) => {
                buf.put_u8(2);
                buf.put_f32_le(*v);
            }
            Dimension::Fr(v) => {
                buf.put_u8(3);
                buf.put_f32_le(*v);
            }
            Dimension::Vw(v) => {
                buf.put_u8(4);
                buf.put_f32_le(*v);
            }
            Dimension::Vh(v) => {
                buf.put_u8(5);
                buf.put_f32_le(*v);
            }
            Dimension::MinContent => buf.put_u8(6),
            Dimension::MaxContent => buf.put_u8(7),
        }
    }
}

impl Decode for Dimension {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(Dimension::Auto),
            1 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                Ok(Dimension::Cells(validate_f32(buf.get_f32_le())?))
            }
            2 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                Ok(Dimension::Percent(validate_f32(buf.get_f32_le())?))
            }
            3 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                Ok(Dimension::Fr(validate_f32(buf.get_f32_le())?))
            }
            4 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                Ok(Dimension::Vw(validate_f32(buf.get_f32_le())?))
            }
            5 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                Ok(Dimension::Vh(validate_f32(buf.get_f32_le())?))
            }
            6 => Ok(Dimension::MinContent),
            7 => Ok(Dimension::MaxContent),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid dimension type: {}",
                v
            ))),
        }
    }
}

impl<T: Encode> Encode for Edges<T> {
    fn encode(&self, buf: &mut BytesMut) {
        self.top.encode(buf);
        self.right.encode(buf);
        self.bottom.encode(buf);
        self.left.encode(buf);
    }
}

impl<T: Decode> Decode for Edges<T> {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(Edges {
            top: T::decode(buf)?,
            right: T::decode(buf)?,
            bottom: T::decode(buf)?,
            left: T::decode(buf)?,
        })
    }
}

impl<T: Encode> Encode for Size<T> {
    fn encode(&self, buf: &mut BytesMut) {
        self.width.encode(buf);
        self.height.encode(buf);
    }
}

impl<T: Decode> Decode for Size<T> {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(Size {
            width: T::decode(buf)?,
            height: T::decode(buf)?,
        })
    }
}

impl Encode for Overflow {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(self.x as u8);
        buf.put_u8(self.y as u8);
    }
}

impl Decode for Overflow {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 2 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 2,
                have: buf.remaining(),
            });
        }
        let x = match buf.get_u8() {
            0 => OverflowAxis::Visible,
            1 => OverflowAxis::Hidden,
            2 => OverflowAxis::Scroll,
            3 => OverflowAxis::Auto,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid overflow axis: {}",
                    v
                )))
            }
        };
        let y = match buf.get_u8() {
            0 => OverflowAxis::Visible,
            1 => OverflowAxis::Hidden,
            2 => OverflowAxis::Scroll,
            3 => OverflowAxis::Auto,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid overflow axis: {}",
                    v
                )))
            }
        };
        Ok(Overflow { x, y })
    }
}

impl Encode for GridLine {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            GridLine::Auto => buf.put_u8(0),
            GridLine::Line(n) => {
                buf.put_u8(1);
                buf.put_i16_le(*n);
            }
            GridLine::Span(n) => {
                buf.put_u8(2);
                buf.put_u16_le(*n);
            }
        }
    }
}

impl Decode for GridLine {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(GridLine::Auto),
            1 => {
                if buf.remaining() < 2 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 2,
                        have: buf.remaining(),
                    });
                }
                Ok(GridLine::Line(buf.get_i16_le()))
            }
            2 => {
                if buf.remaining() < 2 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 2,
                        have: buf.remaining(),
                    });
                }
                Ok(GridLine::Span(buf.get_u16_le()))
            }
            v => Err(ProtocolError::InvalidData(format!(
                "invalid grid line type: {}",
                v
            ))),
        }
    }
}

impl Encode for GridPlacement {
    fn encode(&self, buf: &mut BytesMut) {
        self.start.encode(buf);
        self.end.encode(buf);
    }
}

impl Decode for GridPlacement {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(GridPlacement {
            start: GridLine::decode(buf)?,
            end: GridLine::decode(buf)?,
        })
    }
}

impl Encode for TrackSizingFunction {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            TrackSizingFunction::Single(d) => {
                buf.put_u8(0);
                d.encode(buf);
            }
            TrackSizingFunction::MinMax(a, b) => {
                buf.put_u8(1);
                a.encode(buf);
                b.encode(buf);
            }
            TrackSizingFunction::Repeat(count, dims) => {
                buf.put_u8(2);
                buf.put_u16_le(*count);
                buf.put_u32_le(dims.len() as u32);
                for d in dims {
                    d.encode(buf);
                }
            }
        }
    }
}

impl Decode for TrackSizingFunction {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(TrackSizingFunction::Single(Dimension::decode(buf)?)),
            1 => Ok(TrackSizingFunction::MinMax(
                Dimension::decode(buf)?,
                Dimension::decode(buf)?,
            )),
            2 => {
                if buf.remaining() < 6 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 6,
                        have: buf.remaining(),
                    });
                }
                let count = buf.get_u16_le();
                let len = buf.get_u32_le() as usize;
                if len > MAX_TRACK_COUNT {
                    return Err(ProtocolError::InvalidData(format!(
                        "track count {} exceeds maximum {}",
                        len, MAX_TRACK_COUNT
                    )));
                }
                let mut dims = Vec::with_capacity(len);
                for _ in 0..len {
                    dims.push(Dimension::decode(buf)?);
                }
                Ok(TrackSizingFunction::Repeat(count, dims))
            }
            v => Err(ProtocolError::InvalidData(format!(
                "invalid track sizing function: {}",
                v
            ))),
        }
    }
}

fn encode_vec_track(vec: &[TrackSizingFunction], buf: &mut BytesMut) {
    buf.put_u32_le(vec.len() as u32);
    for item in vec {
        item.encode(buf);
    }
}

fn decode_vec_track(buf: &mut impl Buf) -> Result<Vec<TrackSizingFunction>> {
    if buf.remaining() < 4 {
        return Err(ProtocolError::UnexpectedEof {
            needed: 4,
            have: buf.remaining(),
        });
    }
    let len = buf.get_u32_le() as usize;
    if len > MAX_TRACK_COUNT {
        return Err(ProtocolError::InvalidData(format!(
            "track count {} exceeds maximum {}",
            len, MAX_TRACK_COUNT
        )));
    }
    let mut vec = Vec::with_capacity(len);
    for _ in 0..len {
        vec.push(TrackSizingFunction::decode(buf)?);
    }
    Ok(vec)
}

impl Encode for Constraints {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(self.display as u8);
        buf.put_u8(self.flex_direction as u8);
        buf.put_u8(self.flex_wrap as u8);
        buf.put_u8(self.justify_content as u8);
        buf.put_u8(self.align_items as u8);
        buf.put_u8(self.align_self as u8);
        buf.put_u8(self.align_content as u8);
        buf.put_f32_le(self.flex_grow);
        buf.put_f32_le(self.flex_shrink);
        self.flex_basis.encode(buf);
        self.width.encode(buf);
        self.height.encode(buf);
        self.min_width.encode(buf);
        self.min_height.encode(buf);
        self.max_width.encode(buf);
        self.max_height.encode(buf);
        encode_option_f32(self.aspect_ratio, buf);
        self.margin.encode(buf);
        self.padding.encode(buf);
        self.gap.encode(buf);
        self.overflow.encode(buf);
        encode_vec_track(&self.grid_template_columns, buf);
        encode_vec_track(&self.grid_template_rows, buf);
        self.grid_column.encode(buf);
        self.grid_row.encode(buf);
    }
}

impl Decode for Constraints {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 15 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 15,
                have: buf.remaining(),
            });
        }
        let display = match buf.get_u8() {
            0 => Display::Block,
            1 => Display::Flex,
            2 => Display::Grid,
            3 => Display::None,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid display: {}",
                    v
                )))
            }
        };
        let flex_direction = match buf.get_u8() {
            0 => FlexDirection::Row,
            1 => FlexDirection::RowReverse,
            2 => FlexDirection::Column,
            3 => FlexDirection::ColumnReverse,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid flex direction: {}",
                    v
                )))
            }
        };
        let flex_wrap = match buf.get_u8() {
            0 => FlexWrap::NoWrap,
            1 => FlexWrap::Wrap,
            2 => FlexWrap::WrapReverse,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid flex wrap: {}",
                    v
                )))
            }
        };
        let justify_content = match buf.get_u8() {
            0 => JustifyContent::Start,
            1 => JustifyContent::End,
            2 => JustifyContent::Center,
            3 => JustifyContent::SpaceBetween,
            4 => JustifyContent::SpaceAround,
            5 => JustifyContent::SpaceEvenly,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid justify content: {}",
                    v
                )))
            }
        };
        let align_items = match buf.get_u8() {
            0 => AlignItems::Start,
            1 => AlignItems::End,
            2 => AlignItems::Center,
            3 => AlignItems::Baseline,
            4 => AlignItems::Stretch,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid align items: {}",
                    v
                )))
            }
        };
        let align_self = match buf.get_u8() {
            0 => AlignSelf::Auto,
            1 => AlignSelf::Start,
            2 => AlignSelf::End,
            3 => AlignSelf::Center,
            4 => AlignSelf::Baseline,
            5 => AlignSelf::Stretch,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid align self: {}",
                    v
                )))
            }
        };
        let align_content = match buf.get_u8() {
            0 => AlignContent::Start,
            1 => AlignContent::End,
            2 => AlignContent::Center,
            3 => AlignContent::SpaceBetween,
            4 => AlignContent::SpaceAround,
            5 => AlignContent::SpaceEvenly,
            6 => AlignContent::Stretch,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid align content: {}",
                    v
                )))
            }
        };
        let flex_grow = validate_f32(buf.get_f32_le())?;
        let flex_shrink = validate_f32(buf.get_f32_le())?;
        let flex_basis = Dimension::decode(buf)?;
        let width = Dimension::decode(buf)?;
        let height = Dimension::decode(buf)?;
        let min_width = Dimension::decode(buf)?;
        let min_height = Dimension::decode(buf)?;
        let max_width = Dimension::decode(buf)?;
        let max_height = Dimension::decode(buf)?;
        let aspect_ratio = decode_option_f32(buf)?;
        let margin = Edges::decode(buf)?;
        let padding = Edges::decode(buf)?;
        let gap = Size::decode(buf)?;
        let overflow = Overflow::decode(buf)?;
        let grid_template_columns = decode_vec_track(buf)?;
        let grid_template_rows = decode_vec_track(buf)?;
        let grid_column = GridPlacement::decode(buf)?;
        let grid_row = GridPlacement::decode(buf)?;
        Ok(Constraints {
            display,
            flex_direction,
            flex_wrap,
            justify_content,
            align_items,
            align_self,
            align_content,
            flex_grow,
            flex_shrink,
            flex_basis,
            width,
            height,
            min_width,
            min_height,
            max_width,
            max_height,
            aspect_ratio,
            margin,
            padding,
            gap,
            overflow,
            grid_template_columns,
            grid_template_rows,
            grid_column,
            grid_row,
        })
    }
}

// ============================================================================
// Content Types
// ============================================================================

impl Encode for TextWrap {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for TextWrap {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(TextWrap::None),
            1 => Ok(TextWrap::Char),
            2 => Ok(TextWrap::Word),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid text wrap: {}",
                v
            ))),
        }
    }
}

impl Encode for TextAlign {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for TextAlign {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(TextAlign::Left),
            1 => Ok(TextAlign::Center),
            2 => Ok(TextAlign::Right),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid text align: {}",
                v
            ))),
        }
    }
}

impl Encode for TextOverflow {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for TextOverflow {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(TextOverflow::Clip),
            1 => Ok(TextOverflow::Ellipsis),
            2 => Ok(TextOverflow::Fade),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid text overflow: {}",
                v
            ))),
        }
    }
}

impl Encode for TextContent {
    fn encode(&self, buf: &mut BytesMut) {
        encode_string(&self.text, buf);
        self.wrap.encode(buf);
        self.align.encode(buf);
        self.overflow.encode(buf);
    }
}

impl Decode for TextContent {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(TextContent {
            text: decode_string(buf)?,
            wrap: TextWrap::decode(buf)?,
            align: TextAlign::decode(buf)?,
            overflow: TextOverflow::decode(buf)?,
        })
    }
}

impl Encode for Span {
    fn encode(&self, buf: &mut BytesMut) {
        encode_string(&self.text, buf);
        self.style.encode(buf);
    }
}

impl Decode for Span {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(Span {
            text: decode_string(buf)?,
            style: Style::decode(buf)?,
        })
    }
}

impl Encode for RichText {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u32_le(self.spans.len() as u32);
        for span in &self.spans {
            span.encode(buf);
        }
    }
}

impl Decode for RichText {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 4 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 4,
                have: buf.remaining(),
            });
        }
        let len = buf.get_u32_le() as usize;
        if len > MAX_SPAN_COUNT {
            return Err(ProtocolError::InvalidData(format!(
                "span count {} exceeds maximum {}",
                len, MAX_SPAN_COUNT
            )));
        }
        let mut spans = Vec::with_capacity(len);
        for _ in 0..len {
            spans.push(Span::decode(buf)?);
        }
        Ok(RichText { spans })
    }
}

impl Encode for InputWidget {
    fn encode(&self, buf: &mut BytesMut) {
        encode_string(&self.value, buf);
        encode_string(&self.placeholder, buf);
        buf.put_u8((self.multiline as u8) | ((self.password as u8) << 1));
    }
}

impl Decode for InputWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let value = decode_string(buf)?;
        let placeholder = decode_string(buf)?;
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        let flags = buf.get_u8();
        Ok(InputWidget {
            value,
            placeholder,
            multiline: flags & 1 != 0,
            password: flags & 2 != 0,
        })
    }
}

impl Encode for MarkdownWidget {
    fn encode(&self, buf: &mut BytesMut) {
        encode_string(&self.source, buf);
    }
}

impl Decode for MarkdownWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        Ok(MarkdownWidget {
            source: decode_string(buf)?,
        })
    }
}

impl Encode for CodeWidget {
    fn encode(&self, buf: &mut BytesMut) {
        encode_string(&self.source, buf);
        encode_string(&self.language, buf);
        buf.put_u8(self.line_numbers as u8);
    }
}

impl Decode for CodeWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let source = decode_string(buf)?;
        let language = decode_string(buf)?;
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        Ok(CodeWidget {
            source,
            language,
            line_numbers: buf.get_u8() != 0,
        })
    }
}

impl Encode for ProgressWidget {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_f32_le(self.value);
        encode_option_string(&self.label, buf);
        buf.put_u8(self.animated as u8);
        buf.put_u8(self.indeterminate as u8);
    }
}

impl Decode for ProgressWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 4 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 4,
                have: buf.remaining(),
            });
        }
        let value = buf.get_f32_le();
        let label = decode_option_string(buf)?;
        // Read optional trailing bytes for backward compatibility
        let animated = if buf.remaining() >= 1 {
            buf.get_u8() != 0
        } else {
            false
        };
        let indeterminate = if buf.remaining() >= 1 {
            buf.get_u8() != 0
        } else {
            false
        };
        Ok(ProgressWidget {
            value,
            label,
            animated,
            indeterminate,
        })
    }
}

impl Encode for SpinnerWidget {
    fn encode(&self, buf: &mut BytesMut) {
        encode_option_string(&self.label, buf);
        buf.put_u8(self.style as u8);
    }
}

impl Decode for SpinnerWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let label = decode_option_string(buf)?;
        // Read optional trailing style byte for backward compatibility
        let style = if buf.remaining() >= 1 {
            match buf.get_u8() {
                0 => SpinnerStyle::Braille,
                1 => SpinnerStyle::Dots,
                2 => SpinnerStyle::Line,
                3 => SpinnerStyle::Bounce,
                _ => SpinnerStyle::Braille,
            }
        } else {
            SpinnerStyle::default()
        };
        Ok(SpinnerWidget { label, style })
    }
}

impl Encode for ImageWidget {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u32_le(self.width);
        buf.put_u32_le(self.height);
        buf.put_u8(self.scale as u8);
        buf.put_u32_le(self.data.len() as u32);
        buf.put_slice(&self.data);
    }
}

impl Decode for ImageWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 9 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 9,
                have: buf.remaining(),
            });
        }
        let width = buf.get_u32_le();
        let height = buf.get_u32_le();
        let scale = match buf.get_u8() {
            0 => ImageScale::Fit,
            1 => ImageScale::Fill,
            2 => ImageScale::Stretch,
            3 => ImageScale::None,
            v => {
                return Err(ProtocolError::InvalidData(format!(
                    "invalid image scale: {}",
                    v
                )))
            }
        };
        if buf.remaining() < 4 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 4,
                have: buf.remaining(),
            });
        }
        let data_len = buf.get_u32_le() as usize;
        if data_len > MAX_IMAGE_DATA_LEN {
            return Err(ProtocolError::InvalidData(format!(
                "image data length {} exceeds maximum {}",
                data_len, MAX_IMAGE_DATA_LEN
            )));
        }
        if buf.remaining() < data_len {
            return Err(ProtocolError::UnexpectedEof {
                needed: data_len,
                have: buf.remaining(),
            });
        }
        let mut data = vec![0u8; data_len];
        buf.copy_to_slice(&mut data);
        Ok(ImageWidget {
            data,
            width,
            height,
            scale,
        })
    }
}

impl Encode for ButtonStyle {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for ButtonStyle {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(ButtonStyle::Default),
            1 => Ok(ButtonStyle::Primary),
            2 => Ok(ButtonStyle::Danger),
            3 => Ok(ButtonStyle::Success),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid button style: {}",
                v
            ))),
        }
    }
}

impl Encode for ButtonWidget {
    fn encode(&self, buf: &mut BytesMut) {
        encode_string(&self.label, buf);
        self.style.encode(buf);
    }
}

impl Decode for ButtonWidget {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let label = decode_string(buf)?;
        let style = ButtonStyle::decode(buf)?;
        Ok(ButtonWidget { label, style })
    }
}

impl Encode for WidgetContent {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            WidgetContent::Input(w) => {
                buf.put_u8(0);
                w.encode(buf);
            }
            WidgetContent::Markdown(w) => {
                buf.put_u8(1);
                w.encode(buf);
            }
            WidgetContent::Code(w) => {
                buf.put_u8(2);
                w.encode(buf);
            }
            WidgetContent::Progress(w) => {
                buf.put_u8(3);
                w.encode(buf);
            }
            WidgetContent::Spinner(w) => {
                buf.put_u8(4);
                w.encode(buf);
            }
            WidgetContent::Image(w) => {
                buf.put_u8(5);
                w.encode(buf);
            }
            WidgetContent::Button(w) => {
                buf.put_u8(6);
                w.encode(buf);
            }
        }
    }
}

impl Decode for WidgetContent {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(WidgetContent::Input(InputWidget::decode(buf)?)),
            1 => Ok(WidgetContent::Markdown(MarkdownWidget::decode(buf)?)),
            2 => Ok(WidgetContent::Code(CodeWidget::decode(buf)?)),
            3 => Ok(WidgetContent::Progress(ProgressWidget::decode(buf)?)),
            4 => Ok(WidgetContent::Spinner(SpinnerWidget::decode(buf)?)),
            5 => Ok(WidgetContent::Image(ImageWidget::decode(buf)?)),
            6 => Ok(WidgetContent::Button(ButtonWidget::decode(buf)?)),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid widget type: {}",
                v
            ))),
        }
    }
}

impl Encode for Content {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            Content::None => buf.put_u8(0),
            Content::Text(t) => {
                buf.put_u8(1);
                t.encode(buf);
            }
            Content::RichText(r) => {
                buf.put_u8(2);
                r.encode(buf);
            }
            Content::Widget(w) => {
                buf.put_u8(3);
                w.encode(buf);
            }
        }
    }
}

impl Decode for Content {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(Content::None),
            1 => Ok(Content::Text(TextContent::decode(buf)?)),
            2 => Ok(Content::RichText(RichText::decode(buf)?)),
            3 => Ok(Content::Widget(WidgetContent::decode(buf)?)),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid content type: {}",
                v
            ))),
        }
    }
}

// ============================================================================
// Event Types
// ============================================================================

impl Encode for MouseButton {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for MouseButton {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(MouseButton::Left),
            1 => Ok(MouseButton::Middle),
            2 => Ok(MouseButton::Right),
            3 => Ok(MouseButton::Button4),
            4 => Ok(MouseButton::Button5),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid mouse button: {}",
                v
            ))),
        }
    }
}

impl Encode for Key {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            Key::Char(c) => {
                buf.put_u8(0);
                buf.put_u32_le(*c as u32);
            }
            Key::Function(n) => {
                buf.put_u8(1);
                buf.put_u8(*n);
            }
            Key::Escape => buf.put_u8(2),
            Key::Enter => buf.put_u8(3),
            Key::Tab => buf.put_u8(4),
            Key::Backspace => buf.put_u8(5),
            Key::Delete => buf.put_u8(6),
            Key::Insert => buf.put_u8(7),
            Key::Home => buf.put_u8(8),
            Key::End => buf.put_u8(9),
            Key::PageUp => buf.put_u8(10),
            Key::PageDown => buf.put_u8(11),
            Key::Up => buf.put_u8(12),
            Key::Down => buf.put_u8(13),
            Key::Left => buf.put_u8(14),
            Key::Right => buf.put_u8(15),
        }
    }
}

impl Decode for Key {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => {
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                char::from_u32(buf.get_u32_le())
                    .map(Key::Char)
                    .ok_or_else(|| ProtocolError::InvalidData("invalid char".into()))
            }
            1 => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 1,
                        have: buf.remaining(),
                    });
                }
                Ok(Key::Function(buf.get_u8()))
            }
            2 => Ok(Key::Escape),
            3 => Ok(Key::Enter),
            4 => Ok(Key::Tab),
            5 => Ok(Key::Backspace),
            6 => Ok(Key::Delete),
            7 => Ok(Key::Insert),
            8 => Ok(Key::Home),
            9 => Ok(Key::End),
            10 => Ok(Key::PageUp),
            11 => Ok(Key::PageDown),
            12 => Ok(Key::Up),
            13 => Ok(Key::Down),
            14 => Ok(Key::Left),
            15 => Ok(Key::Right),
            v => Err(ProtocolError::InvalidData(format!("invalid key: {}", v))),
        }
    }
}

impl Encode for Modifiers {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(self.bits);
    }
}

impl Decode for Modifiers {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        Ok(Modifiers { bits: buf.get_u8() })
    }
}

impl Encode for InputType {
    fn encode(&self, buf: &mut BytesMut) {
        match self {
            InputType::MouseDown { button } => {
                buf.put_u8(0);
                button.encode(buf);
            }
            InputType::MouseUp { button } => {
                buf.put_u8(1);
                button.encode(buf);
            }
            InputType::MouseMove => buf.put_u8(2),
            InputType::MouseScroll { delta_x, delta_y } => {
                buf.put_u8(3);
                buf.put_f32_le(*delta_x);
                buf.put_f32_le(*delta_y);
            }
            InputType::KeyDown { key, modifiers } => {
                buf.put_u8(4);
                key.encode(buf);
                modifiers.encode(buf);
            }
            InputType::KeyUp { key, modifiers } => {
                buf.put_u8(5);
                key.encode(buf);
                modifiers.encode(buf);
            }
            InputType::TextInput { text } => {
                buf.put_u8(6);
                encode_string(text, buf);
            }
        }
    }
}

impl Decode for InputType {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(InputType::MouseDown {
                button: MouseButton::decode(buf)?,
            }),
            1 => Ok(InputType::MouseUp {
                button: MouseButton::decode(buf)?,
            }),
            2 => Ok(InputType::MouseMove),
            3 => {
                if buf.remaining() < 8 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 8,
                        have: buf.remaining(),
                    });
                }
                Ok(InputType::MouseScroll {
                    delta_x: buf.get_f32_le(),
                    delta_y: buf.get_f32_le(),
                })
            }
            4 => Ok(InputType::KeyDown {
                key: Key::decode(buf)?,
                modifiers: Modifiers::decode(buf)?,
            }),
            5 => Ok(InputType::KeyUp {
                key: Key::decode(buf)?,
                modifiers: Modifiers::decode(buf)?,
            }),
            6 => Ok(InputType::TextInput {
                text: decode_string(buf)?,
            }),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid input type: {}",
                v
            ))),
        }
    }
}

impl Encode for InputEvent {
    fn encode(&self, buf: &mut BytesMut) {
        self.region_id.encode(buf);
        self.event_type.encode(buf);
        buf.put_f32_le(self.x);
        buf.put_f32_le(self.y);
        self.modifiers.encode(buf);
    }
}

impl Decode for InputEvent {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let region_id = RegionId::decode(buf)?;
        let event_type = InputType::decode(buf)?;
        if buf.remaining() < 8 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 8,
                have: buf.remaining(),
            });
        }
        let x = buf.get_f32_le();
        let y = buf.get_f32_le();
        let modifiers = if buf.has_remaining() {
            Modifiers::decode(buf)?
        } else {
            Modifiers::NONE
        };
        Ok(InputEvent {
            region_id,
            event_type,
            x,
            y,
            modifiers,
        })
    }
}

impl Encode for ResizeEvent {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u16_le(self.width);
        buf.put_u16_le(self.height);
    }
}

impl Decode for ResizeEvent {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 4 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 4,
                have: buf.remaining(),
            });
        }
        Ok(ResizeEvent {
            width: buf.get_u16_le(),
            height: buf.get_u16_le(),
        })
    }
}

impl Encode for FocusEvent {
    fn encode(&self, buf: &mut BytesMut) {
        self.region_id.encode(buf);
        buf.put_u8(self.focused as u8);
    }
}

impl Decode for FocusEvent {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let region_id = RegionId::decode(buf)?;
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        Ok(FocusEvent {
            region_id,
            focused: buf.get_u8() != 0,
        })
    }
}

impl Encode for ScrollEvent {
    fn encode(&self, buf: &mut BytesMut) {
        self.region_id.encode(buf);
        buf.put_f32_le(self.scroll_x);
        buf.put_f32_le(self.scroll_y);
    }
}

impl Decode for ScrollEvent {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let region_id = RegionId::decode(buf)?;
        if buf.remaining() < 8 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 8,
                have: buf.remaining(),
            });
        }
        Ok(ScrollEvent {
            region_id,
            scroll_x: buf.get_f32_le(),
            scroll_y: buf.get_f32_le(),
        })
    }
}

impl Encode for LayoutTrigger {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(*self as u8);
    }
}

impl Decode for LayoutTrigger {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 1 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 1,
                have: buf.remaining(),
            });
        }
        match buf.get_u8() {
            0 => Ok(LayoutTrigger::Resize),
            1 => Ok(LayoutTrigger::ConstraintChange),
            2 => Ok(LayoutTrigger::ContentChange),
            3 => Ok(LayoutTrigger::TreeChange),
            v => Err(ProtocolError::InvalidData(format!(
                "invalid layout trigger: {}",
                v
            ))),
        }
    }
}

impl Encode for ComputedRegionLayout {
    fn encode(&self, buf: &mut BytesMut) {
        self.id.encode(buf);
        buf.put_f32_le(self.x);
        buf.put_f32_le(self.y);
        buf.put_f32_le(self.width);
        buf.put_f32_le(self.height);
        buf.put_f32_le(self.content_x);
        buf.put_f32_le(self.content_y);
        buf.put_f32_le(self.content_width);
        buf.put_f32_le(self.content_height);
        buf.put_u8(self.visible as u8);
    }
}

impl Decode for ComputedRegionLayout {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let id = RegionId::decode(buf)?;
        if buf.remaining() < 33 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 33,
                have: buf.remaining(),
            });
        }
        Ok(ComputedRegionLayout {
            id,
            x: buf.get_f32_le(),
            y: buf.get_f32_le(),
            width: buf.get_f32_le(),
            height: buf.get_f32_le(),
            content_x: buf.get_f32_le(),
            content_y: buf.get_f32_le(),
            content_width: buf.get_f32_le(),
            content_height: buf.get_f32_le(),
            visible: buf.get_u8() != 0,
        })
    }
}

// ============================================================================
// Messages
// ============================================================================

// Message type codes
mod msg_type {
    // App -> Terminal
    pub const CLIENT_HELLO: u16 = 0x0001;
    pub const CREATE_REGION: u16 = 0x0010;
    pub const SET_REGION_METADATA: u16 = 0x0014;
    pub const SET_CONTENT: u16 = 0x0020;
    pub const SET_STYLE: u16 = 0x0021;
    pub const SET_CONSTRAINTS: u16 = 0x0022;
    pub const APPEND_CONTENT: u16 = 0x0023;
    pub const STREAM_START: u16 = 0x0024;
    pub const STREAM_END: u16 = 0x0025;
    pub const DELETE_REGION: u16 = 0x0012;
    pub const REPARENT_REGION: u16 = 0x0013;
    pub const BEGIN_FRAME: u16 = 0x0030;
    pub const END_FRAME: u16 = 0x0031;
    pub const SCROLL: u16 = 0x0040;
    pub const FOCUS: u16 = 0x0041;
    pub const BLUR: u16 = 0x0042;
    pub const SUBSCRIBE: u16 = 0x0050;
    pub const UNSUBSCRIBE: u16 = 0x0051;
    pub const CLIPBOARD_GET: u16 = 0x0060;
    pub const CLIPBOARD_SET: u16 = 0x0061;
    pub const PONG: u16 = 0x0070;
    pub const DISCONNECT: u16 = 0x00FF;

    // Terminal -> App
    pub const SERVER_HELLO: u16 = 0x0001;
    pub const SESSION_READY: u16 = 0x0002;
    pub const LAYOUT_COMPUTED: u16 = 0x0010;
    pub const INPUT: u16 = 0x0020;
    pub const RESIZE: u16 = 0x0022;
    pub const FOCUS_EVENT: u16 = 0x0023;
    pub const SCROLL_EVENT: u16 = 0x0024;
    pub const CLIPBOARD_CONTENT: u16 = 0x0025;
    pub const PING: u16 = 0x0070;
    pub const ERROR: u16 = 0x00FF;
}

impl Encode for AppMessage {
    fn encode(&self, buf: &mut BytesMut) {
        let frame = begin_frame(buf);

        let type_code = match self {
            AppMessage::ClientHello {
                version,
                requested_capabilities,
            } => {
                buf.put_u16_le(*version);
                buf.put_u32_le(*requested_capabilities);
                msg_type::CLIENT_HELLO
            }
            AppMessage::CreateRegion {
                id,
                parent,
                insert_index,
                constraints,
                style,
            } => {
                id.encode(buf);
                parent.encode(buf);
                buf.put_i32_le(*insert_index);
                constraints.encode(buf);
                style.encode(buf);
                msg_type::CREATE_REGION
            }
            AppMessage::SetConstraints { id, constraints } => {
                id.encode(buf);
                constraints.encode(buf);
                msg_type::SET_CONSTRAINTS
            }
            AppMessage::SetStyle { id, style } => {
                id.encode(buf);
                style.encode(buf);
                msg_type::SET_STYLE
            }
            AppMessage::SetContent { id, content } => {
                id.encode(buf);
                content.encode(buf);
                msg_type::SET_CONTENT
            }
            AppMessage::AppendContent { id, text } => {
                id.encode(buf);
                encode_string(text, buf);
                msg_type::APPEND_CONTENT
            }
            AppMessage::StreamStart { id, stream_id } => {
                id.encode(buf);
                buf.put_u32_le(*stream_id);
                msg_type::STREAM_START
            }
            AppMessage::StreamEnd { id, stream_id } => {
                id.encode(buf);
                buf.put_u32_le(*stream_id);
                msg_type::STREAM_END
            }
            AppMessage::SetRegionMetadata { id, metadata } => {
                id.encode(buf);
                buf.put_u32_le(metadata.len() as u32);
                for (key, value) in metadata {
                    encode_string(key, buf);
                    encode_string(value, buf);
                }
                msg_type::SET_REGION_METADATA
            }
            AppMessage::DeleteRegion { id } => {
                id.encode(buf);
                msg_type::DELETE_REGION
            }
            AppMessage::ReparentRegion {
                id,
                new_parent,
                insert_index,
            } => {
                id.encode(buf);
                new_parent.encode(buf);
                buf.put_i32_le(*insert_index);
                msg_type::REPARENT_REGION
            }
            AppMessage::BeginFrame { frame_id } => {
                frame_id.encode(buf);
                msg_type::BEGIN_FRAME
            }
            AppMessage::EndFrame { frame_id } => {
                frame_id.encode(buf);
                msg_type::END_FRAME
            }
            AppMessage::Scroll { id, x, y, animate } => {
                id.encode(buf);
                buf.put_f32_le(*x);
                buf.put_f32_le(*y);
                buf.put_u8(*animate as u8);
                msg_type::SCROLL
            }
            AppMessage::Focus { id } => {
                id.encode(buf);
                msg_type::FOCUS
            }
            AppMessage::Blur => msg_type::BLUR,
            AppMessage::Subscribe { id, events } => {
                id.encode(buf);
                buf.put_u32_le(*events);
                msg_type::SUBSCRIBE
            }
            AppMessage::Unsubscribe { id, events } => {
                id.encode(buf);
                buf.put_u32_le(*events);
                msg_type::UNSUBSCRIBE
            }
            AppMessage::ClipboardGet => msg_type::CLIPBOARD_GET,
            AppMessage::ClipboardSet { text } => {
                encode_string(text, buf);
                msg_type::CLIPBOARD_SET
            }
            AppMessage::Disconnect => msg_type::DISCONNECT,
            AppMessage::Pong => msg_type::PONG,
        };

        finish_frame(buf, frame, type_code);
    }
}

impl Decode for AppMessage {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let header = decode_frame_header(buf, MAX_PAYLOAD_LEN)?;
        let expected_remaining = buf.remaining() - header.payload_len;

        let result = match header.type_code {
            msg_type::CLIENT_HELLO => {
                if buf.remaining() < 6 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 6,
                        have: buf.remaining(),
                    });
                }
                Ok(AppMessage::ClientHello {
                    version: buf.get_u16_le(),
                    requested_capabilities: buf.get_u32_le(),
                })
            }
            msg_type::CREATE_REGION => {
                let id = RegionId::decode(buf)?;
                let parent = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let insert_index = buf.get_i32_le();
                let constraints = Constraints::decode(buf)?;
                let style = Style::decode(buf)?;
                Ok(AppMessage::CreateRegion {
                    id,
                    parent,
                    insert_index,
                    constraints,
                    style,
                })
            }
            msg_type::SET_CONSTRAINTS => {
                let id = RegionId::decode(buf)?;
                let constraints = Constraints::decode(buf)?;
                Ok(AppMessage::SetConstraints { id, constraints })
            }
            msg_type::SET_STYLE => {
                let id = RegionId::decode(buf)?;
                let style = Style::decode(buf)?;
                Ok(AppMessage::SetStyle { id, style })
            }
            msg_type::SET_CONTENT => {
                let id = RegionId::decode(buf)?;
                let content = Content::decode(buf)?;
                Ok(AppMessage::SetContent { id, content })
            }
            msg_type::APPEND_CONTENT => {
                let id = RegionId::decode(buf)?;
                let text = decode_string(buf)?;
                Ok(AppMessage::AppendContent { id, text })
            }
            msg_type::STREAM_START => {
                let id = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let stream_id = buf.get_u32_le();
                Ok(AppMessage::StreamStart { id, stream_id })
            }
            msg_type::STREAM_END => {
                let id = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let stream_id = buf.get_u32_le();
                Ok(AppMessage::StreamEnd { id, stream_id })
            }
            msg_type::SET_REGION_METADATA => {
                let id = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let len = buf.get_u32_le() as usize;
                if len > MAX_METADATA_PAIRS {
                    return Err(ProtocolError::InvalidData(format!(
                        "metadata pair count {} exceeds maximum {}",
                        len, MAX_METADATA_PAIRS
                    )));
                }
                let mut metadata = Vec::with_capacity(len);
                let mut total_metadata_bytes = 0usize;
                for _ in 0..len {
                    let key = decode_string(buf)?;
                    let value = decode_string(buf)?;
                    total_metadata_bytes += key.len() + value.len();
                    if total_metadata_bytes > MAX_METADATA_TOTAL_BYTES {
                        return Err(ProtocolError::InvalidData(format!(
                            "metadata total size {} exceeds maximum {}",
                            total_metadata_bytes, MAX_METADATA_TOTAL_BYTES
                        )));
                    }
                    metadata.push((key, value));
                }
                Ok(AppMessage::SetRegionMetadata { id, metadata })
            }
            msg_type::DELETE_REGION => {
                let id = RegionId::decode(buf)?;
                Ok(AppMessage::DeleteRegion { id })
            }
            msg_type::REPARENT_REGION => {
                let id = RegionId::decode(buf)?;
                let new_parent = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let insert_index = buf.get_i32_le();
                Ok(AppMessage::ReparentRegion {
                    id,
                    new_parent,
                    insert_index,
                })
            }
            msg_type::BEGIN_FRAME => {
                let frame_id = FrameId::decode(buf)?;
                Ok(AppMessage::BeginFrame { frame_id })
            }
            msg_type::END_FRAME => {
                let frame_id = FrameId::decode(buf)?;
                Ok(AppMessage::EndFrame { frame_id })
            }
            msg_type::SCROLL => {
                let id = RegionId::decode(buf)?;
                if buf.remaining() < 9 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 9,
                        have: buf.remaining(),
                    });
                }
                let x = buf.get_f32_le();
                let y = buf.get_f32_le();
                let animate = buf.get_u8() != 0;
                Ok(AppMessage::Scroll { id, x, y, animate })
            }
            msg_type::FOCUS => {
                let id = RegionId::decode(buf)?;
                Ok(AppMessage::Focus { id })
            }
            msg_type::BLUR => Ok(AppMessage::Blur),
            msg_type::SUBSCRIBE => {
                let id = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let events = buf.get_u32_le();
                Ok(AppMessage::Subscribe { id, events })
            }
            msg_type::UNSUBSCRIBE => {
                let id = RegionId::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let events = buf.get_u32_le();
                Ok(AppMessage::Unsubscribe { id, events })
            }
            msg_type::CLIPBOARD_GET => Ok(AppMessage::ClipboardGet),
            msg_type::CLIPBOARD_SET => {
                let text = decode_string(buf)?;
                Ok(AppMessage::ClipboardSet { text })
            }
            msg_type::DISCONNECT => Ok(AppMessage::Disconnect),
            msg_type::PONG => Ok(AppMessage::Pong),
            _ => Err(ProtocolError::UnknownMessageType(header.type_code)),
        };

        // Verify we consumed exactly the declared payload length
        if result.is_ok() {
            let actual_remaining = buf.remaining();
            if actual_remaining < expected_remaining {
                // Consumed more than declared - protocol violation
                let over_read = expected_remaining - actual_remaining;
                return Err(ProtocolError::InvalidData(format!(
                    "payload overrun: consumed {} bytes beyond declared length {}",
                    over_read, header.payload_len
                )));
            } else if actual_remaining > expected_remaining {
                // Consumed less than declared - skip remaining bytes to stay in sync
                let under_read = actual_remaining - expected_remaining;
                buf.advance(under_read);
            }
        }

        result
    }
}

impl Encode for TerminalMessage {
    fn encode(&self, buf: &mut BytesMut) {
        let frame = begin_frame(buf);

        let type_code = match self {
            TerminalMessage::ServerHello {
                version,
                capabilities,
            } => {
                buf.put_u16_le(*version);
                buf.put_u32_le(*capabilities);
                msg_type::SERVER_HELLO
            }
            TerminalMessage::SessionReady {
                session_id,
                root_region,
                width,
                height,
            } => {
                buf.put_u64_le(*session_id);
                root_region.encode(buf);
                buf.put_u16_le(*width);
                buf.put_u16_le(*height);
                msg_type::SESSION_READY
            }
            TerminalMessage::LayoutComputed {
                frame_id,
                trigger,
                regions,
            } => {
                match frame_id {
                    Some(id) => {
                        buf.put_u8(1);
                        id.encode(buf);
                    }
                    None => buf.put_u8(0),
                }
                trigger.encode(buf);
                buf.put_u32_le(regions.len() as u32);
                for r in regions {
                    r.encode(buf);
                }
                msg_type::LAYOUT_COMPUTED
            }
            TerminalMessage::Input(event) => {
                event.encode(buf);
                msg_type::INPUT
            }
            TerminalMessage::Resize(event) => {
                event.encode(buf);
                msg_type::RESIZE
            }
            TerminalMessage::Focus(event) => {
                event.encode(buf);
                msg_type::FOCUS_EVENT
            }
            TerminalMessage::Scroll(event) => {
                event.encode(buf);
                msg_type::SCROLL_EVENT
            }
            TerminalMessage::ClipboardContent { text } => {
                encode_option_string(text, buf);
                msg_type::CLIPBOARD_CONTENT
            }
            TerminalMessage::Ping => msg_type::PING,
            TerminalMessage::Error { code, message } => {
                buf.put_u16_le(*code);
                encode_string(message, buf);
                msg_type::ERROR
            }
        };

        finish_frame(buf, frame, type_code);
    }
}

impl Decode for TerminalMessage {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let header = decode_frame_header(buf, MAX_PAYLOAD_LEN)?;
        let expected_remaining = buf.remaining() - header.payload_len;

        let result = match header.type_code {
            msg_type::SERVER_HELLO => {
                if buf.remaining() < 6 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 6,
                        have: buf.remaining(),
                    });
                }
                Ok(TerminalMessage::ServerHello {
                    version: buf.get_u16_le(),
                    capabilities: buf.get_u32_le(),
                })
            }
            msg_type::SESSION_READY => {
                if buf.remaining() < 16 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 16,
                        have: buf.remaining(),
                    });
                }
                let session_id = buf.get_u64_le();
                let root_region = RegionId::decode(buf)?;
                let width = buf.get_u16_le();
                let height = buf.get_u16_le();
                Ok(TerminalMessage::SessionReady {
                    session_id,
                    root_region,
                    width,
                    height,
                })
            }
            msg_type::LAYOUT_COMPUTED => {
                if buf.remaining() < 1 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 1,
                        have: buf.remaining(),
                    });
                }
                let frame_id = match buf.get_u8() {
                    0 => None,
                    1 => Some(FrameId::decode(buf)?),
                    _ => return Err(ProtocolError::InvalidData("invalid option tag".into())),
                };
                let trigger = LayoutTrigger::decode(buf)?;
                if buf.remaining() < 4 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 4,
                        have: buf.remaining(),
                    });
                }
                let count = buf.get_u32_le() as usize;
                if count > MAX_REGION_COUNT {
                    return Err(ProtocolError::InvalidData(format!(
                        "region count {} exceeds maximum {}",
                        count, MAX_REGION_COUNT
                    )));
                }
                let mut regions = Vec::with_capacity(count);
                for _ in 0..count {
                    regions.push(ComputedRegionLayout::decode(buf)?);
                }
                Ok(TerminalMessage::LayoutComputed {
                    frame_id,
                    trigger,
                    regions,
                })
            }
            msg_type::INPUT => Ok(TerminalMessage::Input(InputEvent::decode(buf)?)),
            msg_type::RESIZE => Ok(TerminalMessage::Resize(ResizeEvent::decode(buf)?)),
            msg_type::FOCUS_EVENT => Ok(TerminalMessage::Focus(FocusEvent::decode(buf)?)),
            msg_type::SCROLL_EVENT => Ok(TerminalMessage::Scroll(ScrollEvent::decode(buf)?)),
            msg_type::PING => Ok(TerminalMessage::Ping),
            msg_type::CLIPBOARD_CONTENT => {
                let text = decode_option_string(buf)?;
                Ok(TerminalMessage::ClipboardContent { text })
            }
            msg_type::ERROR => {
                if buf.remaining() < 2 {
                    return Err(ProtocolError::UnexpectedEof {
                        needed: 2,
                        have: buf.remaining(),
                    });
                }
                let code = buf.get_u16_le();
                let message = decode_string(buf)?;
                Ok(TerminalMessage::Error { code, message })
            }
            _ => Err(ProtocolError::UnknownMessageType(header.type_code)),
        };

        // Verify we consumed exactly the declared payload length
        if result.is_ok() {
            let actual_remaining = buf.remaining();
            if actual_remaining < expected_remaining {
                // Consumed more than declared - protocol violation
                let over_read = expected_remaining - actual_remaining;
                return Err(ProtocolError::InvalidData(format!(
                    "payload overrun: consumed {} bytes beyond declared length {}",
                    over_read, header.payload_len
                )));
            } else if actual_remaining > expected_remaining {
                // Consumed less than declared - skip remaining bytes to stay in sync
                let under_read = actual_remaining - expected_remaining;
                buf.advance(under_read);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FRAME_MAGIC;

    #[test]
    fn test_roundtrip_disconnect() {
        let msg = AppMessage::Disconnect;
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, AppMessage::Disconnect);
    }

    #[test]
    fn app_disconnect_has_golden_frame_bytes() {
        let bytes = AppMessage::Disconnect.encode_to_bytes();
        assert_eq!(&bytes[..], &[0x55, 0x54, 0xff, 0x00, 0, 0, 0, 0]);
    }

    #[test]
    fn terminal_ping_has_golden_frame_bytes() {
        let bytes = TerminalMessage::Ping.encode_to_bytes();
        assert_eq!(&bytes[..], &[0x55, 0x54, 0x70, 0x00, 0, 0, 0, 0]);
    }

    #[test]
    fn client_hello_has_golden_frame_bytes() {
        let bytes = AppMessage::ClientHello {
            version: 2,
            requested_capabilities: 0x0102_0304,
        }
        .encode_to_bytes();
        assert_eq!(
            &bytes[..],
            &[
                0x55, 0x54, // magic
                0x01, 0x00, // type
                0x06, 0x00, 0x00, 0x00, // payload len
                0x02, 0x00, // version
                0x04, 0x03, 0x02, 0x01, // capabilities
            ]
        );
    }

    #[test]
    fn test_roundtrip_blur() {
        let msg = AppMessage::Blur;
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, AppMessage::Blur);
    }

    #[test]
    fn test_roundtrip_client_hello() {
        let msg = AppMessage::ClientHello {
            version: 1,
            requested_capabilities: 0xDEADBEEF,
        };
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_set_region_metadata_roundtrip() {
        let msg = AppMessage::SetRegionMetadata {
            id: RegionId(42),
            metadata: vec![
                ("owner".into(), "agent".into()),
                ("task_id".into(), "auth-refactor".into()),
                ("locked".into(), "true".into()),
            ],
        };
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_set_region_metadata_empty() {
        let msg = AppMessage::SetRegionMetadata {
            id: RegionId(1),
            metadata: vec![],
        };
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_roundtrip_append_content() {
        let msg = AppMessage::AppendContent {
            id: RegionId(5),
            text: "streaming chunk".to_string(),
        };
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_roundtrip_stream_start() {
        let msg = AppMessage::StreamStart {
            id: RegionId(3),
            stream_id: 42,
        };
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_roundtrip_stream_end() {
        let msg = AppMessage::StreamEnd {
            id: RegionId(3),
            stream_id: 42,
        };
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_reject_oversized_metadata_pair_count() {
        let mut buf = BytesMut::new();
        buf.put_slice(&FRAME_MAGIC);
        buf.put_u16_le(msg_type::SET_REGION_METADATA);
        buf.put_u32_le(8);
        RegionId(1).encode(&mut buf);
        buf.put_u32_le((MAX_METADATA_PAIRS + 1) as u32);

        let mut cursor = buf.freeze();
        let result = AppMessage::decode(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_reject_oversized_metadata_total_bytes() {
        let key = "k";
        let value = "x".repeat(MAX_METADATA_TOTAL_BYTES);

        let mut buf = BytesMut::new();
        buf.put_slice(&FRAME_MAGIC);
        buf.put_u16_le(msg_type::SET_REGION_METADATA);
        let payload_len = 4 + 4 + 4 + key.len() + 4 + value.len();
        buf.put_u32_le(payload_len as u32);
        RegionId(1).encode(&mut buf);
        buf.put_u32_le(1);
        encode_string(key, &mut buf);
        encode_string(&value, &mut buf);

        let mut cursor = buf.freeze();
        let result = AppMessage::decode(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_roundtrip_style() {
        let style = Style {
            fg: Some(Color::Rgb(255, 128, 0)),
            bg: Some(Color::Named(NamedColor::Blue)),
            border: BorderStyle::thin(),
            border_color: Some(Color::Indexed(196)),
            bold: true,
            italic: false,
            underline: true,
            dim: false,
            opacity: 0.9,
            transition: None,
        };
        let mut buf = BytesMut::new();
        style.encode(&mut buf);
        let mut cursor = buf.freeze();
        let decoded = Style::decode(&mut cursor).unwrap();
        assert_eq!(decoded, style);
    }

    #[test]
    fn test_roundtrip_style_with_transition() {
        let style = Style {
            fg: Some(Color::Rgb(100, 200, 50)),
            bg: None,
            border: BorderStyle::NONE,
            border_color: None,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            opacity: 0.7,
            transition: Some(Transition {
                duration_ms: 500,
                easing: Easing::EaseOut,
                properties: 0x03, // OPACITY | BG
            }),
        };
        let mut buf = BytesMut::new();
        style.encode(&mut buf);
        let mut cursor = buf.freeze();
        let decoded = Style::decode(&mut cursor).unwrap();
        assert_eq!(decoded, style);
        assert!(decoded.transition.is_some());
        let t = decoded.transition.unwrap();
        assert_eq!(t.duration_ms, 500);
        assert_eq!(t.easing, Easing::EaseOut);
        assert_eq!(t.properties, 0x03);
    }

    #[test]
    fn test_roundtrip_transition() {
        let transition = Transition {
            duration_ms: 1000,
            easing: Easing::EaseInOut,
            properties: 0x0F,
        };
        let mut buf = BytesMut::new();
        transition.encode(&mut buf);
        let mut cursor = buf.freeze();
        let decoded = Transition::decode(&mut cursor).unwrap();
        assert_eq!(decoded, transition);
    }

    #[test]
    fn test_roundtrip_constraints() {
        let constraints = Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            flex_grow: 1.0,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(50.0),
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        constraints.encode(&mut buf);
        let mut cursor = buf.freeze();
        let decoded = Constraints::decode(&mut cursor).unwrap();
        assert_eq!(decoded, constraints);
    }

    #[test]
    fn test_reject_oversized_payload() {
        let mut buf = BytesMut::new();
        buf.put_slice(&FRAME_MAGIC);
        buf.put_u16_le(msg_type::DISCONNECT);
        buf.put_u32_le((MAX_PAYLOAD_LEN + 1) as u32);

        let mut cursor = buf.freeze();
        let result = AppMessage::decode(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_reject_oversized_string() {
        let mut buf = BytesMut::new();
        buf.put_u32_le((MAX_STRING_LEN + 1) as u32);

        let mut cursor = buf.freeze();
        let result = decode_string(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_reject_oversized_region_count() {
        let mut buf = BytesMut::new();
        buf.put_slice(&FRAME_MAGIC);
        buf.put_u16_le(msg_type::LAYOUT_COMPUTED);
        buf.put_u32_le(6);
        buf.put_u8(0);
        buf.put_u8(0);
        buf.put_u32_le((MAX_REGION_COUNT + 1) as u32);

        let mut cursor = buf.freeze();
        let result = TerminalMessage::decode(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_reject_oversized_span_count() {
        let mut buf = BytesMut::new();
        buf.put_u32_le((MAX_SPAN_COUNT + 1) as u32);

        let mut cursor = buf.freeze();
        let result = RichText::decode(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_reject_oversized_track_count() {
        let mut buf = BytesMut::new();
        buf.put_u32_le((MAX_TRACK_COUNT + 1) as u32);

        let mut cursor = buf.freeze();
        let result = decode_vec_track(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidData(_))));
    }

    #[test]
    fn test_roundtrip_pong() {
        let msg = AppMessage::Pong;
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, AppMessage::Pong);
    }

    #[test]
    fn test_roundtrip_ping() {
        let msg = TerminalMessage::Ping;
        let bytes = msg.encode_to_bytes();
        let mut cursor = bytes.as_ref();
        let decoded = TerminalMessage::decode(&mut cursor).unwrap();
        assert_eq!(decoded, TerminalMessage::Ping);
    }

    #[test]
    fn test_invalid_magic_rejected() {
        let mut buf = BytesMut::new();
        buf.put_slice(&[0xBA, 0xD0]);
        buf.put_u16_le(msg_type::DISCONNECT);
        buf.put_u32_le(0);

        let mut cursor = buf.freeze();
        let result = AppMessage::decode(&mut cursor);
        assert!(matches!(result, Err(ProtocolError::InvalidMagic(_))));
    }

    // ── Float validation tests ──────────────────────────────────────

    #[test]
    fn validate_f32_rejects_nan() {
        assert!(validate_f32(f32::NAN).is_err());
    }

    #[test]
    fn validate_f32_rejects_infinity() {
        assert!(validate_f32(f32::INFINITY).is_err());
        assert!(validate_f32(f32::NEG_INFINITY).is_err());
    }

    #[test]
    fn validate_f32_accepts_normal_values() {
        assert_eq!(validate_f32(0.0).unwrap(), 0.0);
        assert_eq!(validate_f32(1.5).unwrap(), 1.5);
        assert_eq!(validate_f32(-100.0).unwrap(), -100.0);
    }

    #[test]
    fn dimension_decode_rejects_nan() {
        let mut buf = BytesMut::new();
        buf.put_u8(1); // Cells variant
        buf.put_f32_le(f32::NAN);
        let mut cursor = buf.freeze();
        assert!(Dimension::decode(&mut cursor).is_err());
    }

    #[test]
    fn dimension_decode_rejects_infinity() {
        let mut buf = BytesMut::new();
        buf.put_u8(2); // Percent variant
        buf.put_f32_le(f32::INFINITY);
        let mut cursor = buf.freeze();
        assert!(Dimension::decode(&mut cursor).is_err());
    }

    #[test]
    fn constraints_decode_rejects_nan_flex_grow() {
        use crate::Constraints;

        // Verify we can decode valid constraints
        let mut buf2 = BytesMut::new();
        Constraints::default().encode(&mut buf2);
        let mut cursor = buf2.freeze();
        assert!(Constraints::decode(&mut cursor).is_ok());

        // Now encode with NaN flex_grow via raw construction
        let mut raw = BytesMut::new();
        raw.put_u8(1); // display: Flex
        raw.put_u8(0); // flex_direction: Row
        raw.put_u8(0); // flex_wrap: NoWrap
        raw.put_u8(0); // justify_content: Start
        raw.put_u8(0); // align_items: Start
        raw.put_u8(0); // align_self: Auto
        raw.put_u8(0); // align_content: Start
        raw.put_f32_le(f32::NAN); // flex_grow = NaN
        raw.put_f32_le(1.0); // flex_shrink
                             // flex_basis = Auto
        raw.put_u8(0);
        // width, height, min_*, max_* = Auto
        for _ in 0..6 {
            raw.put_u8(0);
        }
        // aspect_ratio = None
        raw.put_u8(0);
        // margin edges (4x Auto)
        for _ in 0..4 {
            raw.put_u8(0);
        }
        // padding edges (4x Auto)
        for _ in 0..4 {
            raw.put_u8(0);
        }
        // gap (2x Auto)
        raw.put_u8(0);
        raw.put_u8(0);
        // overflow (2x Visible)
        raw.put_u8(0);
        raw.put_u8(0);
        // grid fields
        raw.put_u32_le(0); // template_columns len
        raw.put_u32_le(0); // template_rows len
                           // grid_column placement
        raw.put_u8(0);
        raw.put_u8(0);
        // grid_row placement
        raw.put_u8(0);
        raw.put_u8(0);

        let mut cursor = raw.freeze();
        let result = Constraints::decode(&mut cursor);
        assert!(result.is_err(), "NaN flex_grow should be rejected");
    }
}
