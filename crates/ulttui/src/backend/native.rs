//! Native protocol backend for Ultty terminal.

mod in_band;
mod transport;

use std::collections::{HashMap, VecDeque};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bytes::BytesMut;
use ultty_protocol::Encode;
use ultty_protocol::{
    AppMessage, ComputedRegionLayout, Content, FrameId, InputEvent, InputType, Key, Modifiers,
    RegionId, TerminalMessage, CAP_APPEND_CONTENT, CLIENT_DEFAULT_CAPABILITIES, EVENT_KEY,
    EVENT_MOUSE, EVENT_SCROLL, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};

use crate::event::Event;
use crate::widgets::View;

use super::detect;
use super::native_diff::{
    begin_native_key_recording, optimize_frame_messages, record_native_key, take_native_keys,
    RetainedTree,
};
use transport::{FrameError, ReadOutcome};

/// How long one event poll waits for terminal messages.
const POLL_TIMEOUT: Duration = Duration::from_millis(16);

/// How long the in-band probe waits when the terminal answers neither the
/// probe nor DA1. Terminals answer DA1 at once, so this only matters for
/// ones that do not; set `ULTTY_IN_BAND=0` to skip the probe entirely.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

pub struct NativeBackend {
    stream: Option<UnixStream>,
    /// The in-band link, when the app talks through its own terminal.
    /// Boxed: it holds terminal settings and buffers the socket path never
    /// uses.
    in_band: Option<Box<in_band::InBandTty>>,
    /// `init` should probe for in-band support instead of using the socket.
    prefer_in_band: bool,
    negotiated_capabilities: u32,
    session_id: u64,
    root_region: RegionId,
    viewport_width: u16,
    viewport_height: u16,
    read_buf: BytesMut,
    next_frame_id: u32,
    region_layouts: HashMap<RegionId, ComputedRegionLayout>,
    /// Regions the terminal retains from the last frame, with stable ids.
    retained: RetainedTree,
    pending_events: VecDeque<Event>,
}

impl NativeBackend {
    pub fn new() -> Self {
        Self {
            stream: None,
            in_band: None,
            prefer_in_band: false,
            negotiated_capabilities: 0,
            session_id: 0,
            root_region: RegionId::ROOT,
            viewport_width: 80,
            viewport_height: 24,
            read_buf: BytesMut::with_capacity(4096),
            next_frame_id: 1,
            region_layouts: HashMap::new(),
            retained: RetainedTree::new(),
            pending_events: VecDeque::new(),
        }
    }

    /// A backend that talks in-band through the app's own terminal
    /// (ADR 0018). [`NativeBackend::init`] fails if the terminal does not
    /// answer the in-band probe.
    pub fn new_in_band() -> Self {
        Self {
            prefer_in_band: true,
            ..Self::new()
        }
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        if self.prefer_in_band {
            return self.init_in_band();
        }
        let socket_path =
            detect::socket_path().ok_or_else(|| anyhow::anyhow!("ULTTY_SOCKET not set"))?;

        let stream = UnixStream::connect(&socket_path)?;
        // Start blocking for handshake — read_timeout set after session is ready
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        self.stream = Some(stream);

        self.send_client_hello()?;
        self.wait_for_session_ready()?;
        self.send_message(&AppMessage::Subscribe {
            id: self.root_region,
            events: EVENT_KEY | EVENT_MOUSE | EVENT_SCROLL,
        })?;

        self.pending_events.push_back(Event::Resize {
            width: self.viewport_width,
            height: self.viewport_height,
        });

        // Keep blocking mode for writes; use short read_timeout for the event poll loop
        if let Some(ref stream) = self.stream {
            stream.set_read_timeout(Some(Duration::from_millis(16)))?;
        }

        Ok(())
    }

    /// Probe for in-band support and, if the terminal has it, run the
    /// handshake over the app's own terminal.
    ///
    /// The probe is `ClientHello` followed by DA1. A terminal that speaks
    /// the protocol answers `ServerHello` before its DA1 reply; any other
    /// terminal ignores the APC string and answers only DA1. Inside tmux or
    /// screen the bare string is dropped, so apps there fall back to ANSI,
    /// which is right: the outer terminal cannot know where the pane is.
    fn init_in_band(&mut self) -> anyhow::Result<()> {
        let mut tty = in_band::InBandTty::open()?;
        if !probe_in_band(&mut tty, &mut self.read_buf, PROBE_TIMEOUT)? {
            anyhow::bail!("terminal does not support in-band native mode");
        }

        self.in_band = Some(Box::new(tty));
        self.wait_for_session_ready()?;
        self.send_message(&AppMessage::Subscribe {
            id: self.root_region,
            events: EVENT_KEY | EVENT_MOUSE | EVENT_SCROLL,
        })?;
        self.pending_events.push_back(Event::Resize {
            width: self.viewport_width,
            height: self.viewport_height,
        });
        tracing::info!("native mode over the in-band transport");
        Ok(())
    }

    /// Whether this backend talks in-band rather than over the socket.
    pub fn is_in_band(&self) -> bool {
        self.in_band.is_some()
    }

    /// Whether `init` will probe for in-band support rather than use the
    /// socket.
    pub fn prefers_in_band(&self) -> bool {
        self.prefer_in_band
    }

    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.is_connected() {
            self.send_message(&AppMessage::Disconnect)?;
            self.stream = None;
            self.in_band = None;
        }
        Ok(())
    }

    /// Append text to an existing native region without rebuilding a full frame.
    pub fn append_content(
        &mut self,
        region_id: RegionId,
        text: impl Into<String>,
    ) -> anyhow::Result<()> {
        self.send_message(&AppMessage::AppendContent {
            id: region_id,
            text: text.into(),
        })
    }

    pub fn render(&mut self, view: &impl View) -> anyhow::Result<()> {
        let frame_id = FrameId(self.next_frame_id);
        self.next_frame_id += 1;

        self.send_message(&AppMessage::BeginFrame { frame_id })?;

        let (frame_messages, native_keys) = self.collect_frame(view)?;
        let (optimized_messages, next_tree) = optimize_frame_messages(
            &self.retained,
            frame_messages,
            &native_keys,
            self.supports_capability(CAP_APPEND_CONTENT),
        );
        self.retained = next_tree;

        for msg in optimized_messages {
            self.send_message(&msg)?;
        }

        self.send_message(&AppMessage::EndFrame { frame_id })?;

        self.process_pending_messages()?;

        Ok(())
    }

    /// Walk the view tree into provisional-id messages, recording the native
    /// key of every region created along the way.
    fn collect_frame(
        &self,
        view: &impl View,
    ) -> anyhow::Result<(Vec<AppMessage>, HashMap<RegionId, String>)> {
        begin_native_key_recording();
        let walked = self.render_view_tree(view);
        let keys = take_native_keys();
        walked.map(|messages| (messages, keys))
    }

    fn render_view_tree(&self, view: &impl View) -> anyhow::Result<Vec<AppMessage>> {
        let mut messages = Vec::new();
        let mut next_id = 1u32;
        let region_id = RegionId::new(next_id);
        next_id += 1;

        // Root view fills the entire viewport — use absolute cells since
        // the ROOT parent region has height:auto and percentage heights
        // cannot resolve against an auto-height parent.
        let mut constraints = view.constraints();
        constraints.width = ultty_protocol::Dimension::Cells(self.viewport_width as f32);
        constraints.height = ultty_protocol::Dimension::Cells(self.viewport_height as f32);
        // Opaque background so native app covers the PTY terminal content
        let mut style = view.style();
        if style.bg.is_none() {
            let theme = crate::theme::Theme::current();
            style.bg = Some(theme.background);
        }
        style.opacity = 1.0;

        messages.push(AppMessage::CreateRegion {
            id: region_id,
            parent: self.root_region,
            insert_index: -1,
            constraints,
            style,
        });
        if let Some(key) = view.native_key() {
            record_native_key(region_id, key);
        }

        let content = view.content();
        if content != Content::None {
            messages.push(AppMessage::SetContent {
                id: region_id,
                content,
            });
        }

        if let Some((sx, sy)) = view.scroll_offset() {
            messages.push(AppMessage::Scroll {
                id: region_id,
                x: sx,
                y: sy,
                animate: false,
            });
        }

        let mut sink = |msg: &AppMessage| -> anyhow::Result<()> {
            messages.push(msg.clone());
            Ok(())
        };
        for i in 0..view.children_count() {
            view.render_child_native(i, &mut sink, region_id, &mut next_id)?;
        }

        Ok(messages)
    }

    fn supports_capability(&self, capability: u32) -> bool {
        self.negotiated_capabilities & capability != 0
    }

    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        if self.in_band.as_ref().is_some_and(|tty| tty.heartbeat_due()) {
            // An unsolicited Pong tells the terminal an idle app is alive.
            self.send_message(&AppMessage::Pong)?;
        }
        self.process_pending_messages()?;

        if let Some(event) = self.pending_events.pop_front() {
            return Ok(Some(event));
        }

        Ok(None)
    }

    fn send_client_hello(&mut self) -> anyhow::Result<()> {
        let msg = AppMessage::ClientHello {
            version: PROTOCOL_VERSION,
            requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
        };
        self.send_message(&msg)
    }

    fn wait_for_session_ready(&mut self) -> anyhow::Result<()> {
        let mut attempts = 0;
        while attempts < 50 {
            if self.read_available()? == ReadOutcome::Closed {
                anyhow::bail!("Connection closed during handshake");
            }

            while let Some(msg) = self.next_decoded_message()? {
                match msg {
                    TerminalMessage::ServerHello {
                        version,
                        capabilities,
                    } => {
                        if version < MIN_PROTOCOL_VERSION {
                            anyhow::bail!(
                                "Protocol version mismatch: {} vs minimum {}",
                                version,
                                MIN_PROTOCOL_VERSION
                            );
                        }
                        self.negotiated_capabilities = capabilities;
                    }
                    TerminalMessage::SessionReady {
                        session_id,
                        root_region,
                        width,
                        height,
                    } => {
                        self.session_id = session_id;
                        self.root_region = root_region;
                        self.retained.reserve(root_region);
                        self.viewport_width = width;
                        self.viewport_height = height;
                        return Ok(());
                    }
                    _ => {}
                }
            }

            std::thread::sleep(Duration::from_millis(50));
            attempts += 1;
        }

        anyhow::bail!("Timeout waiting for SessionReady")
    }

    /// Next well-formed buffered message, skipping (and logging) malformed
    /// frames so one bad frame from the terminal does not take the app down.
    fn next_decoded_message(&mut self) -> anyhow::Result<Option<TerminalMessage>> {
        loop {
            match self.try_decode_message() {
                Ok(msg) => return Ok(msg),
                Err(err @ FrameError::Malformed { .. }) => {
                    tracing::warn!("skipping {err}");
                }
                Err(err @ FrameError::BadMagic(_)) => return Err(err.into()),
            }
        }
    }

    fn process_pending_messages(&mut self) -> anyhow::Result<()> {
        if !self.is_connected() {
            return Ok(());
        }

        let outcome = self.read_available()?;

        while let Some(msg) = self.next_decoded_message()? {
            match msg {
                TerminalMessage::LayoutComputed { regions, .. } => {
                    for layout in regions {
                        self.region_layouts.insert(layout.id, layout);
                    }
                }
                TerminalMessage::Resize(resize) => {
                    self.viewport_width = resize.width;
                    self.viewport_height = resize.height;
                    self.pending_events.push_back(Event::Resize {
                        width: resize.width,
                        height: resize.height,
                    });
                }
                TerminalMessage::Input(input) => {
                    if let Some(event) = self.convert_input_event(input) {
                        self.pending_events.push_back(event);
                    }
                }
                TerminalMessage::Focus(focus) => {
                    if focus.focused {
                        self.pending_events.push_back(Event::FocusGained);
                    } else {
                        self.pending_events.push_back(Event::FocusLost);
                    }
                }
                TerminalMessage::Scroll(_scroll) => {
                    // Scroll position updates from the terminal are informational;
                    // the app already knows its own scroll state.
                }
                _ => {}
            }
        }

        if outcome == ReadOutcome::Closed {
            tracing::info!("terminal closed the native connection; quitting");
            self.stream = None;
            self.in_band = None;
            self.pending_events.push_back(Event::Quit);
        }

        Ok(())
    }

    fn convert_input_event(&mut self, input: InputEvent) -> Option<Event> {
        match input.event_type {
            InputType::KeyDown { key, modifiers } => Some(Event::Key(crate::event::KeyEvent {
                key: self.convert_key(key),
                modifiers: self.convert_modifiers(modifiers),
                focused_id: None,
            })),
            InputType::MouseDown { button } => Some(Event::Mouse(crate::event::MouseEvent {
                kind: crate::event::MouseEventKind::Down(Self::convert_mouse_button(button)),
                x: input.x as u16,
                y: input.y as u16,
                modifiers: self.convert_modifiers(input.modifiers),
            })),
            InputType::MouseUp { button } => Some(Event::Mouse(crate::event::MouseEvent {
                kind: crate::event::MouseEventKind::Up(Self::convert_mouse_button(button)),
                x: input.x as u16,
                y: input.y as u16,
                modifiers: self.convert_modifiers(input.modifiers),
            })),
            InputType::MouseMove => Some(Event::Mouse(crate::event::MouseEvent {
                kind: crate::event::MouseEventKind::Moved,
                x: input.x as u16,
                y: input.y as u16,
                modifiers: self.convert_modifiers(input.modifiers),
            })),
            InputType::MouseScroll { delta_y, .. } => {
                let kind = if delta_y > 0.0 {
                    crate::event::MouseEventKind::ScrollUp
                } else {
                    crate::event::MouseEventKind::ScrollDown
                };
                Some(Event::Mouse(crate::event::MouseEvent {
                    kind,
                    x: input.x as u16,
                    y: input.y as u16,
                    modifiers: self.convert_modifiers(input.modifiers),
                }))
            }
            InputType::TextInput { text } => {
                // Convert bulk text input into individual key events.
                // Queue all into pending_events (not returning the first separately,
                // which would place it after the rest when the caller pushes it back).
                for ch in text.chars() {
                    self.pending_events
                        .push_back(Event::Key(crate::event::KeyEvent {
                            key: crate::event::Key::Char(ch),
                            modifiers: crate::event::Modifiers::default(),
                            focused_id: None,
                        }));
                }
                None
            }
            _ => None,
        }
    }

    fn convert_mouse_button(button: ultty_protocol::MouseButton) -> crate::event::MouseButton {
        match button {
            ultty_protocol::MouseButton::Left => crate::event::MouseButton::Left,
            ultty_protocol::MouseButton::Middle => crate::event::MouseButton::Middle,
            ultty_protocol::MouseButton::Right => crate::event::MouseButton::Right,
            _ => crate::event::MouseButton::Left,
        }
    }

    fn convert_key(&self, key: Key) -> crate::event::Key {
        match key {
            Key::Char(c) => crate::event::Key::Char(c),
            Key::Function(n) => crate::event::Key::F(n),
            Key::Escape => crate::event::Key::Escape,
            Key::Enter => crate::event::Key::Enter,
            Key::Tab => crate::event::Key::Tab,
            Key::Backspace => crate::event::Key::Backspace,
            Key::Delete => crate::event::Key::Delete,
            Key::Insert => crate::event::Key::Insert,
            Key::Home => crate::event::Key::Home,
            Key::End => crate::event::Key::End,
            Key::PageUp => crate::event::Key::PageUp,
            Key::PageDown => crate::event::Key::PageDown,
            Key::Up => crate::event::Key::Up,
            Key::Down => crate::event::Key::Down,
            Key::Left => crate::event::Key::Left,
            Key::Right => crate::event::Key::Right,
        }
    }

    fn convert_modifiers(&self, mods: Modifiers) -> crate::event::Modifiers {
        crate::event::Modifiers {
            shift: mods.shift(),
            ctrl: mods.ctrl(),
            alt: mods.alt(),
            meta: mods.meta(),
        }
    }
}

impl Default for NativeBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Send the in-band probe on `tty` and report whether the terminal speaks
/// the protocol: `ClientHello`, then DA1. A protocol terminal's first frame
/// (normally `ServerHello`) arrives before its DA1 reply; other terminals
/// answer only DA1. Frame bytes received are left in `read_buf`.
fn probe_in_band(
    tty: &mut in_band::InBandTty,
    read_buf: &mut BytesMut,
    timeout: Duration,
) -> anyhow::Result<bool> {
    let hello = AppMessage::ClientHello {
        version: PROTOCOL_VERSION,
        requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
    };
    tty.write_frames(&hello.encode_to_bytes())?;
    tty.write_raw(b"\x1b[c")?;

    let deadline = std::time::Instant::now() + timeout;
    let mut da1 = in_band::Da1Scanner::default();
    // Both callbacks see both flags; whichever arrives first decides.
    let answered = std::cell::Cell::new(false);
    let da1_first = std::cell::Cell::new(false);
    while !answered.get() && !da1_first.get() {
        let now = std::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("terminal answered neither the in-band probe nor DA1");
        }
        let outcome = tty.read(
            (deadline - now).min(Duration::from_millis(50)),
            |frames| {
                read_buf.extend_from_slice(frames);
                if !da1_first.get() && transport::holds_complete_frame(read_buf) {
                    answered.set(true);
                }
            },
            |other| {
                if !answered.get() && da1.feed(other) {
                    da1_first.set(true);
                }
            },
        )?;
        if outcome == ReadOutcome::Closed {
            anyhow::bail!("terminal closed during the in-band probe");
        }
    }
    Ok(answered.get())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    use ultty_protocol::{AppMessage, Content, Decode, RegionId, WidgetContent};

    use super::NativeBackend;
    use crate::widgets::{Button, Column};

    /// Backend writing into one end of a socket pair; returns the reader end.
    fn backend_over_pair() -> (NativeBackend, UnixStream) {
        let (reader, writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        writer.set_nonblocking(true).unwrap();
        let mut backend = NativeBackend::new();
        backend.stream = Some(writer);
        (backend, reader)
    }

    fn drain_messages(reader: &mut UnixStream) -> Vec<AppMessage> {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("read failed: {e}"),
            }
        }
        let mut cursor = &bytes[..];
        let mut messages = Vec::new();
        while !cursor.is_empty() {
            messages.push(AppMessage::decode(&mut cursor).unwrap());
        }
        messages
    }

    fn button_labels(messages: &[AppMessage]) -> Vec<(RegionId, String)> {
        messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::SetContent {
                    id,
                    content: Content::Widget(WidgetContent::Button(button)),
                } => Some((*id, button.label.clone())),
                _ => None,
            })
            .collect()
    }

    fn buttons(labels: &[&str]) -> Column {
        labels.iter().fold(Column::new(), |col, label| {
            col.child(Button::new(*label).id(*label))
        })
    }

    #[test]
    fn keyed_widgets_keep_region_ids_when_a_sibling_is_prepended() {
        let (mut backend, mut reader) = backend_over_pair();

        backend.render(&buttons(&["a", "b", "c"])).unwrap();
        let first = drain_messages(&mut reader);
        let first_ids = button_labels(&first);
        assert_eq!(first_ids.len(), 3);

        backend.render(&buttons(&["x", "a", "b", "c"])).unwrap();
        let second = drain_messages(&mut reader);

        let sent = button_labels(&second);
        assert_eq!(sent.len(), 1, "only the new button's content is sent");
        assert_eq!(sent[0].1, "x");
        assert!(second
            .iter()
            .all(|m| !matches!(m, AppMessage::DeleteRegion { .. })));
        assert!(matches!(
            second
                .iter()
                .find(|m| matches!(m, AppMessage::CreateRegion { .. })),
            Some(AppMessage::CreateRegion {
                insert_index: 0,
                ..
            })
        ));

        let column = backend.retained.children_of(backend.root_region)[0];
        let retained_children = backend.retained.children_of(column);
        assert_eq!(retained_children.len(), 4);
        assert_eq!(retained_children[0], sent[0].0);
        for (index, (id, _)) in first_ids.iter().enumerate() {
            assert_eq!(retained_children[index + 1], *id);
        }
    }

    use std::io::Write;
    use std::time::Duration;

    use bytes::BytesMut;
    use ultty_protocol::TerminalMessage;

    /// Run the probe against a fake terminal on a PTY that answers with
    /// `reply` once it has seen the probe.
    fn probe_against(reply: Vec<u8>) -> anyhow::Result<bool> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let (mut master, slave) = {
            let (mut m, mut s) = (0, 0);
            // SAFETY: openpty writes two fds; the name/termios/winsize
            // arguments may be null. `null_mut` fits both platforms: libc
            // declares termios/winsize `*const` on Linux, `*mut` on macOS.
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
        let terminal = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while !(seen.windows(3).any(|w| w == b"\x1b[c")) {
                let n = master.read(&mut buf).expect("read probe");
                seen.extend_from_slice(&buf[..n]);
            }
            master.write_all(&reply).expect("reply");
            // Closing the master would hang up the slave and discard the
            // reply before the probe reads it.
            (seen, master)
        });
        let mut tty = super::in_band::InBandTty::open_on(slave.as_raw_fd(), slave.as_raw_fd())?;
        let mut read_buf = BytesMut::new();
        let result = super::probe_in_band(&mut tty, &mut read_buf, Duration::from_secs(2));
        let (seen, _master) = terminal.join().unwrap();
        assert!(
            seen.starts_with(b"\x1b_U"),
            "probe starts with an in-band hello"
        );
        drop(tty);
        result
    }

    fn server_hello_wire() -> Vec<u8> {
        use ultty_protocol::Encode;
        let hello = TerminalMessage::ServerHello {
            version: ultty_protocol::PROTOCOL_VERSION,
            capabilities: 0,
        };
        ultty_protocol::in_band::encode_to_vec(&hello.encode_to_bytes())
    }

    #[test]
    fn probe_succeeds_when_the_hello_reply_precedes_da1() {
        let mut reply = server_hello_wire();
        reply.extend_from_slice(b"\x1b[?62;22c");
        assert!(probe_against(reply).unwrap());
    }

    #[test]
    fn probe_fails_on_a_terminal_that_only_answers_da1() {
        assert!(!probe_against(b"\x1b[?62;22c".to_vec()).unwrap());
    }

    #[test]
    fn probe_fails_when_da1_comes_first() {
        let mut reply = b"\x1b[?1;2c".to_vec();
        reply.extend(server_hello_wire());
        assert!(!probe_against(reply).unwrap());
    }
}
