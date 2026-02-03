//! Native protocol session management.
//!
//! A session represents a connection from a TUI application to the terminal.
//! It owns the scene graph and layout engine for that connection.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use bytes::{Buf, BytesMut};
use tracing::{debug, error, info, warn};
use ultty_protocol::{
    AppMessage, ComputedRegionLayout, Constraints, Content, Decode, Encode, FrameId, LayoutTrigger,
    RegionId, Style, TerminalMessage, FRAME_MAGIC, PROTOCOL_VERSION,
};

use crate::scene::layout::LayoutEngine;
use crate::scene::region::{ComputedLayout, Region};
use crate::scene::tree::SceneTree;

/// Capabilities supported by this terminal.
const TERMINAL_CAPABILITIES: u32 = 0x0001 | 0x0010; // LAYOUT_FLEXBOX | WIDGET_SCROLL

/// A native protocol session with a TUI application.
pub struct NativeSession {
    /// Session ID.
    session_id: u64,
    /// Scene tree for this session.
    scene: SceneTree,
    /// Layout engine.
    layout: LayoutEngine,
    /// Socket connection.
    stream: UnixStream,
    /// Read buffer.
    read_buf: BytesMut,
    /// Write buffer.
    write_buf: BytesMut,
    /// Viewport dimensions.
    viewport_width: f32,
    viewport_height: f32,
    /// Current frame being built (if any).
    current_frame: Option<FrameId>,
    /// Whether scene changed during current frame.
    scene_dirty: bool,
    /// Negotiated capabilities.
    capabilities: u32,
    /// Event subscriptions: region_id -> event mask.
    subscriptions: HashMap<RegionId, u32>,
    /// Currently focused region (if any).
    focused_region: Option<RegionId>,
    /// Per-region scroll offsets: region_id -> (scroll_x, scroll_y).
    scroll_offsets: HashMap<RegionId, (f32, f32)>,
    /// Animated scroll targets: region_id -> (target_x, target_y).
    scroll_targets: HashMap<RegionId, (f32, f32)>,
}

impl NativeSession {
    /// Create a new session from an accepted socket connection.
    pub fn new(stream: UnixStream, session_id: u64) -> io::Result<Self> {
        // Set non-blocking for polling
        stream.set_nonblocking(true)?;

        Ok(Self {
            session_id,
            scene: SceneTree::new(),
            layout: LayoutEngine::new(),
            stream,
            read_buf: BytesMut::with_capacity(8192),
            write_buf: BytesMut::with_capacity(8192),
            viewport_width: 80.0,
            viewport_height: 24.0,
            current_frame: None,
            scene_dirty: false,
            capabilities: 0,
            subscriptions: HashMap::new(),
            focused_region: None,
            scroll_offsets: HashMap::new(),
            scroll_targets: HashMap::new(),
        })
    }

    /// Set viewport dimensions (in terminal cells).
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        if (self.viewport_width - width).abs() > 0.1 || (self.viewport_height - height).abs() > 0.1
        {
            self.viewport_width = width;
            self.viewport_height = height;
            self.scene_dirty = true;
        }
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn viewport(&self) -> (f32, f32) {
        (self.viewport_width, self.viewport_height)
    }

    /// Read and process incoming messages.
    /// Returns true if the session should continue, false if disconnected.
    pub fn poll(&mut self) -> io::Result<bool> {
        // Try to read more data
        match self.read_from_socket() {
            Ok(0) => {
                // EOF - client disconnected
                info!("Session {} disconnected (EOF)", self.session_id);
                return Ok(false);
            }
            Ok(n) => {
                debug!("Session {} read {} bytes", self.session_id, n);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // No data available, that's fine
            }
            Err(e) => {
                error!("Session {} read error: {}", self.session_id, e);
                return Err(e);
            }
        }

        // Process complete messages
        while let Some(msg) = self.try_decode_message()? {
            if !self.handle_message(msg)? {
                return Ok(false);
            }
        }

        // Flush any pending writes
        self.flush_writes()?;

        Ok(true)
    }

    /// Read data from socket into buffer.
    fn read_from_socket(&mut self) -> io::Result<usize> {
        let mut temp = [0u8; 4096];
        let n = self.stream.read(&mut temp)?;
        if n > 0 {
            self.read_buf.extend_from_slice(&temp[..n]);
        }
        Ok(n)
    }

    /// Try to decode a complete message from the buffer.
    fn try_decode_message(&mut self) -> io::Result<Option<AppMessage>> {
        // Need at least header: Magic(2) + Type(2) + Length(4) = 8 bytes
        if self.read_buf.len() < 8 {
            return Ok(None);
        }

        // Peek at header to get length
        let magic = u16::from_le_bytes([self.read_buf[0], self.read_buf[1]]);
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            error!("Session {} invalid magic: 0x{:04x}", self.session_id, magic);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid frame magic",
            ));
        }

        let payload_len = u32::from_le_bytes([
            self.read_buf[4],
            self.read_buf[5],
            self.read_buf[6],
            self.read_buf[7],
        ]) as usize;

        // Check if we have the full message
        let total_len = 8 + payload_len;
        if self.read_buf.len() < total_len {
            return Ok(None);
        }

        // Decode the message
        let mut cursor = &self.read_buf[..total_len];
        match AppMessage::decode(&mut cursor) {
            Ok(msg) => {
                self.read_buf.advance(total_len);
                Ok(Some(msg))
            }
            Err(e) => {
                error!("Session {} decode error: {}", self.session_id, e);
                Err(io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
            }
        }
    }

    /// Handle a decoded message. Returns false if session should close.
    fn handle_message(&mut self, msg: AppMessage) -> io::Result<bool> {
        debug!("Session {} received: {:?}", self.session_id, msg);

        match msg {
            AppMessage::ClientHello {
                version,
                requested_capabilities,
            } => {
                self.handle_client_hello(version, requested_capabilities)?;
            }

            AppMessage::CreateRegion {
                id,
                parent,
                insert_index,
                constraints,
                style,
            } => {
                self.handle_create_region(id, parent, insert_index, constraints, style);
            }

            AppMessage::SetConstraints { id, constraints } => {
                self.handle_set_constraints(id, constraints);
            }

            AppMessage::SetStyle { id, style } => {
                self.handle_set_style(id, style);
            }

            AppMessage::SetContent { id, content } => {
                self.handle_set_content(id, content);
            }

            AppMessage::DeleteRegion { id } => {
                self.handle_delete_region(id);
            }

            AppMessage::ReparentRegion {
                id,
                new_parent,
                insert_index,
            } => {
                self.handle_reparent_region(id, new_parent, insert_index);
            }

            AppMessage::BeginFrame { frame_id } => {
                self.handle_begin_frame(frame_id);
            }

            AppMessage::EndFrame { frame_id } => {
                self.handle_end_frame(frame_id)?;
            }

            AppMessage::Subscribe { id, events } => {
                self.subscriptions.insert(id, events);
            }

            AppMessage::Unsubscribe { id, events } => {
                if let Some(mask) = self.subscriptions.get_mut(&id) {
                    *mask &= !events;
                    if *mask == 0 {
                        self.subscriptions.remove(&id);
                    }
                }
            }

            AppMessage::Scroll {
                id,
                x,
                y,
                animate,
            } => {
                if animate {
                    self.scroll_targets.insert(id, (x, y));
                    // Initialize current position if not yet set
                    self.scroll_offsets.entry(id).or_insert((0.0, 0.0));
                } else {
                    self.scroll_offsets.insert(id, (x, y));
                    self.scroll_targets.remove(&id);
                }
                self.scene_dirty = true;
                debug!("Scroll set for {:?}: ({}, {}), animate={}", id, x, y, animate);
            }

            AppMessage::Focus { id } => {
                let old = self.focused_region.take();
                self.focused_region = Some(id);
                debug!("Focus set to {:?} (was {:?})", id, old);

                // Send focus lost event to old region if subscribed
                if let Some(old_id) = old {
                    if old_id != id && self.is_subscribed(old_id, ultty_protocol::EVENT_FOCUS) {
                        let _ = self.send_message(&TerminalMessage::Focus(
                            ultty_protocol::FocusEvent {
                                region_id: old_id,
                                focused: false,
                            },
                        ));
                    }
                }

                // Send focus gained event to new region if subscribed
                if self.is_subscribed(id, ultty_protocol::EVENT_FOCUS) {
                    let _ = self.send_message(&TerminalMessage::Focus(
                        ultty_protocol::FocusEvent {
                            region_id: id,
                            focused: true,
                        },
                    ));
                }
            }

            AppMessage::Blur => {
                if let Some(old_id) = self.focused_region.take() {
                    debug!("Blur from {:?}", old_id);
                    if self.is_subscribed(old_id, ultty_protocol::EVENT_FOCUS) {
                        let _ = self.send_message(&TerminalMessage::Focus(
                            ultty_protocol::FocusEvent {
                                region_id: old_id,
                                focused: false,
                            },
                        ));
                    }
                } else {
                    debug!("Blur (no focus)");
                }
            }

            AppMessage::Disconnect => {
                info!("Session {} disconnected (explicit)", self.session_id);
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Handle ClientHello - send ServerHello and SessionReady.
    fn handle_client_hello(&mut self, version: u16, requested: u32) -> io::Result<()> {
        info!(
            "Session {} client hello: version={}, caps=0x{:04x}",
            self.session_id, version, requested
        );

        // Negotiate capabilities
        self.capabilities = requested & TERMINAL_CAPABILITIES;

        // Send ServerHello
        let hello = TerminalMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: TERMINAL_CAPABILITIES,
        };
        self.send_message(&hello)?;

        // Send SessionReady
        let ready = TerminalMessage::SessionReady {
            session_id: self.session_id,
            root_region: RegionId::ROOT,
            width: self.viewport_width as u16,
            height: self.viewport_height as u16,
        };
        self.send_message(&ready)?;

        Ok(())
    }

    /// Handle CreateRegion.
    fn handle_create_region(
        &mut self,
        id: RegionId,
        parent: RegionId,
        insert_index: i32,
        constraints: Constraints,
        style: Style,
    ) {
        let mut region = Region::new(id);
        region.parent = Some(parent);
        region.constraints = constraints;
        region.style = style;

        // Insert into tree (handles parent's children list)
        if insert_index >= 0 {
            self.scene.insert_at(region, insert_index as usize);
        } else {
            self.scene.insert(region);
        }

        self.scene_dirty = true;
        debug!("Created region {:?} under {:?}", id, parent);
    }

    /// Handle SetConstraints.
    fn handle_set_constraints(&mut self, id: RegionId, constraints: Constraints) {
        if let Some(region) = self.scene.get_mut(id) {
            region.constraints = constraints;
            region.mark_dirty();
            self.scene_dirty = true;
        } else {
            warn!("SetConstraints for unknown region {:?}", id);
        }
    }

    /// Handle SetStyle.
    fn handle_set_style(&mut self, id: RegionId, style: Style) {
        if let Some(region) = self.scene.get_mut(id) {
            region.style = style;
            region.mark_dirty();
            self.scene_dirty = true;
        } else {
            warn!("SetStyle for unknown region {:?}", id);
        }
    }

    /// Handle SetContent.
    fn handle_set_content(&mut self, id: RegionId, content: Content) {
        if let Some(region) = self.scene.get_mut(id) {
            region.content = content;
            region.mark_dirty();
            self.scene_dirty = true;
        } else {
            warn!("SetContent for unknown region {:?}", id);
        }
    }

    /// Handle DeleteRegion.
    fn handle_delete_region(&mut self, id: RegionId) {
        if self.scene.remove(id).is_some() {
            self.scene_dirty = true;
            self.subscriptions.remove(&id);
            debug!("Deleted region {:?}", id);
        } else {
            warn!("DeleteRegion for unknown region {:?}", id);
        }
    }

    /// Handle ReparentRegion.
    fn handle_reparent_region(&mut self, id: RegionId, new_parent: RegionId, insert_index: i32) {
        // Remove from old parent, add to new parent
        if let Some(region) = self.scene.get_mut(id) {
            let old_parent = region.parent;
            region.parent = Some(new_parent);

            // Remove from old parent's children
            if let Some(old_id) = old_parent {
                if let Some(old) = self.scene.get_mut(old_id) {
                    old.children.retain(|&c| c != id);
                }
            }

            // Add to new parent's children at specified index
            if let Some(new) = self.scene.get_mut(new_parent) {
                if insert_index >= 0 && (insert_index as usize) <= new.children.len() {
                    new.children.insert(insert_index as usize, id);
                } else {
                    new.children.push(id);
                }
            }

            self.scene_dirty = true;
            debug!("Reparented region {:?} to {:?}", id, new_parent);
        } else {
            warn!("ReparentRegion for unknown region {:?}", id);
        }
    }

    /// Handle BeginFrame.
    fn handle_begin_frame(&mut self, frame_id: FrameId) {
        self.current_frame = Some(frame_id);
        debug!("Begin frame {:?}", frame_id);
    }

    /// Handle EndFrame - compute layout and send LayoutComputed.
    fn handle_end_frame(&mut self, frame_id: FrameId) -> io::Result<()> {
        debug!("End frame {:?}", frame_id);

        if self.current_frame != Some(frame_id) {
            warn!(
                "EndFrame mismatch: expected {:?}, got {:?}",
                self.current_frame, frame_id
            );
        }

        self.current_frame = None;

        // Compute layout if scene changed
        if self.scene_dirty {
            self.compute_and_send_layout(Some(frame_id), LayoutTrigger::ConstraintChange)?;
            self.scene_dirty = false;
        }

        Ok(())
    }

    /// Compute layout and send LayoutComputed to client.
    fn compute_and_send_layout(
        &mut self,
        frame_id: Option<FrameId>,
        trigger: LayoutTrigger,
    ) -> io::Result<()> {
        // Sync scene tree to Taffy
        self.layout.sync(&self.scene);

        // Compute layout
        self.layout
            .compute(self.viewport_width, self.viewport_height);

        // Collect computed layouts
        let mut regions = Vec::new();
        for region in self.scene.iter_depth_first() {
            if let Some(taffy_layout) = self.layout.get_layout(region.id) {
                // Update the region's computed layout
                // (We can't mutate while iterating, so we'll do a second pass)
                let computed = ComputedRegionLayout {
                    id: region.id,
                    x: taffy_layout.location.x,
                    y: taffy_layout.location.y,
                    width: taffy_layout.size.width,
                    height: taffy_layout.size.height,
                    content_x: taffy_layout.location.x + taffy_layout.padding.left,
                    content_y: taffy_layout.location.y + taffy_layout.padding.top,
                    content_width: taffy_layout.size.width
                        - taffy_layout.padding.left
                        - taffy_layout.padding.right,
                    content_height: taffy_layout.size.height
                        - taffy_layout.padding.top
                        - taffy_layout.padding.bottom,
                    visible: true,
                };
                regions.push(computed);
            }
        }

        // Update regions with computed layouts
        for layout in &regions {
            if let Some(region) = self.scene.get_mut(layout.id) {
                region.computed = ComputedLayout {
                    x: layout.x,
                    y: layout.y,
                    width: layout.width,
                    height: layout.height,
                };
                region.dirty = false;
            }
        }

        // Send LayoutComputed
        let msg = TerminalMessage::LayoutComputed {
            frame_id,
            trigger,
            regions,
        };
        self.send_message(&msg)?;

        debug!(
            "Computed layout for {} regions",
            self.scene.iter_depth_first().count()
        );

        Ok(())
    }

    /// Send a message to the client.
    fn send_message(&mut self, msg: &TerminalMessage) -> io::Result<()> {
        msg.encode(&mut self.write_buf);
        debug!("Session {} queued message: {:?}", self.session_id, msg);
        Ok(())
    }

    /// Flush pending writes to socket.
    fn flush_writes(&mut self) -> io::Result<()> {
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
                    // Can't write more now, try again later
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn scene(&self) -> &SceneTree {
        &self.scene
    }

    /// Get the currently focused region.
    pub fn focused_region(&self) -> Option<RegionId> {
        self.focused_region
    }

    /// Get scroll offset for a region.
    pub fn scroll_offset(&self, id: RegionId) -> (f32, f32) {
        self.scroll_offsets.get(&id).copied().unwrap_or((0.0, 0.0))
    }

    /// Advance animated scroll positions toward their targets.
    /// Returns `true` if any animations are still active (needs another frame).
    pub fn update_scroll_animations(&mut self) -> bool {
        const LERP_FACTOR: f32 = 0.15;
        const EPSILON: f32 = 0.5;

        let mut done = Vec::new();
        for (&id, &(tx, ty)) in &self.scroll_targets {
            let (cx, cy) = self.scroll_offsets.get(&id).copied().unwrap_or((0.0, 0.0));
            let nx = cx + (tx - cx) * LERP_FACTOR;
            let ny = cy + (ty - cy) * LERP_FACTOR;
            let nx = if (nx - tx).abs() < EPSILON { tx } else { nx };
            let ny = if (ny - ty).abs() < EPSILON { ty } else { ny };
            self.scroll_offsets.insert(id, (nx, ny));
            if nx == tx && ny == ty {
                done.push(id);
            }
        }
        for id in done {
            self.scroll_targets.remove(&id);
        }
        if !self.scroll_targets.is_empty() {
            self.scene_dirty = true;
        }
        !self.scroll_targets.is_empty()
    }

    pub fn has_pending_writes(&self) -> bool {
        !self.write_buf.is_empty()
    }

    pub fn stream_fd(&self) -> std::os::unix::io::BorrowedFd<'_> {
        use std::os::unix::io::AsFd;
        self.stream.as_fd()
    }

    pub fn send_input(&mut self, event: ultty_protocol::InputEvent) -> io::Result<()> {
        self.send_message(&TerminalMessage::Input(event))
    }

    pub fn send_resize(&mut self, width: u16, height: u16) -> io::Result<()> {
        self.send_message(&TerminalMessage::Resize(ultty_protocol::ResizeEvent {
            width,
            height,
        }))
    }

    /// Hit-test: find the deepest region containing the point (in cell coords).
    /// Returns (region_id, region-local x, region-local y).
    pub fn hit_test(&self, x: f32, y: f32) -> (RegionId, f32, f32) {
        let mut best = (RegionId::ROOT, x, y);
        self.hit_test_recursive(self.scene.root(), x, y, &mut best);
        best
    }

    fn hit_test_recursive(
        &self,
        region_id: RegionId,
        x: f32,
        y: f32,
        best: &mut (RegionId, f32, f32),
    ) {
        if let Some(region) = self.scene.get(region_id) {
            let c = &region.computed;
            if x >= c.x && x < c.x + c.width && y >= c.y && y < c.y + c.height {
                *best = (region_id, x - c.x, y - c.y);
                // Check children (later children are rendered on top, so iterate forward)
                for &child_id in &region.children {
                    self.hit_test_recursive(child_id, x, y, best);
                }
            }
        }
    }

    /// Check if a region is subscribed to the given event mask.
    pub fn is_subscribed(&self, region_id: RegionId, event_mask: u32) -> bool {
        self.subscriptions
            .get(&region_id)
            .is_some_and(|mask| mask & event_mask != 0)
    }

    pub fn send_key(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) -> io::Result<()> {
        self.send_input(ultty_protocol::InputEvent {
            region_id: RegionId::ROOT,
            event_type: ultty_protocol::InputType::KeyDown { key, modifiers },
            x: 0.0,
            y: 0.0,
        })
    }
}
