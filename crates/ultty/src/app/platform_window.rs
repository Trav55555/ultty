//! Unified window handle over winit windows and Wayland `wlr-layer-shell` surfaces.

use std::sync::Arc;

use winit::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use winit::error::{ExternalError, NotSupportedError};
use winit::monitor::MonitorHandle;
use winit::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
use winit::window::{
    CursorIcon, Fullscreen, ResizeDirection, UserAttentionType, Window, WindowId, WindowLevel,
};

#[cfg(all(unix, not(target_os = "macos")))]
use ultty::platform::wayland_layer_shell::WaylandLayerSurface;
#[cfg(all(unix, not(target_os = "macos")))]
use ultty::terminal_manager::WindowKey;
#[cfg(all(unix, not(target_os = "macos")))]
use winit::event_loop::EventLoopProxy;

#[cfg(all(unix, not(target_os = "macos")))]
use super::AppEvent;

/// Owned handle to a Wayland `wlr-layer-shell` surface paired with a redraw proxy.
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) struct WaylandLayerHandle {
    key: WindowKey,
    surface: WaylandLayerSurface,
    proxy: EventLoopProxy<AppEvent>,
}

#[cfg(all(unix, not(target_os = "macos")))]
impl WaylandLayerHandle {
    pub(super) fn new(
        key: WindowKey,
        surface: WaylandLayerSurface,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        Self {
            key,
            surface,
            proxy,
        }
    }
}

/// Platform window backing a [`super::WindowState`].
#[derive(Clone)]
pub(super) enum PlatformWindow {
    Winit(Arc<Window>),
    #[cfg(all(unix, not(target_os = "macos")))]
    WaylandLayer(Arc<WaylandLayerHandle>),
}

impl std::fmt::Debug for PlatformWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Winit(window) => f.debug_tuple("Winit").field(&window.id()).finish(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => f
                .debug_tuple("WaylandLayer")
                .field(&layer.key.raw())
                .finish(),
        }
    }
}

impl PlatformWindow {
    pub(super) fn from_winit(window: Arc<Window>) -> Self {
        Self::Winit(window)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) fn from_wayland_layer(
        key: WindowKey,
        surface: WaylandLayerSurface,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        Self::WaylandLayer(Arc::new(WaylandLayerHandle::new(key, surface, proxy)))
    }

    pub(super) fn as_winit(&self) -> Option<&Arc<Window>> {
        match self {
            Self::Winit(window) => Some(window),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => None,
        }
    }

    pub(super) fn winit_id(&self) -> Option<WindowId> {
        self.as_winit().map(|window| window.id())
    }

    pub(super) fn is_wayland_layer(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            matches!(self, Self::WaylandLayer(_))
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            false
        }
    }

    pub(super) fn inner_size(&self) -> PhysicalSize<u32> {
        match self {
            Self::Winit(window) => window.inner_size(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => {
                let (w, h) = layer.surface.physical_size();
                PhysicalSize::new(w, h)
            }
        }
    }

    pub(super) fn scale_factor(&self) -> f64 {
        match self {
            Self::Winit(window) => window.scale_factor(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => layer.surface.scale_factor(),
        }
    }

    pub(super) fn has_focus(&self) -> bool {
        match self {
            Self::Winit(window) => window.has_focus(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => layer.surface.has_focus(),
        }
    }

    pub(super) fn is_maximized(&self) -> bool {
        match self {
            Self::Winit(window) => window.is_maximized(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => false,
        }
    }

    pub(super) fn request_redraw(&self) {
        match self {
            Self::Winit(window) => window.request_redraw(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => {
                let _ = layer.proxy.send_event(AppEvent::LayerRedraw(layer.key));
            }
        }
    }

    pub(super) fn pre_present_notify(&self) {
        match self {
            Self::Winit(window) => window.pre_present_notify(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => layer.surface.request_frame_callback(),
        }
    }

    pub(super) fn set_visible(&self, visible: bool) {
        if let Self::Winit(window) = self {
            window.set_visible(visible);
        }
    }

    pub(super) fn focus_window(&self) {
        if let Self::Winit(window) = self {
            window.focus_window();
        }
    }

    pub(super) fn set_title(&self, title: &str) {
        if let Self::Winit(window) = self {
            window.set_title(title);
        }
    }

    pub(super) fn set_cursor(&self, cursor: CursorIcon) {
        if let Self::Winit(window) = self {
            window.set_cursor(cursor);
        }
    }

    pub(super) fn set_ime_cursor_area<P: Into<Position>, S: Into<Size>>(&self, pos: P, size: S) {
        if let Self::Winit(window) = self {
            window.set_ime_cursor_area(pos, size);
        }
    }

    pub(super) fn request_user_attention(&self, request_type: Option<UserAttentionType>) {
        if let Self::Winit(window) = self {
            window.request_user_attention(request_type);
        }
    }

    pub(super) fn request_inner_size<S: Into<Size>>(&self, size: S) -> Option<PhysicalSize<u32>> {
        match self {
            Self::Winit(window) => window.request_inner_size(size),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => {
                let (w, h) = layer.surface.physical_size();
                Some(PhysicalSize::new(w, h))
            }
        }
    }

    pub(super) fn set_outer_position<P: Into<Position>>(&self, position: P) {
        if let Self::Winit(window) = self {
            window.set_outer_position(position);
        }
    }

    pub(super) fn set_window_level(&self, level: WindowLevel) {
        if let Self::Winit(window) = self {
            window.set_window_level(level);
        }
    }

    pub(super) fn set_min_inner_size<S: Into<Size>>(&self, min_size: Option<S>) {
        if let Self::Winit(window) = self {
            window.set_min_inner_size(min_size);
        }
    }

    pub(super) fn fullscreen(&self) -> Option<Fullscreen> {
        match self {
            Self::Winit(window) => window.fullscreen(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => None,
        }
    }

    pub(super) fn set_fullscreen(&self, fullscreen: Option<Fullscreen>) {
        if let Self::Winit(window) = self {
            window.set_fullscreen(fullscreen);
        }
    }

    pub(super) fn set_maximized(&self, maximized: bool) {
        if let Self::Winit(window) = self {
            window.set_maximized(maximized);
        }
    }

    pub(super) fn set_minimized(&self, minimized: bool) {
        if let Self::Winit(window) = self {
            window.set_minimized(minimized);
        }
    }

    pub(super) fn drag_window(&self) -> Result<(), ExternalError> {
        match self {
            Self::Winit(window) => window.drag_window(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => Ok(()),
        }
    }

    pub(super) fn drag_resize_window(
        &self,
        direction: ResizeDirection,
    ) -> Result<(), ExternalError> {
        match self {
            Self::Winit(window) => window.drag_resize_window(direction),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => Ok(()),
        }
    }

    pub(super) fn current_monitor(&self) -> Option<MonitorHandle> {
        match self {
            Self::Winit(window) => window.current_monitor(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => None,
        }
    }

    pub(super) fn outer_position(&self) -> Result<PhysicalPosition<i32>, NotSupportedError> {
        match self {
            Self::Winit(window) => window.outer_position(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(_) => Ok(PhysicalPosition::new(0, 0)),
        }
    }
}

impl HasDisplayHandle for PlatformWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        match self {
            Self::Winit(window) => window.display_handle(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => {
                use winit::raw_window_handle::WaylandDisplayHandle;
                let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                    layer.surface.display_ptr(),
                ));
                // SAFETY: `layer.surface` keeps the `wl_display` alive for `'self`.
                Ok(unsafe { DisplayHandle::borrow_raw(raw) })
            }
        }
    }
}

impl HasWindowHandle for PlatformWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        match self {
            Self::Winit(window) => window.window_handle(),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandLayer(layer) => {
                use winit::raw_window_handle::WaylandWindowHandle;
                let raw =
                    RawWindowHandle::Wayland(WaylandWindowHandle::new(layer.surface.surface_ptr()));
                // SAFETY: `layer.surface` keeps the `wl_surface` alive for `'self`.
                Ok(unsafe { WindowHandle::borrow_raw(raw) })
            }
        }
    }
}

/// Translate an XKB keysym to a winit [`winit::keyboard::Key`] and [`winit::keyboard::KeyLocation`].
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn keysym_to_winit_key(
    raw: u32,
) -> (winit::keyboard::Key, winit::keyboard::KeyLocation) {
    use winit::keyboard::{Key, KeyLocation, NamedKey, NativeKey};
    use xkeysym::{key, Keysym};

    match raw {
        key::Return => (Key::Named(NamedKey::Enter), KeyLocation::Standard),
        key::KP_Enter => (Key::Named(NamedKey::Enter), KeyLocation::Numpad),
        key::Escape => (Key::Named(NamedKey::Escape), KeyLocation::Standard),
        key::BackSpace => (Key::Named(NamedKey::Backspace), KeyLocation::Standard),
        key::Tab | key::ISO_Left_Tab => (Key::Named(NamedKey::Tab), KeyLocation::Standard),
        key::KP_Tab => (Key::Named(NamedKey::Tab), KeyLocation::Numpad),
        key::Delete => (Key::Named(NamedKey::Delete), KeyLocation::Standard),
        key::KP_Delete => (Key::Named(NamedKey::Delete), KeyLocation::Numpad),
        key::Insert => (Key::Named(NamedKey::Insert), KeyLocation::Standard),
        key::KP_Insert => (Key::Named(NamedKey::Insert), KeyLocation::Numpad),
        key::Home => (Key::Named(NamedKey::Home), KeyLocation::Standard),
        key::KP_Home => (Key::Named(NamedKey::Home), KeyLocation::Numpad),
        key::End => (Key::Named(NamedKey::End), KeyLocation::Standard),
        key::KP_End => (Key::Named(NamedKey::End), KeyLocation::Numpad),
        key::Page_Up => (Key::Named(NamedKey::PageUp), KeyLocation::Standard),
        key::KP_Page_Up => (Key::Named(NamedKey::PageUp), KeyLocation::Numpad),
        key::Page_Down => (Key::Named(NamedKey::PageDown), KeyLocation::Standard),
        key::KP_Page_Down => (Key::Named(NamedKey::PageDown), KeyLocation::Numpad),
        key::Up => (Key::Named(NamedKey::ArrowUp), KeyLocation::Standard),
        key::KP_Up => (Key::Named(NamedKey::ArrowUp), KeyLocation::Numpad),
        key::Down => (Key::Named(NamedKey::ArrowDown), KeyLocation::Standard),
        key::KP_Down => (Key::Named(NamedKey::ArrowDown), KeyLocation::Numpad),
        key::Left => (Key::Named(NamedKey::ArrowLeft), KeyLocation::Standard),
        key::KP_Left => (Key::Named(NamedKey::ArrowLeft), KeyLocation::Numpad),
        key::Right => (Key::Named(NamedKey::ArrowRight), KeyLocation::Standard),
        key::KP_Right => (Key::Named(NamedKey::ArrowRight), KeyLocation::Numpad),
        key::space => (Key::Named(NamedKey::Space), KeyLocation::Standard),
        key::KP_Space => (Key::Named(NamedKey::Space), KeyLocation::Numpad),
        key::F1 => (Key::Named(NamedKey::F1), KeyLocation::Standard),
        key::F2 => (Key::Named(NamedKey::F2), KeyLocation::Standard),
        key::F3 => (Key::Named(NamedKey::F3), KeyLocation::Standard),
        key::F4 => (Key::Named(NamedKey::F4), KeyLocation::Standard),
        key::F5 => (Key::Named(NamedKey::F5), KeyLocation::Standard),
        key::F6 => (Key::Named(NamedKey::F6), KeyLocation::Standard),
        key::F7 => (Key::Named(NamedKey::F7), KeyLocation::Standard),
        key::F8 => (Key::Named(NamedKey::F8), KeyLocation::Standard),
        key::F9 => (Key::Named(NamedKey::F9), KeyLocation::Standard),
        key::F10 => (Key::Named(NamedKey::F10), KeyLocation::Standard),
        key::F11 => (Key::Named(NamedKey::F11), KeyLocation::Standard),
        key::F12 => (Key::Named(NamedKey::F12), KeyLocation::Standard),
        key::Shift_L => (Key::Named(NamedKey::Shift), KeyLocation::Left),
        key::Shift_R => (Key::Named(NamedKey::Shift), KeyLocation::Right),
        key::Control_L => (Key::Named(NamedKey::Control), KeyLocation::Left),
        key::Control_R => (Key::Named(NamedKey::Control), KeyLocation::Right),
        key::Alt_L => (Key::Named(NamedKey::Alt), KeyLocation::Left),
        key::Alt_R => (Key::Named(NamedKey::Alt), KeyLocation::Right),
        key::Super_L => (Key::Named(NamedKey::Super), KeyLocation::Left),
        key::Super_R => (Key::Named(NamedKey::Super), KeyLocation::Right),
        key::Caps_Lock => (Key::Named(NamedKey::CapsLock), KeyLocation::Standard),
        key::Num_Lock => (Key::Named(NamedKey::NumLock), KeyLocation::Standard),
        key::Scroll_Lock => (Key::Named(NamedKey::ScrollLock), KeyLocation::Standard),
        other => {
            if let Some(ch) = Keysym::new(other).key_char() {
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                (Key::Character(s.into()), KeyLocation::Standard)
            } else {
                (
                    Key::Unidentified(NativeKey::Xkb(other)),
                    KeyLocation::Standard,
                )
            }
        }
    }
}

/// Map a Linux evdev scancode to a winit [`winit::keyboard::PhysicalKey`].
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn evdev_to_physical_key(scancode: u32) -> winit::keyboard::PhysicalKey {
    use winit::keyboard::{KeyCode, NativeKeyCode, PhysicalKey};

    let code = match scancode {
        1 => KeyCode::Escape,
        2 => KeyCode::Digit1,
        3 => KeyCode::Digit2,
        4 => KeyCode::Digit3,
        5 => KeyCode::Digit4,
        6 => KeyCode::Digit5,
        7 => KeyCode::Digit6,
        8 => KeyCode::Digit7,
        9 => KeyCode::Digit8,
        10 => KeyCode::Digit9,
        11 => KeyCode::Digit0,
        12 => KeyCode::Minus,
        13 => KeyCode::Equal,
        14 => KeyCode::Backspace,
        15 => KeyCode::Tab,
        16 => KeyCode::KeyQ,
        17 => KeyCode::KeyW,
        18 => KeyCode::KeyE,
        19 => KeyCode::KeyR,
        20 => KeyCode::KeyT,
        21 => KeyCode::KeyY,
        22 => KeyCode::KeyU,
        23 => KeyCode::KeyI,
        24 => KeyCode::KeyO,
        25 => KeyCode::KeyP,
        26 => KeyCode::BracketLeft,
        27 => KeyCode::BracketRight,
        28 => KeyCode::Enter,
        29 => KeyCode::ControlLeft,
        30 => KeyCode::KeyA,
        31 => KeyCode::KeyS,
        32 => KeyCode::KeyD,
        33 => KeyCode::KeyF,
        34 => KeyCode::KeyG,
        35 => KeyCode::KeyH,
        36 => KeyCode::KeyJ,
        37 => KeyCode::KeyK,
        38 => KeyCode::KeyL,
        39 => KeyCode::Semicolon,
        40 => KeyCode::Quote,
        41 => KeyCode::Backquote,
        42 => KeyCode::ShiftLeft,
        43 => KeyCode::Backslash,
        44 => KeyCode::KeyZ,
        45 => KeyCode::KeyX,
        46 => KeyCode::KeyC,
        47 => KeyCode::KeyV,
        48 => KeyCode::KeyB,
        49 => KeyCode::KeyN,
        50 => KeyCode::KeyM,
        51 => KeyCode::Comma,
        52 => KeyCode::Period,
        53 => KeyCode::Slash,
        54 => KeyCode::ShiftRight,
        56 => KeyCode::AltLeft,
        57 => KeyCode::Space,
        58 => KeyCode::CapsLock,
        59 => KeyCode::F1,
        60 => KeyCode::F2,
        61 => KeyCode::F3,
        62 => KeyCode::F4,
        63 => KeyCode::F5,
        64 => KeyCode::F6,
        65 => KeyCode::F7,
        66 => KeyCode::F8,
        67 => KeyCode::F9,
        68 => KeyCode::F10,
        87 => KeyCode::F11,
        88 => KeyCode::F12,
        96 => KeyCode::NumpadEnter,
        97 => KeyCode::ControlRight,
        100 => KeyCode::AltRight,
        102 => KeyCode::Home,
        103 => KeyCode::ArrowUp,
        104 => KeyCode::PageUp,
        105 => KeyCode::ArrowLeft,
        106 => KeyCode::ArrowRight,
        107 => KeyCode::End,
        108 => KeyCode::ArrowDown,
        109 => KeyCode::PageDown,
        110 => KeyCode::Insert,
        111 => KeyCode::Delete,
        125 => KeyCode::SuperLeft,
        126 => KeyCode::SuperRight,
        _ => return PhysicalKey::Unidentified(NativeKeyCode::Xkb(scancode + 8)),
    };
    PhysicalKey::Code(code)
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::{evdev_to_physical_key, keysym_to_winit_key};
    use winit::keyboard::{Key, KeyCode, KeyLocation, NamedKey, PhysicalKey};
    use xkeysym::key;

    #[test]
    fn test_keysym_and_evdev_translation() {
        assert_eq!(
            keysym_to_winit_key(key::Return),
            (Key::Named(NamedKey::Enter), KeyLocation::Standard)
        );
        assert_eq!(
            keysym_to_winit_key(key::KP_Enter),
            (Key::Named(NamedKey::Enter), KeyLocation::Numpad)
        );
        assert_eq!(
            keysym_to_winit_key(key::Escape),
            (Key::Named(NamedKey::Escape), KeyLocation::Standard)
        );
        assert_eq!(
            keysym_to_winit_key(key::Shift_L),
            (Key::Named(NamedKey::Shift), KeyLocation::Left)
        );
        assert_eq!(
            keysym_to_winit_key(key::a),
            (Key::Character("a".into()), KeyLocation::Standard)
        );
        assert_eq!(
            keysym_to_winit_key(key::grave),
            (Key::Character("`".into()), KeyLocation::Standard)
        );
        assert_eq!(evdev_to_physical_key(30), PhysicalKey::Code(KeyCode::KeyA));
        assert_eq!(
            evdev_to_physical_key(41),
            PhysicalKey::Code(KeyCode::Backquote)
        );
        assert_eq!(evdev_to_physical_key(28), PhysicalKey::Code(KeyCode::Enter));
    }
}
