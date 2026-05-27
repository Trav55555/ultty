//! Shared protocol frame header encoding and decoding.

use bytes::{Buf, BufMut, BytesMut};

use crate::{ProtocolError, Result, FRAME_MAGIC};

pub(super) const FRAME_HEADER_LEN: usize = 8;

pub(super) struct FrameStart {
    header_pos: usize,
    payload_start: usize,
}

pub(super) struct FrameHeader {
    pub(super) type_code: u16,
    pub(super) payload_len: usize,
}

pub(super) fn begin_frame(buf: &mut BytesMut) -> FrameStart {
    let header_pos = buf.len();
    buf.put_slice(&FRAME_MAGIC);
    buf.put_u16_le(0);
    buf.put_u32_le(0);

    FrameStart {
        header_pos,
        payload_start: buf.len(),
    }
}

pub(super) fn finish_frame(buf: &mut BytesMut, frame: FrameStart, type_code: u16) {
    let payload_len = (buf.len() - frame.payload_start) as u32;
    let header = &mut buf[frame.header_pos..frame.payload_start];
    header[2..4].copy_from_slice(&type_code.to_le_bytes());
    header[4..8].copy_from_slice(&payload_len.to_le_bytes());
}

pub(super) fn decode_frame_header(
    buf: &mut impl Buf,
    max_payload_len: usize,
) -> Result<FrameHeader> {
    if buf.remaining() < FRAME_HEADER_LEN {
        return Err(ProtocolError::UnexpectedEof {
            needed: FRAME_HEADER_LEN,
            have: buf.remaining(),
        });
    }

    let magic = buf.get_u16_le();
    if magic != u16::from_le_bytes(FRAME_MAGIC) {
        return Err(ProtocolError::InvalidMagic(magic));
    }

    let type_code = buf.get_u16_le();
    let payload_len = buf.get_u32_le() as usize;
    if payload_len > max_payload_len {
        return Err(ProtocolError::InvalidData(format!(
            "payload length {} exceeds maximum {}",
            payload_len, max_payload_len
        )));
    }

    if buf.remaining() < payload_len {
        return Err(ProtocolError::UnexpectedEof {
            needed: payload_len,
            have: buf.remaining(),
        });
    }

    Ok(FrameHeader {
        type_code,
        payload_len,
    })
}
