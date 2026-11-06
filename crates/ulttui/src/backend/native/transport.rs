//! Native backend I/O (socket or in-band) and terminal-message framing.

use std::io::{Read, Write};

use bytes::Buf;
use ultty_protocol::{AppMessage, Decode, Encode, TerminalMessage, FRAME_MAGIC};

use super::NativeBackend;

const FRAME_HEADER_LEN: usize = 8;

/// Result of draining the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadOutcome {
    /// The socket is still open (it may or may not have produced bytes).
    Open,
    /// The peer closed the connection; no more bytes will arrive.
    Closed,
}

/// Framing failure while decoding the next terminal message.
#[derive(Debug, thiserror::Error)]
pub(super) enum FrameError {
    /// The frame header was valid but the payload did not decode. The frame
    /// has already been discarded, so decoding can continue with the next one.
    #[error("malformed frame (type {msg_type:#06x}, {payload_len} bytes): {source}")]
    Malformed {
        msg_type: u16,
        payload_len: usize,
        #[source]
        source: ultty_protocol::ProtocolError,
    },
    /// The stream is out of sync; there is no way to find the next frame.
    #[error("invalid frame magic: {0:#06x}")]
    BadMagic(u16),
}

/// Whether `buf` starts with a complete frame.
pub(super) fn holds_complete_frame(buf: &[u8]) -> bool {
    buf.len() >= FRAME_HEADER_LEN
        && buf.len()
            >= FRAME_HEADER_LEN + u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize
}

impl NativeBackend {
    pub(super) fn send_message(&mut self, msg: &AppMessage) -> anyhow::Result<()> {
        let bytes = msg.encode_to_bytes();
        if let Some(tty) = self.in_band.as_mut() {
            tty.write_frames(&bytes)?;
            return Ok(());
        }
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;
        stream.write_all(&bytes)?;
        Ok(())
    }

    /// Whether a socket or in-band link is up.
    pub(super) fn is_connected(&self) -> bool {
        self.stream.is_some() || self.in_band.is_some()
    }

    /// Read everything currently available without blocking past the socket's
    /// read timeout. Returns `Closed` on EOF instead of an error so callers can
    /// turn a disconnect into an orderly quit.
    pub(super) fn read_available(&mut self) -> anyhow::Result<ReadOutcome> {
        if let Some(tty) = self.in_band.as_mut() {
            let read_buf = &mut self.read_buf;
            return Ok(tty.read(
                super::POLL_TIMEOUT,
                |frames| read_buf.extend_from_slice(frames),
                |other| tracing::trace!("ignoring {} non-protocol bytes on stdin", other.len()),
            )?);
        }
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return Ok(ReadOutcome::Closed),
                Ok(n) => {
                    self.read_buf.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(ReadOutcome::Open)
    }

    /// Decode the next complete frame, if one is buffered.
    ///
    /// A frame whose payload fails to decode is consumed (its header told us
    /// its length) and reported as `FrameError::Malformed`; the caller can
    /// skip it and keep going.
    pub(super) fn try_decode_message(&mut self) -> Result<Option<TerminalMessage>, FrameError> {
        if self.read_buf.len() < FRAME_HEADER_LEN {
            return Ok(None);
        }

        let magic = u16::from_le_bytes([self.read_buf[0], self.read_buf[1]]);
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            return Err(FrameError::BadMagic(magic));
        }

        let msg_type = u16::from_le_bytes([self.read_buf[2], self.read_buf[3]]);
        let payload_len = u32::from_le_bytes([
            self.read_buf[4],
            self.read_buf[5],
            self.read_buf[6],
            self.read_buf[7],
        ]) as usize;

        let total_len = FRAME_HEADER_LEN + payload_len;
        if self.read_buf.len() < total_len {
            return Ok(None);
        }

        // Decode from a slice bounded to this frame so a short payload can
        // never read into the next frame's bytes.
        let decoded = TerminalMessage::decode(&mut &self.read_buf[..total_len]);
        self.read_buf.advance(total_len);

        match decoded {
            Ok(msg) => Ok(Some(msg)),
            Err(source) => Err(FrameError::Malformed {
                msg_type,
                payload_len,
                source,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    use bytes::{BufMut, BytesMut};
    use ultty_protocol::{AppMessage, Decode, Encode, RegionId, TerminalMessage, FRAME_MAGIC};

    use super::{FrameError, NativeBackend, ReadOutcome};
    use crate::event::Event;

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

        assert!(matches!(err, FrameError::BadMagic(_)));
    }

    /// A frame with a valid header, an unknown message type and two payload
    /// bytes: the header is honest about the length, the payload is garbage.
    fn malformed_frame() -> Vec<u8> {
        let mut buf = BytesMut::new();
        buf.put_slice(&FRAME_MAGIC);
        buf.put_u16_le(0xEEEE);
        buf.put_u32_le(2);
        buf.put_slice(&[0xAB, 0xCD]);
        buf.to_vec()
    }

    #[test]
    fn try_decode_message_skips_malformed_frame_and_continues() {
        let mut backend = NativeBackend::new();
        backend.read_buf.extend_from_slice(&malformed_frame());
        backend
            .read_buf
            .extend_from_slice(&TerminalMessage::Ping.encode_to_bytes());

        let err = backend.try_decode_message().unwrap_err();
        assert!(matches!(
            err,
            FrameError::Malformed {
                msg_type: 0xEEEE,
                payload_len: 2,
                ..
            }
        ));
        // The bad frame is consumed; the following frame is intact.
        assert!(matches!(
            backend.try_decode_message().unwrap(),
            Some(TerminalMessage::Ping)
        ));
        assert!(backend.read_buf.is_empty());
    }

    #[test]
    fn process_pending_messages_survives_malformed_frame() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(reader);
        writer.write_all(&malformed_frame()).unwrap();
        writer
            .write_all(&TerminalMessage::Ping.encode_to_bytes())
            .unwrap();

        backend.process_pending_messages().unwrap();

        assert!(backend.read_buf.is_empty());
        assert!(backend.stream.is_some());
        assert!(backend.poll_event().unwrap().is_none());
    }

    #[test]
    fn read_available_reports_closed_on_eof() {
        let (writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(reader);
        drop(writer);

        assert_eq!(backend.read_available().unwrap(), ReadOutcome::Closed);
    }

    #[test]
    fn disconnect_becomes_quit_event_instead_of_busy_loop() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(reader);
        writer
            .write_all(&TerminalMessage::Ping.encode_to_bytes())
            .unwrap();
        drop(writer);

        // Bytes written before the close are still processed, then the
        // disconnect surfaces as a single Quit and the stream is released.
        assert!(matches!(backend.poll_event().unwrap(), Some(Event::Quit)));
        assert!(backend.stream.is_none());
        assert!(backend.poll_event().unwrap().is_none());
        assert!(backend.shutdown().is_ok());
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

        assert_eq!(backend.read_available().unwrap(), ReadOutcome::Open);

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
