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
//! use ultty_protocol::{AppMessage, RegionId, Constraints};
//!
//! // Create a region
//! let msg = AppMessage::CreateRegion {
//!     id: RegionId(1),
//!     parent: RegionId::ROOT,
//!     constraints: Constraints::default(),
//! };
//!
//! // Encode for wire
//! let bytes = msg.encode();
//! ```

mod codec;
mod content;
mod error;
mod events;
mod layout;
mod messages;
mod style;
pub mod vim;

pub use codec::{Decode, Encode};
pub use content::{
    CodeWidget, Content, InputWidget, MarkdownWidget, ProgressWidget, RichText, Span,
    SpinnerWidget, TextAlign, TextContent, TextOverflow, TextWrap, WidgetContent,
};
pub use error::{ProtocolError, Result};
pub use events::{
    FocusEvent, InputEvent, InputType, Key, Modifiers, MouseButton, ResizeEvent, ScrollEvent,
};
pub use layout::{
    AlignContent, AlignItems, AlignSelf, Constraints, Dimension, Display, Edges, FlexDirection,
    FlexWrap, GridLine, GridPlacement, JustifyContent, Overflow, OverflowAxis, Size,
    TrackSizingFunction,
};
pub use messages::{AppMessage, ComputedRegionLayout, FrameId, LayoutTrigger, TerminalMessage};
pub use style::{BorderEdges, BorderStyle, BorderType, Color, NamedColor, Style};
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

/// Magic bytes for frame header.
pub const FRAME_MAGIC: [u8; 2] = [0x55, 0x54]; // "UT"
