//! Event types.

/// Application event.
#[derive(Debug, Clone)]
pub enum Event {
    /// Quit the application.
    Quit,
    /// Key press.
    Key(KeyEvent),
    /// Mouse event.
    Mouse(MouseEvent),
    /// Terminal resized.
    Resize { width: u16, height: u16 },
    /// Focus gained.
    FocusGained,
    /// Focus lost.
    FocusLost,
}

/// Key event.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    /// The key.
    pub key: Key,
    /// Modifier keys.
    pub modifiers: Modifiers,
}

/// Key code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    F(u8),
}

/// Modifier keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Mouse event.
#[derive(Debug, Clone)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub x: u16,
    pub y: u16,
    pub modifiers: Modifiers,
}

/// Mouse event kind.
#[derive(Debug, Clone)]
pub enum MouseEventKind {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
}

/// Mouse button.
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}
