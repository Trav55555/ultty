//! Native backend I/O (socket or in-band) and terminal-message framing.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::time::Duration;

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

impl NativeBackend {
    /// Reports use the PTY even when native frames use a Unix socket.
    /// The app loop is the sole caller, so in-band bytes cannot splice frames.
    pub(in crate::backend) fn write_status(&mut self, wire: &[u8]) -> crate::Result<()> {
        if let Some(tty) = self.in_band.as_mut() {
            tty.write_raw(wire)?;
        } else {
            if self.stream.is_none() {
                return Err(crate::Error::NotConnected);
            }
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(wire)?;
            stdout.flush()?;
        }
        Ok(())
    }

    pub(super) fn send_message(&mut self, msg: &AppMessage) -> crate::Result<()> {
        let bytes = msg.encode_to_bytes();
        if let Some(tty) = self.in_band.as_mut() {
            tty.write_frames(&bytes)?;
            return Ok(());
        }
        let stream = self.stream.as_mut().ok_or(crate::Error::NotConnected)?;
        stream.write_all(&bytes)?;
        Ok(())
    }

    /// Whether a socket or in-band link is up.
    pub(super) fn is_connected(&self) -> bool {
        self.stream.is_some() || self.in_band.is_some()
    }

    /// Wait up to `timeout` for input, then read everything available.
    /// Returns `Closed` on EOF instead of an error so callers can turn a
    /// disconnect into an orderly quit.
    pub(super) fn read_available(&mut self, timeout: Duration) -> crate::Result<ReadOutcome> {
        if let Some(tty) = self.in_band.as_mut() {
            let read_buf = &mut self.read_buf;
            let status = tty.read(
                timeout,
                |frames| read_buf.extend_from_slice(frames),
                |other| tracing::trace!("ignoring {} non-protocol bytes on stdin", other.len()),
            )?;
            return Ok(match status {
                ultty_client::in_band::ReadStatus::Open => ReadOutcome::Open,
                ultty_client::in_band::ReadStatus::Closed => ReadOutcome::Closed,
            });
        }
        let stream = self.stream.as_mut().ok_or(crate::Error::NotConnected)?;

        let mut buf = [0u8; 4096];
        let mut wait = timeout;
        loop {
            // The socket blocks (for writes); read only what has arrived.
            if !crate::wake::poll_readable(&[stream.as_raw_fd()], Some(wait))? {
                break;
            }
            wait = Duration::ZERO;
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
    fn test_status_and_large_native_frame_serialize_on_real_pty() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use ultty_client::in_band::InBandTty;
        use ultty_client::program_status::{Report, State};
        use ultty_protocol::{Content, TextContent};

        let (mut master_fd, mut slave_fd) = (0, 0);
        // SAFETY: openpty fills two owned fds; optional arguments may be null.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master_fd,
                    &mut slave_fd,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: the descriptors were just created and are owned here.
        let (mut master, slave) = unsafe {
            (
                std::fs::File::from(OwnedFd::from_raw_fd(master_fd)),
                OwnedFd::from_raw_fd(slave_fd),
            )
        };
        let tty = InBandTty::open_on(slave.as_raw_fd(), slave.as_raw_fd()).unwrap();
        let frame = AppMessage::SetContent {
            id: RegionId::new(1),
            content: Content::Text(TextContent::new("x".repeat(70_000))),
        };
        let status = Report::new(State::Blocked).encode().unwrap();
        let encoded_frame = ultty_protocol::in_band::encode_to_vec(&frame.encode_to_bytes());
        let mut expected = encoded_frame.clone();
        expected.extend_from_slice(&status);
        expected.extend_from_slice(&encoded_frame);
        let length = expected.len();
        let reader = std::thread::spawn(move || {
            let mut bytes = vec![0; length];
            master.read_exact(&mut bytes).unwrap();
            bytes
        });
        let mut backend = NativeBackend::new_in_band();
        backend.in_band = Some(Box::new(tty));
        backend.send_message(&frame).unwrap();
        backend.write_status(&status).unwrap();
        backend.send_message(&frame).unwrap();
        assert_eq!(reader.join().unwrap(), expected);
        drop(backend);
        drop(slave);
    }

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

        assert_eq!(
            backend.read_available(std::time::Duration::ZERO).unwrap(),
            ReadOutcome::Closed
        );
    }

    #[test]
    fn disconnect_becomes_transport_closed_instead_of_busy_loop() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(reader);
        writer
            .write_all(&TerminalMessage::Ping.encode_to_bytes())
            .unwrap();
        drop(writer);

        // Bytes written before the close are still processed, then the
        // disconnect surfaces as a single TransportClosed and the stream is released.
        assert!(matches!(
            backend.poll_event().unwrap(),
            Some(Event::TransportClosed)
        ));
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

        assert_eq!(
            backend.read_available(std::time::Duration::ZERO).unwrap(),
            ReadOutcome::Open
        );

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
