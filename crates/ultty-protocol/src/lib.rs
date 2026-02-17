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

mod codec;
mod content;
pub mod control;
mod error;
mod events;
mod layout;
mod messages;
mod style;
pub mod vim;

pub use codec::{Decode, Encode};
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
pub use style::{
    BorderEdges, BorderStyle, BorderType, Color, Easing, NamedColor, Style, Transition,
    TRANSITION_ALL, TRANSITION_BG, TRANSITION_BORDER_COLOR, TRANSITION_FG, TRANSITION_OPACITY,
};
pub use vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator, VimRegister, VimState};

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

/// All capabilities known by this protocol version.
pub const CAP_ALL_KNOWN: u32 = CAP_LAYOUT_FLEXBOX
    | CAP_WIDGET_SCROLL
    | CAP_APPEND_CONTENT
    | CAP_STREAM_LIFECYCLE
    | CAP_REGION_METADATA;

/// Default capability request for current clients.
pub const CLIENT_DEFAULT_CAPABILITIES: u32 = CAP_ALL_KNOWN;

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
