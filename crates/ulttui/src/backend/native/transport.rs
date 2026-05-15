//! Native backend socket I/O and terminal-message framing.

use std::io::{Read, Write};

use bytes::Buf;
use ultty_protocol::{AppMessage, Decode, Encode, TerminalMessage, FRAME_MAGIC};

use super::NativeBackend;

impl NativeBackend {
    pub(super) fn send_message(&mut self, msg: &AppMessage) -> anyhow::Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;
        let bytes = msg.encode_to_bytes();
        stream.write_all(&bytes)?;
        Ok(())
    }

    pub(super) fn read_available(&mut self) -> anyhow::Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return Err(anyhow::anyhow!("Connection closed")),
                Ok(n) => {
                    self.read_buf.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    pub(super) fn try_decode_message(&mut self) -> anyhow::Result<Option<TerminalMessage>> {
        if self.read_buf.len() < 8 {
            return Ok(None);
        }

        let magic = u16::from_le_bytes([self.read_buf[0], self.read_buf[1]]);
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            anyhow::bail!("Invalid magic: {:04x}", magic);
        }

        let payload_len = u32::from_le_bytes([
            self.read_buf[4],
            self.read_buf[5],
            self.read_buf[6],
            self.read_buf[7],
        ]) as usize;

        let total_len = 8 + payload_len;
        if self.read_buf.len() < total_len {
            return Ok(None);
        }

        let msg = TerminalMessage::decode(&mut self.read_buf.as_ref())?;
        self.read_buf.advance(total_len);

        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    use bytes::BytesMut;
    use ultty_protocol::{AppMessage, Decode, Encode, RegionId, TerminalMessage};

    use super::NativeBackend;

    #[test]
    fn try_decode_message_waits_for_complete_frame() {
        let mut backend = NativeBackend::new();
        let bytes = TerminalMessage::Ping.encode_to_bytes();

        backend
            .read_buf
            .extend_from_slice(&bytes[..bytes.len() - 1]);

        assert!(backend.try_decode_message().unwrap().is_none());
        assert_eq!(backend.read_buf.len(), bytes.len() - 1);
    }

    #[test]
    fn try_decode_message_decodes_complete_frame() {
        let mut backend = NativeBackend::new();
        backend
            .read_buf
            .extend_from_slice(&TerminalMessage::Ping.encode_to_bytes());

        let decoded = backend.try_decode_message().unwrap();

        assert!(matches!(decoded, Some(TerminalMessage::Ping)));
        assert!(backend.read_buf.is_empty());
    }

    #[test]
    fn try_decode_message_rejects_invalid_magic() {
        let mut backend = NativeBackend::new();
        backend.read_buf = BytesMut::from(&b"badmagic"[..]);

        let err = backend.try_decode_message().unwrap_err();

        assert!(err.to_string().contains("Invalid magic"));
    }

    #[test]
    fn read_available_drains_until_would_block() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(reader);
        writer
            .write_all(&TerminalMessage::Ping.encode_to_bytes())
            .unwrap();

        backend.read_available().unwrap();

        assert!(matches!(
            backend.try_decode_message().unwrap(),
            Some(TerminalMessage::Ping)
        ));
    }

    #[test]
    fn append_content_serializes_append_message() {
        let (mut reader, writer) = UnixStream::pair().unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(writer);
        let expected = AppMessage::AppendContent {
            id: RegionId::new(7),
            text: " tail".to_string(),
        };
        let mut bytes = vec![0; expected.encode_to_bytes().len()];

        backend.append_content(RegionId::new(7), " tail").unwrap();
        reader.read_exact(&mut bytes).unwrap();

        let decoded = AppMessage::decode(&mut &bytes[..]).unwrap();
        assert_eq!(decoded, expected);
    }
}
