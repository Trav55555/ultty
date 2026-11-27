//! macOS quick terminal window polish and Carbon global hotkey (ADR 0028 Slice 5).
//!
//! Keybinding parsing and ANSI US virtual keycode translation compile on every
//! platform so the mapping stays tested on Linux CI as well as macOS. The
//! Carbon `RegisterEventHotKey` registration and AppKit `NSWindow` /
//! `NSWorkspace` helpers compile on `target_os = "macos"`.

use thiserror::Error;

use crate::config::{KeybindingConfig, ParsedKeybinding};

/// Carbon `cmdKey` modifier bit (`<Carbon/HIToolbox/Events.h>`).
pub const CARBON_CMD_KEY: u32 = 1 << 8;
/// Carbon `shiftKey` modifier bit (`<Carbon/HIToolbox/Events.h>`).
pub const CARBON_SHIFT_KEY: u32 = 1 << 9;
/// Carbon `optionKey` modifier bit (`<Carbon/HIToolbox/Events.h>`).
pub const CARBON_OPTION_KEY: u32 = 1 << 11;
/// Carbon `controlKey` modifier bit (`<Carbon/HIToolbox/Events.h>`).
pub const CARBON_CONTROL_KEY: u32 = 1 << 12;

/// Parsed macOS Carbon hotkey specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacosHotkeySpec {
    /// macOS HIToolbox virtual keycode (`kVK_*`).
    pub key_code: u32,
    /// Carbon modifier bitmask (`cmdKey` | `shiftKey` | `optionKey` | `controlKey`).
    pub modifiers: u32,
}

/// Errors returned while registering a macOS global hotkey.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MacosHotkeyError {
    /// The binding string did not parse under `[keybindings]` syntax.
    #[error("invalid hotkey syntax '{binding}' (expected e.g. 'cmd+grave' or 'ctrl+grave')")]
    InvalidSyntax { binding: String },
    /// The key name is not mapped to a macOS virtual keycode.
    #[error("unsupported key '{key}' in hotkey '{binding}'")]
    UnsupportedKey { binding: String, key: String },
    /// Carbon hotkey registration was attempted off the main thread.
    #[error("macOS global hotkey registration must run on the main thread")]
    MissingMainThread,
    /// `InstallEventHandler` returned a non-zero `OSStatus`.
    #[error("InstallEventHandler failed for '{binding}' with OSStatus {status}")]
    InstallHandlerFailed { binding: String, status: i32 },
    /// `RegisterEventHotKey` returned a non-zero `OSStatus`.
    #[error("RegisterEventHotKey failed for '{binding}' with OSStatus {status}")]
    RegisterFailed { binding: String, status: i32 },
}

/// Parse a `[quick_terminal].hotkey` binding string into a macOS Carbon virtual
/// keycode and modifier mask.
pub fn parse_macos_hotkey(binding: &str) -> Result<MacosHotkeySpec, MacosHotkeyError> {
    let parsed = KeybindingConfig::parse_keybinding(binding).ok_or_else(|| {
        MacosHotkeyError::InvalidSyntax {
            binding: binding.to_owned(),
        }
    })?;
    if parsed.key.is_empty() {
        return Err(MacosHotkeyError::InvalidSyntax {
            binding: binding.to_owned(),
        });
    }
    let key_code =
        key_name_to_macos_keycode(&parsed.key).ok_or_else(|| MacosHotkeyError::UnsupportedKey {
            binding: binding.to_owned(),
            key: parsed.key.clone(),
        })?;
    Ok(MacosHotkeySpec {
        key_code,
        modifiers: carbon_modifiers_for(&parsed),
    })
}

/// Convert `[keybindings]` modifier flags to Carbon modifier bits.
pub fn carbon_modifiers_for(parsed: &ParsedKeybinding) -> u32 {
    let mut mask = 0u32;
    if parsed.super_key {
        mask |= CARBON_CMD_KEY;
    }
    if parsed.shift {
        mask |= CARBON_SHIFT_KEY;
    }
    if parsed.alt {
        mask |= CARBON_OPTION_KEY;
    }
    if parsed.ctrl {
        mask |= CARBON_CONTROL_KEY;
    }
    mask
}

/// Translate a normalized lowercase `[keybindings]` key name to a macOS
/// HIToolbox virtual keycode (`kVK_*`).
pub fn key_name_to_macos_keycode(key: &str) -> Option<u32> {
    match key {
        "a" => Some(0x00),
        "s" => Some(0x01),
        "d" => Some(0x02),
        "f" => Some(0x03),
        "h" => Some(0x04),
        "g" => Some(0x05),
        "z" => Some(0x06),
        "x" => Some(0x07),
        "c" => Some(0x08),
        "v" => Some(0x09),
        "b" => Some(0x0B),
        "q" => Some(0x0C),
        "w" => Some(0x0D),
        "e" => Some(0x0E),
        "r" => Some(0x0F),
        "y" => Some(0x10),
        "t" => Some(0x11),
        "1" => Some(0x12),
        "2" => Some(0x13),
        "3" => Some(0x14),
        "4" => Some(0x15),
        "6" => Some(0x16),
        "5" => Some(0x17),
        "equal" | "equals" | "plus" | "=" => Some(0x18),
        "9" => Some(0x19),
        "7" => Some(0x1A),
        "minus" | "hyphen" | "-" => Some(0x1B),
        "8" => Some(0x1C),
        "0" => Some(0x1D),
        "rightbracket" | "rbracket" | "]" => Some(0x1E),
        "o" => Some(0x1F),
        "u" => Some(0x20),
        "leftbracket" | "lbracket" | "[" => Some(0x21),
        "i" => Some(0x22),
        "p" => Some(0x23),
        "return" | "enter" => Some(0x24),
        "l" => Some(0x25),
        "j" => Some(0x26),
        "quote" | "apostrophe" | "'" => Some(0x27),
        "k" => Some(0x28),
        "semicolon" | ";" => Some(0x29),
        "backslash" | "\\" => Some(0x2A),
        "comma" | "," => Some(0x2B),
        "slash" | "/" => Some(0x2C),
        "n" => Some(0x2D),
        "m" => Some(0x2E),
        "period" | "dot" | "." => Some(0x2F),
        "tab" => Some(0x30),
        "space" => Some(0x31),
        "grave" | "backquote" | "tilde" | "`" => Some(0x32),
        "backspace" | "delete" => Some(0x33),
        "escape" | "esc" => Some(0x35),
        "f17" => Some(0x40),
        "f18" => Some(0x4F),
        "f19" => Some(0x50),
        "f20" => Some(0x5A),
        "f5" => Some(0x60),
        "f6" => Some(0x61),
        "f7" => Some(0x62),
        "f3" => Some(0x63),
        "f8" => Some(0x64),
        "f9" => Some(0x65),
        "f11" => Some(0x67),
        "f13" => Some(0x69),
        "f16" => Some(0x6A),
        "f14" => Some(0x6B),
        "f10" => Some(0x6D),
        "f12" => Some(0x6F),
        "f15" => Some(0x71),
        "home" => Some(0x73),
        "pageup" | "pgup" => Some(0x74),
        "forwarddelete" | "del" => Some(0x75),
        "f4" => Some(0x76),
        "end" => Some(0x77),
        "f2" => Some(0x78),
        "pagedown" | "pgdn" => Some(0x79),
        "f1" => Some(0x7A),
        "left" => Some(0x7B),
        "right" => Some(0x7C),
        "down" => Some(0x7D),
        "up" => Some(0x7E),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::c_void;
    use std::ptr::{self, NonNull};
    use std::sync::atomic::{AtomicU32, Ordering};

    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationOptions, NSFloatingWindowLevel, NSRunningApplication,
        NSView, NSWindowCollectionBehavior, NSWorkspace,
    };

    use super::{parse_macos_hotkey, MacosHotkeyError};

    type OSStatus = i32;
    type EventTargetRef = *mut c_void;
    type EventHandlerCallRef = *mut c_void;
    type EventRef = *mut c_void;
    type EventHandlerRef = *mut c_void;
    type EventHotKeyRef = *mut c_void;
    type EventHandlerProcPtr =
        unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

    const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
    const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
    const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
    const HOT_KEY_SIGNATURE: u32 = u32::from_be_bytes(*b"ULTQ");
    const EVENT_NOT_HANDLED_ERR: OSStatus = -9874;

    static NEXT_HOT_KEY_ID: AtomicU32 = AtomicU32::new(1);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EventHotKeyID {
        signature: u32,
        id: u32,
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn GetApplicationEventTarget() -> EventTargetRef;
        fn InstallEventHandler(
            in_target: EventTargetRef,
            in_handler: EventHandlerProcPtr,
            in_num_types: usize,
            in_list: *const EventTypeSpec,
            in_user_data: *mut c_void,
            out_ref: *mut EventHandlerRef,
        ) -> OSStatus;
        fn RemoveEventHandler(in_handler_ref: EventHandlerRef) -> OSStatus;
        fn RegisterEventHotKey(
            in_hot_key_code: u32,
            in_hot_key_modifiers: u32,
            in_hot_key_id: EventHotKeyID,
            in_target: EventTargetRef,
            in_options: u32,
            out_ref: *mut EventHotKeyRef,
        ) -> OSStatus;
        fn UnregisterEventHotKey(in_hot_key: EventHotKeyRef) -> OSStatus;
        fn GetEventParameter(
            in_event: EventRef,
            in_name: u32,
            in_desired_type: u32,
            out_actual_type: *mut u32,
            in_buffer_size: usize,
            out_actual_size: *mut usize,
            out_data: *mut c_void,
        ) -> OSStatus;
    }

    struct HotkeyCallbackState {
        id: u32,
        callback: Box<dyn Fn() + Send + 'static>,
    }

    unsafe extern "C" fn hotkey_event_handler(
        _call_ref: EventHandlerCallRef,
        event: EventRef,
        user_data: *mut c_void,
    ) -> OSStatus {
        if user_data.is_null() {
            return EVENT_NOT_HANDLED_ERR;
        }
        let mut hot_key_id = EventHotKeyID {
            signature: 0,
            id: 0,
        };
        // SAFETY: `event` is a valid Carbon EventRef delivered by HIToolbox, and `hot_key_id` is a stack-allocated EventHotKeyID matching its size.
        let status = unsafe {
            GetEventParameter(
                event,
                K_EVENT_PARAM_DIRECT_OBJECT,
                TYPE_EVENT_HOT_KEY_ID,
                ptr::null_mut(),
                std::mem::size_of::<EventHotKeyID>(),
                ptr::null_mut(),
                (&mut hot_key_id as *mut EventHotKeyID).cast::<c_void>(),
            )
        };
        if status != 0 {
            return EVENT_NOT_HANDLED_ERR;
        }
        // SAFETY: `user_data` was created with `Box::into_raw` in `MacosGlobalHotkey::register` and stays alive until `RemoveEventHandler` finishes in `Drop`.
        let state = unsafe { &*(user_data as *const HotkeyCallbackState) };
        if hot_key_id.signature != HOT_KEY_SIGNATURE || hot_key_id.id != state.id {
            return EVENT_NOT_HANDLED_ERR;
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (state.callback)()));
        0
    }

    /// Active macOS Carbon global hotkey guard. Dropping it unregisters the
    /// hotkey, removes the Carbon event handler, and frees callback state.
    pub struct MacosGlobalHotkey {
        hot_key_ref: EventHotKeyRef,
        handler_ref: EventHandlerRef,
        state: *mut HotkeyCallbackState,
    }

    impl MacosGlobalHotkey {
        /// Register `binding` globally via Carbon `RegisterEventHotKey` on the
        /// main thread and invoke `on_trigger` whenever it is pressed.
        pub fn register<F>(binding: &str, on_trigger: F) -> Result<Self, MacosHotkeyError>
        where
            F: Fn() + Send + 'static,
        {
            if MainThreadMarker::new().is_none() {
                return Err(MacosHotkeyError::MissingMainThread);
            }
            let spec = parse_macos_hotkey(binding)?;
            let id = NEXT_HOT_KEY_ID.fetch_add(1, Ordering::Relaxed).max(1);
            let state = Box::into_raw(Box::new(HotkeyCallbackState {
                id,
                callback: Box::new(on_trigger),
            }));
            let event_spec = EventTypeSpec {
                event_class: K_EVENT_CLASS_KEYBOARD,
                event_kind: K_EVENT_HOT_KEY_PRESSED,
            };
            let mut handler_ref: EventHandlerRef = ptr::null_mut();
            // SAFETY: Verified on main thread via `MainThreadMarker::new`; `GetApplicationEventTarget` returns the application Carbon target and `state` outlives the handler.
            let install_status = unsafe {
                let target = GetApplicationEventTarget();
                InstallEventHandler(
                    target,
                    hotkey_event_handler,
                    1,
                    &event_spec,
                    state.cast::<c_void>(),
                    &mut handler_ref,
                )
            };
            if install_status != 0 {
                // SAFETY: `InstallEventHandler` failed, so Carbon does not hold `state` and we reclaim it here.
                let _ = unsafe { Box::from_raw(state) };
                return Err(MacosHotkeyError::InstallHandlerFailed {
                    binding: binding.to_owned(),
                    status: install_status,
                });
            }

            let hot_key_id = EventHotKeyID {
                signature: HOT_KEY_SIGNATURE,
                id,
            };
            let mut hot_key_ref: EventHotKeyRef = ptr::null_mut();
            // SAFETY: Verified on main thread; `GetApplicationEventTarget` is valid and `hot_key_ref` receives the new registration handle.
            let register_status = unsafe {
                let target = GetApplicationEventTarget();
                RegisterEventHotKey(
                    spec.key_code,
                    spec.modifiers,
                    hot_key_id,
                    target,
                    0,
                    &mut hot_key_ref,
                )
            };
            if register_status != 0 {
                // SAFETY: `handler_ref` was installed above and removing it guarantees Carbon will no longer dereference `state` before we free it.
                unsafe {
                    RemoveEventHandler(handler_ref);
                    let _ = Box::from_raw(state);
                }
                return Err(MacosHotkeyError::RegisterFailed {
                    binding: binding.to_owned(),
                    status: register_status,
                });
            }

            Ok(Self {
                hot_key_ref,
                handler_ref,
                state,
            })
        }
    }

    impl Drop for MacosGlobalHotkey {
        fn drop(&mut self) {
            // SAFETY: `hot_key_ref` and `handler_ref` were registered by Carbon; unregistering both before `Box::from_raw(self.state)` ensures no callback can run after freeing.
            unsafe {
                if !self.hot_key_ref.is_null() {
                    UnregisterEventHotKey(self.hot_key_ref);
                }
                if !self.handler_ref.is_null() {
                    RemoveEventHandler(self.handler_ref);
                }
                if !self.state.is_null() {
                    let _ = Box::from_raw(self.state);
                }
            }
        }
    }

    /// Configure a winit `NSWindow` for the quick terminal: floating window
    /// level, visible across all Spaces and over full-screen apps, and excluded
    /// from Cmd+` window cycling.
    pub fn configure_quick_window(ns_view: NonNull<c_void>) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        // SAFETY: `ns_view` comes from winit's `AppKitWindowHandle` on the main thread and points to a live `NSView`.
        let view: &NSView = unsafe { ns_view.cast::<NSView>().as_ref() };
        let Some(window) = view.window() else {
            return;
        };
        window.setLevel(NSFloatingWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
    }

    /// Record the currently frontmost external application's PID before Ultty
    /// activates to show the quick terminal. Returns `None` if Ultty itself is
    /// already frontmost.
    pub fn capture_previous_app() -> Option<i32> {
        let _mtm = MainThreadMarker::new()?;
        let workspace = NSWorkspace::sharedWorkspace();
        let frontmost = workspace.frontmostApplication()?;
        let pid = frontmost.processIdentifier();
        let self_pid = NSRunningApplication::currentApplication().processIdentifier();
        (pid > 0 && pid != self_pid).then_some(pid)
    }

    /// Activate Ultty and bring the quick terminal window to the front as key.
    pub fn activate_and_focus_quick_window(ns_view: NonNull<c_void>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        // SAFETY: `ns_view` comes from winit's `AppKitWindowHandle` on the main thread and points to a live `NSView`.
        let view: &NSView = unsafe { ns_view.cast::<NSView>().as_ref() };
        if let Some(window) = view.window() {
            window.setLevel(NSFloatingWindowLevel);
            window.makeKeyAndOrderFront(None);
        }
    }

    /// Order the quick terminal window out and, when no other Ultty window is
    /// visible, return focus to `previous_app_pid`.
    pub fn hide_quick_window_and_restore_focus(
        ns_view: NonNull<c_void>,
        previous_app_pid: Option<i32>,
        has_other_visible_windows: bool,
    ) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        // SAFETY: `ns_view` comes from winit's `AppKitWindowHandle` on the main thread and points to a live `NSView`.
        let view: &NSView = unsafe { ns_view.cast::<NSView>().as_ref() };
        if let Some(window) = view.window() {
            window.orderOut(None);
        }
        if !has_other_visible_windows {
            if let Some(pid) = previous_app_pid {
                if let Some(prev) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                {
                    #[allow(deprecated)]
                    prev.activateWithOptions(
                        NSApplicationActivationOptions::ActivateIgnoringOtherApps,
                    );
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use native::{
    activate_and_focus_quick_window, capture_previous_app, configure_quick_window,
    hide_quick_window_and_restore_focus, MacosGlobalHotkey,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_macos_hotkey_modifiers_and_keys() {
        let spec = parse_macos_hotkey("cmd+grave").unwrap();
        assert_eq!(spec.key_code, 0x32);
        assert_eq!(spec.modifiers, CARBON_CMD_KEY);

        let spec = parse_macos_hotkey("ctrl+shift+alt+space").unwrap();
        assert_eq!(spec.key_code, 0x31);
        assert_eq!(
            spec.modifiers,
            CARBON_CONTROL_KEY | CARBON_SHIFT_KEY | CARBON_OPTION_KEY
        );

        let spec = parse_macos_hotkey("super+f12").unwrap();
        assert_eq!(spec.key_code, 0x6F);
        assert_eq!(spec.modifiers, CARBON_CMD_KEY);

        let spec = parse_macos_hotkey("CMD + Backquote").unwrap();
        assert_eq!(spec.key_code, 0x32);
        assert_eq!(spec.modifiers, CARBON_CMD_KEY);
    }

    #[test]
    fn test_parse_macos_hotkey_all_letters_and_digits() {
        for ch in 'a'..='z' {
            let s = ch.to_string();
            assert!(
                key_name_to_macos_keycode(&s).is_some(),
                "missing keycode for {s}"
            );
        }
        for ch in '0'..='9' {
            let s = ch.to_string();
            assert!(
                key_name_to_macos_keycode(&s).is_some(),
                "missing keycode for {s}"
            );
        }
        for n in 1..=20 {
            let s = format!("f{n}");
            assert!(
                key_name_to_macos_keycode(&s).is_some(),
                "missing keycode for {s}"
            );
        }
    }

    #[test]
    fn test_parse_macos_hotkey_rejects_invalid_and_unsupported() {
        assert!(matches!(
            parse_macos_hotkey("bogus+grave"),
            Err(MacosHotkeyError::InvalidSyntax { .. })
        ));
        assert!(matches!(
            parse_macos_hotkey(""),
            Err(MacosHotkeyError::InvalidSyntax { .. })
        ));
        assert!(matches!(
            parse_macos_hotkey("cmd+unknownkey"),
            Err(MacosHotkeyError::UnsupportedKey { .. })
        ));
    }
}
