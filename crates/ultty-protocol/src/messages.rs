//! Protocol messages.

use crate::{
    content::Content,
    events::{FocusEvent, InputEvent, ResizeEvent, ScrollEvent},
    layout::Constraints,
    semantics::Semantics,
    style::Style,
    surface::SurfaceEventKind,
    RegionId,
};

/// Frame identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FrameId(pub u32);

/// Messages from app to terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum AppMessage {
    // Connection
    /// Client hello (version negotiation).
    ClientHello {
        version: u16,
        requested_capabilities: u32,
    },

    // Region management
    /// Create a new region.
    CreateRegion {
        id: RegionId,
        parent: RegionId,
        insert_index: i32, // -1 = append
        constraints: Constraints,
        style: Style,
    },

    /// Update region constraints.
    SetConstraints {
        id: RegionId,
        constraints: Constraints,
    },

    /// Update region style.
    SetStyle { id: RegionId, style: Style },

    /// Set region content.
    SetContent { id: RegionId, content: Content },

    /// Append text to existing content of a region (for streaming).
    AppendContent { id: RegionId, text: String },

    /// Signal the start of a content stream on a region.
    StreamStart {
        id: RegionId,
        /// Stream identifier (for correlating start/end).
        stream_id: u32,
    },

    /// Signal the end of a content stream.
    StreamEnd { id: RegionId, stream_id: u32 },

    /// Set metadata on a region (for agent-aware workflows).
    SetRegionMetadata {
        id: RegionId,
        metadata: Vec<(String, String)>,
    },

    /// Set a region's accessibility semantics, replacing earlier ones.
    SetSemantics { id: RegionId, semantics: Semantics },

    /// Delete a region.
    DeleteRegion { id: RegionId },

    // Inline surfaces
    /// Create region `id` as a second root of the scene, laid out at
    /// `cols x rows` cells (ADR 0020). Its width and height constraints are
    /// ignored.
    CreateSurface {
        id: RegionId,
        /// `MIN_SURFACE_ROWS..=MAX_SURFACE_ROWS`.
        rows: u16,
        constraints: Constraints,
        style: Style,
    },

    /// Close a surface and delete its subtree.
    CloseSurface { id: RegionId },

    /// Move region to new parent.
    ReparentRegion {
        id: RegionId,
        new_parent: RegionId,
        insert_index: i32,
    },

    // Frame batching
    /// Begin atomic frame update.
    BeginFrame { frame_id: FrameId },

    /// End atomic frame update.
    EndFrame { frame_id: FrameId },

    // Interaction
    /// Programmatic scroll.
    Scroll {
        id: RegionId,
        x: f32,
        y: f32,
        animate: bool,
    },

    /// Set keyboard focus.
    Focus { id: RegionId },

    /// Blur (remove focus).
    Blur,

    // Events
    /// Subscribe to events.
    Subscribe { id: RegionId, events: u32 },

    /// Unsubscribe from events.
    Unsubscribe { id: RegionId, events: u32 },

    // Clipboard
    /// Request clipboard contents.
    ClipboardGet,

    /// Set clipboard contents.
    ClipboardSet { text: String },

    // Session
    /// Disconnect cleanly.
    Disconnect,

    // Keepalive
    /// Pong response to a Ping from the terminal.
    Pong,
}

/// Messages from terminal to app.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalMessage {
    // Connection
    /// Server hello (capabilities).
    ServerHello { version: u16, capabilities: u32 },

    /// Session established.
    SessionReady {
        session_id: u64,
        root_region: RegionId,
        width: u16,
        height: u16,
    },

    // Layout
    /// Layout computed after change.
    LayoutComputed {
        frame_id: Option<FrameId>,
        trigger: LayoutTrigger,
        regions: Vec<ComputedRegionLayout>,
    },

    // Events
    /// Input event.
    Input(InputEvent),

    /// Terminal resized.
    Resize(ResizeEvent),

    /// Focus changed.
    Focus(FocusEvent),

    /// Scroll position changed.
    Scroll(ScrollEvent),

    // Clipboard
    /// Clipboard content response.
    ClipboardContent { text: Option<String> },

    // Inline surfaces
    /// Something happened to a surface.
    SurfaceEvent {
        id: RegionId,
        kind: SurfaceEventKind,
    },

    // Keepalive
    /// Ping request to the app (expects a Pong reply).
    Ping,

    // Errors
    /// Error response.
    Error { code: u16, message: String },
}

/// What triggered a layout recomputation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LayoutTrigger {
    /// Terminal window resized.
    Resize = 0,
    /// App changed constraints.
    ConstraintChange = 1,
    /// Content size changed.
    ContentChange = 2,
    /// Region tree changed.
    TreeChange = 3,
}

/// Computed layout for a single region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputedRegionLayout {
    /// Region ID.
    pub id: RegionId,
    /// X position relative to parent content area.
    pub x: f32,
    /// Y position relative to parent content area.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// Content area X (after padding).
    pub content_x: f32,
    /// Content area Y (after padding).
    pub content_y: f32,
    /// Content area width.
    pub content_width: f32,
    /// Content area height.
    pub content_height: f32,
    /// Is visible (not display:none, not clipped).
    pub visible: bool,
}
