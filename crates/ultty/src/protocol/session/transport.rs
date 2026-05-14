//! Native protocol session socket I/O and framing.

use std::io::{self, Read, Write};

use bytes::Buf;
use tracing::{debug, warn};
use ultty_protocol::{AppMessage, Decode, Encode, TerminalMessage, FRAME_MAGIC};

use super::{NativeSession, MAX_READ_BUF};

impl NativeSession {
    /// Read data from socket into the session read buffer.
    pub(super) fn read_from_socket(&mut self) -> io::Result<usize> {
        let mut temp = [0u8; 4096];
        let n = self.stream.read(&mut temp)?;
        if n > 0 {
            self.read_buf.extend_from_slice(&temp[..n]);
            self.last_activity = std::time::Instant::now();
            if self.read_buf.len() > MAX_READ_BUF {
                warn!(
                    "Session {} read buffer exceeded {} bytes, disconnecting",
                    self.session_id, MAX_READ_BUF
                );
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "read buffer exceeded maximum size",
                ));
            }
        }
        Ok(n)
    }

    /// Try to decode a complete message from the session read buffer.
    pub(super) fn try_decode_message(&mut self) -> io::Result<Option<AppMessage>> {
        if self.read_buf.len() < 8 {
            return Ok(None);
        }

        let magic = u16::from_le_bytes([self.read_buf[0], self.read_buf[1]]);
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            self.resync_read_buffer(magic);
            return Ok(None);
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

        let mut cursor = &self.read_buf[..total_len];
        match AppMessage::decode(&mut cursor) {
            Ok(msg) => {
                self.read_buf.advance(total_len);
                Ok(Some(msg))
            }
            Err(e) => {
                let message = format!("decode error: skipped {} byte message: {}", total_len, e);
                warn!(
                    "Session {} decode error (skipping {} byte message): {}",
                    self.session_id, total_len, e
                );
                self.queue_error_message(400, message);
                self.read_buf.advance(total_len);
                Ok(None)
            }
        }
    }

    fn resync_read_buffer(&mut self, magic: u16) {
        warn!(
            "Session {} invalid magic 0x{:04x}, attempting resync",
            self.session_id, magic
        );

        let magic_bytes = FRAME_MAGIC;
        let scan_limit = self.read_buf.len().saturating_sub(1).min(65536);

        for i in 1..scan_limit {
            if self.read_buf[i] == magic_bytes[0]
                && i + 1 < self.read_buf.len()
                && self.read_buf[i + 1] == magic_bytes[1]
            {
                warn!(
                    "Session {} resync: skipped {} bytes to next magic",
                    self.session_id, i
                );
                self.read_buf.advance(i);
                return;
            }
        }

        let discard_end = scan_limit.min(self.read_buf.len());
        let keep =
            if discard_end < self.read_buf.len() || self.read_buf.last() == Some(&magic_bytes[0]) {
                1
            } else {
                0
            };
        let discard = discard_end - keep;
        self.read_buf.advance(discard);
    }

    /// Queue a terminal message for the client.
    pub(super) fn send_message(&mut self, msg: &TerminalMessage) -> io::Result<()> {
        msg.encode(&mut self.write_buf);
        debug!("Session {} queued message: {:?}", self.session_id, msg);
        Ok(())
    }

    /// Flush pending writes to the session socket.
    pub(super) fn flush_writes(&mut self) -> io::Result<()> {
        while !self.write_buf.is_empty() {
            match self.stream.write(&self.write_buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write to socket",
                    ));
                }
                Ok(n) => {
                    self.write_buf.advance(n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}
