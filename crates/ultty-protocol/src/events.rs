//! Event types from terminal to app.

use crate::RegionId;

/// Subscription bitmask for mouse events.
pub const EVENT_MOUSE: u32 = 0x01;
/// Subscription bitmask for keyboard events.
pub const EVENT_KEY: u32 = 0x02;
/// Subscription bitmask for focus/blur events.
pub const EVENT_FOCUS: u32 = 0x04;
/// Subscription bitmask for scroll events.
pub const EVENT_SCROLL: u32 = 0x08;

/// Input event.
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    /// Region that received the event.
    pub region_id: RegionId,
    /// Event type and data.
    pub event_type: InputType,
    /// X position relative to region content area.
    pub x: f32,
    /// Y position relative to region content area.
    pub y: f32,
}

/// Input event type.
#[derive(Debug, Clone, PartialEq)]
pub enum InputType {
    /// Mouse button pressed.
    MouseDown { button: MouseButton },
    /// Mouse button released.
    MouseUp { button: MouseButton },
    /// Mouse moved.
    MouseMove,
    /// Mouse scrolled.
    MouseScroll { delta_x: f32, delta_y: f32 },
    /// Key pressed.
    KeyDown { key: Key, modifiers: Modifiers },
    /// Key released.
    KeyUp { key: Key, modifiers: Modifiers },
    /// Text input (for IME).
    TextInput { text: String },
}

/// Mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MouseButton {
    /// Primary (left) mouse button.
    Left = 0,
    /// Middle mouse button (scroll wheel click).
    Middle = 1,
    /// Secondary (right) mouse button.
    Right = 2,
    /// Extra mouse button 4 (back).
    Button4 = 3,
    /// Extra mouse button 5 (forward).
    Button5 = 4,
}

use serde::{Deserialize, Serialize};

/// Keyboard key identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// A Unicode character key.
    Char(char),
    /// A function key (F1-F24).
    Function(u8),
    /// Escape key.
    Escape,
    /// Enter / Return key.
    Enter,
    /// Tab key.
    Tab,
    /// Backspace key.
    Backspace,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    /// Up arrow key.
    Up,
    /// Down arrow key.
    Down,
    /// Left arrow key.
    Left,
    /// Right arrow key.
    Right,
}

/// Keyboard modifier flags (shift, ctrl, alt, meta).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(rename = "bits")]
    pub(crate) bits: u8,
}

impl Modifiers {
    /// No modifiers held.
    pub const NONE: Modifiers = Modifiers { bits: 0 };
    /// Shift key modifier.
    pub const SHIFT: Modifiers = Modifiers { bits: 0x01 };
    /// Control key modifier.
    pub const CTRL: Modifiers = Modifiers { bits: 0x02 };
    /// Alt / Option key modifier.
    pub const ALT: Modifiers = Modifiers { bits: 0x04 };
    /// Meta / Command / Windows key modifier.
    pub const META: Modifiers = Modifiers { bits: 0x08 };

    /// Check if shift is pressed.
    pub const fn shift(self) -> bool {
        self.bits & 0x01 != 0
    }

    /// Check if ctrl is pressed.
    pub const fn ctrl(self) -> bool {
        self.bits & 0x02 != 0
    }

    /// Check if alt is pressed.
    pub const fn alt(self) -> bool {
        self.bits & 0x04 != 0
    }

    /// Check if meta (cmd/win) is pressed.
    pub const fn meta(self) -> bool {
        self.bits & 0x08 != 0
    }

    /// Combine modifiers.
    pub const fn union(self, other: Modifiers) -> Modifiers {
        Modifiers {
            bits: self.bits | other.bits,
        }
    }

    /// Check if all bits in `other` are set in this modifier set.
    pub const fn contains(self, other: Modifiers) -> bool {
        self.bits & other.bits == other.bits
    }
}

/// Resize event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeEvent {
    /// New terminal width in cells.
    pub width: u16,
    /// New terminal height in cells.
    pub height: u16,
}

/// Focus event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusEvent {
    /// Region that gained/lost focus.
    pub region_id: RegionId,
    /// True if gained focus, false if lost.
    pub focused: bool,
}

/// Scroll event (user-initiated scroll).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollEvent {
    /// Region that was scrolled.
    pub region_id: RegionId,
    /// New scroll X offset.
    pub scroll_x: f32,
    /// New scroll Y offset.
    pub scroll_y: f32,
}
