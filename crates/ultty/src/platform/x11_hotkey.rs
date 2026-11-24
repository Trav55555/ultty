//! X11 global hotkey registration via `XGrabKey` (ADR 0028 Slice 3).
//!
//! Uses a dedicated `x11rb` `RustConnection` watched with `polling::Poller`
//! alongside a stop pipe so idle grabs incur zero timer wakeups and release
//! cleanly on drop or config reload.

use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::thread::{self, JoinHandle};

use polling::{Event as PollEvent, Events, Poller};
use thiserror::Error;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{
    ConnectionExt, GrabMode, Keycode, Keysym, ModMask, Window as X11Window,
};
use x11rb::protocol::Event as X11Event;
use x11rb::rust_connection::RustConnection;

use crate::config::{KeybindingConfig, ParsedKeybinding};

const X11_FD_KEY: usize = 1;
const STOP_FD_KEY: usize = 2;

const XK_ALT_L: Keysym = 0xffe9;
const XK_ALT_R: Keysym = 0xffea;
const XK_META_L: Keysym = 0xffe7;
const XK_META_R: Keysym = 0xffe8;
const XK_SUPER_L: Keysym = 0xffeb;
const XK_SUPER_R: Keysym = 0xffec;
const XK_NUM_LOCK: Keysym = 0xff7f;

/// Errors returned while registering an X11 global hotkey.
#[derive(Debug, Error)]
pub enum HotkeyError {
    /// The binding string did not parse under `[keybindings]` syntax.
    #[error("invalid hotkey syntax '{binding}' (expected e.g. 'ctrl+grave' or 'super+grave')")]
    InvalidSyntax { binding: String },
    /// The key name is not mapped to an X11 keysym.
    #[error("unsupported key '{key}' in hotkey '{binding}'")]
    UnsupportedKey { binding: String, key: String },
    /// Could not open an X11 connection.
    #[error("failed to connect to X11 display: {0}")]
    Connect(String),
    /// The active X11 keymap has no keycode producing the requested keysym.
    #[error("key '{key}' in hotkey '{binding}' is not mapped in the current X11 keymap")]
    UnmappedKey { binding: String, key: String },
    /// Another X11 client already holds an overlapping passive grab.
    #[error("hotkey '{binding}' is already grabbed by another X11 client")]
    AlreadyGrabbed { binding: String },
    /// Any other X11 protocol or OS error.
    #[error("X11 hotkey error for '{binding}': {detail}")]
    Protocol { binding: String, detail: String },
}

/// Active X11 global hotkey grab. Dropping it wakes the listener thread,
/// ungrabs the key on the root window, and joins the thread.
pub struct X11GlobalHotkey {
    stop_tx: Option<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

impl X11GlobalHotkey {
    /// Parse `binding` using `[keybindings]` syntax, grab it on the X11 root
    /// window, and invoke `on_trigger` whenever it is pressed globally.
    pub fn register<F>(binding: &str, on_trigger: F) -> Result<Self, HotkeyError>
    where
        F: Fn() + Send + 'static,
    {
        let parsed = KeybindingConfig::parse_keybinding(binding).ok_or_else(|| {
            HotkeyError::InvalidSyntax {
                binding: binding.to_owned(),
            }
        })?;
        let keysym =
            key_name_to_keysym(&parsed.key).ok_or_else(|| HotkeyError::UnsupportedKey {
                binding: binding.to_owned(),
                key: parsed.key.clone(),
            })?;

        let (conn, screen_num) =
            RustConnection::connect(None).map_err(|e| HotkeyError::Connect(e.to_string()))?;
        let root = conn.setup().roots[screen_num].root;

        let keycodes = find_keycodes_for_keysym(&conn, keysym, binding)?;
        if keycodes.is_empty() {
            return Err(HotkeyError::UnmappedKey {
                binding: binding.to_owned(),
                key: parsed.key,
            });
        }

        let detected = detect_modifier_masks(&conn);
        let base_mods = modifier_mask_for(&parsed, detected);
        let ignored_lock_mask = u16::from(ModMask::LOCK) | detected.num_lock;
        let grab_masks = expand_lock_variants(base_mods, detected.num_lock);

        let mut grabbed: Vec<(Keycode, u16)> = Vec::new();
        for &keycode in &keycodes {
            for &mask in &grab_masks {
                let cookie = conn
                    .grab_key(
                        false,
                        root,
                        ModMask::from(mask),
                        keycode,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )
                    .map_err(|e| HotkeyError::Protocol {
                        binding: binding.to_owned(),
                        detail: e.to_string(),
                    })?;
                if let Err(err) = cookie.check() {
                    ungrab_all(&conn, root, &grabbed);
                    return Err(classify_grab_error(binding, err));
                }
                grabbed.push((keycode, mask));
            }
        }
        conn.flush().map_err(|e| {
            ungrab_all(&conn, root, &grabbed);
            HotkeyError::Protocol {
                binding: binding.to_owned(),
                detail: e.to_string(),
            }
        })?;

        let (stop_rx, stop_tx) = nix::unistd::pipe().map_err(|e| {
            ungrab_all(&conn, root, &grabbed);
            HotkeyError::Protocol {
                binding: binding.to_owned(),
                detail: format!("failed to create wakeup pipe: {e}"),
            }
        })?;

        let thread = thread::Builder::new()
            .name("ultty-x11-hotkey".to_string())
            .spawn(move || {
                run_hotkey_loop(
                    conn,
                    root,
                    keycodes,
                    base_mods,
                    ignored_lock_mask,
                    grabbed,
                    stop_rx,
                    on_trigger,
                );
            })
            .map_err(|e| HotkeyError::Protocol {
                binding: binding.to_owned(),
                detail: format!("failed to spawn hotkey thread: {e}"),
            })?;

        Ok(Self {
            stop_tx: Some(stop_tx),
            thread: Some(thread),
        })
    }
}

impl Drop for X11GlobalHotkey {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let mut file = std::fs::File::from(stop_tx);
            let _ = file.write_all(&[1u8]);
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_hotkey_loop<F>(
    conn: RustConnection,
    root: X11Window,
    keycodes: Vec<Keycode>,
    base_mods: u16,
    ignored_lock_mask: u16,
    grabbed: Vec<(Keycode, u16)>,
    stop_rx: OwnedFd,
    on_trigger: F,
) where
    F: Fn(),
{
    let Ok(poller) = Poller::new() else {
        ungrab_all(&conn, root, &grabbed);
        return;
    };
    let x11_fd = conn.stream().as_raw_fd();
    let stop_fd = stop_rx.as_raw_fd();
    // SAFETY: `conn` and `stop_rx` outlive the poller and are deleted before returning.
    unsafe {
        if poller.add(x11_fd, PollEvent::readable(X11_FD_KEY)).is_err()
            || poller
                .add(stop_fd, PollEvent::readable(STOP_FD_KEY))
                .is_err()
        {
            ungrab_all(&conn, root, &grabbed);
            return;
        }
    }

    let relevant_mask = !ignored_lock_mask & 0x00ff;
    let mut events = Events::new();
    let mut last_release_time: Option<u32> = None;

    'outer: loop {
        events.clear();
        if let Err(err) = poller.wait(&mut events, None) {
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::debug!("X11 hotkey poller wait failed: {err}");
            break;
        }

        let mut x11_ready = false;
        for ev in events.iter() {
            if ev.key == STOP_FD_KEY {
                break 'outer;
            }
            if ev.key == X11_FD_KEY {
                x11_ready = true;
            }
        }

        if x11_ready {
            loop {
                match conn.poll_for_event() {
                    Ok(Some(X11Event::KeyPress(ev))) => {
                        let mods = u16::from(ev.state) & relevant_mask;
                        let is_auto_repeat = last_release_time == Some(ev.time);
                        if !is_auto_repeat && keycodes.contains(&ev.detail) && mods == base_mods {
                            on_trigger();
                        }
                    }
                    Ok(Some(X11Event::KeyRelease(ev))) => {
                        if keycodes.contains(&ev.detail) {
                            last_release_time = Some(ev.time);
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(err) => {
                        tracing::debug!("X11 hotkey connection closed: {err}");
                        break 'outer;
                    }
                }
            }
            let _ = poller.modify(conn.stream(), PollEvent::readable(X11_FD_KEY));
        }
    }

    let _ = poller.delete(conn.stream());
    let _ = poller.delete(&stop_rx);
    ungrab_all(&conn, root, &grabbed);
}

fn ungrab_all(conn: &RustConnection, root: X11Window, grabbed: &[(Keycode, u16)]) {
    for &(keycode, mask) in grabbed {
        let _ = conn.ungrab_key(keycode, root, ModMask::from(mask));
    }
    let _ = conn.flush();
}

fn classify_grab_error(binding: &str, err: ReplyError) -> HotkeyError {
    if let ReplyError::X11Error(ref xerr) = err {
        if xerr.error_kind == x11rb::protocol::ErrorKind::Access {
            return HotkeyError::AlreadyGrabbed {
                binding: binding.to_owned(),
            };
        }
    }
    HotkeyError::Protocol {
        binding: binding.to_owned(),
        detail: err.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetectedModifiers {
    alt: u16,
    super_key: u16,
    num_lock: u16,
}

impl Default for DetectedModifiers {
    fn default() -> Self {
        Self {
            alt: u16::from(ModMask::M1),
            super_key: u16::from(ModMask::M4),
            num_lock: u16::from(ModMask::M2),
        }
    }
}

fn detect_modifier_masks(conn: &RustConnection) -> DetectedModifiers {
    let mut detected = DetectedModifiers::default();
    let Ok(mod_cookie) = conn.get_modifier_mapping() else {
        return detected;
    };
    let Ok(mod_reply) = mod_cookie.reply() else {
        return detected;
    };
    let setup = conn.setup();
    let min_kc = setup.min_keycode;
    let count = setup.max_keycode.saturating_sub(min_kc).saturating_add(1);
    let Ok(map_cookie) = conn.get_keyboard_mapping(min_kc, count) else {
        return detected;
    };
    let Ok(map_reply) = map_cookie.reply() else {
        return detected;
    };
    let per = usize::from(map_reply.keysyms_per_keycode);
    if per == 0 {
        return detected;
    }

    let keysyms_for = |kc: Keycode| -> &[Keysym] {
        if kc < min_kc {
            return &[];
        }
        let start = usize::from(kc - min_kc) * per;
        let end = (start + per).min(map_reply.keysyms.len());
        map_reply.keysyms.get(start..end).unwrap_or(&[])
    };

    let per_mod = usize::from(mod_reply.keycodes_per_modifier());
    let mut found_alt = false;
    let mut found_super = false;
    let mut found_num = false;
    const MOD1_TO_MOD5_ROWS: std::ops::Range<usize> = 3..8;
    for row in MOD1_TO_MOD5_ROWS {
        let bit = 1u16 << row;
        let slice = mod_reply
            .keycodes
            .get(row * per_mod..(row + 1) * per_mod)
            .unwrap_or(&[]);
        for &kc in slice {
            if kc == 0 {
                continue;
            }
            for &sym in keysyms_for(kc) {
                if !found_alt && matches!(sym, XK_ALT_L | XK_ALT_R | XK_META_L | XK_META_R) {
                    detected.alt = bit;
                    found_alt = true;
                }
                if !found_super && matches!(sym, XK_SUPER_L | XK_SUPER_R) {
                    detected.super_key = bit;
                    found_super = true;
                }
                if !found_num && sym == XK_NUM_LOCK {
                    detected.num_lock = bit;
                    found_num = true;
                }
            }
        }
    }
    detected
}

fn modifier_mask_for(parsed: &ParsedKeybinding, detected: DetectedModifiers) -> u16 {
    let mut mask = 0u16;
    if parsed.ctrl {
        mask |= u16::from(ModMask::CONTROL);
    }
    if parsed.shift {
        mask |= u16::from(ModMask::SHIFT);
    }
    if parsed.alt {
        mask |= detected.alt;
    }
    if parsed.super_key {
        mask |= detected.super_key;
    }
    mask
}

/// Expand `base` across every combination of CapsLock and NumLock so the grab
/// fires regardless of lock key state.
fn expand_lock_variants(base: u16, num_lock: u16) -> Vec<u16> {
    let lock = u16::from(ModMask::LOCK);
    let mut variants = Vec::with_capacity(4);
    for extra in [0, lock, num_lock, lock | num_lock] {
        let combined = base | extra;
        if !variants.contains(&combined) {
            variants.push(combined);
        }
    }
    variants
}

fn find_keycodes_for_keysym(
    conn: &RustConnection,
    target: Keysym,
    binding: &str,
) -> Result<Vec<Keycode>, HotkeyError> {
    let setup = conn.setup();
    let min_kc = setup.min_keycode;
    let count = setup.max_keycode.saturating_sub(min_kc).saturating_add(1);
    let reply = conn
        .get_keyboard_mapping(min_kc, count)
        .map_err(|e| HotkeyError::Protocol {
            binding: binding.to_owned(),
            detail: e.to_string(),
        })?
        .reply()
        .map_err(|e| HotkeyError::Protocol {
            binding: binding.to_owned(),
            detail: e.to_string(),
        })?;

    let per = usize::from(reply.keysyms_per_keycode);
    if per == 0 {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for offset in 0..usize::from(count) {
        let start = offset * per;
        let end = (start + per).min(reply.keysyms.len());
        if let Some(syms) = reply.keysyms.get(start..end) {
            const UNSHIFTED_AND_SHIFTED_LEVELS: usize = 2;
            let check_len = syms.len().min(UNSHIFTED_AND_SHIFTED_LEVELS);
            if syms[..check_len].contains(&target) {
                out.push(min_kc + offset as u8);
            }
        }
    }
    Ok(out)
}

/// Map a lowercased key name from `[keybindings]` syntax to an X11 keysym.
pub fn key_name_to_keysym(name: &str) -> Option<Keysym> {
    let name = name.trim();
    if name.len() == 1 {
        let b = name.as_bytes()[0];
        if b.is_ascii_lowercase() || b.is_ascii_digit() {
            return Some(u32::from(b));
        }
    }
    if let Some(rest) = name.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0xffbe + (n - 1));
            }
        }
    }
    match name {
        "grave" | "`" | "backquote" | "tilde" | "~" => Some(0x0060),
        "space" | " " => Some(0x0020),
        "tab" => Some(0xff09),
        "enter" | "return" => Some(0xff0d),
        "escape" | "esc" => Some(0xff1b),
        "backspace" => Some(0xff08),
        "delete" | "del" => Some(0xffff),
        "insert" | "ins" => Some(0xff63),
        "home" => Some(0xff50),
        "end" => Some(0xff57),
        "pageup" | "page_up" => Some(0xff55),
        "pagedown" | "page_down" => Some(0xff56),
        "up" => Some(0xff52),
        "down" => Some(0xff54),
        "left" => Some(0xff51),
        "right" => Some(0xff53),
        "minus" | "-" => Some(0x002d),
        "equal" | "equals" | "=" => Some(0x003d),
        "plus" | "+" => Some(0x002b),
        "bracketleft" | "[" => Some(0x005b),
        "bracketright" | "]" => Some(0x005d),
        "backslash" | "\\" => Some(0x005c),
        "semicolon" | ";" => Some(0x003b),
        "apostrophe" | "'" | "quote" => Some(0x0027),
        "comma" | "," => Some(0x002c),
        "period" | "." => Some(0x002e),
        "slash" | "/" => Some(0x002f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_expected_x11_keysyms() {
        assert_eq!(key_name_to_keysym("grave"), Some(0x0060));
        assert_eq!(key_name_to_keysym("`"), Some(0x0060));
        assert_eq!(key_name_to_keysym("backquote"), Some(0x0060));
        assert_eq!(key_name_to_keysym("space"), Some(0x0020));
        assert_eq!(key_name_to_keysym("a"), Some(u32::from(b'a')));
        assert_eq!(key_name_to_keysym("9"), Some(u32::from(b'9')));
        assert_eq!(key_name_to_keysym("f1"), Some(0xffbe));
        assert_eq!(key_name_to_keysym("f12"), Some(0xffc9));
        assert_eq!(key_name_to_keysym("f24"), Some(0xffd5));
        assert_eq!(key_name_to_keysym("f25"), None);
        assert_eq!(key_name_to_keysym("not_a_key"), None);
    }

    #[test]
    fn modifier_mask_and_lock_variants_expand_cleanly() {
        let parsed = KeybindingConfig::parse_keybinding("ctrl+super+grave").unwrap();
        let detected = DetectedModifiers::default();
        let mask = modifier_mask_for(&parsed, detected);
        assert_eq!(mask, u16::from(ModMask::CONTROL) | u16::from(ModMask::M4));

        let variants = expand_lock_variants(mask, detected.num_lock);
        assert_eq!(variants.len(), 4);
        assert!(variants.contains(&mask));
        assert!(variants.contains(&(mask | u16::from(ModMask::LOCK))));
        assert!(variants.contains(&(mask | detected.num_lock)));
        assert!(variants.contains(&(mask | u16::from(ModMask::LOCK) | detected.num_lock)));

        let no_num = expand_lock_variants(mask, 0);
        assert_eq!(no_num.len(), 2);
    }

    #[test]
    fn error_messages_are_clear_for_conflicts_and_syntax() {
        let conflict = HotkeyError::AlreadyGrabbed {
            binding: "ctrl+grave".to_string(),
        };
        assert!(conflict.to_string().contains("already grabbed"));
        assert!(conflict.to_string().contains("ctrl+grave"));

        let bad_syntax = X11GlobalHotkey::register("bogus+grave", || {})
            .err()
            .unwrap();
        assert!(matches!(bad_syntax, HotkeyError::InvalidSyntax { .. }));

        let bad_key = X11GlobalHotkey::register("ctrl+hyperkey", || {})
            .err()
            .unwrap();
        assert!(matches!(bad_key, HotkeyError::UnsupportedKey { .. }));
    }
}
