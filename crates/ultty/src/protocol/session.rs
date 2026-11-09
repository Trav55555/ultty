//! Native protocol session management.
//!
//! A session represents a connection from a TUI application to the terminal.
//! It owns the scene graph and layout engine for that connection.

mod commands;
mod content_limits;
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
    TerminalMessage, CAP_APPEND_CONTENT, CAP_LAYOUT_FLEXBOX, CAP_REGION_METADATA, CAP_SEMANTICS,
    CAP_STREAM_LIFECYCLE, CAP_WIDGET_SCROLL, PROTOCOL_VERSION,
};
#[cfg(test)]
use ultty_protocol::{Content, Style};

use ultty_protocol::control::{NativeScene, RegionInfo};

use crate::scene::layout::LayoutEngine;
#[cfg(test)]
use crate::scene::region::Region;
use crate::scene::semantics::effective_semantics;
use crate::scene::tree::SceneTree;
use crate::terminal_manager::TabId;

/// A region's content kind as `ulttyctl regions` names it, and its text
/// when it has some. A password input's value is never reported.
fn describe_content(content: &ultty_protocol::Content) -> (&'static str, Option<String>) {
    use ultty_protocol::{Content, WidgetContent};
    match content {
        Content::None => ("none", None),
        Content::Text(text) => ("text", Some(text.text.clone())),
        Content::RichText(rich) => (
            "rich_text",
            Some(rich.spans.iter().map(|span| span.text.as_str()).collect()),
        ),
        Content::Widget(widget) => match widget {
            WidgetContent::Input(input) => {
                ("input", (!input.password).then(|| input.value.clone()))
            }
            WidgetContent::Markdown(markdown) => ("markdown", Some(markdown.source.clone())),
            WidgetContent::Code(code) => ("code", Some(code.source.clone())),
            WidgetContent::Progress(_) => ("progress", None),
            WidgetContent::Spinner(_) => ("spinner", None),
            WidgetContent::Image(_) => ("image", None),
            WidgetContent::Button(button) => ("button", Some(button.label.clone())),
        },
    }
}

/// Capabilities supported by this terminal.
const TERMINAL_CAPABILITIES: u32 = CAP_LAYOUT_FLEXBOX
    | CAP_WIDGET_SCROLL
    | CAP_APPEND_CONTENT
    | CAP_STREAM_LIFECYCLE
    | CAP_REGION_METADATA
    | CAP_SEMANTICS;

/// An in-band session with no traffic from its app for this long has ended.
/// Clients send a heartbeat (an unsolicited `Pong`) at least every few
/// seconds while idle. The terminal never pings in-band: if the app has
/// died, a ping would land in the shell's input as typed text.
pub const IN_BAND_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long an in-band app has, after the terminal answers its
/// `ClientHello`, to send its next message (spec §1.3).
///
/// Bytes that merely contain a hello, such as a file being displayed,
/// never answer, so their session ends this soon instead of after
/// [`IN_BAND_IDLE_TIMEOUT`], and it is never shown or sent keys.
pub const IN_BAND_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Maximum read buffer size (16 MB). Sessions exceeding this are disconnected.
const MAX_READ_BUF: usize = 16 * 1024 * 1024;
const MAX_RECENT_PROTOCOL_ERRORS: usize = 5;

/// A native protocol session with a TUI application.
pub struct NativeSession {
    /// Session ID.
    session_id: u64,
    /// Tab whose socket accepted this connection.
    ///
    /// Input and clipboard access are routed by tab, and only sessions of
    /// the visible tab are rendered.
    tab_id: TabId,
    /// Scene tree for this session.
    scene: SceneTree,
    /// Upper bound on the bytes of text in `scene`; see
    /// [`NativeSession::admit_text`].
    text_bytes_bound: usize,
    /// Layout engine.
    layout: LayoutEngine,
    /// How the app is connected.
    link: Link,
    /// For a socket session, the process that connected (`SO_PEERCRED`).
    /// Keys go to it only while its process group is in the foreground of
    /// the focused pane.
    peer_pid: Option<i32>,
    /// Read buffer.
    read_buf: BytesMut,
    /// Write buffer.
    write_buf: BytesMut,
    /// Viewport dimensions.
    viewport_width: f32,
    viewport_height: f32,
    /// Current frame being built (if any).
    current_frame: Option<FrameId>,
    /// Operations queued since `BeginFrame`, applied at `EndFrame`.
    pending_frame_ops: Vec<AppMessage>,
    /// Wire bytes represented by `pending_frame_ops`, for bounding memory.
    pending_frame_bytes: usize,
    /// Whether scene changed during current frame.
    scene_dirty: bool,
    /// Whether layout constraints changed (requires Taffy recomputation).
    constraints_dirty: bool,
    /// Whether the region tree changed shape (create/delete/reparent).
    tree_dirty: bool,
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

/// How a session's app is connected.
enum Link {
    /// The tab's protocol socket.
    Socket(UnixStream),
    /// In-band strings on the tab's PTY (ADR 0018). The server feeds input
    /// with [`NativeSession::feed_in_band`] and collects output with
    /// [`NativeSession::take_in_band_output`].
    InBand {
        /// Frame bytes waiting to be written to the PTY.
        outgoing: Vec<u8>,
        /// Foreground process group of the PTY when the session started.
        pgrp: Option<i32>,
        /// Non-shell processes in that group then. One of them is the app.
        pids: Vec<i32>,
        /// How far the app has shown it is a live app reading its input.
        phase: InBandPhase,
    },
}

/// Progress of an in-band session from hello to a confirmed app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InBandPhase {
    /// The `ClientHello` is being answered.
    Hello,
    /// Answered at the given time; nothing has come back since. The scene is
    /// not shown, keys are not sent, and replies are held.
    Unconfirmed(std::time::Instant),
    /// The app sent more after the answer, so it reads its input.
    Confirmed,
}

fn process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists and may be
    // signalled; nothing is sent.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
    /// Create a new session from a connection accepted on `tab_id`'s socket.
    pub fn new(stream: UnixStream, session_id: u64, tab_id: TabId) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        let peer_pid = crate::platform::socket::peer_pid(&stream)
            .map_err(|e| warn!("Session {session_id}: cannot read peer pid: {e}"))
            .ok();
        let mut session = Self::with_link(Link::Socket(stream), session_id, tab_id);
        session.peer_pid = peer_pid;
        Ok(session)
    }

    /// Create a session whose app talks in-band on `tab_id`'s PTY, which had
    /// process group `pgrp`, holding `pids`, in the foreground when the app
    /// said hello.
    pub fn new_in_band(session_id: u64, tab_id: TabId, pgrp: Option<i32>, pids: Vec<i32>) -> Self {
        Self::with_link(
            Link::InBand {
                outgoing: Vec::new(),
                pgrp,
                pids,
                phase: InBandPhase::Hello,
            },
            session_id,
            tab_id,
        )
    }

    fn with_link(link: Link, session_id: u64, tab_id: TabId) -> Self {
        let mut scene = SceneTree::new();
        if let Some(root) = scene.get_mut(RegionId::ROOT) {
            root.constraints.width = Dimension::Percent(100.0);
            root.constraints.height = Dimension::Percent(100.0);
            root.constraints.display = Display::Flex;
            root.constraints.flex_direction = FlexDirection::Column;
        }

        Self {
            session_id,
            tab_id,
            scene,
            text_bytes_bound: 0,
            layout: LayoutEngine::new(),
            link,
            peer_pid: None,
            read_buf: BytesMut::with_capacity(8192),
            write_buf: BytesMut::with_capacity(8192),
            viewport_width: 80.0,
            viewport_height: 24.0,
            current_frame: None,
            pending_frame_ops: Vec::new(),
            pending_frame_bytes: 0,
            scene_dirty: false,
            constraints_dirty: false,
            tree_dirty: false,
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
        }
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

    /// Tab that owns this session.
    pub fn tab_id(&self) -> TabId {
        self.tab_id
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
        while let Some((msg, wire_len)) = self.try_decode_message()? {
            if !self.handle_wire_message(msg, wire_len)? {
                return Ok(false);
            }
        }

        // Flush any pending writes
        self.flush_writes()?;

        Ok(true)
    }

    /// Handle a decoded message that occupied `wire_len` bytes on the socket.
    /// Returns false if session should close.
    fn handle_wire_message(&mut self, msg: AppMessage, wire_len: usize) -> io::Result<bool> {
        debug!("Session {} received: {:?}", self.session_id, msg);

        match self.handle_frame_message(msg, wire_len)? {
            frame::FrameMessageOutcome::Handled(result) => Ok(result),
            frame::FrameMessageOutcome::Apply(msg) => self.apply_message(*msg),
        }
    }

    /// Test helper: handle a message as if it had arrived with its encoded size.
    #[cfg(test)]
    fn handle_message(&mut self, msg: AppMessage) -> io::Result<bool> {
        use ultty_protocol::Encode;
        let wire_len = msg.encode_to_bytes().len();
        self.handle_wire_message(msg, wire_len)
    }

    /// Recompute layout after a viewport resize and notify the client.
    ///
    /// Sessions that have not built a scene yet have nothing to lay out and
    /// receive only the `Resize` event.
    pub fn relayout_for_resize(&mut self) -> io::Result<()> {
        if self.scene.is_empty() {
            return Ok(());
        }
        self.compute_and_send_layout(None, LayoutTrigger::Resize)?;
        self.mark_scene_layout_clean();
        Ok(())
    }

    /// Compute layout and send LayoutComputed to client.
    ///
    /// The layout engine is reused across frames; `LayoutEngine::sync`
    /// reconciles created, deleted and moved regions incrementally.
    fn compute_and_send_layout(
        &mut self,
        frame_id: Option<FrameId>,
        trigger: LayoutTrigger,
    ) -> io::Result<()> {
        let layout_start = std::time::Instant::now();
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

        if self.is_in_band() {
            if let Link::InBand {
                phase: InBandPhase::Unconfirmed(since),
                ..
            } = self.link
            {
                if now.duration_since(since) > IN_BAND_CONFIRM_TIMEOUT {
                    info!(
                        "In-band session {} never answered its hello reply",
                        self.session_id
                    );
                    return Ok(false);
                }
            }
            if idle > IN_BAND_IDLE_TIMEOUT {
                info!(
                    "In-band session {} silent for {}s",
                    self.session_id,
                    idle.as_secs()
                );
                return Ok(false);
            }
            return Ok(true);
        }

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

    /// Force a full rebuild on the next render, e.g. when the session's tab
    /// becomes visible again and the renderer's cached buffers belong to
    /// another tab.
    pub fn mark_render_dirty(&mut self) {
        self.render_dirty = true;
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

    /// This app's scene for `ulttyctl regions`: every region, root first,
    /// in drawing order, with its layout, scroll, content and effective
    /// accessibility semantics.
    pub fn describe(&self) -> NativeScene {
        let regions = self
            .scene
            .iter_depth_first()
            .map(|region| {
                let (content, text) = describe_content(&region.content);
                let (scroll_x, scroll_y) = self.scroll_offset(region.id);
                let layout = region.computed;
                let semantics = effective_semantics(region);
                RegionInfo {
                    id: region.id.0,
                    parent: region.parent.map(|parent| parent.0),
                    depth: self.scene.depth_of(region.id).unwrap_or(0),
                    x: layout.x,
                    y: layout.y,
                    width: layout.width,
                    height: layout.height,
                    scroll_x,
                    scroll_y,
                    content: content.to_string(),
                    text,
                    metadata: region.metadata.iter().cloned().collect(),
                    role: semantics.role.as_str().to_string(),
                    label: semantics.label,
                    live: semantics.live.as_str().to_string(),
                }
            })
            .collect();
        NativeScene {
            session: self.session_id,
            in_band: self.is_in_band(),
            focused: self.focused_region.map(|id| id.0),
            regions,
        }
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
            // Snap when close, and also when the value is not finite: a NaN
            // or infinite offset would otherwise never converge.
            let nx = if !nx.is_finite() || (nx - tx).abs() < EPSILON {
                tx
            } else {
                nx
            };
            let ny = if !ny.is_finite() || (ny - ty).abs() < EPSILON {
                ty
            } else {
                ny
            };
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

    /// The session socket, for readiness polling. In-band sessions have none.
    pub fn stream_fd(&self) -> Option<std::os::unix::io::BorrowedFd<'_>> {
        use std::os::unix::io::AsFd;
        match &self.link {
            Link::Socket(stream) => Some(stream.as_fd()),
            Link::InBand { .. } => None,
        }
    }

    /// Whether the app talks in-band on the tab's PTY.
    pub fn is_in_band(&self) -> bool {
        matches!(self.link, Link::InBand { .. })
    }

    /// Whether the app has shown it is live: always for a socket session; for
    /// an in-band one, once it sent something after the hello was answered.
    ///
    /// Until then the session's scene is not shown, it gets no input, and
    /// replies other than the hello answer are held back, so a stray hello
    /// (a `cat`ed file, say) neither blanks the tab nor has input typed
    /// into the shell on its behalf.
    pub fn is_confirmed(&self) -> bool {
        match &self.link {
            Link::Socket(_) => true,
            Link::InBand { phase, .. } => *phase == InBandPhase::Confirmed,
        }
    }

    /// Note that more in-band data reached an answered session, which
    /// confirms it (see [`NativeSession::is_confirmed`]).
    pub fn confirm_in_band(&mut self) {
        if let Link::InBand { phase, .. } = &mut self.link {
            if matches!(phase, InBandPhase::Unconfirmed(_)) {
                *phase = InBandPhase::Confirmed;
                self.render_dirty = true;
            }
        }
    }

    /// After an in-band hello is answered, send the answer and hold what
    /// follows until the app confirms.
    pub(super) fn end_in_band_hello(&mut self) -> io::Result<()> {
        if !matches!(
            self.link,
            Link::InBand {
                phase: InBandPhase::Hello,
                ..
            }
        ) {
            return Ok(());
        }
        self.flush_writes()?;
        if let Link::InBand { phase, .. } = &mut self.link {
            *phase = InBandPhase::Unconfirmed(std::time::Instant::now());
        }
        Ok(())
    }

    /// For an in-band session, the PTY's foreground process group when it
    /// started.
    pub fn in_band_pgrp(&self) -> Option<i32> {
        match self.link {
            Link::InBand { pgrp, .. } => pgrp,
            Link::Socket(_) => None,
        }
    }

    /// Whether the in-band app still runs, judged from outside: the PTY's
    /// foreground group is still the one at hello, and at least one of the
    /// non-shell processes that were in it then is alive.
    ///
    /// The group check catches an app exiting under a job-control shell.
    /// The process check catches it without job control (a script, `bash -c`,
    /// `script`), where the group stays the same after the app dies; shells
    /// are left out because a wrapper script outlives the app. Requiring
    /// only one survivor means a helper the app ran exiting does not end the
    /// session. Neither check sees a remote app over ssh; that relies on
    /// `Disconnect` and the idle timeout.
    pub fn in_band_app_alive(&self, pgrp: Option<i32>) -> bool {
        match &self.link {
            Link::InBand {
                pgrp: started,
                pids,
                ..
            } => {
                *started == pgrp && (pids.is_empty() || pids.iter().any(|&pid| process_exists(pid)))
            }
            Link::Socket(_) => true,
        }
    }

    /// Add frame bytes that arrived in-band. Call [`NativeSession::poll`]
    /// afterwards to process them.
    pub fn feed_in_band(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.last_activity = std::time::Instant::now();
        self.read_buf.extend_from_slice(bytes);
        if self.read_buf.len() > MAX_READ_BUF {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "read buffer exceeded maximum size",
            ));
        }
        Ok(())
    }

    /// Frame bytes queued for an in-band app since the last call.
    pub fn take_in_band_output(&mut self) -> Vec<u8> {
        if let Err(e) = self.flush_writes() {
            warn!("Session {} flush failed: {}", self.session_id, e);
        }
        match &mut self.link {
            Link::InBand { outgoing, .. } => std::mem::take(outgoing),
            Link::Socket(_) => Vec::new(),
        }
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

    /// Hit-test against the last committed computed layout.
    ///
    /// During input handling this is effectively previous-frame hit testing:
    /// events are routed using what the user could actually see, not a
    /// speculative next layout. Returns `(region_id, region-local x,
    /// region-local y)` in cell coordinates.
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
                // Iterate in render order; because matches overwrite `best`,
                // later siblings win overlapping hit tests.
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

    /// Whether keys typed into a pane whose PTY has `foreground_pgrp` in the
    /// foreground may reach this session.
    ///
    /// An in-band app talks over that PTY and its session ends when the
    /// foreground changes, so it qualifies. A socket session qualifies only
    /// while the process that connected is in the foreground group: a
    /// background job that subscribed to keys must not see what the user
    /// types to the shell or to another program.
    pub fn is_in_foreground(&self, foreground_pgrp: Option<i32>) -> bool {
        if self.is_in_band() {
            return true;
        }
        let peer_group = self
            .peer_pid
            .and_then(crate::platform::socket::process_group);
        peer_group.is_some() && peer_group == foreground_pgrp
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
        NativeSession::new(s1, 1, TabId::new(0)).unwrap()
    }

    fn negotiate_all_test_capabilities(session: &mut NativeSession) {
        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_LAYOUT_FLEXBOX
                    | CAP_WIDGET_SCROLL
                    | CAP_APPEND_CONTENT
                    | CAP_STREAM_LIFECYCLE
                    | CAP_REGION_METADATA
                    | CAP_SEMANTICS,
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
    fn hit_test_overlapping_siblings_prefers_later_rendered_child() {
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

        let first_id = RegionId::new(1);
        let mut first = Region::new(first_id);
        first.parent = Some(RegionId::ROOT);
        first.computed = ComputedLayout {
            x: 10.0,
            y: 5.0,
            width: 20.0,
            height: 10.0,
            ..Default::default()
        };
        session.scene.insert(first);

        let second_id = RegionId::new(2);
        let mut second = Region::new(second_id);
        second.parent = Some(RegionId::ROOT);
        second.computed = ComputedLayout {
            x: 15.0,
            y: 6.0,
            width: 20.0,
            height: 10.0,
            ..Default::default()
        };
        session.scene.insert(second);

        let (id, lx, ly) = session.hit_test(16.0, 7.0);
        assert_eq!(id, second_id);
        assert_eq!(lx, 1.0);
        assert_eq!(ly, 1.0);
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

    // ── Frame gate, layout replies, and scene-edit rejection ────────

    /// Decode and clear every message queued for the client.
    fn drain_messages(session: &mut NativeSession) -> Vec<TerminalMessage> {
        use ultty_protocol::Decode;
        let bytes = session.write_buf.split().freeze();
        let mut cursor = bytes.as_ref();
        let mut out = Vec::new();
        while !cursor.is_empty() {
            out.push(TerminalMessage::decode(&mut cursor).expect("valid terminal message"));
        }
        out
    }

    fn error_codes(messages: &[TerminalMessage]) -> Vec<u16> {
        messages
            .iter()
            .filter_map(|m| match m {
                TerminalMessage::Error { code, .. } => Some(*code),
                _ => None,
            })
            .collect()
    }

    fn layout_replies(messages: &[TerminalMessage]) -> Vec<(Option<FrameId>, LayoutTrigger)> {
        messages
            .iter()
            .filter_map(|m| match m {
                TerminalMessage::LayoutComputed {
                    frame_id, trigger, ..
                } => Some((*frame_id, *trigger)),
                _ => None,
            })
            .collect()
    }

    fn create_region(id: u32, parent: u32) -> AppMessage {
        AppMessage::CreateRegion {
            id: RegionId::new(id),
            parent: RegionId::new(parent),
            insert_index: -1,
            constraints: ultty_protocol::Constraints {
                height: Dimension::Cells(2.0),
                width: Dimension::Percent(100.0),
                // Default margins are Auto, which would absorb free space.
                margin: ultty_protocol::Edges::all(Dimension::Cells(0.0)),
                ..Default::default()
            },
            style: Style::default(),
        }
    }

    fn run_frame(session: &mut NativeSession, frame: u32, ops: Vec<AppMessage>) {
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(frame),
            })
            .unwrap();
        for op in ops {
            session.handle_message(op).unwrap();
        }
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(frame),
            })
            .unwrap();
    }

    /// `ulttyctl regions --json` for a small app: a title, a log the app
    /// named with metadata, and a focused input.
    #[test]
    fn describe_reports_the_tree_layout_content_and_focus() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        session.set_viewport(80.0, 24.0);
        let text = |id, text: &str| AppMessage::SetContent {
            id: RegionId::new(id),
            content: Content::Text(ultty_protocol::TextContent::new(text)),
        };
        run_frame(
            &mut session,
            1,
            vec![
                create_region(1, 0),
                text(1, "Build"),
                create_region(2, 0),
                text(2, "compiling ultty\nfinished"),
                create_region(3, 0),
                AppMessage::SetContent {
                    id: RegionId::new(3),
                    content: Content::Widget(ultty_protocol::WidgetContent::Input(
                        ultty_protocol::InputWidget {
                            value: "cargo test".to_string(),
                            ..Default::default()
                        },
                    )),
                },
            ],
        );
        session
            .handle_message(AppMessage::SetRegionMetadata {
                id: RegionId::new(2),
                metadata: vec![("id".to_string(), "log".to_string())],
            })
            .unwrap();
        session
            .handle_message(AppMessage::Focus {
                id: RegionId::new(3),
            })
            .unwrap();

        insta::assert_json_snapshot!(session.describe());
    }

    fn set_semantics(id: u32, semantics: ultty_protocol::Semantics) -> AppMessage {
        AppMessage::SetSemantics {
            id: RegionId::new(id),
            semantics,
        }
    }

    fn polite_log() -> ultty_protocol::Semantics {
        ultty_protocol::Semantics {
            role: ultty_protocol::Role::Log,
            label: Some("Build log".to_string()),
            live: ultty_protocol::Live::Polite,
            ..Default::default()
        }
    }

    #[test]
    fn regions_never_report_a_password_input_value() {
        use crate::control::format_response;
        use ultty_protocol::control::{ControlData, ControlResponse, OutputFormat};
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        run_frame(
            &mut session,
            1,
            vec![
                create_region(1, 0),
                AppMessage::SetContent {
                    id: RegionId::new(1),
                    content: Content::Widget(ultty_protocol::WidgetContent::Input(
                        ultty_protocol::InputWidget {
                            value: "hunter2".to_string(),
                            password: true,
                            ..Default::default()
                        },
                    )),
                },
            ],
        );

        let scene = session.describe();
        assert_eq!(scene.regions[1].content, "input");
        assert_eq!(scene.regions[1].text, None);
        let response = ControlResponse::Data(ControlData::Regions {
            scenes: vec![scene],
        });
        for format in [OutputFormat::Json, OutputFormat::Text] {
            let shown = format_response(&response, format);
            assert!(shown.contains("input"), "{shown}");
            assert!(!shown.contains("hunter2"), "{format:?} leaks: {shown}");
        }
    }

    #[test]
    fn set_semantics_requires_negotiated_capability() {
        let mut session = make_session();
        session
            .handle_message(AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_LAYOUT_FLEXBOX | CAP_REGION_METADATA,
            })
            .unwrap();
        run_frame(&mut session, 1, vec![create_region(1, 0)]);
        drain_messages(&mut session);

        session
            .handle_message(set_semantics(1, polite_log()))
            .unwrap();

        assert_eq!(error_codes(&drain_messages(&mut session)), vec![400]);
        assert_eq!(
            session
                .scene
                .get(RegionId::new(1))
                .unwrap()
                .semantics()
                .clone(),
            ultty_protocol::Semantics::default()
        );
    }

    #[test]
    fn terminal_advertises_semantics() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        assert_ne!(session.capabilities & CAP_SEMANTICS, 0);
    }

    #[test]
    fn set_semantics_for_unknown_region_is_not_found() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        drain_messages(&mut session);

        session
            .handle_message(set_semantics(9, polite_log()))
            .unwrap();

        assert_eq!(error_codes(&drain_messages(&mut session)), vec![404]);
    }

    #[test]
    fn set_semantics_applies_at_end_frame_without_dirtying_the_scene() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        run_frame(&mut session, 1, vec![create_region(1, 0)]);
        drain_messages(&mut session);
        session.scene_dirty = false;
        session.constraints_dirty = false;
        session.render_dirty = false;

        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        session
            .handle_message(set_semantics(1, polite_log()))
            .unwrap();
        assert_eq!(
            session
                .scene
                .get(RegionId::new(1))
                .unwrap()
                .semantics()
                .clone(),
            ultty_protocol::Semantics::default(),
            "queued until EndFrame"
        );
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(2),
            })
            .unwrap();

        assert_eq!(
            session
                .scene
                .get(RegionId::new(1))
                .unwrap()
                .semantics()
                .clone(),
            polite_log()
        );
        assert!(error_codes(&drain_messages(&mut session)).is_empty());
        assert!(!session.scene_dirty);
        assert!(!session.constraints_dirty);
        assert!(!session.render_dirty);
    }

    #[test]
    fn delete_region_drops_its_semantics() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        run_frame(
            &mut session,
            1,
            vec![create_region(1, 0), set_semantics(1, polite_log())],
        );
        assert_eq!(
            session
                .scene
                .get(RegionId::new(1))
                .unwrap()
                .semantics()
                .clone(),
            polite_log()
        );

        run_frame(
            &mut session,
            2,
            vec![
                AppMessage::DeleteRegion {
                    id: RegionId::new(1),
                },
                create_region(1, 0),
            ],
        );

        assert!(error_codes(&drain_messages(&mut session)).is_empty());
        assert_eq!(
            session
                .scene
                .get(RegionId::new(1))
                .unwrap()
                .semantics()
                .clone(),
            ultty_protocol::Semantics::default()
        );
    }

    #[test]
    fn text_region_is_reported_as_log_after_append_content() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        run_frame(
            &mut session,
            1,
            vec![
                create_region(1, 0),
                AppMessage::SetContent {
                    id: RegionId::new(1),
                    content: Content::Text(ultty_protocol::TextContent::new("compiling")),
                },
            ],
        );
        let role = |session: &NativeSession| session.describe().regions[1].role.clone();
        assert_eq!(role(&session), "text");

        session
            .handle_message(AppMessage::AppendContent {
                id: RegionId::new(1),
                text: "\nfinished".to_string(),
            })
            .unwrap();
        assert_eq!(role(&session), "log");

        session
            .handle_message(AppMessage::SetContent {
                id: RegionId::new(1),
                content: Content::Text(ultty_protocol::TextContent::new("again")),
            })
            .unwrap();
        assert_eq!(role(&session), "log", "a log stays a log when replaced");
    }

    /// `ulttyctl regions --json` for an app that declares semantics: an
    /// explicit heading, a polite log that is streaming, a button whose
    /// accessible name is longer than its label.
    #[test]
    fn describe_reports_explicit_semantics() {
        use ultty_protocol::{Role, Semantics};
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        session.set_viewport(80.0, 24.0);
        run_frame(
            &mut session,
            1,
            vec![
                create_region(1, 0),
                AppMessage::SetContent {
                    id: RegionId::new(1),
                    content: Content::Text(ultty_protocol::TextContent::new("Build")),
                },
                set_semantics(
                    1,
                    Semantics {
                        role: Role::Heading,
                        level: 1,
                        ..Default::default()
                    },
                ),
                create_region(2, 0),
                set_semantics(2, polite_log()),
                create_region(3, 0),
                AppMessage::SetContent {
                    id: RegionId::new(3),
                    content: Content::Widget(ultty_protocol::WidgetContent::Button(
                        ultty_protocol::ButtonWidget::new("Run"),
                    )),
                },
                set_semantics(
                    3,
                    Semantics {
                        label: Some("Run all tests".to_string()),
                        ..Default::default()
                    },
                ),
            ],
        );
        session
            .handle_message(AppMessage::AppendContent {
                id: RegionId::new(2),
                text: "compiling ultty".to_string(),
            })
            .unwrap();

        insta::assert_json_snapshot!(session.describe());
    }

    #[test]
    fn end_frame_mismatch_discards_frame_and_next_frame_renders() {
        let mut session = make_session();
        session.set_viewport(80.0, 24.0);
        drain_messages(&mut session);

        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        session.handle_message(create_region(9, 0)).unwrap();
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(2),
            })
            .unwrap();

        let messages = drain_messages(&mut session);
        assert_eq!(error_codes(&messages), vec![400]);
        assert!(layout_replies(&messages).is_empty());
        assert_eq!(session.current_frame, None, "mismatch must close the frame");
        assert!(session.pending_frame_ops.is_empty());
        assert!(
            session.scene.get(RegionId::new(9)).is_none(),
            "ops of a mismatched frame are never applied"
        );

        run_frame(
            &mut session,
            3,
            vec![
                create_region(1, 0),
                AppMessage::SetContent {
                    id: RegionId::new(1),
                    content: Content::Text(ultty_protocol::TextContent::new("ok")),
                },
            ],
        );
        let messages = drain_messages(&mut session);
        assert!(error_codes(&messages).is_empty());
        assert_eq!(
            layout_replies(&messages),
            vec![(Some(FrameId(3)), LayoutTrigger::TreeChange)]
        );
        assert!(session.scene.get(RegionId::new(1)).is_some());
        assert_eq!(
            session.scene.get(RegionId::new(1)).unwrap().computed.height,
            2.0
        );
    }

    #[test]
    fn pending_frame_ops_over_limit_drop_frame_and_disconnect() {
        let mut session = make_session();
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        for _ in 0..frame::MAX_PENDING_FRAME_OPS {
            assert!(session.handle_message(AppMessage::Blur).unwrap());
        }
        assert_eq!(
            session.pending_frame_ops.len(),
            frame::MAX_PENDING_FRAME_OPS
        );

        let keep_alive = session.handle_message(AppMessage::Blur).unwrap();
        assert!(!keep_alive, "exceeding the op budget disconnects");
        assert_eq!(session.current_frame, None);
        assert!(session.pending_frame_ops.is_empty());
        assert_eq!(session.pending_frame_bytes, 0);
        let messages = drain_messages(&mut session);
        assert_eq!(error_codes(&messages), vec![400]);
    }

    #[test]
    fn pending_frame_bytes_over_limit_drop_frame_and_disconnect() {
        let mut session = make_session();
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        assert!(session
            .handle_wire_message(AppMessage::Blur, frame::MAX_PENDING_FRAME_BYTES)
            .unwrap());
        assert_eq!(session.pending_frame_bytes, frame::MAX_PENDING_FRAME_BYTES);

        let keep_alive = session.handle_wire_message(AppMessage::Blur, 1).unwrap();
        assert!(!keep_alive, "exceeding the byte budget disconnects");
        assert!(session.pending_frame_ops.is_empty());
        assert_eq!(session.pending_frame_bytes, 0);
        assert_eq!(error_codes(&drain_messages(&mut session)), vec![400]);
    }

    #[test]
    fn begin_frame_resets_pending_byte_budget() {
        let mut session = make_session();
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        session.handle_wire_message(AppMessage::Blur, 1024).unwrap();
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        assert_eq!(session.pending_frame_bytes, 0);
    }

    #[test]
    fn every_scene_changing_frame_gets_a_layout_reply() {
        let mut session = make_session();
        session.set_viewport(80.0, 24.0);
        run_frame(&mut session, 1, vec![create_region(1, 0)]);
        assert_eq!(
            layout_replies(&drain_messages(&mut session)),
            vec![(Some(FrameId(1)), LayoutTrigger::TreeChange)]
        );

        // Content-only frame: no constraint change, still answered.
        run_frame(
            &mut session,
            2,
            vec![AppMessage::SetContent {
                id: RegionId::new(1),
                content: Content::Text(ultty_protocol::TextContent::new("hello")),
            }],
        );
        assert_eq!(
            layout_replies(&drain_messages(&mut session)),
            vec![(Some(FrameId(2)), LayoutTrigger::ContentChange)]
        );

        run_frame(
            &mut session,
            3,
            vec![AppMessage::SetConstraints {
                id: RegionId::new(1),
                constraints: ultty_protocol::Constraints {
                    height: Dimension::Cells(5.0),
                    ..Default::default()
                },
            }],
        );
        assert_eq!(
            layout_replies(&drain_messages(&mut session)),
            vec![(Some(FrameId(3)), LayoutTrigger::ConstraintChange)]
        );
        assert_eq!(
            session.scene.get(RegionId::new(1)).unwrap().computed.height,
            5.0
        );

        // A frame that changes nothing sends no layout.
        run_frame(&mut session, 4, vec![]);
        assert!(layout_replies(&drain_messages(&mut session)).is_empty());
    }

    #[test]
    fn relayout_for_resize_only_when_scene_has_regions() {
        let mut session = make_session();
        session.set_viewport(80.0, 24.0);
        session.relayout_for_resize().unwrap();
        assert!(
            drain_messages(&mut session).is_empty(),
            "empty scene has nothing to lay out"
        );

        run_frame(&mut session, 1, vec![create_region(1, 0)]);
        drain_messages(&mut session);

        session.set_viewport(120.0, 40.0);
        session.relayout_for_resize().unwrap();
        let messages = drain_messages(&mut session);
        assert_eq!(
            layout_replies(&messages),
            vec![(None, LayoutTrigger::Resize)]
        );
        let regions = messages
            .iter()
            .find_map(|m| match m {
                TerminalMessage::LayoutComputed { regions, .. } => Some(regions.clone()),
                _ => None,
            })
            .unwrap();
        let root = regions.iter().find(|r| r.id == RegionId::ROOT).unwrap();
        assert_eq!((root.width, root.height), (120.0, 40.0));
        let child = regions.iter().find(|r| r.id == RegionId::new(1)).unwrap();
        assert_eq!(child.width, 120.0);
        assert!(!session.scene_dirty, "resize layout leaves the scene clean");
    }

    #[test]
    fn layout_engine_is_reused_across_create_delete_recreate_frames() {
        let mut session = make_session();
        session.set_viewport(80.0, 24.0);

        for frame in 0..4u32 {
            run_frame(
                &mut session,
                frame * 10 + 1,
                vec![
                    create_region(1, 0),
                    create_region(2, 1),
                    create_region(3, 0),
                ],
            );
            let messages = drain_messages(&mut session);
            assert!(error_codes(&messages).is_empty(), "frame {}", frame);
            assert_eq!(
                session.scene.get(RegionId::new(3)).unwrap().computed.y,
                2.0,
                "third region stacks under the first (frame {})",
                frame
            );
            assert!(session.layout.get_layout(RegionId::new(2)).is_some());

            // Move 3 under 1 (earlier sibling) then delete 1; this used to
            // corrupt the layout engine and required a full rebuild.
            run_frame(
                &mut session,
                frame * 10 + 2,
                vec![AppMessage::ReparentRegion {
                    id: RegionId::new(3),
                    new_parent: RegionId::new(1),
                    insert_index: 0,
                }],
            );
            run_frame(
                &mut session,
                frame * 10 + 3,
                vec![AppMessage::DeleteRegion {
                    id: RegionId::new(1),
                }],
            );
            let messages = drain_messages(&mut session);
            assert!(error_codes(&messages).is_empty(), "frame {}", frame);
            assert!(session.layout.get_layout(RegionId::new(1)).is_none());
            assert!(session.layout.get_layout(RegionId::new(2)).is_none());
            assert!(session.layout.get_layout(RegionId::new(3)).is_none());
            assert_eq!(session.scene.len(), 1);
        }
    }

    #[test]
    fn create_region_rejects_duplicate_id_and_missing_parent() {
        let mut session = make_session();
        run_frame(&mut session, 1, vec![create_region(1, 0)]);
        drain_messages(&mut session);

        let mut dup = create_region(1, 0);
        if let AppMessage::CreateRegion { style, .. } = &mut dup {
            style.opacity = 0.25;
        }
        session.handle_message(dup).unwrap();
        assert_eq!(error_codes(&drain_messages(&mut session)), vec![400]);
        assert_eq!(
            session.scene.get(RegionId::new(1)).unwrap().style.opacity,
            1.0,
            "duplicate create must not overwrite the existing region"
        );
        assert_eq!(
            session.scene.get(RegionId::ROOT).unwrap().children,
            vec![RegionId::new(1)]
        );

        session.handle_message(create_region(2, 77)).unwrap();
        assert_eq!(error_codes(&drain_messages(&mut session)), vec![404]);
        assert!(session.scene.get(RegionId::new(2)).is_none());
    }

    #[test]
    fn reparent_rejects_self_and_descendant_targets() {
        let mut session = make_session();
        run_frame(
            &mut session,
            1,
            vec![create_region(1, 0), create_region(2, 1)],
        );
        drain_messages(&mut session);

        for target in [1, 2] {
            session
                .handle_message(AppMessage::ReparentRegion {
                    id: RegionId::new(1),
                    new_parent: RegionId::new(target),
                    insert_index: -1,
                })
                .unwrap();
            assert_eq!(error_codes(&drain_messages(&mut session)), vec![400]);
        }
        assert_eq!(
            session.scene.get(RegionId::new(1)).unwrap().parent,
            Some(RegionId::ROOT)
        );
        assert_eq!(session.scene.iter_depth_first().count(), 3);
    }

    #[test]
    fn create_region_rejects_chain_deeper_than_limit() {
        use crate::scene::tree::MAX_TREE_DEPTH;

        let mut session = make_session();
        let mut ops = Vec::new();
        for depth in 1..=(MAX_TREE_DEPTH as u32 + 1) {
            ops.push(create_region(depth, depth - 1));
        }
        run_frame(&mut session, 1, ops);

        let messages = drain_messages(&mut session);
        assert_eq!(
            error_codes(&messages),
            vec![400],
            "exactly one rejected create"
        );
        assert_eq!(
            session.scene.depth_of(RegionId::new(MAX_TREE_DEPTH as u32)),
            Some(MAX_TREE_DEPTH)
        );
        assert!(session
            .scene
            .get(RegionId::new(MAX_TREE_DEPTH as u32 + 1))
            .is_none());
        assert_eq!(
            layout_replies(&messages).len(),
            1,
            "the rest of the frame still renders"
        );
    }

    #[test]
    fn in_band_session_times_out_without_ever_pinging() {
        let mut session = NativeSession::new_in_band(1, TabId::new(0), Some(9), Vec::new());
        assert!(session.is_in_band());
        assert_eq!(session.in_band_pgrp(), Some(9));

        // Idle but within the timeout: alive, and nothing is sent to the PTY.
        session.last_activity = std::time::Instant::now() - std::time::Duration::from_secs(9);
        assert!(session.check_keepalive().unwrap());
        assert!(session.take_in_band_output().is_empty());

        session.last_activity =
            std::time::Instant::now() - IN_BAND_IDLE_TIMEOUT - std::time::Duration::from_secs(1);
        assert!(!session.check_keepalive().unwrap());
        assert!(session.take_in_band_output().is_empty());
    }

    #[test]
    fn unanswered_in_band_hello_expires_after_the_confirm_timeout() {
        let mut session = NativeSession::new_in_band(1, TabId::new(0), Some(9), Vec::new());
        negotiate_all_test_capabilities(&mut session);
        assert!(!session.is_confirmed());
        assert!(session.check_keepalive().unwrap());

        let answered = std::time::Instant::now()
            - IN_BAND_CONFIRM_TIMEOUT
            - std::time::Duration::from_millis(100);
        session.link = Link::InBand {
            outgoing: Vec::new(),
            pgrp: Some(9),
            pids: Vec::new(),
            phase: InBandPhase::Unconfirmed(answered),
        };
        // Far inside the idle timeout, but the app never answered.
        assert!(!session.check_keepalive().unwrap());

        session.confirm_in_band();
        assert!(session.is_confirmed());
        assert!(session.check_keepalive().unwrap(), "a confirmed app stays");
    }

    #[test]
    fn in_band_app_is_gone_when_its_group_changes_or_a_process_exits() {
        let spawn = || {
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap()
        };
        let (mut app, mut helper) = (spawn(), spawn());
        let pids = vec![app.id() as i32, helper.id() as i32];
        let session = NativeSession::new_in_band(1, TabId::new(0), Some(5), pids);
        assert!(session.in_band_app_alive(Some(5)));
        assert!(!session.in_band_app_alive(Some(6)), "new foreground group");
        helper.kill().unwrap();
        helper.wait().unwrap();
        assert!(
            session.in_band_app_alive(Some(5)),
            "a helper exiting is not the app"
        );
        app.kill().unwrap();
        app.wait().unwrap();
        assert!(!session.in_band_app_alive(Some(5)), "every process exited");

        let unknown = NativeSession::new_in_band(2, TabId::new(0), Some(5), Vec::new());
        assert!(
            unknown.in_band_app_alive(Some(5)),
            "no pids: group check only"
        );
    }

    fn create_text_region(session: &mut NativeSession, id: RegionId) {
        session
            .handle_message(AppMessage::CreateRegion {
                id,
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: ultty_protocol::Constraints::default(),
                style: ultty_protocol::Style::default(),
            })
            .unwrap();
    }

    fn append(session: &mut NativeSession, id: RegionId, text: &str) {
        session
            .handle_message(AppMessage::AppendContent {
                id,
                text: text.to_string(),
            })
            .unwrap();
    }

    fn region_text_len(session: &NativeSession, id: RegionId) -> usize {
        content_limits::text_bytes(&session.scene.get(id).unwrap().content)
    }

    /// What the in-band flood generator sends: one region, 1 MiB appends.
    #[test]
    fn append_flood_keeps_a_region_to_its_newest_text() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        let id = RegionId::new(1);
        create_text_region(&mut session, id);
        let chunk = "A".repeat(1024 * 1024 - 64);

        for _ in 0..64 {
            append(&mut session, id, &chunk);
            assert!(region_text_len(&session, id) <= content_limits::MAX_REGION_TEXT_BYTES);
        }
        append(&mut session, id, "NEWEST");

        let Content::Text(text) = &session.scene.get(id).unwrap().content else {
            panic!("expected text");
        };
        assert!(text.text.ends_with("NEWEST"));
        assert!(text.text.len() >= content_limits::TRIMMED_REGION_TEXT_BYTES);
        assert_eq!(session.recent_protocol_errors().count(), 0);
    }

    #[test]
    fn session_text_is_capped_across_regions() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        let chunk = "B".repeat(1024 * 1024);
        let per_region = content_limits::MAX_REGION_TEXT_BYTES / chunk.len();
        let regions =
            content_limits::MAX_SESSION_TEXT_BYTES / content_limits::MAX_REGION_TEXT_BYTES;

        for n in 1..=regions + 4 {
            let id = RegionId::new(n as u32);
            create_text_region(&mut session, id);
            for _ in 0..per_region {
                append(&mut session, id, &chunk);
            }
        }

        let total: usize = session
            .scene
            .iter_depth_first()
            .map(|region| content_limits::text_bytes(&region.content))
            .sum();
        assert!(total <= content_limits::MAX_SESSION_TEXT_BYTES, "{total}");
        assert!(session.recent_protocol_errors().count() > 0);

        // Freeing a region makes room again.
        session
            .handle_message(AppMessage::DeleteRegion {
                id: RegionId::new(1),
            })
            .unwrap();
        let last = RegionId::new((regions + 4) as u32);
        let before = region_text_len(&session, last);
        append(&mut session, last, &chunk);
        assert_eq!(region_text_len(&session, last), before + chunk.len());
    }

    #[test]
    fn set_content_counts_toward_the_session_limit() {
        let mut session = make_session();
        negotiate_all_test_capabilities(&mut session);
        let chunk = "C".repeat(1024 * 1024);
        let count = content_limits::MAX_SESSION_TEXT_BYTES / chunk.len() + 1;

        for n in 1..=count {
            let id = RegionId::new(n as u32);
            create_text_region(&mut session, id);
            session
                .handle_message(AppMessage::SetContent {
                    id,
                    content: Content::Text(ultty_protocol::TextContent::new(chunk.clone())),
                })
                .unwrap();
        }

        assert_eq!(region_text_len(&session, RegionId::new(count as u32)), 0);
        assert_eq!(session.recent_protocol_errors().count(), 1);
    }
}
