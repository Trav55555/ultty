//! Native protocol backend for Ultty terminal.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use ultty_protocol::{
    AppMessage, ComputedRegionLayout, Constraints, Content, Decode, Encode, FrameId, InputEvent,
    InputType, Key, Modifiers, RegionId, Style, TerminalMessage, CAP_APPEND_CONTENT,
    CLIENT_DEFAULT_CAPABILITIES, EVENT_KEY, EVENT_MOUSE, EVENT_SCROLL, FRAME_MAGIC,
    MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};

use crate::event::Event;
use crate::widgets::View;

use super::detect;

pub struct NativeBackend {
    stream: Option<UnixStream>,
    negotiated_capabilities: u32,
    session_id: u64,
    root_region: RegionId,
    viewport_width: u16,
    viewport_height: u16,
    read_buf: BytesMut,
    next_region_id: u32,
    next_frame_id: u32,
    region_layouts: HashMap<RegionId, ComputedRegionLayout>,
    previous_regions: HashMap<RegionId, RegionSnapshot>,
    pending_events: VecDeque<Event>,
}

#[derive(Debug, Clone, PartialEq)]
struct RegionSnapshot {
    parent: RegionId,
    constraints: Constraints,
    style: Style,
    content: Content,
    scroll: Option<(f32, f32, bool)>,
}

impl NativeBackend {
    pub fn new() -> Self {
        Self {
            stream: None,
            negotiated_capabilities: 0,
            session_id: 0,
            root_region: RegionId::ROOT,
            viewport_width: 80,
            viewport_height: 24,
            read_buf: BytesMut::with_capacity(4096),
            next_region_id: 1,
            next_frame_id: 1,
            region_layouts: HashMap::new(),
            previous_regions: HashMap::new(),
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

        self.next_region_id = 1;
        let mut frame_messages = Vec::new();
        let _frame_root =
            self.render_view_tree(view, self.root_region, true, &mut frame_messages)?;
        let (optimized_messages, next_regions) = Self::optimize_frame_messages(
            &self.previous_regions,
            frame_messages,
            self.supports_capability(CAP_APPEND_CONTENT),
        );
        self.previous_regions = next_regions;

        for msg in optimized_messages {
            self.send_message(&msg)?;
        }

        self.send_message(&AppMessage::EndFrame { frame_id })?;

        self.process_pending_messages()?;

        Ok(())
    }

    fn render_view_tree(
        &mut self,
        view: &impl View,
        parent: RegionId,
        is_root: bool,
        frame_messages: &mut Vec<AppMessage>,
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

        frame_messages.push(AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints,
            style,
        });

        let content = view.content();
        if content != Content::None {
            frame_messages.push(AppMessage::SetContent {
                id: region_id,
                content,
            });
        }

        if let Some((sx, sy)) = view.scroll_offset() {
            frame_messages.push(AppMessage::Scroll {
                id: region_id,
                x: sx,
                y: sy,
                animate: false,
            });
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

        frame_messages.extend(pending_messages);

        Ok(region_id)
    }

    fn optimize_frame_messages(
        previous_regions: &HashMap<RegionId, RegionSnapshot>,
        frame_messages: Vec<AppMessage>,
        allow_append_content: bool,
    ) -> (Vec<AppMessage>, HashMap<RegionId, RegionSnapshot>) {
        let mut outgoing = Vec::new();
        let mut next_regions: HashMap<RegionId, RegionSnapshot> = HashMap::new();

        for msg in frame_messages {
            match msg {
                AppMessage::CreateRegion {
                    id,
                    parent,
                    insert_index,
                    constraints,
                    style,
                } => {
                    let next_snapshot = RegionSnapshot {
                        parent,
                        constraints: constraints.clone(),
                        style: style.clone(),
                        content: Content::None,
                        scroll: None,
                    };
                    next_regions.insert(id, next_snapshot);

                    if let Some(prev) = previous_regions.get(&id) {
                        if prev.parent != parent {
                            outgoing.push(AppMessage::ReparentRegion {
                                id,
                                new_parent: parent,
                                insert_index,
                            });
                        }
                        if prev.constraints != constraints {
                            outgoing.push(AppMessage::SetConstraints { id, constraints });
                        }
                        if prev.style != style {
                            outgoing.push(AppMessage::SetStyle { id, style });
                        }
                    } else {
                        outgoing.push(AppMessage::CreateRegion {
                            id,
                            parent,
                            insert_index,
                            constraints,
                            style,
                        });
                    }
                }
                AppMessage::SetContent { id, content } => {
                    if let Some(next) = next_regions.get_mut(&id) {
                        next.content = content.clone();
                    } else if let Some(prev) = previous_regions.get(&id) {
                        next_regions.insert(id, prev.clone());
                        if let Some(next) = next_regions.get_mut(&id) {
                            next.content = content.clone();
                        }
                    }

                    if let Some(prev) = previous_regions.get(&id) {
                        if allow_append_content {
                            if let Some(delta) = Self::append_delta(&prev.content, &content) {
                                if !delta.is_empty() {
                                    outgoing.push(AppMessage::AppendContent { id, text: delta });
                                }
                            } else if prev.content != content {
                                outgoing.push(AppMessage::SetContent { id, content });
                            }
                        } else if prev.content != content {
                            outgoing.push(AppMessage::SetContent { id, content });
                        }
                    } else {
                        outgoing.push(AppMessage::SetContent { id, content });
                    }
                }
                AppMessage::Scroll { id, x, y, animate } => {
                    if let Some(next) = next_regions.get_mut(&id) {
                        next.scroll = Some((x, y, animate));
                    }

                    let changed = previous_regions
                        .get(&id)
                        .map(|prev| prev.scroll != Some((x, y, animate)))
                        .unwrap_or(true);
                    if changed {
                        outgoing.push(AppMessage::Scroll { id, x, y, animate });
                    }
                }
                other => outgoing.push(other),
            }
        }

        for (id, next) in &next_regions {
            if let Some(prev) = previous_regions.get(id) {
                if prev.content != Content::None && next.content == Content::None {
                    outgoing.push(AppMessage::SetContent {
                        id: *id,
                        content: Content::None,
                    });
                }
            }
        }

        for id in previous_regions.keys() {
            if !next_regions.contains_key(id) {
                outgoing.push(AppMessage::DeleteRegion { id: *id });
            }
        }

        (outgoing, next_regions)
    }

    fn append_delta(previous: &Content, next: &Content) -> Option<String> {
        match (previous, next) {
            (Content::Text(prev), Content::Text(next))
                if prev.wrap == next.wrap
                    && prev.align == next.align
                    && prev.overflow == next.overflow
                    && next.text.starts_with(&prev.text)
                    && next.text.len() > prev.text.len() =>
            {
                Some(next.text[prev.text.len()..].to_string())
            }
            _ => None,
        }
    }

    fn supports_capability(&self, capability: u32) -> bool {
        self.negotiated_capabilities & capability != 0
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
            requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
        };
        self.send_message(&msg)
    }

    fn wait_for_session_ready(&mut self) -> anyhow::Result<()> {
        let mut attempts = 0;
        while attempts < 50 {
            self.read_available()?;

            while let Some(msg) = self.try_decode_message()? {
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
    use super::{NativeBackend, RegionSnapshot};
    use std::collections::HashMap;
    use ultty_protocol::{AppMessage, Constraints, Content, RegionId, Style, TextContent};

    fn snapshot_with_text(text: &str) -> RegionSnapshot {
        RegionSnapshot {
            parent: RegionId::ROOT,
            constraints: Constraints::default(),
            style: Style::default(),
            content: Content::Text(TextContent::new(text)),
            scroll: None,
        }
    }

    #[test]
    fn optimize_converts_text_growth_to_append_content() {
        let id = RegionId::new(1);
        let mut previous = HashMap::new();
        previous.insert(id, snapshot_with_text("hello"));

        let frame = vec![
            AppMessage::CreateRegion {
                id,
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent {
                id,
                content: Content::Text(TextContent::new("hello world")),
            },
        ];

        let (outgoing, next) = NativeBackend::optimize_frame_messages(&previous, frame, true);

        assert!(outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::AppendContent { id: mid, text } if *mid == id && text == " world")));
        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::CreateRegion { id: mid, .. } if *mid == id)));
        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::SetContent { id: mid, .. } if *mid == id)));
        assert_eq!(
            next.get(&id).map(|r| &r.content),
            Some(&Content::Text(TextContent::new("hello world")))
        );
    }

    #[test]
    fn optimize_emits_delete_for_removed_region() {
        let removed = RegionId::new(9);
        let mut previous = HashMap::new();
        previous.insert(removed, snapshot_with_text("gone"));

        let (outgoing, _next) = NativeBackend::optimize_frame_messages(&previous, Vec::new(), true);

        assert!(outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::DeleteRegion { id } if *id == removed)));
    }

    #[test]
    fn optimize_clears_content_when_region_becomes_container() {
        let id = RegionId::new(3);
        let mut previous = HashMap::new();
        previous.insert(id, snapshot_with_text("stale"));

        let frame = vec![AppMessage::CreateRegion {
            id,
            parent: RegionId::ROOT,
            insert_index: -1,
            constraints: Constraints::default(),
            style: Style::default(),
        }];

        let (outgoing, _next) = NativeBackend::optimize_frame_messages(&previous, frame, true);

        assert!(outgoing.iter().any(|m| {
            matches!(
                m,
                AppMessage::SetContent {
                    id: mid,
                    content: Content::None
                } if *mid == id
            )
        }));
    }

    #[test]
    fn optimize_falls_back_to_set_content_when_append_not_negotiated() {
        let id = RegionId::new(1);
        let mut previous = HashMap::new();
        previous.insert(id, snapshot_with_text("alpha"));

        let frame = vec![
            AppMessage::CreateRegion {
                id,
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent {
                id,
                content: Content::Text(TextContent::new("alpha beta")),
            },
        ];

        let (outgoing, _next) = NativeBackend::optimize_frame_messages(&previous, frame, false);

        assert!(!outgoing
            .iter()
            .any(|m| matches!(m, AppMessage::AppendContent { id: mid, .. } if *mid == id)));
        assert!(outgoing.iter().any(|m| {
            matches!(
                m,
                AppMessage::SetContent {
                    id: mid,
                    content: Content::Text(_)
                } if *mid == id
            )
        }));
    }
}
