//! Keyboard encoding: the bytes a key event sends to the program in a pane.
//!
//! [`encode`] is a pure function of the key, its modifiers, and the
//! terminal's keyboard modes. It speaks three dialects:
//!
//! - **Legacy** (xterm): control characters for Ctrl, an ESC prefix for
//!   Alt, `CSI 1 ; mods X` and `CSI n ; mods ~` for modified function keys,
//!   SS3 cursor keys under DECCKM.
//! - **modifyOtherKeys** (xterm `CSI > 4 ; n m`): `CSI 27 ; mods ; code ~`
//!   for modified keys the legacy dialect cannot tell apart.
//! - **Kitty keyboard protocol**, progressive enhancement flags 1-16, as
//!   specified at <https://sw.kovidgoyal.net/kitty/keyboard-protocol/> and
//!   implemented by kitty's `key_encoding.c`, which this follows.
//!
//! [`key_from_winit`] and [`base_layout_key`] turn winit's view of a key
//! into the encoder's.

use winit::keyboard::{Key as WinitKey, KeyCode, KeyLocation, NamedKey};

/// Modifier bits, in the kitty protocol's order (`1 + bits` on the wire;
/// xterm uses the same values for Shift, Alt and Ctrl).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SHIFT: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const CTRL: Mods = Mods(4);
    pub const SUPER: Mods = Mods(8);

    /// Build from the state of each modifier.
    pub fn new(shift: bool, alt: bool, ctrl: bool, super_key: bool) -> Self {
        let mut bits = 0;
        for (on, bit) in [(shift, 1), (alt, 2), (ctrl, 4), (super_key, 8)] {
            if on {
                bits |= bit;
            }
        }
        Mods(bits)
    }

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    pub fn without(self, other: Mods) -> Mods {
        Mods(self.0 & !other.0)
    }

    pub fn bits(self) -> u8 {
        self.0
    }
}

/// Press, auto-repeat, or release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

impl KeyAction {
    /// The kitty event type number.
    fn number(self) -> u8 {
        match self {
            KeyAction::Press => 1,
            KeyAction::Repeat => 2,
            KeyAction::Release => 3,
        }
    }
}

/// A key on the numeric keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeypadKey {
    Digit(u8),
    Decimal,
    Divide,
    Multiply,
    Subtract,
    Add,
    Enter,
    Equal,
    Separator,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
}

impl KeypadKey {
    /// The kitty functional key code (KP_0 is 57399).
    fn kitty_code(self) -> u32 {
        match self {
            KeypadKey::Digit(d) => 57399 + u32::from(d.min(9)),
            KeypadKey::Decimal => 57409,
            KeypadKey::Divide => 57410,
            KeypadKey::Multiply => 57411,
            KeypadKey::Subtract => 57412,
            KeypadKey::Add => 57413,
            KeypadKey::Enter => 57414,
            KeypadKey::Equal => 57415,
            KeypadKey::Separator => 57416,
            KeypadKey::Left => 57417,
            KeypadKey::Right => 57418,
            KeypadKey::Up => 57419,
            KeypadKey::Down => 57420,
            KeypadKey::PageUp => 57421,
            KeypadKey::PageDown => 57422,
            KeypadKey::Home => 57423,
            KeypadKey::End => 57424,
            KeypadKey::Insert => 57425,
            KeypadKey::Delete => 57426,
        }
    }

    /// The main-keyboard key it stands for when keypad keys are not
    /// reported separately.
    fn to_main(self) -> Key {
        match self {
            KeypadKey::Digit(d) => Key::Char(char::from(b'0' + d.min(9))),
            KeypadKey::Decimal => Key::Char('.'),
            KeypadKey::Divide => Key::Char('/'),
            KeypadKey::Multiply => Key::Char('*'),
            KeypadKey::Subtract => Key::Char('-'),
            KeypadKey::Add => Key::Char('+'),
            KeypadKey::Equal => Key::Char('='),
            KeypadKey::Enter => Key::Enter,
            KeypadKey::Left => Key::Left,
            KeypadKey::Right => Key::Right,
            KeypadKey::Up => Key::Up,
            KeypadKey::Down => Key::Down,
            KeypadKey::PageUp => Key::PageUp,
            KeypadKey::PageDown => Key::PageDown,
            KeypadKey::Home => Key::Home,
            KeypadKey::End => Key::End,
            KeypadKey::Insert => Key::Insert,
            KeypadKey::Delete => Key::Delete,
            KeypadKey::Separator => Key::Keypad(self),
        }
    }
}

/// Which key was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A key that types a character: the character in the current layout
    /// without Shift (`a` for Shift+A, `1` for Shift+1).
    Char(char),
    /// A key on the numeric keypad.
    Keypad(KeypadKey),
    Escape,
    Enter,
    Tab,
    Backspace,
    Insert,
    Delete,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    /// F1 to F35.
    F(u8),
    Menu,
    /// A key only the kitty protocol reports (lock, media and modifier
    /// keys), by its kitty functional key code.
    Kitty(u32),
}

/// Kitty codes of the modifier keys: LEFT_SHIFT to ISO_LEVEL5_SHIFT.
const KITTY_MODIFIER_KEYS: std::ops::RangeInclusive<u32> = 57441..=57454;
/// Kitty codes of CAPS_LOCK, SCROLL_LOCK and NUM_LOCK.
const KITTY_LOCK_KEYS: std::ops::RangeInclusive<u32> = 57358..=57360;

impl Key {
    /// Modifier and lock keys, reported only with "report all keys".
    fn is_modifier(self) -> bool {
        matches!(self, Key::Kitty(code)
            if KITTY_MODIFIER_KEYS.contains(&code) || KITTY_LOCK_KEYS.contains(&code))
    }
}

/// One key event, as the encoder sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput<'a> {
    pub key: Key,
    /// The key with Shift applied (`A`, `!`), when Shift is held and that
    /// differs from `key`.
    pub shifted: Option<char>,
    /// The key at the same position on a US layout, when that differs from
    /// `key` (kitty's "base layout key").
    pub base: Option<char>,
    pub mods: Mods,
    pub action: KeyAction,
    /// The text the key types, if any.
    pub text: Option<&'a str>,
}

impl<'a> KeyInput<'a> {
    /// A press of `key` with `mods`, no text and no alternates.
    pub fn press(key: Key, mods: Mods) -> Self {
        Self {
            key,
            shifted: None,
            base: None,
            mods,
            action: KeyAction::Press,
            text: None,
        }
    }
}

/// The terminal state that changes what keys send.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM: unmodified cursor keys send SS3 instead of CSI.
    pub application_cursor: bool,
    /// Kitty keyboard protocol flags (`CSI > flags u`).
    pub kitty_flags: u8,
    /// xterm modifyOtherKeys level, 0 to 2 (`CSI > 4 ; level m`).
    pub modify_other_keys: u8,
    /// LNM (ANSI mode 20): Enter sends CR LF instead of CR.
    pub linefeed_newline: bool,
}

const DISAMBIGUATE: u8 = 1;
const REPORT_EVENTS: u8 = 2;
const REPORT_ALTERNATES: u8 = 4;
const REPORT_ALL: u8 = 8;
const REPORT_TEXT: u8 = 16;

/// Encode a key event. Empty means the key sends nothing.
pub fn encode(input: &KeyInput, modes: KeyModes) -> Vec<u8> {
    let flags = modes.kitty_flags & 0x1f;
    let report_all = flags & REPORT_ALL != 0;
    let disambiguate = flags & DISAMBIGUATE != 0;

    // Text is what the key types without Ctrl, Alt or Super; with them the
    // key is a shortcut and is encoded instead.
    let text = input.text.filter(|text| {
        !input.mods.contains(Mods::CTRL)
            && !input.mods.contains(Mods::ALT)
            && !input.mods.contains(Mods::SUPER)
            && text.chars().next().is_some_and(|c| !c.is_control())
    });

    let mut ev = Event {
        key: input.key,
        shifted: input.shifted.filter(|_| input.mods.contains(Mods::SHIFT)),
        base: input.base,
        // The legacy dialects have no way to say Super.
        mods: if flags == 0 {
            input.mods.without(Mods::SUPER)
        } else {
            input.mods
        },
        action: input.action,
        text,
        flags,
        application_cursor: modes.application_cursor,
        linefeed_newline: modes.linefeed_newline,
    };

    if ev.key.is_modifier() && !report_all {
        return Vec::new();
    }
    if !disambiguate && !report_all {
        if let Key::Keypad(keypad) = ev.key {
            ev.key = keypad.to_main();
        }
    }
    if !report_all && ev.action != KeyAction::Release {
        if let Some(text) = text {
            return text.as_bytes().to_vec();
        }
    }
    if flags & REPORT_EVENTS == 0 && ev.action == KeyAction::Release {
        return Vec::new();
    }
    if flags == 0 && modes.modify_other_keys > 0 {
        if let Some(bytes) = modify_other_keys(&ev, modes.modify_other_keys) {
            return bytes;
        }
    }
    match ev.key {
        Key::Char(c) => ev.encode_char(c),
        _ => ev.encode_functional(),
    }
}

/// An event after the mode-independent normalisation in [`encode`].
struct Event<'a> {
    key: Key,
    shifted: Option<char>,
    base: Option<char>,
    mods: Mods,
    action: KeyAction,
    text: Option<&'a str>,
    flags: u8,
    application_cursor: bool,
    linefeed_newline: bool,
}

/// The fields of a `CSI ... u`-style sequence.
struct Csi<'a> {
    key: u32,
    shifted: Option<char>,
    base: Option<char>,
    mods: Mods,
    action: Option<KeyAction>,
    text: Option<&'a str>,
    trailer: char,
}

impl Csi<'_> {
    /// `CSI key:shifted:base ; mods:action ; text trailer`, leaving out
    /// trailing defaults as kitty does.
    fn serialize(&self) -> Vec<u8> {
        let has_mods = !self.mods.is_empty();
        let has_alternates = self.shifted.is_some() || self.base.is_some();
        let second = has_mods || self.action.is_some();
        let mut out = String::from("\x1b[");
        if self.key != 1 || has_alternates || second || self.text.is_some() {
            out.push_str(&self.key.to_string());
        }
        if has_alternates {
            out.push(':');
            if let Some(shifted) = self.shifted {
                out.push_str(&u32::from(shifted).to_string());
            }
            if let Some(base) = self.base {
                out.push(':');
                out.push_str(&u32::from(base).to_string());
            }
        }
        if second || self.text.is_some() {
            out.push(';');
            if second {
                out.push_str(&(u32::from(self.mods.bits()) + 1).to_string());
            }
            if let Some(action) = self.action {
                out.push(':');
                out.push_str(&action.number().to_string());
            }
        }
        if let Some(text) = self.text {
            let codepoints: Vec<String> = text.chars().map(|c| u32::from(c).to_string()).collect();
            out.push(';');
            out.push_str(&codepoints.join(":"));
        }
        out.push(self.trailer);
        out.into_bytes()
    }
}

impl Event<'_> {
    fn legacy_mode(&self) -> bool {
        self.flags & (DISAMBIGUATE | REPORT_EVENTS | REPORT_ALL) == 0
    }

    fn reported_action(&self) -> Option<KeyAction> {
        (self.flags & REPORT_EVENTS != 0 && self.action != KeyAction::Press).then_some(self.action)
    }

    fn reported_text(&self) -> Option<&str> {
        self.text.filter(|_| self.flags & REPORT_TEXT != 0)
    }

    fn csi(&self, key: u32, trailer: char) -> Vec<u8> {
        Csi {
            key,
            shifted: None,
            base: None,
            mods: self.mods,
            action: self.reported_action(),
            text: self.reported_text(),
            trailer,
        }
        .serialize()
    }

    /// Keys without text of their own: Escape, Enter, arrows, F-keys...
    fn encode_functional(&self) -> Vec<u8> {
        let legacy = self.legacy_mode();
        let report_all = self.flags & REPORT_ALL != 0;
        let release = self.action == KeyAction::Release;

        if self.application_cursor && legacy && self.mods.is_empty() {
            let ss3 = match self.key {
                Key::Up => Some(b"\x1bOA"),
                Key::Down => Some(b"\x1bOB"),
                Key::Right => Some(b"\x1bOC"),
                Key::Left => Some(b"\x1bOD"),
                Key::End => Some(b"\x1bOF"),
                Key::Home => Some(b"\x1bOH"),
                _ => None,
            };
            if let Some(ss3) = ss3 {
                return ss3.to_vec();
            }
        }
        if self.mods.is_empty() {
            if self.flags & (DISAMBIGUATE | REPORT_ALL) == 0 && self.key == Key::Escape {
                return b"\x1b".to_vec();
            }
            if legacy {
                match self.key {
                    Key::F(1) => return b"\x1bOP".to_vec(),
                    Key::F(2) => return b"\x1bOQ".to_vec(),
                    Key::F(3) => return b"\x1bOR".to_vec(),
                    Key::F(4) => return b"\x1bOS".to_vec(),
                    _ => {}
                }
            }
            // Enter, Tab and Backspace stay legacy, and send no release,
            // so a shell stays usable after a program leaves flags set.
            if !report_all {
                let plain: Option<&[u8]> = match self.key {
                    Key::Enter if self.linefeed_newline => Some(b"\r\n"),
                    Key::Enter => Some(b"\r"),
                    Key::Backspace => Some(b"\x7f"),
                    Key::Tab => Some(b"\t"),
                    _ => None,
                };
                if let Some(plain) = plain {
                    return if release { Vec::new() } else { plain.to_vec() };
                }
            }
        } else if legacy {
            if let Some(bytes) = self.legacy_modified_functional() {
                return bytes;
            }
        }

        let (number, trailer) = match self.key {
            Key::Escape => (27, 'u'),
            Key::Enter => (13, 'u'),
            Key::Tab => (9, 'u'),
            Key::Backspace => (127, 'u'),
            Key::Insert => (2, '~'),
            Key::Delete => (3, '~'),
            Key::Left => (1, 'D'),
            Key::Right => (1, 'C'),
            Key::Up => (1, 'A'),
            Key::Down => (1, 'B'),
            Key::PageUp => (5, '~'),
            Key::PageDown => (6, '~'),
            Key::Home => (1, 'H'),
            Key::End => (1, 'F'),
            Key::F(1) => (1, 'P'),
            Key::F(2) => (1, 'Q'),
            // Not `CSI 1 ; mods R`: that is a cursor position report.
            Key::F(3) => (13, '~'),
            Key::F(4) => (1, 'S'),
            Key::F(5) => (15, '~'),
            Key::F(6) => (17, '~'),
            Key::F(7) => (18, '~'),
            Key::F(8) => (19, '~'),
            Key::F(9) => (20, '~'),
            Key::F(10) => (21, '~'),
            Key::F(11) => (23, '~'),
            Key::F(12) => (24, '~'),
            Key::F(n @ 13..=35) => (57376 + u32::from(n - 13), 'u'),
            // xterm sends Menu as F16.
            Key::Menu if legacy => (29, '~'),
            Key::Menu => (57363, 'u'),
            Key::Keypad(keypad) => (keypad.kitty_code(), 'u'),
            Key::Kitty(code) => (code, 'u'),
            Key::F(_) | Key::Char(_) => return Vec::new(),
        };
        // Keys with only a kitty code mean nothing to a legacy program.
        if legacy && trailer == 'u' {
            return Vec::new();
        }
        self.csi(number, trailer)
    }

    /// Escape, Enter, Tab and Backspace with modifiers, as xterm sends them.
    fn legacy_modified_functional(&self) -> Option<Vec<u8>> {
        let alt = self.mods.contains(Mods::ALT);
        let mut out = Vec::new();
        if alt {
            out.push(0x1b);
        }
        match self.key {
            Key::Enter if self.linefeed_newline => out.extend_from_slice(b"\r\n"),
            Key::Enter => out.push(b'\r'),
            Key::Escape => out.push(0x1b),
            Key::Backspace if self.mods.contains(Mods::CTRL) => out.push(0x08),
            Key::Backspace => out.push(0x7f),
            Key::Tab if self.mods.contains(Mods::SHIFT) => out.extend_from_slice(b"\x1b[Z"),
            Key::Tab => out.push(b'\t'),
            _ => return None,
        }
        Some(out)
    }

    /// Keys that type a character.
    fn encode_char(&self, key: char) -> Vec<u8> {
        let add_action = self.reported_action();
        let add_alternates =
            self.flags & REPORT_ALTERNATES != 0 && (self.shifted.is_some() || self.base.is_some());
        let add_text = self.reported_text();

        if add_action.is_none() && !add_alternates && add_text.is_none() {
            if self.mods.is_empty() && self.flags & REPORT_ALL == 0 {
                return key.to_string().into_bytes();
            }
            if self.flags & (DISAMBIGUATE | REPORT_ALL) == 0 {
                if let Some(bytes) = self.legacy_char(key) {
                    return bytes;
                }
            }
        }

        let (shifted, base) = if add_alternates {
            (self.shifted, self.base)
        } else {
            (None, None)
        };
        Csi {
            key: u32::from(key),
            shifted,
            base,
            mods: self.mods,
            action: add_action,
            text: add_text,
            trailer: 'u',
        }
        .serialize()
    }

    /// A modified character key in the legacy dialect, or `None` where kitty
    /// falls back to `CSI u`.
    fn legacy_char(&self, key: char) -> Option<Vec<u8>> {
        if is_legacy_ascii(key) || self.shifted.is_some_and(is_legacy_ascii) {
            if let Some(bytes) = legacy_printable(key, self.shifted, self.mods) {
                return Some(bytes);
            }
        }
        // A non-Latin layout: Ctrl+С is Ctrl+C by the key's position.
        let m = self.mods;
        if (m == Mods::CTRL || m == Mods::ALT || m == Mods::CTRL.union(Mods::ALT))
            && !is_legacy_ascii(key)
        {
            if let Some(base) = self.base.filter(|&b| is_legacy_ascii(b)) {
                if let Some(bytes) = legacy_printable(base, None, m) {
                    return Some(bytes);
                }
            }
        }
        // Kitty sends `CSI u` for what is left; a program that has not asked
        // for that gets xterm's answer instead: Ctrl+Shift+A is Ctrl+A,
        // and a character Ctrl cannot change is sent as is.
        if self.flags != 0 {
            return None;
        }
        let shifted = if m.contains(Mods::SHIFT) {
            self.shifted.unwrap_or(key)
        } else {
            key
        };
        let ch = if m.contains(Mods::CTRL) {
            ctrl_char(if key.is_ascii_lowercase() {
                key
            } else {
                shifted
            })
        } else {
            shifted
        };
        let mut out = Vec::new();
        if m.contains(Mods::ALT) {
            out.push(0x1b);
        }
        out.extend_from_slice(ch.to_string().as_bytes());
        Some(out)
    }
}

/// Kitty's `encode_printable_ascii_key_legacy`.
fn legacy_printable(key: char, shifted: Option<char>, mods: Mods) -> Option<Vec<u8>> {
    let mut key = key;
    let mut m = mods;
    if m.contains(Mods::SHIFT) {
        if let Some(shifted) = shifted.filter(|&s| s != key) {
            if !m.contains(Mods::CTRL) || !key.is_ascii_lowercase() {
                key = shifted;
                m = m.without(Mods::SHIFT);
            }
        }
    }
    let alt_ctrl = Mods::CTRL.union(Mods::ALT);
    let bytes = |prefix: bool, c: char| {
        let mut out = Vec::new();
        if prefix {
            out.push(0x1b);
        }
        out.extend_from_slice(c.to_string().as_bytes());
        Some(out)
    };
    if mods == Mods::SHIFT {
        return bytes(false, key);
    }
    if m == Mods::ALT {
        return bytes(true, key);
    }
    if m == Mods::CTRL {
        return bytes(false, ctrl_char(key));
    }
    if m == alt_ctrl {
        return bytes(true, ctrl_char(key));
    }
    if key == ' ' {
        if m == Mods::CTRL.union(Mods::SHIFT) {
            return bytes(false, ctrl_char(key));
        }
        if m == Mods::ALT.union(Mods::SHIFT) {
            return bytes(true, key);
        }
    }
    None
}

/// What Ctrl turns a character into (kitty's table, after xterm's).
fn ctrl_char(key: char) -> char {
    let byte = match key {
        ' ' | '2' | '@' => 0,
        'a'..='z' => key as u8 - b'a' + 1,
        '3' | '[' => 27,
        '4' | '\\' => 28,
        '5' | ']' => 29,
        '6' | '^' | '~' => 30,
        '7' | '/' | '_' => 31,
        '8' | '?' => 127,
        _ => return key,
    };
    char::from(byte)
}

/// Keys of a US keyboard that the legacy dialect knows how to modify.
fn is_legacy_ascii(c: char) -> bool {
    c.is_ascii_lowercase()
        || c.is_ascii_digit()
        || " !@#$%^&*()`~-_=+[{]}\\|;:'\",<.>/?".contains(c)
}

/// `CSI 27 ; mods ; code ~` for keys the legacy dialect would garble, or
/// `None` to encode the key as usual. Level 1 changes only keys whose
/// legacy bytes lose a modifier; level 2 changes every key with Ctrl or
/// Alt, and Enter, Backspace and Escape with Shift.
fn modify_other_keys(ev: &Event, level: u8) -> Option<Vec<u8>> {
    let m = ev.mods.without(Mods::SUPER);
    let shift = m.contains(Mods::SHIFT);
    let ctrl = m.contains(Mods::CTRL);
    let (code, lossy) = match ev.key {
        Key::Char(key) => {
            let effective = if shift {
                ev.shifted.unwrap_or(key)
            } else {
                key
            };
            let lossy = ctrl
                && ((shift && key.is_ascii_lowercase())
                    || (!key.is_ascii_lowercase() && ctrl_char(effective) == effective));
            if m == Mods::SHIFT || m.is_empty() {
                return None;
            }
            (effective, lossy)
        }
        Key::Enter => ('\r', ctrl || shift),
        Key::Escape => ('\x1b', ctrl || shift),
        Key::Tab if m == Mods::SHIFT => return None,
        Key::Tab => ('\t', ctrl),
        Key::Backspace => ('\x7f', shift),
        _ => return None,
    };
    if m.is_empty() || (level < 2 && !lossy) {
        return None;
    }
    Some(format!("\x1b[27;{};{}~", m.bits() + 1, u32::from(code)).into_bytes())
}

/// The encoder's key for a winit key event.
///
/// `logical` is the event's logical key, `unmodified` the same key without
/// modifiers (winit's `key_without_modifiers`). Returns `None` for keys
/// that send nothing, such as dead keys.
pub fn key_from_winit(
    logical: &WinitKey,
    unmodified: &WinitKey,
    location: KeyLocation,
) -> Option<Key> {
    if location == KeyLocation::Numpad {
        if let Some(keypad) = keypad_key(logical) {
            return Some(Key::Keypad(keypad));
        }
    }
    let left = location != KeyLocation::Right;
    let side =
        |left_code: u32, right_code: u32| Key::Kitty(if left { left_code } else { right_code });
    match logical {
        WinitKey::Character(typed) => {
            // The key's own character: without Shift, and lower case so
            // Caps Lock does not change it.
            let source = match unmodified {
                WinitKey::Character(s) => s,
                _ => typed,
            };
            let c = source.chars().next()?;
            let mut lower = c.to_lowercase();
            Some(Key::Char(match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                _ => c,
            }))
        }
        WinitKey::Named(named) => Some(match named {
            NamedKey::Space => Key::Char(' '),
            NamedKey::Escape => Key::Escape,
            NamedKey::Enter => Key::Enter,
            NamedKey::Tab => Key::Tab,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Insert => Key::Insert,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::ContextMenu => Key::Menu,
            NamedKey::CapsLock => Key::Kitty(57358),
            NamedKey::ScrollLock => Key::Kitty(57359),
            NamedKey::NumLock => Key::Kitty(57360),
            NamedKey::PrintScreen => Key::Kitty(57361),
            NamedKey::Pause => Key::Kitty(57362),
            NamedKey::MediaPlay => Key::Kitty(57428),
            NamedKey::MediaPause => Key::Kitty(57429),
            NamedKey::MediaPlayPause => Key::Kitty(57430),
            NamedKey::MediaStop => Key::Kitty(57432),
            NamedKey::MediaFastForward => Key::Kitty(57433),
            NamedKey::MediaRewind => Key::Kitty(57434),
            NamedKey::MediaTrackNext => Key::Kitty(57435),
            NamedKey::MediaTrackPrevious => Key::Kitty(57436),
            NamedKey::MediaRecord => Key::Kitty(57437),
            NamedKey::AudioVolumeDown => Key::Kitty(57438),
            NamedKey::AudioVolumeUp => Key::Kitty(57439),
            NamedKey::AudioVolumeMute => Key::Kitty(57440),
            NamedKey::Shift => side(57441, 57447),
            NamedKey::Control => side(57442, 57448),
            NamedKey::Alt => side(57443, 57449),
            NamedKey::Super => side(57444, 57450),
            NamedKey::Hyper => side(57445, 57451),
            NamedKey::Meta => side(57446, 57452),
            NamedKey::AltGraph => Key::Kitty(57453),
            other => Key::F(function_key_number(other)?),
        }),
        _ => None,
    }
}

/// F1 to F35 as a number.
fn function_key_number(named: &NamedKey) -> Option<u8> {
    use NamedKey::*;
    const KEYS: [NamedKey; 35] = [
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20,
        F21, F22, F23, F24, F25, F26, F27, F28, F29, F30, F31, F32, F33, F34, F35,
    ];
    KEYS.iter()
        .position(|k| k == named)
        .and_then(|i| u8::try_from(i + 1).ok())
}

/// A numeric keypad key from its logical key (which, unlike the
/// unmodified key, reflects Num Lock).
fn keypad_key(logical: &WinitKey) -> Option<KeypadKey> {
    Some(match logical {
        WinitKey::Character(s) => match s.as_str() {
            d @ ("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => {
                KeypadKey::Digit(d.as_bytes()[0] - b'0')
            }
            "." => KeypadKey::Decimal,
            "/" => KeypadKey::Divide,
            "*" => KeypadKey::Multiply,
            "-" => KeypadKey::Subtract,
            "+" => KeypadKey::Add,
            "=" => KeypadKey::Equal,
            "," => KeypadKey::Separator,
            _ => return None,
        },
        WinitKey::Named(named) => match named {
            NamedKey::Enter => KeypadKey::Enter,
            NamedKey::ArrowLeft => KeypadKey::Left,
            NamedKey::ArrowRight => KeypadKey::Right,
            NamedKey::ArrowUp => KeypadKey::Up,
            NamedKey::ArrowDown => KeypadKey::Down,
            NamedKey::PageUp => KeypadKey::PageUp,
            NamedKey::PageDown => KeypadKey::PageDown,
            NamedKey::Home => KeypadKey::Home,
            NamedKey::End => KeypadKey::End,
            NamedKey::Insert => KeypadKey::Insert,
            NamedKey::Delete => KeypadKey::Delete,
            _ => return None,
        },
        _ => return None,
    })
}

/// The character of a physical key on a US layout.
pub fn base_layout_key(code: KeyCode) -> Option<char> {
    use KeyCode::*;
    let letters = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    if let Some(i) = letters.iter().position(|&k| k == code) {
        return Some(char::from(b'a' + i as u8));
    }
    let digits = [
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    ];
    if let Some(i) = digits.iter().position(|&k| k == code) {
        return Some(char::from(b'0' + i as u8));
    }
    Some(match code {
        Backquote => '`',
        Minus => '-',
        Equal => '=',
        BracketLeft => '[',
        BracketRight => ']',
        Backslash => '\\',
        Semicolon => ';',
        Quote => '\'',
        Comma => ',',
        Period => '.',
        Slash => '/',
        Space => ' ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Mods = Mods::SHIFT;
    const A: Mods = Mods::ALT;
    const C: Mods = Mods::CTRL;

    fn m(parts: &[Mods]) -> Mods {
        parts.iter().fold(Mods::NONE, |acc, &p| acc.union(p))
    }

    fn legacy() -> KeyModes {
        KeyModes::default()
    }

    fn kitty(flags: u8) -> KeyModes {
        KeyModes {
            kitty_flags: flags,
            ..KeyModes::default()
        }
    }

    fn enc(key: Key, mods: Mods, modes: KeyModes) -> String {
        String::from_utf8(encode(&KeyInput::press(key, mods), modes)).unwrap()
    }

    /// A character key as a US keyboard types it: its shifted form and its
    /// text follow from the key and the modifiers.
    fn us_key(key: char, mods: Mods) -> KeyInput<'static> {
        KeyInput {
            shifted: us_shifted(key).filter(|_| mods.contains(S)),
            ..KeyInput::press(Key::Char(key), mods)
        }
    }

    fn us_shifted(key: char) -> Option<char> {
        const PAIRS: &str = "`~1!2@3#4$5%6^7&8*9(0)-_=+[{]}\\|;:'\",<.>/?";
        if key.is_ascii_lowercase() {
            return Some(key.to_ascii_uppercase());
        }
        let chars: Vec<char> = PAIRS.chars().collect();
        chars
            .chunks(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    }

    fn enc_us(key: char, mods: Mods, modes: KeyModes) -> String {
        String::from_utf8(encode(&us_key(key, mods), modes)).unwrap()
    }

    #[test]
    fn legacy_functional_keys_with_every_modifier_combination() {
        // (key, plain, shift, ctrl, alt, ctrl+alt, ctrl+shift, alt+shift),
        // from kitty's own tests of its legacy encoder.
        type Bytes = &'static str;
        type Row = (Key, Bytes, Bytes, Bytes, Bytes, Bytes, Bytes, Bytes);
        let rows: &[Row] = &[
            (
                Key::Enter,
                "\r",
                "\r",
                "\r",
                "\x1b\r",
                "\x1b\r",
                "\r",
                "\x1b\r",
            ),
            (
                Key::Escape,
                "\x1b",
                "\x1b",
                "\x1b",
                "\x1b\x1b",
                "\x1b\x1b",
                "\x1b",
                "\x1b\x1b",
            ),
            (
                Key::Backspace,
                "\x7f",
                "\x7f",
                "\x08",
                "\x1b\x7f",
                "\x1b\x08",
                "\x08",
                "\x1b\x7f",
            ),
            (
                Key::Tab,
                "\t",
                "\x1b[Z",
                "\t",
                "\x1b\t",
                "\x1b\t",
                "\x1b[Z",
                "\x1b\x1b[Z",
            ),
        ];
        for &(key, plain, shift, ctrl, alt, calt, cshift, ashift) in rows {
            assert_eq!(enc(key, Mods::NONE, legacy()), plain, "{key:?}");
            assert_eq!(enc(key, S, legacy()), shift, "shift {key:?}");
            assert_eq!(enc(key, C, legacy()), ctrl, "ctrl {key:?}");
            assert_eq!(enc(key, A, legacy()), alt, "alt {key:?}");
            assert_eq!(enc(key, m(&[C, A]), legacy()), calt, "ctrl+alt {key:?}");
            assert_eq!(enc(key, m(&[C, S]), legacy()), cshift, "ctrl+shift {key:?}");
            assert_eq!(enc(key, m(&[A, S]), legacy()), ashift, "alt+shift {key:?}");
        }
    }

    #[test]
    fn keypad_enter_is_enter_in_legacy_mode() {
        let kp = Key::Keypad(KeypadKey::Enter);
        assert_eq!(enc(kp, Mods::NONE, legacy()), "\r");
        assert_eq!(enc(kp, A, legacy()), "\x1b\r");
    }

    #[test]
    fn legacy_cursor_and_function_keys_carry_xterm_modifier_codes() {
        // (key, unmodified, CSI number, final byte)
        let rows: &[(Key, &str, u32, char)] = &[
            (Key::Up, "\x1b[A", 1, 'A'),
            (Key::Down, "\x1b[B", 1, 'B'),
            (Key::Right, "\x1b[C", 1, 'C'),
            (Key::Left, "\x1b[D", 1, 'D'),
            (Key::Home, "\x1b[H", 1, 'H'),
            (Key::End, "\x1b[F", 1, 'F'),
            (Key::Insert, "\x1b[2~", 2, '~'),
            (Key::Delete, "\x1b[3~", 3, '~'),
            (Key::PageUp, "\x1b[5~", 5, '~'),
            (Key::PageDown, "\x1b[6~", 6, '~'),
            (Key::F(1), "\x1bOP", 1, 'P'),
            (Key::F(2), "\x1bOQ", 1, 'Q'),
            (Key::F(3), "\x1bOR", 13, '~'),
            (Key::F(4), "\x1bOS", 1, 'S'),
            (Key::F(5), "\x1b[15~", 15, '~'),
            (Key::F(6), "\x1b[17~", 17, '~'),
            (Key::F(7), "\x1b[18~", 18, '~'),
            (Key::F(8), "\x1b[19~", 19, '~'),
            (Key::F(9), "\x1b[20~", 20, '~'),
            (Key::F(10), "\x1b[21~", 21, '~'),
            (Key::F(11), "\x1b[23~", 23, '~'),
            (Key::F(12), "\x1b[24~", 24, '~'),
            (Key::Keypad(KeypadKey::Up), "\x1b[A", 1, 'A'),
            (Key::Keypad(KeypadKey::PageUp), "\x1b[5~", 5, '~'),
        ];
        for &(key, plain, number, trailer) in rows {
            assert_eq!(enc(key, Mods::NONE, legacy()), plain, "{key:?}");
            for (mods, code) in [
                (S, 2),
                (A, 3),
                (m(&[S, A]), 4),
                (C, 5),
                (m(&[S, C]), 6),
                (m(&[A, C]), 7),
                (m(&[S, A, C]), 8),
            ] {
                assert_eq!(
                    enc(key, mods, legacy()),
                    format!("\x1b[{number};{code}{trailer}"),
                    "{key:?} {mods:?}"
                );
            }
        }
    }

    #[test]
    fn application_cursor_mode_uses_ss3_for_unmodified_cursor_keys() {
        let modes = KeyModes {
            application_cursor: true,
            ..KeyModes::default()
        };
        for (key, expected) in [
            (Key::Up, "\x1bOA"),
            (Key::Down, "\x1bOB"),
            (Key::Right, "\x1bOC"),
            (Key::Left, "\x1bOD"),
            (Key::Home, "\x1bOH"),
            (Key::End, "\x1bOF"),
        ] {
            assert_eq!(enc(key, Mods::NONE, modes), expected, "{key:?}");
        }
        assert_eq!(enc(Key::Up, C, modes), "\x1b[1;5A");
        // Under the kitty protocol the cursor key mode no longer applies.
        let kitty_app = KeyModes {
            application_cursor: true,
            kitty_flags: DISAMBIGUATE,
            ..KeyModes::default()
        };
        assert_eq!(enc(Key::Up, Mods::NONE, kitty_app), "\x1b[A");
    }

    #[test]
    fn legacy_character_keys_follow_kittys_table() {
        // (key, plain, shift, alt, alt+shift, ctrl, ctrl+alt)
        let rows: &[(char, &str, &str, &str, &str, &str, &str)] = &[
            ('`', "`", "~", "\x1b`", "\x1b~", "`", "\x1b`"),
            ('1', "1", "!", "\x1b1", "\x1b!", "1", "\x1b1"),
            ('2', "2", "@", "\x1b2", "\x1b@", "\x00", "\x1b\x00"),
            ('3', "3", "#", "\x1b3", "\x1b#", "\x1b", "\x1b\x1b"),
            ('4', "4", "$", "\x1b4", "\x1b$", "\x1c", "\x1b\x1c"),
            ('5', "5", "%", "\x1b5", "\x1b%", "\x1d", "\x1b\x1d"),
            ('6', "6", "^", "\x1b6", "\x1b^", "\x1e", "\x1b\x1e"),
            ('7', "7", "&", "\x1b7", "\x1b&", "\x1f", "\x1b\x1f"),
            ('8', "8", "*", "\x1b8", "\x1b*", "\x7f", "\x1b\x7f"),
            ('9', "9", "(", "\x1b9", "\x1b(", "9", "\x1b9"),
            ('0', "0", ")", "\x1b0", "\x1b)", "0", "\x1b0"),
            ('-', "-", "_", "\x1b-", "\x1b_", "-", "\x1b-"),
            ('=', "=", "+", "\x1b=", "\x1b+", "=", "\x1b="),
            ('[', "[", "{", "\x1b[", "\x1b{", "\x1b", "\x1b\x1b"),
            (']', "]", "}", "\x1b]", "\x1b}", "\x1d", "\x1b\x1d"),
            ('\\', "\\", "|", "\x1b\\", "\x1b|", "\x1c", "\x1b\x1c"),
            (';', ";", ":", "\x1b;", "\x1b:", ";", "\x1b;"),
            ('\'', "'", "\"", "\x1b'", "\x1b\"", "'", "\x1b'"),
            (',', ",", "<", "\x1b,", "\x1b<", ",", "\x1b,"),
            ('.', ".", ">", "\x1b.", "\x1b>", ".", "\x1b."),
            ('/', "/", "?", "\x1b/", "\x1b?", "\x1f", "\x1b\x1f"),
            ('a', "a", "A", "\x1ba", "\x1bA", "\x01", "\x1b\x01"),
            ('z', "z", "Z", "\x1bz", "\x1bZ", "\x1a", "\x1b\x1a"),
        ];
        for &(key, plain, shift, alt, ashift, ctrl, calt) in rows {
            assert_eq!(enc_us(key, Mods::NONE, legacy()), plain, "{key:?}");
            assert_eq!(enc_us(key, S, legacy()), shift, "shift {key:?}");
            assert_eq!(enc_us(key, A, legacy()), alt, "alt {key:?}");
            assert_eq!(
                enc_us(key, m(&[A, S]), legacy()),
                ashift,
                "alt+shift {key:?}"
            );
            assert_eq!(enc_us(key, C, legacy()), ctrl, "ctrl {key:?}");
            assert_eq!(enc_us(key, m(&[C, A]), legacy()), calt, "ctrl+alt {key:?}");
        }
        for c in 'a'..='z' {
            let control = ((c as u8) - b'a' + 1) as char;
            assert_eq!(enc_us(c, C, legacy()), control.to_string(), "ctrl+{c}");
        }
    }

    #[test]
    fn legacy_ctrl_with_symbols_sends_the_xterm_control_characters() {
        // Ctrl+Space, Ctrl+@, Ctrl+[, Ctrl+\, Ctrl+], Ctrl+^, Ctrl+~,
        // Ctrl+_, Ctrl+/, and Ctrl+? on a US keyboard.
        let rows: &[(char, Mods, &str)] = &[
            (' ', C, "\x00"),
            ('2', m(&[C, S]), "\x00"), // Ctrl+@
            ('[', C, "\x1b"),
            ('\\', C, "\x1c"),
            (']', C, "\x1d"),
            ('6', m(&[C, S]), "\x1e"), // Ctrl+^
            ('`', m(&[C, S]), "\x1e"), // Ctrl+~
            ('-', m(&[C, S]), "\x1f"), // Ctrl+_
            ('/', C, "\x1f"),
            ('/', m(&[C, S]), "\x7f"), // Ctrl+?
        ];
        for &(key, mods, expected) in rows {
            assert_eq!(enc_us(key, mods, legacy()), expected, "{key:?} {mods:?}");
        }
    }

    #[test]
    fn legacy_space_with_modifiers() {
        assert_eq!(enc(Key::Char(' '), Mods::NONE, legacy()), " ");
        assert_eq!(enc(Key::Char(' '), S, legacy()), " ");
        assert_eq!(enc(Key::Char(' '), C, legacy()), "\x00");
        assert_eq!(enc(Key::Char(' '), A, legacy()), "\x1b ");
        assert_eq!(enc(Key::Char(' '), m(&[C, A]), legacy()), "\x1b\x00");
        assert_eq!(enc(Key::Char(' '), m(&[C, S]), legacy()), "\x00");
        assert_eq!(enc(Key::Char(' '), m(&[A, S]), legacy()), "\x1b ");
    }

    #[test]
    fn legacy_ctrl_shift_letter_is_the_ctrl_letter_control_character() {
        // xterm's answer; kitty would send CSI 105;6u here, which a program
        // that has not asked for it cannot read.
        assert_eq!(enc_us('i', m(&[C, S]), legacy()), "\t");
        assert_eq!(enc_us('a', m(&[C, S]), legacy()), "\x01");
        assert_eq!(enc_us('a', m(&[C, S, A]), legacy()), "\x1b\x01");
    }

    #[test]
    fn legacy_unmappable_combinations_send_the_character() {
        assert_eq!(enc_us('1', m(&[C, S]), legacy()), "!");
        assert_eq!(enc(Key::Char('é'), C, legacy()), "é");
        assert_eq!(enc(Key::Char('é'), A, legacy()), "\x1bé");
    }

    #[test]
    fn legacy_ignores_super() {
        let input = KeyInput {
            text: Some("a"),
            ..KeyInput::press(Key::Char('a'), Mods::SUPER)
        };
        assert_eq!(encode(&input, legacy()), b"a");
        assert_eq!(enc(Key::Up, Mods::SUPER, legacy()), "\x1b[A");
    }

    #[test]
    fn non_latin_layout_uses_the_base_layout_key_for_ctrl_and_alt() {
        // Ctrl+С on a Russian layout is Ctrl+C by position.
        let input = KeyInput {
            base: Some('c'),
            ..KeyInput::press(Key::Char('с'), C)
        };
        assert_eq!(encode(&input, legacy()), b"\x03");
    }

    #[test]
    fn text_is_sent_as_typed() {
        let input = KeyInput {
            shifted: Some('A'),
            text: Some("A"),
            ..KeyInput::press(Key::Char('a'), S)
        };
        assert_eq!(encode(&input, legacy()), b"A");
        assert_eq!(encode(&input, kitty(DISAMBIGUATE)), b"A");
        let composed = KeyInput {
            text: Some("é"),
            ..KeyInput::press(Key::Char('e'), Mods::NONE)
        };
        assert_eq!(encode(&composed, legacy()), "é".as_bytes());
    }

    #[test]
    fn modifier_and_lock_keys_send_nothing_unless_all_keys_are_reported() {
        for code in [57441, 57358] {
            assert_eq!(enc(Key::Kitty(code), Mods::NONE, legacy()), "");
            assert_eq!(enc(Key::Kitty(code), Mods::NONE, kitty(DISAMBIGUATE)), "");
        }
        assert_eq!(
            enc(Key::Kitty(57441), S, kitty(REPORT_ALL)),
            "\x1b[57441;2u"
        );
    }

    #[test]
    fn kitty_only_keys_send_nothing_in_legacy_mode() {
        assert_eq!(enc(Key::F(13), Mods::NONE, legacy()), "");
        assert_eq!(enc(Key::Kitty(57428), Mods::NONE, legacy()), "");
        assert_eq!(
            enc(Key::F(13), Mods::NONE, kitty(DISAMBIGUATE)),
            "\x1b[57376u"
        );
        assert_eq!(enc(Key::Menu, Mods::NONE, legacy()), "\x1b[29~");
        assert_eq!(
            enc(Key::Menu, Mods::NONE, kitty(DISAMBIGUATE)),
            "\x1b[57363u"
        );
    }

    #[test]
    fn releases_send_nothing_without_event_reporting() {
        let input = KeyInput {
            action: KeyAction::Release,
            text: Some("a"),
            ..KeyInput::press(Key::Char('a'), Mods::NONE)
        };
        assert_eq!(encode(&input, legacy()), b"");
        assert_eq!(encode(&input, kitty(DISAMBIGUATE)), b"");
        let repeat = KeyInput {
            action: KeyAction::Repeat,
            ..input
        };
        assert_eq!(encode(&repeat, legacy()), b"a");
    }

    // Kitty protocol: the examples of kitty's own test suite.

    #[test]
    fn enter_sends_cr_lf_under_lnm() {
        let lnm = KeyModes {
            linefeed_newline: true,
            ..KeyModes::default()
        };
        assert_eq!(enc(Key::Enter, Mods::NONE, lnm), "\r\n");
        assert_eq!(enc(Key::Enter, A, lnm), "\x1b\r\n");
        assert_eq!(enc(Key::Enter, Mods::NONE, KeyModes::default()), "\r");
    }

    #[test]
    fn kitty_disambiguate() {
        let f = kitty(DISAMBIGUATE);
        assert_eq!(enc(Key::Char('a'), Mods::NONE, f), "a");
        assert_eq!(enc(Key::Escape, Mods::NONE, f), "\x1b[27u");
        assert_eq!(enc(Key::Enter, Mods::NONE, f), "\r");
        assert_eq!(enc(Key::Enter, S, f), "\x1b[13;2u");
        assert_eq!(enc(Key::Tab, Mods::NONE, f), "\t");
        assert_eq!(enc(Key::Backspace, Mods::NONE, f), "\x7f");
        assert_eq!(enc(Key::Tab, S, f), "\x1b[9;2u");
        for (mods, code) in [(C, 5), (A, 3), (m(&[C, S]), 6), (m(&[A, S]), 4)] {
            assert_eq!(enc(Key::Char('a'), mods, f), format!("\x1b[97;{code}u"));
        }
        assert_eq!(enc(Key::Char(' '), C, f), "\x1b[32;5u");
        for keypad in [KeypadKey::PageUp, KeypadKey::Digit(0)] {
            let code = keypad.kitty_code();
            assert_eq!(
                enc(Key::Keypad(keypad), Mods::NONE, f),
                format!("\x1b[{code}u")
            );
            assert_eq!(enc(Key::Keypad(keypad), C, f), format!("\x1b[{code};5u"));
        }
        assert_eq!(enc(Key::Up, Mods::NONE, f), "\x1b[A");
        assert_eq!(enc(Key::Up, C, f), "\x1b[1;5A");
        assert_eq!(enc(Key::F(1), Mods::NONE, f), "\x1b[P");
        assert_eq!(enc(Key::F(3), Mods::NONE, f), "\x1b[13~");
    }

    #[test]
    fn kitty_disambiguate_reports_super() {
        assert_eq!(
            enc(Key::Char('a'), Mods::SUPER, kitty(DISAMBIGUATE)),
            "\x1b[97;9u"
        );
    }

    #[test]
    fn kitty_report_event_types() {
        let f = kitty(REPORT_EVENTS);
        let a = |action, mods| KeyInput {
            action,
            ..KeyInput::press(Key::Char('a'), mods)
        };
        assert_eq!(encode(&a(KeyAction::Press, Mods::NONE), f), b"a");
        assert_eq!(
            encode(&a(KeyAction::Repeat, Mods::NONE), f),
            b"\x1b[97;1:2u"
        );
        assert_eq!(
            encode(&a(KeyAction::Release, Mods::NONE), f),
            b"\x1b[97;1:3u"
        );
        assert_eq!(encode(&a(KeyAction::Release, S), f), b"\x1b[97;2:3u");

        let f = kitty(DISAMBIGUATE | REPORT_EVENTS);
        let bs = |action| KeyInput {
            action,
            ..KeyInput::press(Key::Backspace, Mods::NONE)
        };
        assert_eq!(encode(&bs(KeyAction::Press), f), b"\x7f");
        assert_eq!(encode(&bs(KeyAction::Release), f), b"");
        let up = KeyInput {
            action: KeyAction::Release,
            ..KeyInput::press(Key::Up, Mods::NONE)
        };
        assert_eq!(encode(&up, f), b"\x1b[1;1:3A");
    }

    #[test]
    fn kitty_release_of_a_text_key_is_reported() {
        let input = KeyInput {
            action: KeyAction::Release,
            text: Some("a"),
            ..KeyInput::press(Key::Char('a'), Mods::NONE)
        };
        assert_eq!(
            encode(&input, kitty(DISAMBIGUATE | REPORT_EVENTS)),
            b"\x1b[97;1:3u"
        );
    }

    #[test]
    fn kitty_report_alternate_keys() {
        let f = kitty(REPORT_ALTERNATES);
        let with = |mods, shifted, base| KeyInput {
            shifted,
            base,
            ..KeyInput::press(Key::Char('a'), mods)
        };
        assert_eq!(encode(&with(Mods::NONE, None, None), f), b"a");
        // The shifted key counts only while Shift is held.
        assert_eq!(encode(&with(Mods::NONE, Some('A'), None), f), b"a");
        assert_eq!(encode(&with(S, Some('A'), None), f), b"\x1b[97:65;2u");
        assert_eq!(
            encode(&with(Mods::NONE, None, Some('A')), f),
            b"\x1b[97::65u"
        );
        assert_eq!(
            encode(&with(S, Some('A'), Some('b')), f),
            b"\x1b[97:65:98;2u"
        );
    }

    #[test]
    fn kitty_report_all_keys() {
        let f = kitty(REPORT_ALL);
        assert_eq!(enc(Key::Char('a'), Mods::NONE, f), "\x1b[97u");
        let repeat = KeyInput {
            action: KeyAction::Repeat,
            ..KeyInput::press(Key::Char('a'), Mods::NONE)
        };
        assert_eq!(encode(&repeat, f), b"\x1b[97u");
        assert_eq!(enc(Key::Char('a'), C, f), "\x1b[97;5u");
        assert_eq!(enc(Key::Up, Mods::NONE, f), "\x1b[A");
        assert_eq!(enc(Key::Kitty(57441), Mods::NONE, f), "\x1b[57441u");
        assert_eq!(enc(Key::Enter, Mods::NONE, f), "\x1b[13u");
        assert_eq!(enc(Key::Enter, C, f), "\x1b[13;5u");
        assert_eq!(enc(Key::Tab, Mods::NONE, f), "\x1b[9u");
        assert_eq!(enc(Key::Backspace, Mods::NONE, f), "\x1b[127u");
        assert_eq!(enc(Key::Escape, Mods::NONE, f), "\x1b[27u");
        // Text is not sent on its own.
        let typed = KeyInput {
            text: Some("a"),
            ..KeyInput::press(Key::Char('a'), Mods::NONE)
        };
        assert_eq!(encode(&typed, f), b"\x1b[97u");
    }

    #[test]
    fn kitty_report_associated_text() {
        let f = kitty(REPORT_ALL | REPORT_TEXT);
        let typed = |mods, text| KeyInput {
            text: Some(text),
            ..KeyInput::press(Key::Char('a'), mods)
        };
        assert_eq!(encode(&typed(Mods::NONE, "a"), f), b"\x1b[97;;97u");
        assert_eq!(encode(&typed(S, "A"), f), b"\x1b[97;2;65u");
        assert_eq!(encode(&typed(S, "AB"), f), b"\x1b[97;2;65:66u");
        // A shortcut has no text.
        assert_eq!(encode(&typed(C, "\u{1}"), f), b"\x1b[97;5u");
    }

    #[test]
    fn kitty_flags_without_disambiguate_still_use_csi_u() {
        // Flags 2, 4 or 16 alone do not bring back xterm's fallbacks.
        assert_eq!(enc_us('i', m(&[C, S]), kitty(REPORT_EVENTS)), "\x1b[105;6u");
        assert_eq!(enc_us('a', C, kitty(REPORT_EVENTS)), "\x01");
    }

    // modifyOtherKeys

    fn mok(level: u8) -> KeyModes {
        KeyModes {
            modify_other_keys: level,
            ..KeyModes::default()
        }
    }

    #[test]
    fn modify_other_keys_level_one_changes_only_lossy_keys() {
        let f = mok(1);
        assert_eq!(enc_us('a', C, f), "\x01");
        assert_eq!(enc_us('a', A, f), "\x1ba");
        assert_eq!(enc_us('a', m(&[C, S]), f), "\x1b[27;6;65~");
        assert_eq!(enc_us('1', C, f), "\x1b[27;5;49~");
        assert_eq!(enc_us(';', C, f), "\x1b[27;5;59~");
        assert_eq!(enc_us('/', C, f), "\x1f");
        assert_eq!(enc(Key::Enter, C, f), "\x1b[27;5;13~");
        assert_eq!(enc(Key::Enter, S, f), "\x1b[27;2;13~");
        assert_eq!(enc(Key::Enter, A, f), "\x1b\r");
        assert_eq!(enc(Key::Tab, C, f), "\x1b[27;5;9~");
        assert_eq!(enc(Key::Tab, S, f), "\x1b[Z");
        assert_eq!(enc(Key::Backspace, C, f), "\x08");
        assert_eq!(enc(Key::Up, C, f), "\x1b[1;5A");
    }

    #[test]
    fn modify_other_keys_level_two_changes_every_ctrl_or_alt_key() {
        let f = mok(2);
        assert_eq!(enc_us('a', Mods::NONE, f), "a");
        assert_eq!(enc_us('a', S, f), "A");
        assert_eq!(enc_us('a', C, f), "\x1b[27;5;97~");
        assert_eq!(enc_us('a', A, f), "\x1b[27;3;97~");
        assert_eq!(enc(Key::Char(' '), C, f), "\x1b[27;5;32~");
        assert_eq!(enc(Key::Enter, A, f), "\x1b[27;3;13~");
        assert_eq!(enc(Key::Tab, S, f), "\x1b[Z");
        assert_eq!(enc(Key::Escape, Mods::NONE, f), "\x1b");
        assert_eq!(enc(Key::Up, C, f), "\x1b[1;5A");
    }

    #[test]
    fn kitty_flags_take_precedence_over_modify_other_keys() {
        let modes = KeyModes {
            kitty_flags: DISAMBIGUATE,
            modify_other_keys: 2,
            ..KeyModes::default()
        };
        assert_eq!(enc_us('a', C, modes), "\x1b[97;5u");
    }

    // Translation from winit

    #[test]
    fn winit_keys_translate() {
        use winit::keyboard::SmolStr;
        let ch = |s: &str| WinitKey::Character(SmolStr::new(s));
        let named = WinitKey::Named;
        let st = KeyLocation::Standard;
        let rows: &[(WinitKey, WinitKey, KeyLocation, Option<Key>)] = &[
            (ch("A"), ch("a"), st, Some(Key::Char('a'))),
            (ch("!"), ch("1"), st, Some(Key::Char('1'))),
            // Caps Lock: still the lower-case key.
            (ch("A"), ch("A"), st, Some(Key::Char('a'))),
            (
                named(NamedKey::Space),
                named(NamedKey::Space),
                st,
                Some(Key::Char(' ')),
            ),
            (
                named(NamedKey::Tab),
                named(NamedKey::Tab),
                st,
                Some(Key::Tab),
            ),
            (
                named(NamedKey::F5),
                named(NamedKey::F5),
                st,
                Some(Key::F(5)),
            ),
            (
                named(NamedKey::F35),
                named(NamedKey::F35),
                st,
                Some(Key::F(35)),
            ),
            (
                ch("1"),
                named(NamedKey::End),
                KeyLocation::Numpad,
                Some(Key::Keypad(KeypadKey::Digit(1))),
            ),
            (
                named(NamedKey::End),
                named(NamedKey::End),
                KeyLocation::Numpad,
                Some(Key::Keypad(KeypadKey::End)),
            ),
            (
                named(NamedKey::Enter),
                named(NamedKey::Enter),
                KeyLocation::Numpad,
                Some(Key::Keypad(KeypadKey::Enter)),
            ),
            (
                named(NamedKey::Shift),
                named(NamedKey::Shift),
                KeyLocation::Left,
                Some(Key::Kitty(57441)),
            ),
            (
                named(NamedKey::Control),
                named(NamedKey::Control),
                KeyLocation::Right,
                Some(Key::Kitty(57448)),
            ),
            (WinitKey::Dead(Some('´')), ch("'"), st, None),
        ];
        for (logical, unmodified, location, expected) in rows {
            assert_eq!(
                key_from_winit(logical, unmodified, *location),
                *expected,
                "{logical:?}"
            );
        }
    }

    #[test]
    fn base_layout_keys() {
        assert_eq!(base_layout_key(KeyCode::KeyC), Some('c'));
        assert_eq!(base_layout_key(KeyCode::Digit0), Some('0'));
        assert_eq!(base_layout_key(KeyCode::BracketLeft), Some('['));
        assert_eq!(base_layout_key(KeyCode::F1), None);
    }
}
