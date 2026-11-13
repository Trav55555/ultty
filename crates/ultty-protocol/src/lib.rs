//! # ultimate-protocol
//!
//! Wire protocol for communication between Ultty and TUI applications.
//!
//! ## Overview
//!
//! This crate defines the binary protocol used for:
//! - Apps sending UI declarations to the terminal
//! - Terminal sending events back to apps
//! - Layout constraint specification
//! - Atomic frame updates
//!
//! ## Example
//!
//! ```rust
//! use ultty_protocol::{AppMessage, RegionId, Constraints, Style, Encode};
//!
//! // Create a region
//! let msg = AppMessage::CreateRegion {
//!     id: RegionId(1),
//!     parent: RegionId::ROOT,
//!     insert_index: -1,
//!     constraints: Constraints::default(),
//!     style: Style::default(),
//! };
//!
//! // Encode for wire
//! let bytes = msg.encode_to_bytes();
//! ```
//!
//! ## Robustness testing
//!
//! `src/codec.rs` is the normative wire format. `tests/malformed.rs` runs
//! proptests that feed arbitrary bytes and validly-framed random payloads to
//! both decoders and asserts they never panic. For deeper coverage the
//! repository ships cargo-fuzz targets in `fuzz/` (not a workspace member):
//! `cargo install cargo-fuzz`, then from the repository root run
//! `cargo +nightly fuzz run decode_app` or
//! `cargo +nightly fuzz run decode_terminal` (add `-- -max_len=65536` to
//! exercise large frames). Any crash artifact should be turned into a
//! regression test in `tests/malformed.rs` or the codec unit tests.

pub mod actions;
mod codec;
mod content;
pub mod control;
mod error;
mod events;
pub mod exec_grant;
pub mod in_band;
mod layout;
mod messages;
mod overrides;
mod semantics;
mod style;
mod surface;

pub use actions::{
    declared_actions, valid_action_widget_id, SemanticActionKind, SemanticActionRequest,
    ACTION_ACTIVATE, ACTION_FOCUS, MAX_ACTION_ERROR_BYTES, MAX_ACTION_WIDGET_ID_BYTES,
    REGION_ACTIONS_METADATA_KEY, REGION_WIDGET_ID_METADATA_KEY,
};
pub use codec::{
    Decode, Encode, MAX_REGION_COUNT, MAX_SEMANTICS_STRING_LEN, MAX_SEMANTICS_TOTAL_BYTES,
    MAX_SPAN_COUNT,
};
pub use content::{
    ButtonStyle, ButtonWidget, CodeWidget, Content, ImageScale, ImageWidget, InputWidget,
    MarkdownWidget, ProgressWidget, RichText, Span, SpinnerStyle, SpinnerWidget, TextAlign,
    TextContent, TextOverflow, TextWrap, WidgetContent,
};
pub use error::{ProtocolError, Result};
pub use events::{
    FocusEvent, InputEvent, InputType, Key, Modifiers, MouseButton, ResizeEvent, ScrollEvent,
    EVENT_FOCUS, EVENT_KEY, EVENT_MOUSE, EVENT_SCROLL,
};
pub use layout::{
    AlignContent, AlignItems, AlignSelf, Constraints, Dimension, Display, Edges, FlexDirection,
    FlexWrap, GridLine, GridPlacement, JustifyContent, Overflow, OverflowAxis, Size,
    TrackSizingFunction,
};
pub use messages::{AppMessage, ComputedRegionLayout, FrameId, LayoutTrigger, TerminalMessage};
pub use overrides::{OverrideFormat, PushedOverride, MAX_OVERRIDE_SOURCE_LEN};
pub use semantics::{
    capped, derive_semantics, Expanded, Live, Role, SemanticValue, Semantics, SetPosition, Toggled,
    MAX_NAME_CHARS, SEMANTICS_FLAGS_ALL, SEMANTICS_FLAG_ATOMIC, SEMANTICS_FLAG_BUSY,
    SEMANTICS_FLAG_DISABLED, SEMANTICS_FLAG_HIDDEN, SEMANTICS_FLAG_MODAL, SEMANTICS_FLAG_READ_ONLY,
    SEMANTICS_FLAG_SELECTED,
};
pub use style::{
    BorderEdges, BorderStyle, BorderType, Color, Easing, NamedColor, Style, Transition,
    TRANSITION_ALL, TRANSITION_BG, TRANSITION_BORDER_COLOR, TRANSITION_FG, TRANSITION_OPACITY,
};
pub use surface::{SurfaceEventKind, MAX_SURFACES, MAX_SURFACE_ROWS, MIN_SURFACE_ROWS};

/// Region identifier, unique within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RegionId(pub u32);

impl RegionId {
    /// The root region (terminal viewport).
    pub const ROOT: RegionId = RegionId(0);

    /// Create a new region ID.
    pub const fn new(id: u32) -> Self {
        RegionId(id)
    }
}

/// Protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

/// Oldest protocol version accepted for handshake negotiation.
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// Capability: Flexbox layout semantics.
pub const CAP_LAYOUT_FLEXBOX: u32 = 0x0001;
/// Capability: Programmatic/interactive scroll handling.
pub const CAP_WIDGET_SCROLL: u32 = 0x0010;
/// Capability: Incremental text append updates.
pub const CAP_APPEND_CONTENT: u32 = 0x0020;
/// Capability: Explicit stream lifecycle markers.
pub const CAP_STREAM_LIFECYCLE: u32 = 0x0040;
/// Capability: Region metadata transport.
pub const CAP_REGION_METADATA: u32 = 0x0080;
/// Capability: Inline surfaces (`CreateSurface`, `CloseSurface`,
/// `SurfaceEvent`, ADR 0020).
pub const CAP_INLINE_SURFACES: u32 = 0x0100;
/// Capability: Accessibility semantics (`SetSemantics`, ADR 0021).
pub const CAP_SEMANTICS: u32 = 0x0200;
/// Capability: the caret position of an input widget (`InputWidget::cursor`,
/// sent as widget tag 7).
pub const CAP_INPUT_CURSOR: u32 = 0x0400;
/// Capability: the terminal may push a user override into the app
/// (`SetOverride`, answered with `OverrideResult`; ADR 0023 §9).
pub const CAP_OVERRIDE_PUSH: u32 = 0x0800;
/// Capability: correlated semantic actions answered by an application handler.
pub const CAP_SEMANTIC_ACTIONS: u32 = 0x1000;

/// All capabilities known by this protocol version.
pub const CAP_ALL_KNOWN: u32 = CAP_LAYOUT_FLEXBOX
    | CAP_WIDGET_SCROLL
    | CAP_APPEND_CONTENT
    | CAP_STREAM_LIFECYCLE
    | CAP_REGION_METADATA
    | CAP_INLINE_SURFACES
    | CAP_SEMANTICS
    | CAP_INPUT_CURSOR
    | CAP_OVERRIDE_PUSH
    | CAP_SEMANTIC_ACTIONS;

/// Default capability request for current clients: every known capability
/// a client gets by decoding what the terminal sends. Override pushes and
/// semantic actions are excluded: a client that negotiates either must dispatch
/// requests and acknowledge them. Ulttui's native backend adds these bits.
pub const CLIENT_DEFAULT_CAPABILITIES: u32 =
    CAP_ALL_KNOWN & !(CAP_OVERRIDE_PUSH | CAP_SEMANTIC_ACTIONS);

/// Negotiate a protocol version for a client/server pair.
pub const fn negotiate_protocol_version(client_version: u16, server_version: u16) -> Option<u16> {
    if client_version < MIN_PROTOCOL_VERSION || server_version < MIN_PROTOCOL_VERSION {
        None
    } else {
        Some(if client_version < server_version {
            client_version
        } else {
            server_version
        })
    }
}

/// Negotiate optional feature capabilities for a session.
pub const fn negotiate_capabilities(requested: u32, supported: u32) -> u32 {
    requested & supported
}

/// Magic bytes for frame header.
pub const FRAME_MAGIC: [u8; 2] = [0x55, 0x54]; // "UT"
