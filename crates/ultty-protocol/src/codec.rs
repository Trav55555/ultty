//! Encoding and decoding for protocol messages.
//!
//! TODO: Implement actual binary encoding.
//! This is a placeholder that establishes the trait interface.

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::{AppMessage, ProtocolError, Result, TerminalMessage, FRAME_MAGIC};

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

impl Encode for AppMessage {
    fn encode(&self, buf: &mut BytesMut) {
        // Write magic
        buf.put_slice(&FRAME_MAGIC);

        // Placeholder: write message type
        let type_code: u16 = match self {
            AppMessage::ClientHello { .. } => 0x0001,
            AppMessage::CreateRegion { .. } => 0x0010,
            AppMessage::SetConstraints { .. } => 0x0022,
            AppMessage::SetStyle { .. } => 0x0021,
            AppMessage::SetContent { .. } => 0x0020,
            AppMessage::DeleteRegion { .. } => 0x0012,
            AppMessage::ReparentRegion { .. } => 0x0013,
            AppMessage::BeginFrame { .. } => 0x0030,
            AppMessage::EndFrame { .. } => 0x0031,
            AppMessage::Scroll { .. } => 0x0040,
            AppMessage::Focus { .. } => 0x0041,
            AppMessage::Blur => 0x0042,
            AppMessage::Subscribe { .. } => 0x0050,
            AppMessage::Unsubscribe { .. } => 0x0051,
            AppMessage::Disconnect => 0x00FF,
        };
        buf.put_u16_le(type_code);

        // TODO: Encode payload based on message type
        // For now, just reserve space for length (will be filled in later)
        buf.put_u32_le(0); // payload length placeholder
    }
}

impl Decode for AppMessage {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        // Check minimum size
        if buf.remaining() < 8 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 8,
                have: buf.remaining(),
            });
        }

        // Read and verify magic
        let magic = buf.get_u16_le();
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            return Err(ProtocolError::InvalidMagic(magic));
        }

        // Read message type
        let type_code = buf.get_u16_le();

        // Read payload length
        let _payload_len = buf.get_u32_le();

        // TODO: Decode payload based on message type
        match type_code {
            0x00FF => Ok(AppMessage::Disconnect),
            _ => Err(ProtocolError::UnknownMessageType(type_code)),
        }
    }
}

impl Encode for TerminalMessage {
    fn encode(&self, buf: &mut BytesMut) {
        // Write magic
        buf.put_slice(&FRAME_MAGIC);

        // Write message type
        let type_code: u16 = match self {
            TerminalMessage::ServerHello { .. } => 0x0001,
            TerminalMessage::SessionReady { .. } => 0x0002,
            TerminalMessage::LayoutComputed { .. } => 0x0010,
            TerminalMessage::Input(_) => 0x0020,
            TerminalMessage::Resize(_) => 0x0022,
            TerminalMessage::Focus(_) => 0x0023,
            TerminalMessage::Scroll(_) => 0x0024,
            TerminalMessage::Error { .. } => 0x00FF,
        };
        buf.put_u16_le(type_code);

        // TODO: Encode payload
        buf.put_u32_le(0);
    }
}

impl Decode for TerminalMessage {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        if buf.remaining() < 8 {
            return Err(ProtocolError::UnexpectedEof {
                needed: 8,
                have: buf.remaining(),
            });
        }

        let magic = buf.get_u16_le();
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            return Err(ProtocolError::InvalidMagic(magic));
        }

        let type_code = buf.get_u16_le();
        let _payload_len = buf.get_u32_le();

        // TODO: Decode payload based on message type
        match type_code {
            0x00FF => Ok(TerminalMessage::Error {
                code: 0,
                message: "unknown".into(),
            }),
            _ => Err(ProtocolError::UnknownMessageType(type_code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_disconnect() {
        let msg = AppMessage::Disconnect;
        let bytes = msg.encode_to_bytes();

        let mut cursor = bytes.as_ref();
        let decoded = AppMessage::decode(&mut cursor).unwrap();

        assert_eq!(decoded, AppMessage::Disconnect);
    }
}
