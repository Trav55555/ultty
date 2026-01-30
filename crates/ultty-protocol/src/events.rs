//! Event types from terminal to app.

use crate::RegionId;

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
    Left = 0,
    Middle = 1,
    Right = 2,
    Button4 = 3,
    Button5 = 4,
}

/// Keyboard key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// Character key.
    Char(char),
    /// Function key (F1-F12).
    Function(u8),
    /// Special keys.
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
}

/// Modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    bits: u8,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { bits: 0 };
    pub const SHIFT: Modifiers = Modifiers { bits: 0x01 };
    pub const CTRL: Modifiers = Modifiers { bits: 0x02 };
    pub const ALT: Modifiers = Modifiers { bits: 0x04 };
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
