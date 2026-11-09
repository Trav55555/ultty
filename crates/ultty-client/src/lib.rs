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

#[cfg(not(unix))]
compile_error!(
    "ultty-client currently uses Unix sockets. Windows support requires a named-pipe native transport."
);

pub mod detect;
pub mod in_band;

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use bytes::{Buf, BytesMut};
use ultty_protocol::{
    negotiate_protocol_version, AppMessage, Constraints, Content, Decode, Encode, FrameId,
    RegionId, Style, TerminalMessage, CAP_INLINE_SURFACES, CLIENT_DEFAULT_CAPABILITIES,
    FRAME_MAGIC, MAX_SURFACE_ROWS, MIN_PROTOCOL_VERSION, MIN_SURFACE_ROWS, PROTOCOL_VERSION,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How long [`UlttyClient::end_frame`] and the handshake wait for the
/// terminal's reply before giving up.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors returned by [`UlttyClient`] operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No Ultty terminal is reachable: this process is not on the tab that
    /// owns `ULTTY_SOCKET`, and the terminal did not answer the in-band probe.
    #[error("not running in an Ultty terminal")]
    NotUltty,

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

    /// The terminal did not answer within [`REPLY_TIMEOUT`].
    #[error("timed out waiting for {0} from the terminal")]
    Timeout(&'static str),

    /// The request is not one the terminal or the protocol accepts, such as
    /// an inline surface on a terminal that did not offer
    /// `CAP_INLINE_SURFACES`. Nothing was written.
    #[error("invalid request: {0}")]
    Invalid(String),
}

/// A client for the ultty native protocol.
///
/// Provides methods for connecting to the terminal, managing regions,
/// setting content, handling frames, and polling events.
pub struct UlttyClient {
    link: Link,
    read_buf: BytesMut,
    /// Messages received while waiting for a specific reply; drained by
    /// [`UlttyClient::poll_events`] so nothing is lost.
    pending_events: VecDeque<TerminalMessage>,
    session_id: u64,
    root_region: RegionId,
    viewport_width: u16,
    viewport_height: u16,
    negotiated_capabilities: u32,
    negotiated_version: u16,
    next_region_id: u32,
    frame_id: u32,
    /// The app's own output (its stdout), where inline-surface anchors and
    /// their fallback text go on the socket transport. In-band, the tty
    /// carries both, so this is unused.
    text_out: Box<dyn Write + Send>,
    /// The terminal hung up: a read found the end of the stream.
    closed: bool,
    /// In-band, what the terminal sent that is not protocol data: keys.
    input: Vec<u8>,
}

/// How the client reaches the terminal.
enum Link {
    /// The tab's protocol socket.
    Socket(UnixStream),
    /// In-band strings on this process's own terminal. Boxed: it holds
    /// terminal settings and buffers the socket path never uses.
    InBand(Box<in_band::InBandTty>),
}

impl UlttyClient {
    /// Connect to the Ultty terminal this process runs on and perform the
    /// handshake.
    ///
    /// Uses the tab's socket when this process runs on the tab that owns
    /// `ULTTY_SOCKET`, and otherwise the in-band transport, which also works
    /// over ssh and under `script` ([`detect`] has the rules). Fails with
    /// [`Error::NotUltty`] when neither reaches Ultty.
    pub fn connect() -> Result<Self, Error> {
        if let Some(socket_path) = detect::socket_path() {
            return Self::connect_to(socket_path);
        }
        if detect::in_band_candidate() {
            return Self::connect_in_band();
        }
        Err(Error::NotUltty)
    }

    /// Connect in-band through this process's own terminal (stdin and
    /// stdout), which stays in raw mode until the client is dropped.
    pub fn connect_in_band() -> Result<Self, Error> {
        Self::connect_in_band_on(in_band::InBandTty::open()?)
    }

    /// Connect in-band through `tty`, a terminal already opened with
    /// [`in_band::InBandTty::open_on`].
    pub fn connect_in_band_on(mut tty: in_band::InBandTty) -> Result<Self, Error> {
        let mut received = Vec::new();
        if !in_band::probe(&mut tty, &mut received, in_band::PROBE_TIMEOUT)? {
            return Err(Error::NotUltty);
        }
        let mut client = Self::with_link(Link::InBand(Box::new(tty)));
        client.read_buf.extend_from_slice(&received);
        client.await_session()?;
        Ok(client)
    }

    /// Connect to the ultty terminal listening on `socket_path`.
    pub fn connect_to(socket_path: impl AsRef<Path>) -> Result<Self, Error> {
        let socket_path = socket_path.as_ref();
        tracing::info!("Connecting to {}", socket_path.display());
        let stream = UnixStream::connect(socket_path)?;
        Self::from_stream(stream)
    }

    /// Build a client on an already-connected stream and perform the handshake.
    fn from_stream(stream: UnixStream) -> Result<Self, Error> {
        stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
        let mut client = Self::with_link(Link::Socket(stream));
        client.handshake()?;
        Ok(client)
    }

    fn with_link(link: Link) -> Self {
        Self {
            link,
            read_buf: BytesMut::with_capacity(4096),
            pending_events: VecDeque::new(),
            session_id: 0,
            root_region: RegionId::ROOT,
            viewport_width: 80,
            viewport_height: 24,
            negotiated_capabilities: 0,
            negotiated_version: PROTOCOL_VERSION,
            next_region_id: 1,
            frame_id: 0,
            text_out: Box::new(std::io::stdout()),
            closed: false,
            input: Vec::new(),
        }
    }

    /// Whether the client talks in-band rather than over the socket.
    pub fn is_in_band(&self) -> bool {
        matches!(self.link, Link::InBand(_))
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

    /// Returns whether the negotiated session supports a capability bit.
    pub fn supports_capability(&self, capability: u32) -> bool {
        self.negotiated_capabilities & capability != 0
    }

    /// The protocol version negotiated with the terminal
    /// (`min(client, server)`).
    pub fn protocol_version(&self) -> u16 {
        self.negotiated_version
    }

    /// Whether the terminal has hung up: a read (in [`UlttyClient::poll_events`]
    /// or while waiting for a reply) found the end of the stream. Nothing
    /// sent from now on arrives.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Bytes typed on the terminal that the client read while looking for
    /// protocol data (in-band only), in the order they came, after reading
    /// whatever is available now. In-band the client reads the app's own
    /// tty, so while it is connected the app reads its keys here rather than
    /// from stdin. Over the socket this is always empty.
    pub fn take_input(&mut self) -> Vec<u8> {
        if self.is_in_band() {
            if let Err(err) = self.read_available(None) {
                tracing::debug!("reading the terminal for input: {err}");
            }
        }
        std::mem::take(&mut self.input)
    }

    /// In-band, tell the terminal the app is alive if it has been quiet for
    /// [`in_band::HEARTBEAT_INTERVAL`], without reading the tty (so no
    /// typed input is consumed). Over the socket it does nothing: the
    /// terminal pings, and [`UlttyClient::poll_events`] answers.
    pub fn heartbeat(&mut self) -> Result<(), Error> {
        if let Link::InBand(tty) = &self.link {
            if tty.heartbeat_due() {
                self.send(&AppMessage::Pong)?;
            }
        }
        Ok(())
    }

    /// Send a raw [`AppMessage`] to the terminal.
    pub fn send(&mut self, msg: &AppMessage) -> Result<(), Error> {
        let bytes = msg.encode_to_bytes();
        match &mut self.link {
            Link::Socket(stream) => stream.write_all(&bytes)?,
            Link::InBand(tty) => tty.write_frames(&bytes)?,
        }
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
    ///
    /// Any other message that arrives meanwhile is queued for
    /// [`UlttyClient::poll_events`]; `Ping` is answered inline. Returns
    /// [`Error::Timeout`] if no `LayoutComputed` arrives within
    /// [`REPLY_TIMEOUT`].
    pub fn end_frame(&mut self, frame_id: FrameId) -> Result<(), Error> {
        self.send(&AppMessage::EndFrame { frame_id })?;
        self.flush()?;

        let regions = self.wait_for("LayoutComputed", |msg| match msg {
            TerminalMessage::LayoutComputed { regions, .. } => Some(regions),
            _ => None,
        })?;
        tracing::info!("Layout computed for {} regions", regions.len());
        for r in &regions {
            tracing::debug!(
                "  Region {:?}: ({}, {}) {}x{}",
                r.id,
                r.x,
                r.y,
                r.width,
                r.height
            );
        }
        Ok(())
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

    /// Append text to an existing region without replacing its content.
    pub fn append_content(&mut self, id: RegionId, text: impl Into<String>) -> Result<(), Error> {
        self.send(&AppMessage::AppendContent {
            id,
            text: text.into(),
        })
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
    pub fn scroll(&mut self, id: RegionId, x: f32, y: f32, animate: bool) -> Result<(), Error> {
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

    /// Write `bytes` to the app's terminal output, in order with inline
    /// surface anchors and, in-band, with protocol frames.
    ///
    /// Anything the app printed through `std::io::stdout` is flushed first.
    /// In-band, the tty is in raw mode, so a line ends with `\r\n`.
    pub fn write_output(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match &mut self.link {
            Link::InBand(tty) => {
                std::io::stdout().flush()?;
                tty.write_raw(bytes)?;
            }
            Link::Socket(_) => {
                self.text_out.write_all(bytes)?;
                self.text_out.flush()?;
            }
        }
        Ok(())
    }

    /// Send [`UlttyClient::write_output`] text, inline surface anchors and
    /// their fallback text to `out` instead of stdout. Socket transport only:
    /// in-band they must share the tty with the frames.
    pub fn set_output(&mut self, out: Box<dyn Write + Send>) {
        self.text_out = out;
    }

    /// Open an inline surface (spec §10.6, ADR 0020) at the cursor, which
    /// must be at column 0 of the app's terminal.
    ///
    /// Writes the anchor `ESC _ u <session>;<id>;<rows> ESC \` and then
    /// exactly `rows` lines of `fallback` text to the app's output (see
    /// [`fallback_lines`]: missing lines are blank, extra ones dropped, each
    /// clipped to the viewport width), and then sends one frame holding
    /// `CreateSurface` followed by `contents`, the messages that build the
    /// surface's children (`CreateRegion { parent: id, .. }` and so on).
    /// Waits for the frame's `LayoutComputed` like [`UlttyClient::end_frame`].
    ///
    /// The text goes out before the frame on both transports. Over the
    /// socket the two travel separately and the terminal pairs them in either
    /// order; in-band they share the tty, so the anchor precedes the frame.
    /// The terminal answers `SurfaceEvent { kind: Placed }` (or an `Error`),
    /// which [`UlttyClient::poll_events`] returns.
    ///
    /// Fails with [`Error::Invalid`], having written nothing, when the
    /// terminal did not offer `CAP_INLINE_SURFACES`, `id` is the root, or
    /// `rows` is outside `MIN_SURFACE_ROWS..=MAX_SURFACE_ROWS`.
    pub fn open_surface(
        &mut self,
        id: RegionId,
        rows: u16,
        fallback: &[impl AsRef<str>],
        constraints: Constraints,
        style: Style,
        contents: Vec<AppMessage>,
    ) -> Result<(), Error> {
        if !self.supports_capability(CAP_INLINE_SURFACES) {
            return Err(Error::Invalid(
                "the terminal does not offer inline surfaces".into(),
            ));
        }
        if id == RegionId::ROOT {
            return Err(Error::Invalid("a surface cannot use the root id".into()));
        }
        if !(MIN_SURFACE_ROWS..=MAX_SURFACE_ROWS).contains(&rows) {
            return Err(Error::Invalid(format!(
                "surface rows {rows} outside {MIN_SURFACE_ROWS}..={MAX_SURFACE_ROWS}"
            )));
        }
        let mut text = format!("\x1b_u{};{};{}\x1b\\", self.session_id, id.0, rows);
        for line in fallback_lines(fallback, rows, self.viewport_width) {
            text.push_str(&line);
            text.push_str("\r\n");
        }
        self.write_output(text.as_bytes())?;

        let sent = self.send_surface_frame(id, rows, constraints, style, &contents);
        if sent.is_err() {
            // The anchor is out; do not leave the surface, if it got as far
            // as the terminal, to wait for a frame that will not come.
            if let Err(err) = self.send(&AppMessage::CloseSurface { id }) {
                tracing::debug!("closing surface {} after a failed open: {err}", id.0);
            }
        }
        sent
    }

    fn send_surface_frame(
        &mut self,
        id: RegionId,
        rows: u16,
        constraints: Constraints,
        style: Style,
        contents: &[AppMessage],
    ) -> Result<(), Error> {
        let frame = self.begin_frame()?;
        self.send(&AppMessage::CreateSurface {
            id,
            rows,
            constraints,
            style,
        })?;
        for message in contents {
            self.send(message)?;
        }
        self.end_frame(frame)
    }

    /// Close inline surface `id` in a frame of its own: the terminal freezes
    /// it into its rows as text and answers `SurfaceEvent { kind: Closed }`.
    pub fn close_surface(&mut self, id: RegionId) -> Result<(), Error> {
        let frame = self.begin_frame()?;
        self.send(&AppMessage::CloseSurface { id })?;
        self.end_frame(frame)
    }

    /// Disconnect cleanly from the terminal.
    pub fn disconnect(&mut self) -> Result<(), Error> {
        self.send(&AppMessage::Disconnect)
    }

    /// Poll for incoming events (non-blocking).
    ///
    /// Returns messages queued while waiting for a reply first, then anything
    /// currently readable on the socket. `Ping` is answered with `Pong` and
    /// not returned.
    pub fn poll_events(&mut self) -> Result<Vec<TerminalMessage>, Error> {
        if let Link::InBand(tty) = &self.link {
            if tty.heartbeat_due() {
                // An unsolicited Pong tells the terminal an idle app is alive.
                self.send(&AppMessage::Pong)?;
            }
        }
        let mut events: Vec<TerminalMessage> = self.pending_events.drain(..).collect();
        let _ = self.read_available(None);
        while let Some(msg) = self.try_decode()? {
            if self.answer_ping(&msg)? {
                continue;
            }
            events.push(msg);
        }
        Ok(events)
    }

    // ── Private helpers ──────────────────────────────────────────

    fn handshake(&mut self) -> Result<(), Error> {
        self.send(&AppMessage::ClientHello {
            version: PROTOCOL_VERSION,
            requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
        })?;
        self.await_session()
    }

    /// Read `ServerHello` and `SessionReady` after `ClientHello` was sent.
    fn await_session(&mut self) -> Result<(), Error> {
        let (server_version, capabilities) = self.wait_for("ServerHello", |msg| match msg {
            TerminalMessage::ServerHello {
                version,
                capabilities,
            } => Some((version, capabilities)),
            _ => None,
        })?;
        let Some(negotiated_version) = negotiate_protocol_version(PROTOCOL_VERSION, server_version)
        else {
            return Err(Error::Protocol(format!(
                "unsupported server protocol version {} (min {})",
                server_version, MIN_PROTOCOL_VERSION
            )));
        };
        self.negotiated_version = negotiated_version;
        // Only trust capability bits this client knows and requested.
        self.negotiated_capabilities = capabilities & CLIENT_DEFAULT_CAPABILITIES;
        tracing::info!(
            "Server version: {} (negotiated={}), caps=0x{:04x}",
            server_version,
            negotiated_version,
            capabilities
        );

        let (session_id, root_region, width, height) =
            self.wait_for("SessionReady", |msg| match msg {
                TerminalMessage::SessionReady {
                    session_id,
                    root_region,
                    width,
                    height,
                } => Some((session_id, root_region, width, height)),
                _ => None,
            })?;
        self.session_id = session_id;
        self.root_region = root_region;
        self.viewport_width = width;
        self.viewport_height = height;
        tracing::info!(
            "Session ready: id={}, viewport={}x{}",
            session_id,
            width,
            height
        );
        Ok(())
    }

    /// Block until `pick` accepts a message, queuing everything else.
    ///
    /// The socket is switched to blocking mode with the read timeout for the
    /// duration of the wait and back to non-blocking afterwards, so
    /// `poll_events` stays non-blocking.
    fn wait_for<T>(
        &mut self,
        what: &'static str,
        pick: impl Fn(TerminalMessage) -> Option<T>,
    ) -> Result<T, Error> {
        let deadline = Instant::now() + REPLY_TIMEOUT;
        self.set_socket_blocking(true)?;
        let result = self.wait_for_inner(what, deadline, pick);
        self.set_socket_blocking(false)?;
        result
    }

    fn wait_for_inner<T>(
        &mut self,
        what: &'static str,
        deadline: Instant,
        pick: impl Fn(TerminalMessage) -> Option<T>,
    ) -> Result<T, Error> {
        loop {
            while let Some(msg) = self.try_decode()? {
                if self.answer_ping(&msg)? {
                    continue;
                }
                // `pick` consumes the message; keep a copy only for the
                // queued path so the common case does not pay for it.
                let queued = msg.clone();
                match pick(msg) {
                    Some(value) => return Ok(value),
                    None => self.pending_events.push_back(queued),
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::Timeout(what));
            }
            self.read_available(Some(deadline.duration_since(now)))?;
        }
    }

    /// Reply to a keepalive `Ping`. Returns true if `msg` was a ping.
    fn answer_ping(&mut self, msg: &TerminalMessage) -> Result<bool, Error> {
        if matches!(msg, TerminalMessage::Ping) {
            self.send(&AppMessage::Pong)?;
            self.flush()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Read what is available: wait up to `timeout`, or not at all for
    /// `None`. In-band input that is not protocol data is kept for
    /// [`UlttyClient::take_input`].
    fn read_available(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        let stream = match &mut self.link {
            Link::InBand(tty) => {
                let read_buf = &mut self.read_buf;
                let input = &mut self.input;
                let status = tty.read(
                    timeout.unwrap_or(Duration::ZERO),
                    |frames| read_buf.extend_from_slice(frames),
                    |other| input.extend_from_slice(other),
                )?;
                return match status {
                    in_band::ReadStatus::Open => Ok(()),
                    in_band::ReadStatus::Closed => {
                        self.closed = true;
                        Err(Error::ConnectionClosed)
                    }
                };
            }
            Link::Socket(stream) => stream,
        };
        if let Some(timeout) = timeout {
            stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        }
        let mut buf = [0u8; 4096];
        match stream.read(&mut buf) {
            Ok(0) => {
                self.closed = true;
                Err(Error::ConnectionClosed)
            }
            Ok(n) => {
                self.read_buf.extend_from_slice(&buf[..n]);
                Ok(())
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(())
            }
            Err(e) => Err(Error::Io(e)),
        }
    }

    fn flush(&mut self) -> Result<(), Error> {
        if let Link::Socket(stream) = &mut self.link {
            stream.flush()?;
        }
        Ok(())
    }

    /// Socket reads block (with a timeout) while waiting for a reply and are
    /// non-blocking otherwise. The in-band tty is polled with explicit
    /// timeouts instead.
    fn set_socket_blocking(&mut self, blocking: bool) -> Result<(), Error> {
        if let Link::Socket(stream) = &self.link {
            stream.set_nonblocking(!blocking)?;
        }
        Ok(())
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
        if let TerminalMessage::Resize(resize) = &msg {
            // Keep the width fallback text is clipped to current; the
            // message still reaches the caller.
            self.viewport_width = resize.width;
            self.viewport_height = resize.height;
        }
        Ok(Some(msg))
    }

    fn alloc_region(&mut self) -> RegionId {
        let id = RegionId::new(self.next_region_id);
        self.next_region_id += 1;
        id
    }
}

/// The fallback text of an inline surface: exactly `rows` lines, each at
/// most `width` columns wide.
///
/// Lines past `rows` are dropped and missing ones are blank. Text is plain:
/// control characters (escape sequences included, which could otherwise
/// carry a second anchor or move the cursor) are dropped, and a wide
/// character that would cross `width` is left out.
pub fn fallback_lines(fallback: &[impl AsRef<str>], rows: u16, width: u16) -> Vec<String> {
    let width = usize::from(width);
    (0..usize::from(rows))
        .map(|index| {
            let mut line = String::new();
            let mut used = 0;
            let text = fallback.get(index).map(AsRef::as_ref).unwrap_or("");
            let plain: String = text.chars().filter(|ch| !ch.is_control()).collect();
            for cluster in plain.graphemes(true) {
                let w = cluster.width();
                if used + w > width {
                    break;
                }
                used += w;
                line.push_str(cluster);
            }
            line
        })
        .collect()
}

impl Drop for UlttyClient {
    /// End an in-band session at once. A socket session ends when the
    /// socket closes, but the terminal cannot see an in-band app leave
    /// without `Disconnect`.
    fn drop(&mut self) {
        if let Link::InBand(tty) = &mut self.link {
            if let Err(e) = tty.write_frames(&AppMessage::Disconnect.encode_to_bytes()) {
                tracing::debug!("in-band disconnect on drop failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use ultty_protocol::{ComputedRegionLayout, InputEvent, InputType, LayoutTrigger, Modifiers};

    /// A scripted terminal on the other end of a socket pair: replies to the
    /// handshake, then plays back `script` for every `EndFrame` it receives.
    struct FakeTerminal {
        stream: UnixStream,
        received: Vec<AppMessage>,
    }

    impl FakeTerminal {
        fn new(stream: UnixStream) -> Self {
            Self {
                stream,
                received: Vec::new(),
            }
        }

        fn send(&mut self, msg: &TerminalMessage) {
            self.stream.write_all(&msg.encode_to_bytes()).unwrap();
        }

        fn read_one(&mut self) -> AppMessage {
            let mut header = [0u8; 8];
            self.stream.read_exact(&mut header).unwrap();
            let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
            let mut payload = vec![0u8; len];
            self.stream.read_exact(&mut payload).unwrap();
            let mut frame = header.to_vec();
            frame.extend(payload);
            let msg = AppMessage::decode(&mut frame.as_slice()).unwrap();
            self.received.push(msg.clone());
            msg
        }

        fn answer_handshake(&mut self, server_version: u16) {
            assert!(matches!(self.read_one(), AppMessage::ClientHello { .. }));
            self.send(&TerminalMessage::ServerHello {
                version: server_version,
                capabilities: CLIENT_DEFAULT_CAPABILITIES,
            });
            self.send(&TerminalMessage::SessionReady {
                session_id: 7,
                root_region: RegionId::ROOT,
                width: 100,
                height: 30,
            });
        }
    }

    fn layout_reply(frame_id: FrameId) -> TerminalMessage {
        TerminalMessage::LayoutComputed {
            frame_id: Some(frame_id),
            trigger: LayoutTrigger::ContentChange,
            regions: vec![ComputedRegionLayout {
                id: RegionId::ROOT,
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 30.0,
                content_x: 0.0,
                content_y: 0.0,
                content_width: 100.0,
                content_height: 30.0,
                visible: true,
            }],
        }
    }

    fn key_event(c: char) -> TerminalMessage {
        TerminalMessage::Input(InputEvent {
            region_id: RegionId::ROOT,
            event_type: InputType::KeyDown {
                key: ultty_protocol::Key::Char(c),
                modifiers: Modifiers::NONE,
            },
            x: 0.0,
            y: 0.0,
            modifiers: Modifiers::NONE,
        })
    }

    /// Spawn a fake terminal that runs `script` after the handshake and
    /// return the connected client.
    fn connect_with(
        server_version: u16,
        script: impl FnOnce(&mut FakeTerminal) + Send + 'static,
    ) -> (UlttyClient, thread::JoinHandle<Vec<AppMessage>>) {
        let (client_side, server_side) = UnixStream::pair().unwrap();
        let handle = thread::spawn(move || {
            let mut terminal = FakeTerminal::new(server_side);
            terminal.answer_handshake(server_version);
            script(&mut terminal);
            terminal.received
        });
        let client = UlttyClient::from_stream(client_side).unwrap();
        (client, handle)
    }

    #[test]
    fn handshake_stores_negotiated_version_as_min_of_client_and_server() {
        let (client, handle) = connect_with(PROTOCOL_VERSION + 5, |_| {});
        assert_eq!(client.protocol_version(), PROTOCOL_VERSION);
        assert_eq!(client.session_id(), 7);
        assert_eq!(
            (client.viewport_width(), client.viewport_height()),
            (100, 30)
        );
        drop(client);
        handle.join().unwrap();

        let (client, handle) = connect_with(MIN_PROTOCOL_VERSION, |_| {});
        assert_eq!(client.protocol_version(), MIN_PROTOCOL_VERSION);
        drop(client);
        handle.join().unwrap();
    }

    #[test]
    fn handshake_rejects_server_below_minimum_version() {
        let (client_side, server_side) = UnixStream::pair().unwrap();
        let handle = thread::spawn(move || {
            let mut terminal = FakeTerminal::new(server_side);
            assert!(matches!(
                terminal.read_one(),
                AppMessage::ClientHello { .. }
            ));
            // The client rejects this and hangs up, so nothing may follow it:
            // a SessionReady written after the hangup fails with EPIPE.
            terminal.send(&TerminalMessage::ServerHello {
                version: MIN_PROTOCOL_VERSION.saturating_sub(1),
                capabilities: CLIENT_DEFAULT_CAPABILITIES,
            });
        });
        let err = match UlttyClient::from_stream(client_side) {
            Ok(_) => panic!("handshake must fail for an unsupported server version"),
            Err(err) => err,
        };
        assert!(matches!(err, Error::Protocol(_)), "got {:?}", err);
        handle.join().unwrap();
    }

    #[test]
    fn end_frame_queues_other_messages_and_answers_ping() {
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, |terminal| {
            // BeginFrame, EndFrame arrive; reply with noise before the layout.
            assert!(matches!(terminal.read_one(), AppMessage::BeginFrame { .. }));
            assert!(matches!(terminal.read_one(), AppMessage::EndFrame { .. }));
            terminal.send(&key_event('a'));
            terminal.send(&TerminalMessage::Ping);
            terminal.send(&key_event('b'));
            terminal.send(&layout_reply(FrameId(1)));
            // The client must have answered the ping inline.
            assert_eq!(terminal.read_one(), AppMessage::Pong);
            terminal.send(&key_event('c'));
        });

        let frame = client.begin_frame().unwrap();
        client.end_frame(frame).unwrap();

        // Give the fake terminal a moment to send the trailing key.
        let received = handle.join().unwrap();
        assert!(received.contains(&AppMessage::Pong));

        let events = client.poll_events().unwrap();
        assert_eq!(
            events,
            vec![key_event('a'), key_event('b'), key_event('c')],
            "queued messages come out in order, ping is consumed"
        );
    }

    #[test]
    fn end_frame_times_out_instead_of_spinning() {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, move |terminal| {
            assert!(matches!(terminal.read_one(), AppMessage::BeginFrame { .. }));
            // Never reply; keep the socket open until the client gave up.
            let _ = release_rx.recv();
        });

        client.begin_frame().unwrap();
        let started = Instant::now();
        // Shorten the wait for the test by driving the inner loop directly.
        client.set_socket_blocking(true).unwrap();
        let err = client
            .wait_for_inner(
                "LayoutComputed",
                Instant::now() + Duration::from_millis(200),
                |msg| match msg {
                    TerminalMessage::LayoutComputed { .. } => Some(()),
                    _ => None,
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, Error::Timeout("LayoutComputed")),
            "got {:?}",
            err
        );
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(
            started.elapsed() < REPLY_TIMEOUT,
            "must not spin until the full timeout"
        );
        release_tx.send(()).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn poll_events_answers_ping_without_returning_it() {
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, |terminal| {
            terminal.send(&TerminalMessage::Ping);
            terminal.send(&key_event('z'));
            assert_eq!(terminal.read_one(), AppMessage::Pong);
        });
        // Wait until the fake terminal has written and observed the pong.
        let mut events = Vec::new();
        let deadline = Instant::now() + REPLY_TIMEOUT;
        while events.is_empty() && Instant::now() < deadline {
            events = client.poll_events().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(events, vec![key_event('z')]);
        handle.join().unwrap();
    }

    /// A `Write` the test can read back: the app's stdout on the socket path.
    #[derive(Clone, Default)]
    struct SharedOutput(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl SharedOutput {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedOutput {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn surface_child(surface: RegionId) -> AppMessage {
        AppMessage::CreateRegion {
            id: RegionId::new(901),
            parent: surface,
            insert_index: -1,
            constraints: Constraints::default(),
            style: Style::default(),
        }
    }

    #[test]
    fn open_surface_writes_anchor_and_fallback_before_its_frame() {
        let surface = RegionId::new(900);
        let output = SharedOutput::default();
        let seen_by_terminal = output.clone();
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, move |terminal| {
            assert!(matches!(terminal.read_one(), AppMessage::BeginFrame { .. }));
            // The text was written before any of the frame was sent.
            assert_eq!(
                seen_by_terminal.text(),
                "\x1b_u7;900;3\x1b\\card\r\nline two\r\n\r\n"
            );
            assert!(matches!(
                terminal.read_one(),
                AppMessage::CreateSurface { id, rows: 3, .. } if id == RegionId::new(900)
            ));
            assert_eq!(terminal.read_one(), surface_child(RegionId::new(900)));
            assert_eq!(
                terminal.read_one(),
                AppMessage::EndFrame {
                    frame_id: FrameId(1)
                }
            );
            terminal.send(&TerminalMessage::SurfaceEvent {
                id: RegionId::new(900),
                kind: ultty_protocol::SurfaceEventKind::Placed,
            });
            terminal.send(&layout_reply(FrameId(1)));

            assert!(matches!(terminal.read_one(), AppMessage::BeginFrame { .. }));
            assert_eq!(
                terminal.read_one(),
                AppMessage::CloseSurface {
                    id: RegionId::new(900)
                }
            );
            assert!(matches!(terminal.read_one(), AppMessage::EndFrame { .. }));
            terminal.send(&layout_reply(FrameId(2)));
        });
        client.set_output(Box::new(output.clone()));

        client
            .open_surface(
                surface,
                3,
                &["card", "line two"],
                Constraints::default(),
                Style::default(),
                vec![surface_child(surface)],
            )
            .unwrap();
        client.close_surface(surface).unwrap();
        handle.join().unwrap();
        assert_eq!(
            client.poll_events().unwrap(),
            vec![TerminalMessage::SurfaceEvent {
                id: surface,
                kind: ultty_protocol::SurfaceEventKind::Placed,
            }],
            "surface events reach the caller"
        );
    }

    #[test]
    fn open_surface_refuses_without_the_capability_and_writes_nothing() {
        let output = SharedOutput::default();
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, |_| {});
        client.set_output(Box::new(output.clone()));
        client.negotiated_capabilities &= !CAP_INLINE_SURFACES;
        let err = client
            .open_surface(
                RegionId::new(5),
                2,
                &["x"],
                Constraints::default(),
                Style::default(),
                Vec::new(),
            )
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "got {err:?}");
        client.negotiated_capabilities |= CAP_INLINE_SURFACES;
        for (id, rows) in [
            (RegionId::ROOT, 2),
            (RegionId::new(5), 0),
            (RegionId::new(5), 129),
        ] {
            let err = client
                .open_surface(
                    id,
                    rows,
                    &["x"],
                    Constraints::default(),
                    Style::default(),
                    Vec::new(),
                )
                .unwrap_err();
            assert!(
                matches!(err, Error::Invalid(_)),
                "{id:?} {rows}: got {err:?}"
            );
        }
        assert_eq!(output.text(), "");
        drop(client);
        handle.join().unwrap();
    }

    #[test]
    fn fallback_lines_are_exactly_rows_lines_clipped_to_width() {
        assert_eq!(
            fallback_lines(&["abcdef", "xy", "dropped"], 2, 4),
            vec!["abcd".to_string(), "xy".to_string()]
        );
        assert_eq!(
            fallback_lines(&["one"], 3, 10),
            vec!["one".to_string(), String::new(), String::new()]
        );
        // A wide character that would cross the edge is left out, and escape
        // sequences cannot ride along to move the cursor or anchor again.
        assert_eq!(
            fallback_lines(&["ab\u{4e16}"], 1, 3),
            vec!["ab".to_string()]
        );
        assert_eq!(
            fallback_lines(&["a\x1b_u1;2;3\x1b\\b\tc\r"], 1, 20),
            vec!["a_u1;2;3\\bc".to_string()]
        );
    }

    #[test]
    fn fallback_lines_measure_grapheme_clusters() {
        // A heart with the emoji presentation selector is two columns wide,
        // though its characters add up to one.
        assert_eq!(
            fallback_lines(&["\u{2764}\u{fe0f}x"], 1, 2),
            vec!["\u{2764}\u{fe0f}".to_string()]
        );
    }

    #[test]
    fn a_hang_up_is_noticed() {
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, |_| {});
        handle.join().unwrap(); // the fake terminal's socket closes
        assert!(!client.is_closed());
        let deadline = Instant::now() + REPLY_TIMEOUT;
        while !client.is_closed() && Instant::now() < deadline {
            client.poll_events().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(client.is_closed());
    }

    #[test]
    fn resize_updates_the_width_fallback_text_is_clipped_to() {
        let (mut client, handle) = connect_with(PROTOCOL_VERSION, |terminal| {
            terminal.send(&TerminalMessage::Resize(ultty_protocol::ResizeEvent {
                width: 42,
                height: 10,
            }));
        });
        handle.join().unwrap();
        let deadline = Instant::now() + REPLY_TIMEOUT;
        while client.viewport_width() != 42 && Instant::now() < deadline {
            client.poll_events().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            (client.viewport_width(), client.viewport_height()),
            (42, 10)
        );
    }

    /// What a fake in-band terminal saw after the handshake: the messages
    /// the client sent, everything it wrote to the tty, and the pty master
    /// (kept open so the slave does not hang up).
    type InBandSeen = (Vec<AppMessage>, Vec<u8>, std::fs::File);

    /// Run a client in-band on a pty against a fake Ultty that answers the
    /// probe and handshake (session 9, 120 x 40), then answers every
    /// `EndFrame` with `LayoutComputed`, until the client sends `Disconnect`.
    fn in_band_client_on_fake_terminal(use_client: impl FnOnce(&mut UlttyClient)) -> InBandSeen {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use ultty_protocol::in_band::{decode_payload, encode_to_vec, Piece, Splitter};

        let (mut master, slave) = {
            let (mut m, mut s) = (0, 0);
            // SAFETY: openpty writes two fds; the other arguments may be null.
            let rc = unsafe {
                libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(rc, 0, "openpty");
            // SAFETY: both fds were just opened and are owned here.
            unsafe {
                (
                    std::fs::File::from(OwnedFd::from_raw_fd(m)),
                    OwnedFd::from_raw_fd(s),
                )
            }
        };
        let fake = thread::spawn(move || {
            // Answer the probe the way Ultty does: its name, then frames
            // before DA1.
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while !seen.windows(3).any(|w| w == b"\x1b[c") {
                let n = master.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
            }
            master
                .write_all(b"\x1bP>|ultty 0.1.0\x1b\\\x1b[?62;22c")
                .unwrap();
            while seen.windows(3).filter(|w| w == b"\x1b[c").count() < 2 {
                let n = master.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
            }
            let mut reply = Vec::new();
            for msg in [
                TerminalMessage::ServerHello {
                    version: PROTOCOL_VERSION,
                    capabilities: CLIENT_DEFAULT_CAPABILITIES,
                },
                TerminalMessage::SessionReady {
                    session_id: 9,
                    root_region: RegionId::ROOT,
                    width: 120,
                    height: 40,
                },
            ] {
                reply.extend_from_slice(&msg.encode_to_bytes());
            }
            let mut wire = encode_to_vec(&reply);
            wire.extend_from_slice(b"\x1b[?62;22c");
            master.write_all(&wire).unwrap();

            // Collect what the client sends until Disconnect.
            let mut splitter = Splitter::default();
            let mut frames = Vec::new();
            let mut raw = Vec::new();
            let mut got = Vec::new();
            while !got.iter().any(|m| matches!(m, AppMessage::Disconnect)) {
                let n = master.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
                splitter.feed(&buf[..n], |piece| {
                    if let Piece::Payload(text) = piece {
                        decode_payload(text, &mut frames).unwrap();
                    }
                });
                let mut cursor = &frames[..];
                let mut now = Vec::new();
                while let Ok(msg) = AppMessage::decode(&mut cursor) {
                    now.push(msg);
                }
                for msg in &now[got.len()..] {
                    if let AppMessage::EndFrame { frame_id } = msg {
                        let reply = layout_reply(*frame_id).encode_to_bytes();
                        master.write_all(&encode_to_vec(&reply)).unwrap();
                    }
                }
                got = now;
            }
            (got, raw, master)
        });

        let tty = in_band::InBandTty::open_on(slave.as_raw_fd(), slave.as_raw_fd()).unwrap();
        let mut client = UlttyClient::connect_in_band_on(tty).unwrap();
        use_client(&mut client);
        drop(client); // sends Disconnect
        fake.join().unwrap()
    }

    #[test]
    fn in_band_client_handshakes_and_sends_frames_on_its_terminal() {
        let (got, _raw, _master) = in_band_client_on_fake_terminal(|client| {
            assert!(client.is_in_band());
            assert_eq!(client.session_id(), 9);
            assert_eq!(
                (client.viewport_width(), client.viewport_height()),
                (120, 40)
            );
            client.subscribe(RegionId::ROOT, 1).unwrap();
        });
        assert!(
            matches!(got.first(), Some(AppMessage::Subscribe { .. })),
            "got {got:?}"
        );
        assert!(matches!(got.last(), Some(AppMessage::Disconnect)));
    }

    #[test]
    fn in_band_open_surface_puts_anchor_and_fallback_before_the_frame() {
        let surface = RegionId::new(900);
        let (got, raw, _master) = in_band_client_on_fake_terminal(|client| {
            client
                .open_surface(
                    surface,
                    2,
                    &["card"],
                    Constraints::default(),
                    Style::default(),
                    vec![surface_child(surface)],
                )
                .unwrap();
        });
        // One tty stream: the anchor, its two lines (raw mode, so CR LF),
        // and only then the frame's in-band string.
        let text = b"\x1b_u9;900;2\x1b\\card\r\n\r\n\x1b_U";
        let at = raw
            .windows(text.len())
            .position(|w| w == text)
            .unwrap_or_else(|| panic!("no anchor then frame in {raw:?}"));
        assert!(
            !raw[..at].windows(3).any(|w| w == b"\x1b_U"),
            "a frame went out before the anchor"
        );
        assert!(matches!(
            got.as_slice(),
            [
                AppMessage::BeginFrame { .. },
                AppMessage::CreateSurface { rows: 2, .. },
                AppMessage::CreateRegion { .. },
                AppMessage::EndFrame { .. },
                AppMessage::Disconnect,
            ]
        ));
    }
}
