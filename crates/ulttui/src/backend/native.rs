//! Native protocol backend for Ultty terminal.

mod transport;

use std::collections::{HashMap, VecDeque};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bytes::BytesMut;
use ultty_protocol::{
    AppMessage, ComputedRegionLayout, Content, FrameId, InputEvent, InputType, Key, Modifiers,
    RegionId, TerminalMessage, CAP_APPEND_CONTENT, CAP_REGION_METADATA, CAP_SEMANTICS,
    CLIENT_DEFAULT_CAPABILITIES, EVENT_KEY, EVENT_MOUSE, EVENT_SCROLL, MIN_PROTOCOL_VERSION,
    PROTOCOL_VERSION,
};

use crate::event::Event;
use crate::widgets::View;

use super::native_diff::{
    begin_frame_notes, optimize_frame_messages, record_native_key, take_frame_notes,
    FrameCapabilities, FrameNotes, RetainedTree,
};
use transport::{FrameError, ReadOutcome};
use ultty_client::{detect, in_band};

/// The metadata key that carries a region's native key (its widget id),
/// which `ulttyctl regions` shows and tests select regions by.
pub const REGION_ID_METADATA_KEY: &str = "id";

/// Name every region this frame creates that has a native key, so tools
/// reading the terminal's scene can find it. A retained region keeps its
/// key, so it is named once.
fn name_new_regions(messages: &mut Vec<AppMessage>, tree: &RetainedTree) {
    let names: Vec<AppMessage> = messages
        .iter()
        .filter_map(|message| match message {
            AppMessage::CreateRegion { id, .. } => {
                tree.key_of(*id).map(|key| AppMessage::SetRegionMetadata {
                    id: *id,
                    metadata: vec![(REGION_ID_METADATA_KEY.to_string(), key.to_string())],
                })
            }
            _ => None,
        })
        .collect();
    messages.extend(names);
}

/// How long one event poll waits for terminal messages.
const POLL_TIMEOUT: Duration = Duration::from_millis(16);

/// How long the terminal has to answer `ClientHello` with `SessionReady`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

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
    /// The region last reported to the terminal as focused.
    reported_focus: Option<RegionId>,
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
            reported_focus: None,
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

    pub fn init(&mut self) -> crate::Result<()> {
        if self.prefer_in_band {
            return self.init_in_band();
        }
        let socket_path = detect::socket_path()
            .ok_or(crate::Error::NativeUnavailable("ULTTY_SOCKET is not set"))?;

        self.connect_socket(UnixStream::connect(&socket_path)?)
    }

    /// Run the handshake over a connected socket and start the session.
    fn connect_socket(&mut self, stream: UnixStream) -> crate::Result<()> {
        // Blocking writes; reads wait at most one poll interval, so the
        // handshake finishes as soon as SessionReady arrives. A longer read
        // timeout here made every read wait out that timeout once the
        // terminal had answered, delaying startup by its full length.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(POLL_TIMEOUT))?;
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

        Ok(())
    }

    /// Probe for in-band support and, if the terminal has it, run the
    /// handshake over the app's own terminal.
    ///
    /// The probe asks the terminal's name and, only if it is Ultty, sends
    /// `ClientHello` followed by DA1 ([`in_band::probe`]). Inside tmux or
    /// screen the multiplexer answers for itself, so apps there fall back
    /// to ANSI, which is right: the outer terminal cannot know where the
    /// pane is.
    fn init_in_band(&mut self) -> crate::Result<()> {
        let mut tty = in_band::InBandTty::open()?;
        let mut received = Vec::new();
        if !in_band::probe(&mut tty, &mut received, in_band::PROBE_TIMEOUT)? {
            return Err(crate::Error::NativeUnavailable(
                "the terminal does not answer the in-band probe",
            ));
        }
        self.read_buf.extend_from_slice(&received);

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

    pub fn shutdown(&mut self) -> crate::Result<()> {
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
    ) -> crate::Result<()> {
        self.send_message(&AppMessage::AppendContent {
            id: region_id,
            text: text.into(),
        })
    }

    /// Send the changes between the last frame and `view`. A view that
    /// renders the same as the last frame sends nothing at all.
    pub fn render(&mut self, view: &impl View) -> crate::Result<()> {
        let (frame_messages, notes) = self.collect_frame(view)?;
        let (mut optimized_messages, next_tree) = optimize_frame_messages(
            &self.retained,
            frame_messages,
            &notes,
            FrameCapabilities {
                append_content: self.supports_capability(CAP_APPEND_CONTENT),
                semantics: self.supports_capability(CAP_SEMANTICS),
            },
        );
        if self.supports_capability(CAP_REGION_METADATA) {
            name_new_regions(&mut optimized_messages, &next_tree);
        }
        self.retained = next_tree;

        if !optimized_messages.is_empty() {
            let frame_id = FrameId(self.next_frame_id);
            self.next_frame_id += 1;
            self.send_message(&AppMessage::BeginFrame { frame_id })?;
            for msg in optimized_messages {
                self.send_message(&msg)?;
            }
            self.send_message(&AppMessage::EndFrame { frame_id })?;
        }

        self.process_pending_messages()?;

        Ok(())
    }

    /// Tell the terminal which region has focus: the one whose native key is
    /// `widget_id`. Only for the terminal's view of the app (`ulttyctl
    /// regions`); key routing in the app does not depend on it. Sends
    /// nothing while the focused region is unchanged.
    pub fn report_focus(&mut self, widget_id: Option<&str>) -> crate::Result<()> {
        if !self.is_connected() {
            return Ok(());
        }
        let region = widget_id.and_then(|key| self.retained.region_with_key(key));
        if region == self.reported_focus {
            return Ok(());
        }
        self.reported_focus = region;
        match region {
            Some(id) => self.send_message(&AppMessage::Focus { id }),
            None => self.send_message(&AppMessage::Blur),
        }
    }

    /// Walk the view tree into provisional-id messages, noting the native
    /// key of every region created along the way.
    fn collect_frame(&self, view: &impl View) -> crate::Result<(Vec<AppMessage>, FrameNotes)> {
        begin_frame_notes();
        let walked = self.render_view_tree(view);
        let notes = take_frame_notes();
        walked.map(|messages| (messages, notes))
    }

    fn render_view_tree(&self, view: &impl View) -> crate::Result<Vec<AppMessage>> {
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
        crate::widgets::send_semantics(
            &mut |message| {
                messages.push(message.clone());
                Ok(())
            },
            region_id,
            view.semantics(),
            &content,
        )?;
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

        let mut sink = |msg: &AppMessage| -> crate::Result<()> {
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

    pub fn poll_event(&mut self) -> crate::Result<Option<Event>> {
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

    fn send_client_hello(&mut self) -> crate::Result<()> {
        let msg = AppMessage::ClientHello {
            version: PROTOCOL_VERSION,
            requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
        };
        self.send_message(&msg)
    }

    fn wait_for_session_ready(&mut self) -> crate::Result<()> {
        let deadline = std::time::Instant::now() + HANDSHAKE_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if self.read_available()? == ReadOutcome::Closed {
                return Err(crate::Error::Handshake(
                    "the terminal closed the connection".to_string(),
                ));
            }

            while let Some(msg) = self.next_decoded_message()? {
                match msg {
                    TerminalMessage::ServerHello {
                        version,
                        capabilities,
                    } => {
                        if version < MIN_PROTOCOL_VERSION {
                            return Err(crate::Error::Handshake(format!(
                                "protocol version {version} is older than the minimum {MIN_PROTOCOL_VERSION}"
                            )));
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
        }

        Err(crate::Error::Handshake(
            "timed out waiting for SessionReady".to_string(),
        ))
    }

    /// Next well-formed buffered message, skipping (and logging) malformed
    /// frames so one bad frame from the terminal does not take the app down.
    fn next_decoded_message(&mut self) -> crate::Result<Option<TerminalMessage>> {
        loop {
            match self.try_decode_message() {
                Ok(msg) => return Ok(msg),
                Err(err @ FrameError::Malformed { .. }) => {
                    tracing::warn!("skipping {err}");
                }
                Err(FrameError::BadMagic(magic)) => return Err(crate::Error::BadFrameMagic(magic)),
            }
        }
    }

    fn process_pending_messages(&mut self) -> crate::Result<()> {
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

    use std::time::Duration;

    use ultty_protocol::TerminalMessage;

    #[test]
    fn socket_handshake_finishes_as_soon_as_the_terminal_answers() {
        use ultty_protocol::Encode;
        let (app_side, mut terminal) = UnixStream::pair().unwrap();
        let fake = std::thread::spawn(move || {
            let mut hello = [0u8; 14];
            terminal.read_exact(&mut hello).unwrap();
            for msg in [
                TerminalMessage::ServerHello {
                    version: ultty_protocol::PROTOCOL_VERSION,
                    capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
                },
                TerminalMessage::SessionReady {
                    session_id: 1,
                    root_region: RegionId::ROOT,
                    width: 80,
                    height: 24,
                },
            ] {
                std::io::Write::write_all(&mut terminal, &msg.encode_to_bytes()).unwrap();
            }
            terminal
        });
        let mut backend = NativeBackend::new();
        let started = std::time::Instant::now();
        backend.connect_socket(app_side).unwrap();
        let elapsed = started.elapsed();
        let _terminal = fake.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(500),
            "handshake took {elapsed:?}"
        );
    }

    #[test]
    fn rerendering_an_unchanged_view_sends_nothing() {
        let (mut backend, mut reader) = backend_over_pair();
        backend.render(&buttons(&["a", "b"])).unwrap();
        assert!(!drain_messages(&mut reader).is_empty());

        backend.render(&buttons(&["a", "b"])).unwrap();
        let second = drain_messages(&mut reader);
        assert!(second.is_empty(), "an unchanged view sent {second:?}");

        // A real change still goes out as one complete frame.
        backend.render(&buttons(&["a", "c"])).unwrap();
        let third = drain_messages(&mut reader);
        assert!(matches!(third.first(), Some(AppMessage::BeginFrame { .. })));
        assert!(matches!(third.last(), Some(AppMessage::EndFrame { .. })));
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

    fn names(messages: &[AppMessage]) -> Vec<(RegionId, String)> {
        messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::SetRegionMetadata { id, metadata } => {
                    Some((*id, metadata[0].1.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn new_keyed_regions_are_named_once_when_the_terminal_takes_metadata() {
        let (mut backend, mut reader) = backend_over_pair();
        backend.render(&buttons(&["a"])).unwrap();
        assert!(
            names(&drain_messages(&mut reader)).is_empty(),
            "not negotiated"
        );

        let (mut backend, mut reader) = backend_over_pair();
        backend.negotiated_capabilities = ultty_protocol::CAP_REGION_METADATA;
        backend.render(&buttons(&["a", "b"])).unwrap();
        let first = drain_messages(&mut reader);
        let labels = button_labels(&first);
        assert_eq!(
            names(&first),
            vec![
                (labels[0].0, "a".to_string()),
                (labels[1].0, "b".to_string())
            ]
        );
        assert!(matches!(first.last(), Some(AppMessage::EndFrame { .. })));

        backend.render(&buttons(&["a", "b", "c"])).unwrap();
        let second = drain_messages(&mut reader);
        assert_eq!(
            names(&second),
            vec![(button_labels(&second)[0].0, "c".to_string())]
        );
    }

    fn semantics_sent(messages: &[AppMessage]) -> Vec<ultty_protocol::Semantics> {
        messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::SetSemantics { semantics, .. } => Some(semantics.clone()),
                _ => None,
            })
            .collect()
    }

    fn checkbox(checked: bool) -> Column {
        Column::new().child(crate::widgets::Checkbox::new("Accept").checked(checked))
    }

    #[test]
    fn semantics_go_once_when_negotiated_again_on_change_and_never_for_the_default() {
        use ultty_protocol::{Role, Toggled};

        let (mut backend, mut reader) = backend_over_pair();
        backend.render(&checkbox(false)).unwrap();
        assert!(
            semantics_sent(&drain_messages(&mut reader)).is_empty(),
            "not negotiated"
        );

        let (mut backend, mut reader) = backend_over_pair();
        backend.negotiated_capabilities = ultty_protocol::CAP_SEMANTICS;
        backend.render(&checkbox(false)).unwrap();
        let first = semantics_sent(&drain_messages(&mut reader));
        assert_eq!(
            first.len(),
            1,
            "the column is a group, as derived: {first:?}"
        );
        assert_eq!(first[0].role, Role::Checkbox);
        assert_eq!(first[0].toggled, Toggled::False);

        backend.render(&checkbox(false)).unwrap();
        assert!(drain_messages(&mut reader).is_empty());

        backend.render(&checkbox(true)).unwrap();
        let changed = semantics_sent(&drain_messages(&mut reader));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].toggled, Toggled::True);

        // A button says nothing its content does not: nothing is sent.
        let (mut backend, mut reader) = backend_over_pair();
        backend.negotiated_capabilities = ultty_protocol::CAP_SEMANTICS;
        backend.render(&buttons(&["a", "b"])).unwrap();
        assert!(semantics_sent(&drain_messages(&mut reader)).is_empty());
    }

    #[test]
    fn focus_is_reported_when_it_moves_to_another_region() {
        let (mut backend, mut reader) = backend_over_pair();
        backend.render(&buttons(&["a", "b"])).unwrap();
        let ids = button_labels(&drain_messages(&mut reader));

        backend.report_focus(Some("b")).unwrap();
        assert_eq!(
            drain_messages(&mut reader),
            vec![AppMessage::Focus { id: ids[1].0 }]
        );
        backend.report_focus(Some("b")).unwrap();
        assert!(
            drain_messages(&mut reader).is_empty(),
            "unchanged focus is not resent"
        );
        backend.report_focus(Some("missing")).unwrap();
        assert_eq!(drain_messages(&mut reader), vec![AppMessage::Blur]);
    }
}
