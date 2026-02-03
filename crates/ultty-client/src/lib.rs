//! Client SDK for building native TUI applications on the ultty terminal.
//!
//! This crate provides `UlttyClient`, a high-level interface for connecting to
//! the ultty terminal's native protocol server and building rich TUI layouts.
//!
//! # Example
//!
//! ```no_run
//! use ultty_client::UlttyClient;
//! use ultty_protocol::{Constraints, Content, Style, TextContent, TextWrap, TextAlign, TextOverflow};
//!
//! let mut client = UlttyClient::connect()?;
//! let frame = client.begin_frame()?;
//! let region = client.create_region(client.root_region(), Constraints::default(), Style::default())?;
//! client.set_content(region, Content::Text(TextContent {
//!     text: "Hello!".into(),
//!     wrap: TextWrap::None,
//!     align: TextAlign::Left,
//!     overflow: TextOverflow::Clip,
//! }))?;
//! client.end_frame(frame)?;
//! # Ok::<(), ultty_client::Error>(())
//! ```

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use ultty_protocol::{
    AppMessage, Constraints, Content, Decode, Encode, FrameId, RegionId, Style, TerminalMessage,
    FRAME_MAGIC, PROTOCOL_VERSION,
};

/// Errors returned by [`UlttyClient`] operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The ULTTY_SOCKET environment variable is not set.
    #[error("ULTTY_SOCKET not set - run inside ultty terminal")]
    SocketNotSet,

    /// An I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A protocol decoding error occurred.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The connection was closed by the server.
    #[error("connection closed")]
    ConnectionClosed,

    /// Invalid frame magic received.
    #[error("invalid frame magic")]
    InvalidMagic,
}

/// A client for the ultty native protocol.
///
/// Provides methods for connecting to the terminal, managing regions,
/// setting content, handling frames, and polling events.
pub struct UlttyClient {
    stream: UnixStream,
    read_buf: BytesMut,
    session_id: u64,
    root_region: RegionId,
    viewport_width: u16,
    viewport_height: u16,
    next_region_id: u32,
    frame_id: u32,
}

impl UlttyClient {
    /// Connect to the ultty terminal via the `ULTTY_SOCKET` environment variable.
    ///
    /// Performs the protocol handshake and returns a ready client.
    pub fn connect() -> Result<Self, Error> {
        let socket_path = std::env::var("ULTTY_SOCKET").map_err(|_| Error::SocketNotSet)?;

        tracing::info!("Connecting to {}", socket_path);
        let stream = UnixStream::connect(&socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let mut client = Self {
            stream,
            read_buf: BytesMut::with_capacity(4096),
            session_id: 0,
            root_region: RegionId::ROOT,
            viewport_width: 80,
            viewport_height: 24,
            next_region_id: 1,
            frame_id: 0,
        };

        client.handshake()?;
        Ok(client)
    }

    /// The session ID assigned by the server.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// The root region ID for this session.
    pub fn root_region(&self) -> RegionId {
        self.root_region
    }

    /// The viewport width in cells.
    pub fn viewport_width(&self) -> u16 {
        self.viewport_width
    }

    /// The viewport height in cells.
    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    /// Send a raw [`AppMessage`] to the terminal.
    pub fn send(&mut self, msg: &AppMessage) -> Result<(), Error> {
        let bytes = msg.encode_to_bytes();
        self.stream.write_all(&bytes)?;
        Ok(())
    }

    /// Begin an atomic frame update.
    pub fn begin_frame(&mut self) -> Result<FrameId, Error> {
        self.frame_id += 1;
        let frame_id = FrameId(self.frame_id);
        self.send(&AppMessage::BeginFrame { frame_id })?;
        Ok(frame_id)
    }

    /// End an atomic frame update and wait for the layout computation response.
    pub fn end_frame(&mut self, frame_id: FrameId) -> Result<(), Error> {
        self.send(&AppMessage::EndFrame { frame_id })?;
        self.stream.flush()?;

        self.stream.set_nonblocking(false)?;
        loop {
            self.read_available()?;
            while let Some(msg) = self.try_decode()? {
                if let TerminalMessage::LayoutComputed { regions, .. } = msg {
                    tracing::info!("Layout computed for {} regions", regions.len());
                    for r in &regions {
                        tracing::debug!(
                            "  Region {:?}: ({}, {}) {}x{}",
                            r.id, r.x, r.y, r.width, r.height
                        );
                    }
                    self.stream.set_nonblocking(true)?;
                    return Ok(());
                }
            }
        }
    }

    /// Create a new region as a child of `parent` with the given constraints and style.
    pub fn create_region(
        &mut self,
        parent: RegionId,
        constraints: Constraints,
        style: Style,
    ) -> Result<RegionId, Error> {
        let id = self.alloc_region();
        self.send(&AppMessage::CreateRegion {
            id,
            parent,
            insert_index: -1,
            constraints,
            style,
        })?;
        Ok(id)
    }

    /// Set the content of a region.
    pub fn set_content(&mut self, id: RegionId, content: Content) -> Result<(), Error> {
        self.send(&AppMessage::SetContent { id, content })
    }

    /// Update region constraints.
    pub fn set_constraints(&mut self, id: RegionId, constraints: Constraints) -> Result<(), Error> {
        self.send(&AppMessage::SetConstraints { id, constraints })
    }

    /// Update region style.
    pub fn set_style(&mut self, id: RegionId, style: Style) -> Result<(), Error> {
        self.send(&AppMessage::SetStyle { id, style })
    }

    /// Delete a region.
    pub fn delete_region(&mut self, id: RegionId) -> Result<(), Error> {
        self.send(&AppMessage::DeleteRegion { id })
    }

    /// Move a region to a new parent.
    pub fn reparent_region(
        &mut self,
        id: RegionId,
        new_parent: RegionId,
        insert_index: i32,
    ) -> Result<(), Error> {
        self.send(&AppMessage::ReparentRegion {
            id,
            new_parent,
            insert_index,
        })
    }

    /// Subscribe to events on a region.
    pub fn subscribe(&mut self, id: RegionId, events: u32) -> Result<(), Error> {
        self.send(&AppMessage::Subscribe { id, events })
    }

    /// Unsubscribe from events on a region.
    pub fn unsubscribe(&mut self, id: RegionId, events: u32) -> Result<(), Error> {
        self.send(&AppMessage::Unsubscribe { id, events })
    }

    /// Programmatic scroll on a region.
    pub fn scroll(
        &mut self,
        id: RegionId,
        x: f32,
        y: f32,
        animate: bool,
    ) -> Result<(), Error> {
        self.send(&AppMessage::Scroll { id, x, y, animate })
    }

    /// Set keyboard focus to a region.
    pub fn focus(&mut self, id: RegionId) -> Result<(), Error> {
        self.send(&AppMessage::Focus { id })
    }

    /// Remove keyboard focus.
    pub fn blur(&mut self) -> Result<(), Error> {
        self.send(&AppMessage::Blur)
    }

    /// Request clipboard contents from the terminal.
    pub fn clipboard_get(&mut self) -> Result<(), Error> {
        self.send(&AppMessage::ClipboardGet)
    }

    /// Set clipboard contents in the terminal.
    pub fn clipboard_set(&mut self, text: String) -> Result<(), Error> {
        self.send(&AppMessage::ClipboardSet { text })
    }

    /// Disconnect cleanly from the terminal.
    pub fn disconnect(&mut self) -> Result<(), Error> {
        self.send(&AppMessage::Disconnect)
    }

    /// Poll for incoming events (non-blocking).
    pub fn poll_events(&mut self) -> Result<Vec<TerminalMessage>, Error> {
        let mut events = Vec::new();
        let _ = self.read_available();
        while let Some(msg) = self.try_decode()? {
            events.push(msg);
        }
        Ok(events)
    }

    // ── Private helpers ──────────────────────────────────────────

    fn handshake(&mut self) -> Result<(), Error> {
        self.send(&AppMessage::ClientHello {
            version: PROTOCOL_VERSION,
            requested_capabilities: 0,
        })?;

        loop {
            self.read_available()?;
            while let Some(msg) = self.try_decode()? {
                match msg {
                    TerminalMessage::ServerHello { version, .. } => {
                        tracing::info!("Server version: {}", version);
                    }
                    TerminalMessage::SessionReady {
                        session_id,
                        root_region,
                        width,
                        height,
                    } => {
                        self.session_id = session_id;
                        self.root_region = root_region;
                        self.viewport_width = width;
                        self.viewport_height = height;
                        tracing::info!(
                            "Session ready: id={}, viewport={}x{}",
                            session_id, width, height
                        );
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }

    fn read_available(&mut self) -> Result<(), Error> {
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => Err(Error::ConnectionClosed),
            Ok(n) => {
                self.read_buf.extend_from_slice(&buf[..n]);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    fn try_decode(&mut self) -> Result<Option<TerminalMessage>, Error> {
        if self.read_buf.len() < 8 {
            return Ok(None);
        }

        let magic = u16::from_le_bytes([self.read_buf[0], self.read_buf[1]]);
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            return Err(Error::InvalidMagic);
        }

        let payload_len = u32::from_le_bytes([
            self.read_buf[4],
            self.read_buf[5],
            self.read_buf[6],
            self.read_buf[7],
        ]) as usize;

        let total = 8 + payload_len;
        if self.read_buf.len() < total {
            return Ok(None);
        }

        let msg = TerminalMessage::decode(&mut self.read_buf.as_ref())
            .map_err(|e| Error::Protocol(e.to_string()))?;
        self.read_buf.advance(total);
        Ok(Some(msg))
    }

    fn alloc_region(&mut self) -> RegionId {
        let id = RegionId::new(self.next_region_id);
        self.next_region_id += 1;
        id
    }
}
