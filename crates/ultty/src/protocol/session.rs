//! Native protocol session management.
//!
//! A session represents a connection from a TUI application to the terminal.
//! It owns the scene graph and layout engine for that connection.

mod commands;
mod dirty;
mod errors;
mod frame;
mod transport;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::os::unix::net::UnixStream;

use bytes::BytesMut;
use tracing::{debug, error, info, warn};
use ultty_protocol::{
    AppMessage, Dimension, Display, FlexDirection, FrameId, LayoutTrigger, RegionId,
    TerminalMessage, CAP_APPEND_CONTENT, CAP_LAYOUT_FLEXBOX, CAP_REGION_METADATA,
    CAP_STREAM_LIFECYCLE, CAP_WIDGET_SCROLL, PROTOCOL_VERSION,
};
#[cfg(test)]
use ultty_protocol::{Content, Style};

use crate::scene::layout::LayoutEngine;
#[cfg(test)]
use crate::scene::region::Region;
use crate::scene::tree::SceneTree;

/// Capabilities supported by this terminal.
const TERMINAL_CAPABILITIES: u32 = CAP_LAYOUT_FLEXBOX
    | CAP_WIDGET_SCROLL
    | CAP_APPEND_CONTENT
    | CAP_STREAM_LIFECYCLE
    | CAP_REGION_METADATA;

/// Maximum read buffer size (16 MB). Sessions exceeding this are disconnected.
const MAX_READ_BUF: usize = 16 * 1024 * 1024;
const MAX_RECENT_PROTOCOL_ERRORS: usize = 5;

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
    pending_frame_ops: Vec<AppMessage>,
    /// Whether scene changed during current frame.
    scene_dirty: bool,
    /// Whether layout constraints changed (requires Taffy recomputation).
    constraints_dirty: bool,
    /// Whether visual state changed (requires vertex buffer rebuild).
    render_dirty: bool,
    /// Regions whose content/style changed since last render.
    /// When empty and render_dirty is false, the renderer can skip rebuilding.
    /// When scene_dirty is true, this set is meaningless (full rebuild needed).
    dirty_regions: HashSet<RegionId>,
    /// Whether scroll offsets changed since last render (requires clip/scroll cache rebuild).
    scroll_dirty: bool,
    /// Negotiated capabilities.
    capabilities: u32,
    /// Negotiated protocol version.
    protocol_version: u16,
    /// Event subscriptions: region_id -> event mask.
    subscriptions: HashMap<RegionId, u32>,
    /// Currently focused region (if any).
    focused_region: Option<RegionId>,
    /// Per-region scroll offsets: region_id -> (scroll_x, scroll_y).
    scroll_offsets: HashMap<RegionId, (f32, f32)>,
    /// Animated scroll targets: region_id -> (target_x, target_y).
    scroll_targets: HashMap<RegionId, (f32, f32)>,
    /// Active content streams: region_id -> stream_id.
    active_streams: HashMap<RegionId, u32>,
    /// Pending clipboard get request.
    pending_clipboard_get: bool,
    /// Pending clipboard set request.
    pending_clipboard_set: Option<String>,
    /// Active style transitions: region_id -> (start_time, start_opacity, target_style).
    style_animations: HashMap<RegionId, StyleAnimation>,
    /// Time of last data received from client.
    last_activity: std::time::Instant,
    /// Time of last ping sent (if any).
    last_ping_sent: Option<std::time::Instant>,
    /// Whether we're waiting for a pong reply.
    awaiting_pong: bool,
    /// Most recent protocol errors sent to the client.
    recent_protocol_errors: VecDeque<String>,
    /// Last successful layout computation duration.
    last_layout_duration: Option<std::time::Duration>,
}

/// Tracks an in-progress style transition for a region.
struct StyleAnimation {
    /// When the animation started.
    start: std::time::Instant,
    /// Duration of the transition.
    duration: std::time::Duration,
    /// Easing function.
    easing: ultty_protocol::Easing,
    /// Property mask.
    properties: u32,
    /// Starting opacity value.
    start_opacity: f32,
    /// Starting background color (as RGB, if available).
    start_bg: Option<(u8, u8, u8)>,
    /// Starting foreground color (as RGB, if available).
    start_fg: Option<(u8, u8, u8)>,
    /// Target style (the final state).
    target_style: ultty_protocol::Style,
}

impl NativeSession {
    /// Create a new session from an accepted socket connection.
    pub fn new(stream: UnixStream, session_id: u64) -> io::Result<Self> {
        stream.set_nonblocking(true)?;

        let mut scene = SceneTree::new();
        if let Some(root) = scene.get_mut(RegionId::ROOT) {
            root.constraints.width = Dimension::Percent(100.0);
            root.constraints.height = Dimension::Percent(100.0);
            root.constraints.display = Display::Flex;
            root.constraints.flex_direction = FlexDirection::Column;
        }

        Ok(Self {
            session_id,
            scene,
            layout: LayoutEngine::new(),
            stream,
            read_buf: BytesMut::with_capacity(8192),
            write_buf: BytesMut::with_capacity(8192),
            viewport_width: 80.0,
            viewport_height: 24.0,
            current_frame: None,
            pending_frame_ops: Vec::new(),
            scene_dirty: false,
            constraints_dirty: false,
            render_dirty: true,
            dirty_regions: HashSet::new(),
            scroll_dirty: true,
            capabilities: 0,
            protocol_version: PROTOCOL_VERSION,
            subscriptions: HashMap::new(),
            focused_region: None,
            scroll_offsets: HashMap::new(),
            scroll_targets: HashMap::new(),
            active_streams: HashMap::new(),
            pending_clipboard_get: false,
            pending_clipboard_set: None,
            style_animations: HashMap::new(),
            last_activity: std::time::Instant::now(),
            last_ping_sent: None,
            awaiting_pong: false,
            recent_protocol_errors: VecDeque::with_capacity(MAX_RECENT_PROTOCOL_ERRORS),
            last_layout_duration: None,
        })
    }

    /// Set viewport dimensions (in terminal cells).
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        if (self.viewport_width - width).abs() > 0.1 || (self.viewport_height - height).abs() > 0.1
        {
            self.viewport_width = width;
            self.viewport_height = height;
            if let Some(root) = self.scene.get_mut(RegionId::ROOT) {
                root.constraints.width = Dimension::Percent(100.0);
                root.constraints.height = Dimension::Percent(100.0);
                root.constraints.display = Display::Flex;
                root.constraints.flex_direction = FlexDirection::Column;
            }
            self.mark_full_scene_scroll_rebuild();
        }
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn viewport(&self) -> (f32, f32) {
        (self.viewport_width, self.viewport_height)
    }

    /// Return the negotiated protocol version.
    pub fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    /// Return the negotiated capability bitset.
    pub fn capabilities(&self) -> u32 {
        self.capabilities
    }

    /// Return the most recent successful layout computation duration.
    pub fn last_layout_duration(&self) -> Option<std::time::Duration> {
        self.last_layout_duration
    }

    /// Return recent protocol errors sent to the client.
    pub fn recent_protocol_errors(&self) -> impl Iterator<Item = &str> {
        self.recent_protocol_errors
            .iter()
            .map(std::string::String::as_str)
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

    /// Handle a decoded message. Returns false if session should close.
    fn handle_message(&mut self, msg: AppMessage) -> io::Result<bool> {
        debug!("Session {} received: {:?}", self.session_id, msg);

        match self.handle_frame_message(msg)? {
            frame::FrameMessageOutcome::Handled(result) => Ok(result),
            frame::FrameMessageOutcome::Apply(msg) => self.apply_message(*msg),
        }
    }

    /// Compute layout and send LayoutComputed to client.
    fn compute_and_send_layout(
        &mut self,
        frame_id: Option<FrameId>,
        trigger: LayoutTrigger,
    ) -> io::Result<()> {
        let layout_start = std::time::Instant::now();
        if self.constraints_dirty {
            // Taffy panics on stale SlotMap keys, so structural frames rebuild
            // from the authoritative scene tree instead of reusing old nodes.
            self.layout = LayoutEngine::new();
        }
        let regions = match self.layout.compute_scene_layout(
            &mut self.scene,
            self.viewport_width,
            self.viewport_height,
        ) {
            Ok(regions) => regions,
            Err(e) => {
                warn!("Session {} layout error: {}", self.session_id, e);
                self.queue_protocol_error(errors::NativeProtocolError::layout_failed(e));
                return Ok(());
            }
        };
        self.last_layout_duration = Some(layout_start.elapsed());

        // Send LayoutComputed
        let msg = TerminalMessage::LayoutComputed {
            frame_id,
            trigger,
            regions,
        };
        self.send_message(&msg)?;

        debug!(
            "Computed layout for {} regions (viewport {}x{})",
            self.scene.iter_depth_first().count(),
            self.viewport_width,
            self.viewport_height,
        );

        Ok(())
    }

    /// Flush pending writes to socket.
    pub fn flush(&mut self) -> io::Result<()> {
        self.flush_writes()
    }

    /// Check keepalive state for this session.
    ///
    /// Returns `Ok(true)` if the session is alive, `Ok(false)` if it should be
    /// disconnected due to timeout.
    ///
    /// - Idle timeout: 120s with no data → close
    /// - Ping interval: 30s since last activity → send Ping
    /// - Pong timeout: 10s after ping with no pong → close
    pub fn check_keepalive(&mut self) -> io::Result<bool> {
        let now = std::time::Instant::now();
        let idle = now.duration_since(self.last_activity);

        // Hard idle timeout: 120 seconds
        if idle > std::time::Duration::from_secs(120) {
            info!(
                "Session {} idle timeout ({}s)",
                self.session_id,
                idle.as_secs()
            );
            return Ok(false);
        }

        // Pong timeout: if we sent a ping and haven't received pong in 10s
        if self.awaiting_pong {
            if let Some(ping_time) = self.last_ping_sent {
                if now.duration_since(ping_time) > std::time::Duration::from_secs(10) {
                    info!("Session {} pong timeout", self.session_id,);
                    return Ok(false);
                }
            }
        }

        // Ping interval: 30s since last activity, send ping if not already awaiting
        if idle > std::time::Duration::from_secs(30) && !self.awaiting_pong {
            debug!("Session {} sending ping", self.session_id);
            self.send_message(&TerminalMessage::Ping)?;
            self.last_ping_sent = Some(now);
            self.awaiting_pong = true;
        }

        Ok(true)
    }

    /// Returns true if there are active style animations in progress.
    pub fn has_active_style_animations(&self) -> bool {
        !self.style_animations.is_empty()
    }

    /// Returns true if the session's visual state has changed since last render.
    pub fn is_render_dirty(&self) -> bool {
        self.render_dirty
    }

    /// Clear the render dirty flag (call after rebuilding vertex buffers).
    pub fn clear_render_dirty(&mut self) {
        self.render_dirty = false;
        self.dirty_regions.clear();
    }

    /// Returns the set of regions that changed since last render.
    pub fn dirty_regions(&self) -> &HashSet<RegionId> {
        &self.dirty_regions
    }

    /// Returns true if scroll offsets or constraints changed since last render,
    /// meaning the scroll/clip caches need rebuilding.
    pub fn is_scroll_dirty(&self) -> bool {
        self.scroll_dirty
    }

    /// Clear the scroll dirty flag (call after rebuilding scroll/clip caches).
    pub fn clear_scroll_dirty(&mut self) {
        self.scroll_dirty = false;
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

    /// Check if a region is currently streaming content.
    pub fn is_streaming(&self, region_id: ultty_protocol::RegionId) -> bool {
        self.active_streams.contains_key(&region_id)
    }

    /// Advance animated scroll positions toward their targets.
    /// Returns `true` if any animations are still active (needs another frame).
    pub fn update_scroll_animations(&mut self) -> bool {
        const LERP_FACTOR: f32 = 0.15;
        const EPSILON: f32 = 0.5;

        let mut done = Vec::new();
        let mut notify = Vec::new();
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
            if self.is_subscribed(id, ultty_protocol::EVENT_SCROLL) {
                notify.push((id, nx, ny));
            }
        }
        for id in done {
            self.scroll_targets.remove(&id);
        }
        for (id, sx, sy) in notify {
            let _ = self.send_scroll_event(id, sx, sy);
        }
        if !self.scroll_targets.is_empty() {
            self.mark_scroll_animation_changed();
        }
        !self.scroll_targets.is_empty()
    }

    /// Advance style animations toward their targets.
    /// Returns `true` if any animations are still active.
    pub fn update_style_animations(&mut self) -> bool {
        if self.style_animations.is_empty() {
            return false;
        }

        let now = std::time::Instant::now();
        let mut done = Vec::new();

        for (&id, anim) in &self.style_animations {
            let elapsed = now.duration_since(anim.start);
            let t_linear = (elapsed.as_secs_f32() / anim.duration.as_secs_f32()).min(1.0);
            let t = anim.easing.apply(t_linear);
            let finished = t_linear >= 1.0;

            if let Some(region) = self.scene.get_mut(id) {
                // Interpolate opacity
                if anim.properties & ultty_protocol::TRANSITION_OPACITY != 0 {
                    let target_opacity = anim.target_style.opacity;
                    region.style.opacity =
                        anim.start_opacity + (target_opacity - anim.start_opacity) * t;
                }

                // Interpolate background color
                if anim.properties & ultty_protocol::TRANSITION_BG != 0 {
                    if let (Some((sr, sg, sb)), Some(ultty_protocol::Color::Rgb(tr, tg, tb))) =
                        (anim.start_bg, &anim.target_style.bg)
                    {
                        let r = Self::lerp_u8(sr, *tr, t);
                        let g = Self::lerp_u8(sg, *tg, t);
                        let b = Self::lerp_u8(sb, *tb, t);
                        region.style.bg = Some(ultty_protocol::Color::Rgb(r, g, b));
                    }
                }

                // Interpolate foreground color
                if anim.properties & ultty_protocol::TRANSITION_FG != 0 {
                    if let (Some((sr, sg, sb)), Some(ultty_protocol::Color::Rgb(tr, tg, tb))) =
                        (anim.start_fg, &anim.target_style.fg)
                    {
                        let r = Self::lerp_u8(sr, *tr, t);
                        let g = Self::lerp_u8(sg, *tg, t);
                        let b = Self::lerp_u8(sb, *tb, t);
                        region.style.fg = Some(ultty_protocol::Color::Rgb(r, g, b));
                    }
                }

                // Copy non-animatable properties immediately
                region.style.bold = anim.target_style.bold;
                region.style.italic = anim.target_style.italic;
                region.style.underline = anim.target_style.underline;
                region.style.dim = anim.target_style.dim;
                region.style.border = anim.target_style.border;
                region.style.border_color = anim.target_style.border_color;
                region.style.transition = anim.target_style.transition;

                if finished {
                    // Snap to final state
                    region.style = anim.target_style.clone();
                    done.push(id);
                }

                region.mark_dirty();
            } else {
                done.push(id);
            }
        }

        for id in done {
            self.style_animations.remove(&id);
        }

        let active = !self.style_animations.is_empty();
        if active {
            self.mark_animation_visual_changed();
        }
        active
    }

    /// Linearly interpolate between two u8 values.
    fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
        let result = a as f32 + (b as f32 - a as f32) * t;
        result.round().clamp(0.0, 255.0) as u8
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

    /// Target region for key events: the focused region if it has an EVENT_KEY subscription,
    /// otherwise ROOT.
    fn key_target(&self) -> RegionId {
        if let Some(focused) = self.focused_region {
            if self.is_subscribed(focused, ultty_protocol::EVENT_KEY) {
                return focused;
            }
        }
        RegionId::ROOT
    }

    /// Returns true if any region in this session is subscribed to key events.
    pub fn has_key_subscription(&self) -> bool {
        self.subscriptions
            .values()
            .any(|mask| mask & ultty_protocol::EVENT_KEY != 0)
    }

    pub fn send_key(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) -> io::Result<()> {
        if !self.has_key_subscription() {
            return Ok(());
        }
        let target = self.key_target();
        self.send_input(ultty_protocol::InputEvent {
            region_id: target,
            event_type: ultty_protocol::InputType::KeyDown { key, modifiers },
            x: 0.0,
            y: 0.0,
            modifiers,
        })
    }

    pub fn send_key_up(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) -> io::Result<()> {
        if !self.has_key_subscription() {
            return Ok(());
        }
        let target = self.key_target();
        self.send_input(ultty_protocol::InputEvent {
            region_id: target,
            event_type: ultty_protocol::InputType::KeyUp { key, modifiers },
            x: 0.0,
            y: 0.0,
            modifiers,
        })
    }

    pub fn send_text_input(&mut self, text: String) -> io::Result<()> {
        if !self.has_key_subscription() {
            return Ok(());
        }
        let target = self.key_target();
        self.send_input(ultty_protocol::InputEvent {
            region_id: target,
            event_type: ultty_protocol::InputType::TextInput { text },
            x: 0.0,
            y: 0.0,
            modifiers: ultty_protocol::Modifiers::NONE,
        })
    }

    /// Take the pending clipboard get flag (returns true if a get was requested, then clears it).
    pub fn take_clipboard_get(&mut self) -> bool {
        std::mem::take(&mut self.pending_clipboard_get)
    }

    /// Take the pending clipboard set text (returns the text if a set was requested, then clears it).
    pub fn take_clipboard_set(&mut self) -> Option<String> {
        self.pending_clipboard_set.take()
    }

    /// Send clipboard content to the client.
    pub fn send_clipboard_content(&mut self, text: Option<String>) -> io::Result<()> {
        self.send_message(&TerminalMessage::ClipboardContent { text })
    }

    /// Send a scroll event notification to the client for a specific region.
    pub fn send_scroll_event(
        &mut self,
        id: RegionId,
        scroll_x: f32,
        scroll_y: f32,
    ) -> io::Result<()> {
        self.send_message(&TerminalMessage::Scroll(ultty_protocol::ScrollEvent {
            region_id: id,
            scroll_x,
            scroll_y,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::region::ComputedLayout;
    use std::os::unix::net::UnixStream;

    fn make_session() -> NativeSession {
        let (s1, _s2) = UnixStream::pair().unwrap();
        NativeSession::new(s1, 1).unwrap()
    }

    fn negotiate_all_test_capabilities(session: &mut NativeSession) {
        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_LAYOUT_FLEXBOX
                    | CAP_WIDGET_SCROLL
                    | CAP_APPEND_CONTENT
                    | CAP_STREAM_LIFECYCLE
                    | CAP_REGION_METADATA,
            })
            .unwrap();
    }

    #[test]
    fn hit_test_returns_root_when_empty() {
        let session = make_session();
        let (id, x, y) = session.hit_test(5.0, 5.0);
        assert_eq!(id, RegionId::ROOT);
        assert_eq!(x, 5.0);
        assert_eq!(y, 5.0);
    }

    #[test]
    fn hit_test_finds_child_region() {
        let mut session = make_session();

        // Set root computed layout
        if let Some(root) = session.scene.get_mut(RegionId::ROOT) {
            root.computed = ComputedLayout {
                x: 0.0,
                y: 0.0,
                width: 80.0,
                height: 24.0,
                ..Default::default()
            };
        }

        // Add a child with known bounds
        let mut child = Region::new(RegionId::new(1));
        child.parent = Some(RegionId::ROOT);
        child.computed = ComputedLayout {
            x: 10.0,
            y: 5.0,
            width: 20.0,
            height: 10.0,
            ..Default::default()
        };
        session.scene.insert(child);

        let (id, lx, ly) = session.hit_test(15.0, 8.0);
        assert_eq!(id, RegionId::new(1));
        assert_eq!(lx, 5.0); // 15 - 10
        assert_eq!(ly, 3.0); // 8 - 5
    }

    #[test]
    fn hit_test_misses_child_returns_root() {
        let mut session = make_session();
        if let Some(root) = session.scene.get_mut(RegionId::ROOT) {
            root.computed = ComputedLayout {
                x: 0.0,
                y: 0.0,
                width: 80.0,
                height: 24.0,
                ..Default::default()
            };
        }

        let mut child = Region::new(RegionId::new(1));
        child.parent = Some(RegionId::ROOT);
        child.computed = ComputedLayout {
            x: 50.0,
            y: 50.0,
            width: 10.0,
            height: 10.0,
            ..Default::default()
        };
        session.scene.insert(child);

        let (id, _, _) = session.hit_test(5.0, 5.0);
        assert_eq!(id, RegionId::ROOT);
    }

    #[test]
    fn scroll_offset_default_zero() {
        let session = make_session();
        assert_eq!(session.scroll_offset(RegionId::new(99)), (0.0, 0.0));
    }

    #[test]
    fn scroll_animation_converges() {
        let mut session = make_session();
        let id = RegionId::new(1);

        // Set a target
        session.scroll_offsets.insert(id, (0.0, 0.0));
        session.scroll_targets.insert(id, (100.0, 200.0));

        // Run animation until convergence
        let mut iterations = 0;
        while session.update_scroll_animations() {
            iterations += 1;
            if iterations > 200 {
                panic!("Scroll animation did not converge");
            }
        }

        let (x, y) = session.scroll_offset(id);
        assert_eq!(x, 100.0);
        assert_eq!(y, 200.0);
        assert!(iterations > 1, "Should take multiple iterations");
    }

    #[test]
    fn scroll_no_animation_when_no_targets() {
        let mut session = make_session();
        assert!(!session.update_scroll_animations());
    }

    #[test]
    fn subscription_check() {
        let mut session = make_session();
        let id = RegionId::new(5);

        assert!(!session.is_subscribed(id, ultty_protocol::EVENT_MOUSE));

        session
            .subscriptions
            .insert(id, ultty_protocol::EVENT_MOUSE);
        assert!(session.is_subscribed(id, ultty_protocol::EVENT_MOUSE));
        assert!(!session.is_subscribed(id, ultty_protocol::EVENT_KEY));
    }

    #[test]
    fn focused_region_default_none() {
        let session = make_session();
        assert_eq!(session.focused_region(), None);
    }

    #[test]
    fn session_id_accessor() {
        let session = make_session();
        assert_eq!(session.session_id(), 1);
    }

    #[test]
    fn viewport_set() {
        let mut session = make_session();
        session.set_viewport(120.0, 40.0);
        // No direct accessor, but shouldn't panic
    }

    #[test]
    fn viewport_resize_marks_full_scene_and_scroll_dirty() {
        let mut session = make_session();
        session.scene_dirty = false;
        session.constraints_dirty = false;
        session.render_dirty = false;
        session.scroll_dirty = false;

        session.set_viewport(120.0, 40.0);

        assert!(session.scene_dirty);
        assert!(session.constraints_dirty);
        assert!(session.render_dirty);
        assert!(session.scroll_dirty);
    }

    #[test]
    fn client_hello_negotiates_version_and_capabilities() {
        let mut session = make_session();

        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION + 2,
                requested_capabilities: CAP_APPEND_CONTENT | CAP_WIDGET_SCROLL,
            })
            .unwrap();

        assert_eq!(session.protocol_version, PROTOCOL_VERSION);
        assert_eq!(session.capabilities, CAP_APPEND_CONTENT | CAP_WIDGET_SCROLL);
    }

    #[test]
    fn append_content_requires_negotiated_capability() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        child.content = Content::Text(ultty_protocol::TextContent::new("Hello"));
        session.scene.insert(child);

        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_LAYOUT_FLEXBOX,
            })
            .unwrap();

        session
            .handle_message(AppMessage::AppendContent {
                id,
                text: " World".to_string(),
            })
            .unwrap();

        let region = session.scene.get(id).unwrap();
        match &region.content {
            Content::Text(tc) => assert_eq!(tc.text, "Hello"),
            other => panic!("Expected Text content, got {:?}", other),
        }
    }

    #[test]
    fn stream_lifecycle_requires_negotiated_capability() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);

        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_LAYOUT_FLEXBOX,
            })
            .unwrap();

        session
            .handle_message(AppMessage::StreamStart { id, stream_id: 7 })
            .unwrap();
        session
            .handle_message(AppMessage::StreamEnd { id, stream_id: 7 })
            .unwrap();

        assert!(!session.active_streams.contains_key(&id));
    }

    #[test]
    fn append_content_to_text_region() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        child.content = Content::Text(ultty_protocol::TextContent::new("Hello"));
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        session
            .handle_message(AppMessage::AppendContent {
                id,
                text: " World".to_string(),
            })
            .unwrap();

        let region = session.scene.get(id).unwrap();
        match &region.content {
            Content::Text(tc) => assert_eq!(tc.text, "Hello World"),
            other => panic!("Expected Text content, got {:?}", other),
        }
    }

    #[test]
    fn append_content_to_rich_text_region_extends_last_span() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        child.content = Content::RichText(ultty_protocol::RichText::new().text("Hello"));
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        session
            .handle_message(AppMessage::AppendContent {
                id,
                text: " World".to_string(),
            })
            .unwrap();

        let region = session.scene.get(id).unwrap();
        match &region.content {
            Content::RichText(rich) => {
                assert_eq!(rich.spans.len(), 1);
                assert_eq!(rich.spans[0].text, "Hello World");
            }
            other => panic!("Expected RichText content, got {:?}", other),
        }
    }

    #[test]
    fn append_content_to_none_initializes_text() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        session
            .handle_message(AppMessage::AppendContent {
                id,
                text: "First chunk".to_string(),
            })
            .unwrap();

        let region = session.scene.get(id).unwrap();
        match &region.content {
            Content::Text(tc) => assert_eq!(tc.text, "First chunk"),
            other => panic!("Expected Text content, got {:?}", other),
        }
    }

    #[test]
    fn stream_start_end_tracking() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        // Start stream
        session
            .handle_message(AppMessage::StreamStart { id, stream_id: 42 })
            .unwrap();
        assert_eq!(session.active_streams.get(&id), Some(&42));

        // End stream
        session
            .handle_message(AppMessage::StreamEnd { id, stream_id: 42 })
            .unwrap();
        assert_eq!(session.active_streams.get(&id), None);
    }

    #[test]
    fn stream_end_mismatch_ignored() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        session
            .handle_message(AppMessage::StreamStart { id, stream_id: 42 })
            .unwrap();

        // End with wrong stream_id — should NOT remove
        session
            .handle_message(AppMessage::StreamEnd { id, stream_id: 99 })
            .unwrap();
        assert_eq!(session.active_streams.get(&id), Some(&42));
    }

    #[test]
    fn test_is_streaming_returns_true_during_stream() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        session
            .handle_message(AppMessage::StreamStart { id, stream_id: 42 })
            .unwrap();

        assert!(session.is_streaming(id));
    }

    #[test]
    fn test_is_streaming_returns_false_after_stream_end() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        session
            .handle_message(AppMessage::StreamStart { id, stream_id: 42 })
            .unwrap();
        session
            .handle_message(AppMessage::StreamEnd { id, stream_id: 42 })
            .unwrap();

        assert!(!session.is_streaming(id));
    }

    #[test]
    fn test_is_streaming_returns_false_for_unknown_region() {
        let session = make_session();

        assert!(!session.is_streaming(RegionId::new(9999)));
    }

    #[test]
    fn session_stores_metadata() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        negotiate_all_test_capabilities(&mut session);

        let metadata = vec![
            ("owner".to_string(), "agent".to_string()),
            ("task_id".to_string(), "auth-refactor".to_string()),
            ("locked".to_string(), "true".to_string()),
        ];

        session.scene_dirty = false;
        session.constraints_dirty = false;
        session.render_dirty = false;

        session
            .handle_message(AppMessage::SetRegionMetadata {
                id,
                metadata: metadata.clone(),
            })
            .unwrap();

        let stored = &session.scene.get(id).unwrap().metadata;
        assert_eq!(stored, &metadata);
        assert!(!session.scene_dirty);
        assert!(!session.constraints_dirty);
        assert!(!session.render_dirty);
    }

    #[test]
    fn dirty_regions_tracks_content_changes() {
        let mut session = make_session();
        let id = RegionId::new(1);

        // Insert a region directly
        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        session.clear_render_dirty();

        // SetContent should add to dirty_regions
        session
            .handle_message(AppMessage::SetContent {
                id,
                content: Content::Text(ultty_protocol::TextContent::new("hello".to_string())),
            })
            .unwrap();
        assert!(session.is_render_dirty());
        assert!(session.dirty_regions().contains(&id));

        // clear_render_dirty should clear dirty_regions
        session.clear_render_dirty();
        assert!(!session.is_render_dirty());
        assert!(session.dirty_regions().is_empty());
    }

    #[test]
    fn set_constraints_marks_constraint_and_scroll_dirty() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        session.scene_dirty = false;
        session.constraints_dirty = false;
        session.render_dirty = false;
        session.scroll_dirty = false;
        session.dirty_regions.clear();

        session
            .handle_message(AppMessage::SetConstraints {
                id,
                constraints: ultty_protocol::Constraints {
                    width: Dimension::Cells(20.0),
                    ..Default::default()
                },
            })
            .unwrap();

        assert!(session.scene_dirty);
        assert!(session.constraints_dirty);
        assert!(session.render_dirty);
        assert!(session.scroll_dirty);
        assert!(session.dirty_regions().contains(&id));
    }

    #[test]
    fn dirty_regions_tracks_style_changes() {
        let mut session = make_session();
        let id = RegionId::new(1);

        let mut child = Region::new(id);
        child.parent = Some(RegionId::ROOT);
        session.scene.insert(child);
        session.clear_render_dirty();

        session
            .handle_message(AppMessage::SetStyle {
                id,
                style: Style {
                    opacity: 0.5,
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(session.dirty_regions().contains(&id));
    }
}
