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
    /// A widget was clicked/activated (Enter on focused Button).
    Click { id: String },
    /// A widget's value changed (text change in TextInput, checkbox toggle, etc.).
    Change { id: String, value: String },
    /// A widget submitted (Enter in TextInput).
    Submit { id: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- Event variant construction ---

    #[test]
    fn event_quit() {
        let e = Event::Quit;
        assert!(matches!(e, Event::Quit));
    }

    #[test]
    fn event_key() {
        let key_event = KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::default(),
        };
        let e = Event::Key(key_event);
        assert!(matches!(e, Event::Key(_)));
    }

    #[test]
    fn event_mouse() {
        let mouse_event = MouseEvent {
            kind: MouseEventKind::Moved,
            x: 10,
            y: 20,
            modifiers: Modifiers::default(),
        };
        let e = Event::Mouse(mouse_event);
        assert!(matches!(e, Event::Mouse(_)));
    }

    #[test]
    fn event_click() {
        let e = Event::Click {
            id: "btn".to_string(),
        };
        match e {
            Event::Click { id } => assert_eq!(id, "btn"),
            _ => panic!("Expected Click event"),
        }
    }

    #[test]
    fn event_change() {
        let e = Event::Change {
            id: "input".to_string(),
            value: "hello".to_string(),
        };
        match e {
            Event::Change { id, value } => {
                assert_eq!(id, "input");
                assert_eq!(value, "hello");
            }
            _ => panic!("Expected Change event"),
        }
    }

    #[test]
    fn event_submit() {
        let e = Event::Submit {
            id: "input".to_string(),
        };
        match e {
            Event::Submit { id } => assert_eq!(id, "input"),
            _ => panic!("Expected Submit event"),
        }
    }

    #[test]
    fn event_resize() {
        let e = Event::Resize {
            width: 80,
            height: 24,
        };
        match e {
            Event::Resize { width, height } => {
                assert_eq!(width, 80);
                assert_eq!(height, 24);
            }
            _ => panic!("Expected Resize event"),
        }
    }

    #[test]
    fn event_focus_gained() {
        let e = Event::FocusGained;
        assert!(matches!(e, Event::FocusGained));
    }

    #[test]
    fn event_focus_lost() {
        let e = Event::FocusLost;
        assert!(matches!(e, Event::FocusLost));
    }

    #[test]
    fn event_is_cloneable() {
        let e = Event::Quit;
        let e2 = e.clone();
        assert!(matches!(e2, Event::Quit));
    }

    #[test]
    fn event_is_debuggable() {
        let e = Event::Quit;
        let s = format!("{:?}", e);
        assert!(s.contains("Quit"));
    }

    // --- Key variant construction ---

    #[test]
    fn key_char() {
        let k = Key::Char('z');
        assert_eq!(k, Key::Char('z'));
    }

    #[test]
    fn key_special_keys() {
        assert_eq!(Key::Enter, Key::Enter);
        assert_eq!(Key::Escape, Key::Escape);
        assert_eq!(Key::Backspace, Key::Backspace);
        assert_eq!(Key::Tab, Key::Tab);
        assert_eq!(Key::Delete, Key::Delete);
        assert_eq!(Key::Insert, Key::Insert);
    }

    #[test]
    fn key_arrow_keys() {
        assert_eq!(Key::Up, Key::Up);
        assert_eq!(Key::Down, Key::Down);
        assert_eq!(Key::Left, Key::Left);
        assert_eq!(Key::Right, Key::Right);
    }

    #[test]
    fn key_navigation_keys() {
        assert_eq!(Key::Home, Key::Home);
        assert_eq!(Key::End, Key::End);
        assert_eq!(Key::PageUp, Key::PageUp);
        assert_eq!(Key::PageDown, Key::PageDown);
    }

    #[test]
    fn key_function_keys() {
        assert_eq!(Key::F(1), Key::F(1));
        assert_eq!(Key::F(12), Key::F(12));
        assert_ne!(Key::F(1), Key::F(2));
    }

    #[test]
    fn key_char_equality() {
        assert_eq!(Key::Char('a'), Key::Char('a'));
        assert_ne!(Key::Char('a'), Key::Char('b'));
        assert_ne!(Key::Char('a'), Key::Enter);
    }

    #[test]
    fn key_is_debuggable() {
        let k = Key::Char('x');
        let s = format!("{:?}", k);
        assert!(s.contains("Char"));
        assert!(s.contains("x"));
    }

    #[test]
    fn key_is_cloneable() {
        let k = Key::F(5);
        let k2 = k.clone();
        assert_eq!(k, k2);
    }

    // --- KeyEvent ---

    #[test]
    fn key_event_construction() {
        let ke = KeyEvent {
            key: Key::Enter,
            modifiers: Modifiers {
                shift: false,
                ctrl: true,
                alt: false,
                meta: false,
            },
        };
        assert_eq!(ke.key, Key::Enter);
        assert!(ke.modifiers.ctrl);
        assert!(!ke.modifiers.shift);
    }

    #[test]
    fn key_event_default_modifiers() {
        let ke = KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::default(),
        };
        assert!(!ke.modifiers.shift);
        assert!(!ke.modifiers.ctrl);
        assert!(!ke.modifiers.alt);
        assert!(!ke.modifiers.meta);
    }

    #[test]
    fn key_event_is_cloneable() {
        let ke = KeyEvent {
            key: Key::Char('j'),
            modifiers: Modifiers {
                shift: true,
                ctrl: false,
                alt: true,
                meta: false,
            },
        };
        let ke2 = ke.clone();
        assert_eq!(ke2.key, Key::Char('j'));
        assert!(ke2.modifiers.shift);
        assert!(ke2.modifiers.alt);
    }

    #[test]
    fn key_event_is_debuggable() {
        let ke = KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::default(),
        };
        let s = format!("{:?}", ke);
        assert!(s.contains("Tab"));
    }

    // --- Modifiers ---

    #[test]
    fn modifiers_default_all_false() {
        let m = Modifiers::default();
        assert!(!m.shift);
        assert!(!m.ctrl);
        assert!(!m.alt);
        assert!(!m.meta);
    }

    #[test]
    fn modifiers_individual_flags() {
        let m = Modifiers {
            shift: true,
            ctrl: false,
            alt: true,
            meta: false,
        };
        assert!(m.shift);
        assert!(!m.ctrl);
        assert!(m.alt);
        assert!(!m.meta);
    }

    #[test]
    fn modifiers_all_true() {
        let m = Modifiers {
            shift: true,
            ctrl: true,
            alt: true,
            meta: true,
        };
        assert!(m.shift);
        assert!(m.ctrl);
        assert!(m.alt);
        assert!(m.meta);
    }

    #[test]
    fn modifiers_is_copy() {
        let m = Modifiers {
            shift: true,
            ctrl: false,
            alt: false,
            meta: false,
        };
        let m2 = m; // copy
        assert!(m.shift); // original still usable
        assert!(m2.shift);
    }

    #[test]
    fn modifiers_is_debuggable() {
        let m = Modifiers {
            shift: true,
            ctrl: true,
            alt: false,
            meta: false,
        };
        let s = format!("{:?}", m);
        assert!(s.contains("shift"));
        assert!(s.contains("ctrl"));
    }

    // --- MouseEvent ---

    #[test]
    fn mouse_event_construction() {
        let me = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            x: 5,
            y: 10,
            modifiers: Modifiers::default(),
        };
        assert_eq!(me.x, 5);
        assert_eq!(me.y, 10);
        assert!(matches!(me.kind, MouseEventKind::Down(MouseButton::Left)));
    }

    #[test]
    fn mouse_event_with_modifiers() {
        let me = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Right),
            x: 0,
            y: 0,
            modifiers: Modifiers {
                shift: true,
                ctrl: false,
                alt: false,
                meta: false,
            },
        };
        assert!(me.modifiers.shift);
        assert!(matches!(me.kind, MouseEventKind::Drag(MouseButton::Right)));
    }

    #[test]
    fn mouse_event_is_cloneable() {
        let me = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            x: 3,
            y: 7,
            modifiers: Modifiers::default(),
        };
        let me2 = me.clone();
        assert_eq!(me2.x, 3);
        assert_eq!(me2.y, 7);
    }

    // --- MouseEventKind ---

    #[test]
    fn mouse_event_kind_variants() {
        let _down = MouseEventKind::Down(MouseButton::Left);
        let _up = MouseEventKind::Up(MouseButton::Right);
        let _drag = MouseEventKind::Drag(MouseButton::Middle);
        let _moved = MouseEventKind::Moved;
        let _scroll_up = MouseEventKind::ScrollUp;
        let _scroll_down = MouseEventKind::ScrollDown;
    }

    // --- MouseButton ---

    #[test]
    fn mouse_button_is_copy() {
        let btn = MouseButton::Left;
        let btn2 = btn; // copy
        assert!(matches!(btn, MouseButton::Left));
        assert!(matches!(btn2, MouseButton::Left));
    }

    #[test]
    fn mouse_button_is_debuggable() {
        let s = format!("{:?}", MouseButton::Middle);
        assert!(s.contains("Middle"));
    }
}
