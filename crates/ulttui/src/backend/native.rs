//! Native protocol backend for Ultty terminal.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use ultty_protocol::{
    AppMessage, ComputedRegionLayout, Content, Decode, Encode, FrameId, InputEvent, InputType, Key,
    Modifiers, RegionId, TerminalMessage, EVENT_KEY, EVENT_MOUSE, EVENT_SCROLL, FRAME_MAGIC,
    PROTOCOL_VERSION,
};

use crate::event::Event;
use crate::widgets::View;

use super::detect;

pub struct NativeBackend {
    stream: Option<UnixStream>,
    session_id: u64,
    root_region: RegionId,
    viewport_width: u16,
    viewport_height: u16,
    read_buf: BytesMut,
    next_region_id: u32,
    next_frame_id: u32,
    region_layouts: HashMap<RegionId, ComputedRegionLayout>,
    previous_frame_root: Option<RegionId>,
    pending_events: VecDeque<Event>,
}

impl NativeBackend {
    pub fn new() -> Self {
        Self {
            stream: None,
            session_id: 0,
            root_region: RegionId::ROOT,
            viewport_width: 80,
            viewport_height: 24,
            read_buf: BytesMut::with_capacity(4096),
            next_region_id: 1,
            next_frame_id: 1,
            region_layouts: HashMap::new(),
            previous_frame_root: None,
            pending_events: VecDeque::new(),
        }
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
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

    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.stream.is_some() {
            self.send_message(&AppMessage::Disconnect)?;
            self.stream = None;
        }
        Ok(())
    }

    pub fn render(&mut self, view: &impl View) -> anyhow::Result<()> {
        let frame_id = FrameId(self.next_frame_id);
        self.next_frame_id += 1;

        self.send_message(&AppMessage::BeginFrame { frame_id })?;

        if let Some(previous_root) = self.previous_frame_root.take() {
            self.send_message(&AppMessage::DeleteRegion { id: previous_root })?;
        }

        let frame_root = self.render_view_tree(view, self.root_region, true)?;
        self.previous_frame_root = Some(frame_root);

        self.send_message(&AppMessage::EndFrame { frame_id })?;

        self.process_pending_messages()?;

        Ok(())
    }

    fn render_view_tree(
        &mut self,
        view: &impl View,
        parent: RegionId,
        is_root: bool,
    ) -> anyhow::Result<RegionId> {
        let region_id = RegionId::new(self.next_region_id);
        self.next_region_id += 1;

        let mut constraints = view.constraints();
        let mut style = view.style();
        if is_root {
            // Root view fills the entire viewport — use absolute cells since
            // the ROOT parent region has height:auto and percentage heights
            // cannot resolve against an auto-height parent.
            constraints.width = ultty_protocol::Dimension::Cells(self.viewport_width as f32);
            constraints.height = ultty_protocol::Dimension::Cells(self.viewport_height as f32);
            // Opaque background so native app covers the PTY terminal content
            if style.bg.is_none() {
                let theme = crate::theme::Theme::current();
                style.bg = Some(theme.background);
            }
            style.opacity = 1.0;
        }

        self.send_message(&AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints,
            style,
        })?;

        let content = view.content();
        if content != Content::None {
            self.send_message(&AppMessage::SetContent {
                id: region_id,
                content,
            })?;
        }

        if let Some((sx, sy)) = view.scroll_offset() {
            self.send_message(&AppMessage::Scroll {
                id: region_id,
                x: sx,
                y: sy,
                animate: false,
            })?;
        }

        let children_count = view.children_count();
        let mut pending_messages: Vec<AppMessage> = Vec::new();

        for i in 0..children_count {
            let mut next_id = self.next_region_id;
            let mut captured_messages: Vec<AppMessage> = Vec::new();
            let mut send_fn = |msg: &AppMessage| -> anyhow::Result<()> {
                captured_messages.push(msg.clone());
                Ok(())
            };

            if view
                .render_child_native(i, &mut send_fn, region_id, &mut next_id)?
                .is_some()
            {
                self.next_region_id = next_id;
            }
            pending_messages.extend(captured_messages);
        }

        for msg in pending_messages {
            self.send_message(&msg)?;
        }

        Ok(region_id)
    }

    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        self.process_pending_messages()?;

        if let Some(event) = self.pending_events.pop_front() {
            return Ok(Some(event));
        }

        Ok(None)
    }

    fn send_client_hello(&mut self) -> anyhow::Result<()> {
        let msg = AppMessage::ClientHello {
            version: PROTOCOL_VERSION,
            requested_capabilities: 0,
        };
        self.send_message(&msg)
    }

    fn wait_for_session_ready(&mut self) -> anyhow::Result<()> {
        let mut attempts = 0;
        while attempts < 50 {
            self.read_available()?;

            while let Some(msg) = self.try_decode_message()? {
                match msg {
                    TerminalMessage::ServerHello { version, .. } => {
                        if version != PROTOCOL_VERSION {
                            anyhow::bail!(
                                "Protocol version mismatch: {} vs {}",
                                version,
                                PROTOCOL_VERSION
                            );
                        }
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

    fn send_message(&mut self, msg: &AppMessage) -> anyhow::Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;
        let bytes = msg.encode_to_bytes();
        stream.write_all(&bytes)?;
        Ok(())
    }

    fn read_available(&mut self) -> anyhow::Result<()> {
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

    fn try_decode_message(&mut self) -> anyhow::Result<Option<TerminalMessage>> {
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

    fn process_pending_messages(&mut self) -> anyhow::Result<()> {
        let _ = self.read_available();

        while let Some(msg) = self.try_decode_message()? {
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
                // Convert bulk text input into individual key events
                let mut events = Vec::new();
                for ch in text.chars() {
                    events.push(Event::Key(crate::event::KeyEvent {
                        key: crate::event::Key::Char(ch),
                        modifiers: crate::event::Modifiers::default(),
                        focused_id: None,
                    }));
                }
                // Return first event; queue the rest
                if events.is_empty() {
                    None
                } else {
                    for event in events.drain(1..) {
                        self.pending_events.push_back(event);
                    }
                    events.into_iter().next()
                }
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
