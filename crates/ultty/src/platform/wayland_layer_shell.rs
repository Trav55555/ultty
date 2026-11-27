//! Wayland `zwlr_layer_shell_v1` backend for the quick terminal (ADR 0028 Slice 2).
//!
//! Creates an edge-anchored overlay surface on compositors that advertise
//! `zwlr_layer_shell_v1` (Hyprland, sway, river, niri, labwc, KWin) while
//! returning `Ok(None)` on compositors without it (such as GNOME/Mutter) so
//! callers fall back cleanly to an `xdg_toplevel` window.

use std::ffi::c_void;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use polling::{Event, Events, Poller};
use thiserror::Error;
use wayland_client::protocol::{
    wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use xkbcommon_dl::{
    xkb_context, xkb_context_flags, xkb_keymap, xkb_keymap_compile_flags, xkb_keymap_format,
    xkb_state, xkb_state_component, xkbcommon_option, XkbCommon, XKB_MOD_NAME_ALT,
    XKB_MOD_NAME_CTRL, XKB_MOD_NAME_LOGO, XKB_MOD_NAME_SHIFT,
};

use crate::config::{QuickTerminalConfig, QuickTerminalPosition, QuickTerminalSize};

const WAYLAND_KEY: usize = 1;
const CMD_KEY: usize = 2;
const CMD_STOP: u8 = b'q';
const CMD_FRAME: u8 = b'f';
const DEFAULT_OUTPUT_WIDTH: u32 = 1280;
const DEFAULT_OUTPUT_HEIGHT: u32 = 800;

/// Errors returned while initializing a Wayland `wlr-layer-shell` surface.
#[derive(Debug, Error)]
pub enum LayerShellError {
    /// Failed to bind a required Wayland global or complete the initial roundtrip.
    #[error("Wayland protocol error: {0}")]
    Protocol(String),
    /// Failed to create OS pipe/poller or spawn the event-dispatch thread.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Active keyboard modifier flags reported by `wl_keyboard::modifiers`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerModifiers {
    /// Shift modifier is active.
    pub shift: bool,
    /// Control modifier is active.
    pub ctrl: bool,
    /// Alt / Mod1 modifier is active.
    pub alt: bool,
    /// Super / Mod4 modifier is active.
    pub logo: bool,
}

/// Events delivered from a [`WaylandLayerSurface`] to the main application loop.
#[derive(Debug, Clone)]
pub enum LayerEvent {
    /// Surface received a compositor configure event with updated physical size.
    Configured {
        width: u32,
        height: u32,
        scale_factor: f64,
    },
    /// Output or surface buffer scale changed.
    ScaleChanged(f64),
    /// Compositor signalled readiness for the next frame (`wl_surface::frame`).
    FrameCallback,
    /// Keyboard focus entered (`true`) or left (`false`) the layer surface.
    Focused(bool),
    /// Keyboard modifier state changed.
    ModifiersChanged(LayerModifiers),
    /// Keyboard key press, repeat, or release translated through libxkbcommon.
    Key {
        keysym: u32,
        unmodified_keysym: u32,
        evdev_keycode: u32,
        pressed: bool,
        repeat: bool,
        text: Option<String>,
    },
    /// Pointer moved over the layer surface in physical pixels.
    PointerMoved { x: f64, y: f64 },
    /// Pointer left the layer surface.
    PointerLeft,
    /// Pointer button pressed or released (Linux evdev button code).
    PointerButton { button: u32, pressed: bool },
    /// Pointer scroll delta in logical pixels (positive = scroll up/left).
    PointerScroll { dx: f64, dy: f64 },
    /// Compositor closed the layer surface (`zwlr_layer_surface_v1::closed`).
    Closed,
}

/// Anchoring and logical dimensions computed for a `zwlr_layer_surface_v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerSurfaceSpec {
    /// Edge anchor bitmask passed to `zwlr_layer_surface_v1::set_anchor`.
    pub anchor: zwlr_layer_surface_v1::Anchor,
    /// Requested logical width (`0` lets the compositor span anchored horizontal edges).
    pub width: u32,
    /// Requested logical height (`0` lets the compositor span anchored vertical edges).
    pub height: u32,
    /// Non-zero fallback logical size before the first configure arrives.
    pub initial_logical_size: (u32, u32),
}

/// Compute `zwlr_layer_surface_v1` anchor and logical size from `config` and an
/// output's logical `(width, height)`.
pub fn layer_surface_spec(
    config: &QuickTerminalConfig,
    output_logical: (u32, u32),
) -> LayerSurfaceSpec {
    use zwlr_layer_surface_v1::Anchor;

    let out_w = output_logical.0.max(200);
    let out_h = output_logical.1.max(120);
    let resolve_axis = |axis: u32, min_px: u32| -> u32 {
        match config.size {
            QuickTerminalSize::Percent(pct) => {
                let pct = u64::from(pct.clamp(10, 100));
                ((u64::from(axis) * pct) / 100) as u32
            }
            QuickTerminalSize::Pixels(px) => px.max(100),
        }
        .clamp(min_px.min(axis), axis)
    };

    match config.position {
        QuickTerminalPosition::Top => {
            let h = resolve_axis(out_h, 120);
            LayerSurfaceSpec {
                anchor: Anchor::Top | Anchor::Left | Anchor::Right,
                width: 0,
                height: h,
                initial_logical_size: (out_w, h),
            }
        }
        QuickTerminalPosition::Bottom => {
            let h = resolve_axis(out_h, 120);
            LayerSurfaceSpec {
                anchor: Anchor::Bottom | Anchor::Left | Anchor::Right,
                width: 0,
                height: h,
                initial_logical_size: (out_w, h),
            }
        }
        QuickTerminalPosition::Left => {
            let w = resolve_axis(out_w, 200);
            LayerSurfaceSpec {
                anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
                width: w,
                height: 0,
                initial_logical_size: (w, out_h),
            }
        }
        QuickTerminalPosition::Right => {
            let w = resolve_axis(out_w, 200);
            LayerSurfaceSpec {
                anchor: Anchor::Right | Anchor::Top | Anchor::Bottom,
                width: w,
                height: 0,
                initial_logical_size: (w, out_h),
            }
        }
        QuickTerminalPosition::Center => {
            let w = resolve_axis(out_w, 200);
            let h = resolve_axis(out_h, 120);
            LayerSurfaceSpec {
                anchor: Anchor::empty(),
                width: w,
                height: h,
                initial_logical_size: (w, h),
            }
        }
    }
}

struct SharedMetrics {
    width: AtomicU32,
    height: AtomicU32,
    scale_x1000: AtomicU32,
    focused: AtomicBool,
}

impl SharedMetrics {
    fn new(width: u32, height: u32, scale: f64) -> Self {
        Self {
            width: AtomicU32::new(width.max(1)),
            height: AtomicU32::new(height.max(1)),
            scale_x1000: AtomicU32::new((scale.max(0.5) * 1000.0).round() as u32),
            focused: AtomicBool::new(false),
        }
    }

    fn set_size(&self, width: u32, height: u32) {
        self.width.store(width.max(1), Ordering::Relaxed);
        self.height.store(height.max(1), Ordering::Relaxed);
    }

    fn set_scale(&self, scale: f64) {
        self.scale_x1000
            .store((scale.max(0.5) * 1000.0).round() as u32, Ordering::Relaxed);
    }

    fn size(&self) -> (u32, u32) {
        (
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        )
    }

    fn scale(&self) -> f64 {
        f64::from(self.scale_x1000.load(Ordering::Relaxed)) / 1000.0
    }
}

/// Active `zwlr_layer_shell_v1` surface with its own Wayland connection and
/// event-dispatch thread.
///
/// The raw `wl_display` and `wl_surface` pointers remain valid until this value
/// is dropped; callers attaching a `wgpu::Surface` must keep this value alive
/// inside an `Arc` owned by the surface target.
pub struct WaylandLayerSurface {
    display_ptr: NonNull<c_void>,
    surface_ptr: NonNull<c_void>,
    metrics: Arc<SharedMetrics>,
    cmd_write: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

// SAFETY: Wayland display/surface pointers remain valid while `WaylandLayerSurface` owns the connection and joins its thread on `Drop`.
unsafe impl Send for WaylandLayerSurface {}
// SAFETY: Shared state is synchronized via atomics and the command pipe fd.
unsafe impl Sync for WaylandLayerSurface {}

impl WaylandLayerSurface {
    /// Attempt to create an anchored `zwlr_layer_shell_v1` surface.
    ///
    /// Returns `Ok(None)` when no Wayland display is reachable or when the
    /// compositor does not advertise `zwlr_layer_shell_v1` (such as GNOME).
    pub fn try_create(
        config: &QuickTerminalConfig,
        wake: impl Fn(LayerEvent) + Send + 'static,
    ) -> Result<Option<Self>, LayerShellError> {
        let conn = match Connection::connect_to_env() {
            Ok(conn) => conn,
            Err(err) => {
                tracing::debug!(%err, "Wayland connection unavailable for layer-shell quick terminal");
                return Ok(None);
            }
        };

        let mut event_queue = conn.new_event_queue::<LayerState>();
        let qh = event_queue.handle();

        let (cmd_read, cmd_write) = create_pipe()?;
        let metrics = Arc::new(SharedMetrics::new(
            DEFAULT_OUTPUT_WIDTH,
            DEFAULT_OUTPUT_HEIGHT,
            1.0,
        ));
        let mut state = LayerState::new(Arc::clone(&metrics), Box::new(wake));

        let display = conn.display();
        let _registry = display.get_registry(&qh, ());
        event_queue.roundtrip(&mut state).map_err(|err| {
            LayerShellError::Protocol(format!("registry roundtrip failed: {err}"))
        })?;

        let (Some(compositor), Some(layer_shell)) =
            (state.compositor.clone(), state.layer_shell.clone())
        else {
            tracing::info!(
                "Compositor does not advertise zwlr_layer_shell_v1; falling back to xdg_toplevel quick terminal"
            );
            return Ok(None);
        };

        event_queue
            .roundtrip(&mut state)
            .map_err(|err| LayerShellError::Protocol(format!("output roundtrip failed: {err}")))?;

        let output_logical = state.primary_output_logical_size();
        let initial_scale = state.primary_output_scale();
        let spec = layer_surface_spec(config, output_logical);
        state.logical_size = spec.initial_logical_size;
        state.scale_factor = initial_scale;

        let wl_surface = compositor.create_surface(&qh, ());
        if initial_scale > 1 {
            wl_surface.set_buffer_scale(initial_scale as i32);
        }

        let layer_surface = layer_shell.get_layer_surface(
            &wl_surface,
            state.primary_output.as_ref(),
            zwlr_layer_shell_v1::Layer::Top,
            "ultty-quick".to_owned(),
            &qh,
            (),
        );
        layer_surface.set_anchor(spec.anchor);
        layer_surface.set_size(spec.width, spec.height);
        layer_surface.set_exclusive_zone(0);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
        wl_surface.commit();

        state.wl_surface = Some(wl_surface.clone());
        state.layer_surface = Some(layer_surface);

        event_queue.roundtrip(&mut state).map_err(|err| {
            LayerShellError::Protocol(format!("layer-surface configure roundtrip failed: {err}"))
        })?;

        let (phys_w, phys_h) = state.physical_size();
        metrics.set_size(phys_w, phys_h);
        metrics.set_scale(f64::from(state.scale_factor));
        state.emit_events = true;

        let display_ptr = NonNull::new(conn.backend().display_ptr().cast::<c_void>())
            .ok_or_else(|| LayerShellError::Protocol("null wl_display pointer".to_owned()))?;
        let surface_ptr = NonNull::new(wl_surface.id().as_ptr().cast::<c_void>())
            .ok_or_else(|| LayerShellError::Protocol("null wl_surface pointer".to_owned()))?;

        let thread = thread::Builder::new()
            .name("ultty-layer-shell".to_owned())
            .spawn(move || {
                run_layer_loop(conn, event_queue, qh, state, cmd_read);
            })?;

        tracing::info!(
            width = phys_w,
            height = phys_h,
            scale = initial_scale,
            position = ?config.position,
            "Quick terminal anchored via zwlr_layer_shell_v1"
        );

        Ok(Some(Self {
            display_ptr,
            surface_ptr,
            metrics,
            cmd_write,
            thread: Some(thread),
        }))
    }

    /// Raw `wl_display*` pointer valid for the lifetime of this surface.
    pub fn display_ptr(&self) -> NonNull<c_void> {
        self.display_ptr
    }

    /// Raw `wl_surface*` pointer valid for the lifetime of this surface.
    pub fn surface_ptr(&self) -> NonNull<c_void> {
        self.surface_ptr
    }

    /// Current configured surface size in physical pixels.
    pub fn physical_size(&self) -> (u32, u32) {
        self.metrics.size()
    }

    /// Current surface scale factor.
    pub fn scale_factor(&self) -> f64 {
        self.metrics.scale()
    }

    /// Whether the layer surface currently holds keyboard focus.
    pub fn has_focus(&self) -> bool {
        self.metrics.focused.load(Ordering::Relaxed)
    }

    /// Ask the Wayland dispatch thread to register a `wl_surface::frame` callback.
    pub fn request_frame_callback(&self) {
        let byte = [CMD_FRAME];
        // SAFETY: `cmd_write` is a valid non-blocking pipe write fd owned by `self`.
        let _ = unsafe { libc::write(self.cmd_write.as_raw_fd(), byte.as_ptr().cast(), 1) };
    }
}

impl Drop for WaylandLayerSurface {
    fn drop(&mut self) {
        let byte = [CMD_STOP];
        // SAFETY: `cmd_write` is a valid pipe write fd owned by `self`.
        let _ = unsafe { libc::write(self.cmd_write.as_raw_fd(), byte.as_ptr().cast(), 1) };
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn create_pipe() -> Result<(OwnedFd, OwnedFd), std::io::Error> {
    use std::os::fd::FromRawFd;

    let mut fds = [-1i32; 2];
    // SAFETY: `fds` is a valid 2-element array for `pipe2`.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `pipe2` returned 0, so both descriptors are valid owned fds.
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: `pipe2` returned 0, so both descriptors are valid owned fds.
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    Ok((read_fd, write_fd))
}

fn run_layer_loop(
    conn: Connection,
    mut event_queue: wayland_client::EventQueue<LayerState>,
    qh: QueueHandle<LayerState>,
    mut state: LayerState,
    cmd_read: OwnedFd,
) {
    let poller = match Poller::new() {
        Ok(poller) => poller,
        Err(err) => {
            tracing::warn!(%err, "Wayland layer-shell poller creation failed");
            return;
        }
    };

    let wayland_fd = conn.as_fd().as_raw_fd();
    // SAFETY: `wayland_fd` and `cmd_read` outlive the poller registration in this function.
    if let Err(err) = unsafe {
        poller
            .add(wayland_fd, Event::readable(WAYLAND_KEY))
            .and_then(|()| poller.add(cmd_read.as_raw_fd(), Event::readable(CMD_KEY)))
    } {
        tracing::warn!(%err, "Wayland layer-shell poller registration failed");
        return;
    }

    let mut events = Events::new();
    let mut running = true;
    while running {
        let _ = event_queue.dispatch_pending(&mut state);
        if state.closed {
            break;
        }
        let _ = conn.flush();

        let timeout = state.next_repeat_timeout();
        let read_guard = conn.prepare_read();
        events.clear();
        if let Err(err) = poller.wait(&mut events, timeout) {
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!(%err, "Wayland layer-shell poll error");
            break;
        }

        let mut wayland_ready = false;
        let mut cmd_ready = false;
        for ev in events.iter() {
            if ev.key == WAYLAND_KEY {
                wayland_ready = true;
            } else if ev.key == CMD_KEY {
                cmd_ready = true;
            }
        }

        if let Some(guard) = read_guard {
            if wayland_ready {
                if let Err(err) = guard.read() {
                    tracing::debug!(%err, "Wayland layer-shell connection closed");
                    break;
                }
            }
        }

        if cmd_ready {
            let mut buf = [0u8; 64];
            loop {
                // SAFETY: `cmd_read` is a valid non-blocking pipe read fd.
                let n =
                    unsafe { libc::read(cmd_read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    break;
                }
                for &byte in &buf[..n as usize] {
                    match byte {
                        CMD_STOP => running = false,
                        CMD_FRAME => {
                            if let Some(surface) = state.wl_surface.as_ref() {
                                if !state.frame_pending {
                                    state.frame_pending = true;
                                    let _ = surface.frame(&qh, ());
                                    surface.commit();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        state.fire_key_repeat_if_due();
        let _ = event_queue.dispatch_pending(&mut state);
        if state.closed {
            break;
        }

        // SAFETY: both fds remain valid for the duration of the loop.
        let _ = unsafe {
            poller
                .modify(
                    BorrowedFd::borrow_raw(wayland_fd),
                    Event::readable(WAYLAND_KEY),
                )
                .and_then(|()| {
                    poller.modify(
                        BorrowedFd::borrow_raw(cmd_read.as_raw_fd()),
                        Event::readable(CMD_KEY),
                    )
                })
        };
    }

    if let Some(layer_surface) = state.layer_surface.take() {
        layer_surface.destroy();
    }
    if let Some(wl_surface) = state.wl_surface.take() {
        wl_surface.destroy();
    }
    let _ = conn.flush();
}

struct OutputInfo {
    logical_size: (u32, u32),
    scale: u32,
}

struct RepeatKey {
    keysym: u32,
    unmodified_keysym: u32,
    evdev_keycode: u32,
    text: Option<String>,
    next_fire: Instant,
}

struct XkbState {
    api: &'static XkbCommon,
    context: *mut xkb_context,
    keymap: *mut xkb_keymap,
    state: *mut xkb_state,
}

// SAFETY: `XkbState` exclusively owns its xkb handles and is accessed on only one thread at a time.
unsafe impl Send for XkbState {}

impl XkbState {
    fn from_keymap_fd(fd: OwnedFd, size: u32) -> Option<Self> {
        let api = xkbcommon_option()?;
        let len = usize::try_from(size).ok()?;
        if len == 0 {
            return None;
        }
        // SAFETY: `fd` is a valid keymap fd from `wl_keyboard::keymap` of `len` bytes.
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                fd.as_raw_fd(),
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return None;
        }
        // SAFETY: `mapped` is a valid read-only mapping of `len` bytes.
        let slice = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), len) };
        let keymap_len = slice.iter().position(|&b| b == 0).unwrap_or(len);

        // SAFETY: `xkb_context_new` is a valid C API entry point from `libxkbcommon`.
        let context = unsafe { (api.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS) };
        let keymap = if context.is_null() {
            std::ptr::null_mut()
        } else {
            // SAFETY: `context` is non-null and `mapped` contains `keymap_len` valid bytes.
            unsafe {
                (api.xkb_keymap_new_from_buffer)(
                    context,
                    mapped.cast(),
                    keymap_len,
                    xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                    xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
                )
            }
        };
        // SAFETY: `mapped` was returned by `mmap` with length `len`.
        unsafe {
            libc::munmap(mapped, len);
        }

        if context.is_null() || keymap.is_null() {
            if !context.is_null() {
                // SAFETY: `context` was created by `xkb_context_new`.
                unsafe { (api.xkb_context_unref)(context) };
            }
            return None;
        }
        // SAFETY: `keymap` is a valid non-null `xkb_keymap`.
        let state = unsafe { (api.xkb_state_new)(keymap) };
        if state.is_null() {
            // SAFETY: `keymap` and `context` are valid non-null pointers.
            unsafe {
                (api.xkb_keymap_unref)(keymap);
                (api.xkb_context_unref)(context);
            }
            return None;
        }
        Some(Self {
            api,
            context,
            keymap,
            state,
        })
    }

    fn update_mask(
        &mut self,
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    ) -> LayerModifiers {
        // SAFETY: `self.state` is a valid non-null `xkb_state`.
        unsafe {
            (self.api.xkb_state_update_mask)(self.state, depressed, latched, locked, 0, 0, group);
        }
        let active = |name: &[u8]| -> bool {
            // SAFETY: `self.state` is valid and `name` is a NUL-terminated static C string.
            unsafe {
                (self.api.xkb_state_mod_name_is_active)(
                    self.state,
                    name.as_ptr().cast(),
                    xkb_state_component::XKB_STATE_MODS_EFFECTIVE,
                ) == 1
            }
        };
        LayerModifiers {
            shift: active(XKB_MOD_NAME_SHIFT),
            ctrl: active(XKB_MOD_NAME_CTRL),
            alt: active(XKB_MOD_NAME_ALT),
            logo: active(XKB_MOD_NAME_LOGO),
        }
    }

    fn lookup(&self, evdev_keycode: u32) -> (u32, u32, Option<String>, bool) {
        let xkb_code = evdev_keycode.saturating_add(8);
        // SAFETY: `self.state` is a valid non-null `xkb_state`.
        let keysym = unsafe { (self.api.xkb_state_key_get_one_sym)(self.state, xkb_code) };
        let unmodified_keysym = self.unmodified_keysym(xkb_code).unwrap_or(keysym);

        let mut buf = [0u8; 64];
        // SAFETY: `self.state` is valid and `buf` has space for 64 bytes.
        let len = unsafe {
            (self.api.xkb_state_key_get_utf8)(
                self.state,
                xkb_code,
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        let text = if len > 0 {
            let used = (len as usize).min(buf.len().saturating_sub(1));
            std::str::from_utf8(&buf[..used])
                .ok()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        } else {
            None
        };

        // SAFETY: `self.keymap` is a valid non-null `xkb_keymap`.
        let repeats = unsafe { (self.api.xkb_keymap_key_repeats)(self.keymap, xkb_code) == 1 };
        (keysym, unmodified_keysym, text, repeats)
    }

    fn unmodified_keysym(&self, xkb_code: u32) -> Option<u32> {
        // SAFETY: `self.state` is a valid non-null `xkb_state`.
        let layout = unsafe { (self.api.xkb_state_key_get_layout)(self.state, xkb_code) };
        let mut syms_ptr: *const u32 = std::ptr::null();
        // SAFETY: `self.keymap` is valid and `syms_ptr` is an out-pointer.
        let count = unsafe {
            (self.api.xkb_keymap_key_get_syms_by_level)(
                self.keymap,
                xkb_code,
                layout,
                0,
                &mut syms_ptr,
            )
        };
        if count > 0 && !syms_ptr.is_null() {
            // SAFETY: `xkb_keymap_key_get_syms_by_level` returned `count > 0` valid elements.
            Some(unsafe { *syms_ptr })
        } else {
            None
        }
    }
}

impl Drop for XkbState {
    fn drop(&mut self) {
        // SAFETY: pointers were allocated by `xkb_*_new` and have not been unreffed yet.
        unsafe {
            (self.api.xkb_state_unref)(self.state);
            (self.api.xkb_keymap_unref)(self.keymap);
            (self.api.xkb_context_unref)(self.context);
        }
    }
}

struct LayerState {
    metrics: Arc<SharedMetrics>,
    wake: Box<dyn Fn(LayerEvent) + Send>,
    emit_events: bool,
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    wl_surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    primary_output: Option<wl_output::WlOutput>,
    outputs: Vec<(wl_output::WlOutput, OutputInfo)>,
    logical_size: (u32, u32),
    scale_factor: u32,
    frame_pending: bool,
    closed: bool,
    xkb: Option<XkbState>,
    repeat_rate: i32,
    repeat_delay_ms: i32,
    repeat_key: Option<RepeatKey>,
    pending_scroll_dx: f64,
    pending_scroll_dy: f64,
}

impl LayerState {
    fn new(metrics: Arc<SharedMetrics>, wake: Box<dyn Fn(LayerEvent) + Send>) -> Self {
        Self {
            metrics,
            wake,
            emit_events: false,
            compositor: None,
            layer_shell: None,
            seat: None,
            keyboard: None,
            pointer: None,
            wl_surface: None,
            layer_surface: None,
            primary_output: None,
            outputs: Vec::new(),
            logical_size: (DEFAULT_OUTPUT_WIDTH, DEFAULT_OUTPUT_HEIGHT),
            scale_factor: 1,
            frame_pending: false,
            closed: false,
            xkb: None,
            repeat_rate: 25,
            repeat_delay_ms: 400,
            repeat_key: None,
            pending_scroll_dx: 0.0,
            pending_scroll_dy: 0.0,
        }
    }

    fn emit(&self, event: LayerEvent) {
        if self.emit_events {
            (self.wake)(event);
        }
    }

    fn primary_output_logical_size(&self) -> (u32, u32) {
        self.outputs
            .first()
            .map(|(_, info)| {
                let scale = info.scale.max(1);
                (
                    (info.logical_size.0 / scale).max(200),
                    (info.logical_size.1 / scale).max(120),
                )
            })
            .unwrap_or((DEFAULT_OUTPUT_WIDTH, DEFAULT_OUTPUT_HEIGHT))
    }

    fn primary_output_scale(&self) -> u32 {
        self.outputs
            .first()
            .map(|(_, info)| info.scale.max(1))
            .unwrap_or(1)
    }

    fn physical_size(&self) -> (u32, u32) {
        let scale = self.scale_factor.max(1);
        (
            self.logical_size.0.saturating_mul(scale).max(1),
            self.logical_size.1.saturating_mul(scale).max(1),
        )
    }

    fn next_repeat_timeout(&self) -> Option<Duration> {
        let repeat = self.repeat_key.as_ref()?;
        Some(repeat.next_fire.saturating_duration_since(Instant::now()))
    }

    fn fire_key_repeat_if_due(&mut self) {
        if self.repeat_rate <= 0 {
            self.repeat_key = None;
            return;
        }
        let now = Instant::now();
        let interval = Duration::from_secs_f64(1.0 / f64::from(self.repeat_rate.max(1)));
        let Some(event) = self.repeat_key.as_mut().and_then(|key| {
            if now >= key.next_fire {
                key.next_fire = now + interval;
                Some(LayerEvent::Key {
                    keysym: key.keysym,
                    unmodified_keysym: key.unmodified_keysym,
                    evdev_keycode: key.evdev_keycode,
                    pressed: true,
                    repeat: true,
                    text: key.text.clone(),
                })
            } else {
                None
            }
        }) else {
            return;
        };
        self.emit(event);
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for LayerState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" if state.compositor.is_none() => {
                    let compositor = registry.bind::<wl_compositor::WlCompositor, _, _>(
                        name,
                        version.min(6),
                        qh,
                        (),
                    );
                    state.compositor = Some(compositor);
                }
                "zwlr_layer_shell_v1" if state.layer_shell.is_none() => {
                    let shell = registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                        name,
                        version.min(4),
                        qh,
                        (),
                    );
                    state.layer_shell = Some(shell);
                }
                "wl_seat" if state.seat.is_none() => {
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), qh, ());
                    state.seat = Some(seat);
                }
                "wl_output" => {
                    let output =
                        registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                    if state.primary_output.is_none() {
                        state.primary_output = Some(output.clone());
                    }
                    state.outputs.push((
                        output,
                        OutputInfo {
                            logical_size: (DEFAULT_OUTPUT_WIDTH, DEFAULT_OUTPUT_HEIGHT),
                            scale: 1,
                        },
                    ));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for LayerState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for LayerState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for LayerState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, info)) = state.outputs.iter_mut().find(|(o, _)| o == output) else {
            return;
        };
        match event {
            wl_output::Event::Mode {
                flags: WEnum::Value(mode_flags),
                width,
                height,
                ..
            } if mode_flags.contains(wl_output::Mode::Current) && width > 0 && height > 0 => {
                info.logical_size = (width as u32, height as u32);
            }
            wl_output::Event::Scale { factor } if factor > 0 => {
                info.scale = factor as u32;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for LayerState {
    fn event(
        state: &mut Self,
        surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::PreferredBufferScale { factor } if factor > 0 => {
                let new_scale = factor as u32;
                if new_scale != state.scale_factor {
                    state.scale_factor = new_scale;
                    surface.set_buffer_scale(factor);
                    let (phys_w, phys_h) = state.physical_size();
                    state.metrics.set_size(phys_w, phys_h);
                    state.metrics.set_scale(f64::from(new_scale));
                    state.emit(LayerEvent::ScaleChanged(f64::from(new_scale)));
                    state.emit(LayerEvent::Configured {
                        width: phys_w,
                        height: phys_h,
                        scale_factor: f64::from(new_scale),
                    });
                }
            }
            wl_surface::Event::Enter { output } => {
                if let Some((_, info)) = state.outputs.iter().find(|(o, _)| *o == output) {
                    let new_scale = info.scale.max(1);
                    if new_scale != state.scale_factor {
                        state.scale_factor = new_scale;
                        surface.set_buffer_scale(new_scale as i32);
                        let (phys_w, phys_h) = state.physical_size();
                        state.metrics.set_size(phys_w, phys_h);
                        state.metrics.set_scale(f64::from(new_scale));
                        state.emit(LayerEvent::ScaleChanged(f64::from(new_scale)));
                        state.emit(LayerEvent::Configured {
                            width: phys_w,
                            height: phys_h,
                            scale_factor: f64::from(new_scale),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for LayerState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.frame_pending = false;
            state.emit(LayerEvent::FrameCallback);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for LayerState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                let logical_w = if width > 0 {
                    width
                } else {
                    state.logical_size.0
                };
                let logical_h = if height > 0 {
                    height
                } else {
                    state.logical_size.1
                };
                state.logical_size = (logical_w.max(1), logical_h.max(1));
                let (phys_w, phys_h) = state.physical_size();
                state.metrics.set_size(phys_w, phys_h);
                state.emit(LayerEvent::Configured {
                    width: phys_w,
                    height: phys_h,
                    scale_factor: f64::from(state.scale_factor),
                });
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.closed = true;
                state.repeat_key = None;
                state.emit(LayerEvent::Closed);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for LayerState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for LayerState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                state.xkb = XkbState::from_keymap_fd(fd, size);
            }
            wl_keyboard::Event::Enter { .. } => {
                state.metrics.focused.store(true, Ordering::Relaxed);
                state.emit(LayerEvent::Focused(true));
            }
            wl_keyboard::Event::Leave { .. } => {
                state.metrics.focused.store(false, Ordering::Relaxed);
                state.repeat_key = None;
                state.emit(LayerEvent::Focused(false));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(xkb) = state.xkb.as_mut() {
                    let mods = xkb.update_mask(mods_depressed, mods_latched, mods_locked, group);
                    state.emit(LayerEvent::ModifiersChanged(mods));
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.repeat_rate = rate;
                state.repeat_delay_ms = delay.max(0);
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(key_state),
                ..
            } => {
                let pressed = key_state == wl_keyboard::KeyState::Pressed;
                let Some((keysym, unmodified_keysym, text, repeats)) =
                    state.xkb.as_ref().map(|xkb| xkb.lookup(key))
                else {
                    return;
                };
                if pressed && repeats && state.repeat_rate > 0 {
                    state.repeat_key = Some(RepeatKey {
                        keysym,
                        unmodified_keysym,
                        evdev_keycode: key,
                        text: text.clone(),
                        next_fire: Instant::now()
                            + Duration::from_millis(state.repeat_delay_ms as u64),
                    });
                } else if !pressed
                    && state
                        .repeat_key
                        .as_ref()
                        .is_some_and(|r| r.evdev_keycode == key)
                {
                    state.repeat_key = None;
                }
                state.emit(LayerEvent::Key {
                    keysym,
                    unmodified_keysym,
                    evdev_keycode: key,
                    pressed,
                    repeat: false,
                    text,
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for LayerState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let scale = f64::from(state.scale_factor.max(1));
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.emit(LayerEvent::PointerMoved {
                    x: surface_x * scale,
                    y: surface_y * scale,
                });
            }
            wl_pointer::Event::Leave { .. } => {
                state.emit(LayerEvent::PointerLeft);
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(btn_state),
                ..
            } => {
                state.emit(LayerEvent::PointerButton {
                    button,
                    pressed: btn_state == wl_pointer::ButtonState::Pressed,
                });
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => match axis {
                wl_pointer::Axis::VerticalScroll => state.pending_scroll_dy -= value,
                wl_pointer::Axis::HorizontalScroll => state.pending_scroll_dx -= value,
                _ => {}
            },
            wl_pointer::Event::Frame
                if state.pending_scroll_dx != 0.0 || state.pending_scroll_dy != 0.0 =>
            {
                let dx = state.pending_scroll_dx;
                let dy = state.pending_scroll_dy;
                state.pending_scroll_dx = 0.0;
                state.pending_scroll_dy = 0.0;
                state.emit(LayerEvent::PointerScroll { dx, dy });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zwlr_layer_surface_v1::Anchor;

    #[test]
    fn test_layer_surface_spec_anchors_every_position() {
        let top = layer_surface_spec(
            &QuickTerminalConfig {
                position: QuickTerminalPosition::Top,
                size: QuickTerminalSize::Percent(40),
                ..Default::default()
            },
            (1920, 1080),
        );
        assert_eq!(top.anchor, Anchor::Top | Anchor::Left | Anchor::Right);
        assert_eq!(top.width, 0);
        assert_eq!(top.height, 432);
        assert_eq!(top.initial_logical_size, (1920, 432));

        let bottom = layer_surface_spec(
            &QuickTerminalConfig {
                position: QuickTerminalPosition::Bottom,
                size: QuickTerminalSize::Pixels(300),
                ..Default::default()
            },
            (1920, 1080),
        );
        assert_eq!(bottom.anchor, Anchor::Bottom | Anchor::Left | Anchor::Right);
        assert_eq!(bottom.width, 0);
        assert_eq!(bottom.height, 300);

        let left = layer_surface_spec(
            &QuickTerminalConfig {
                position: QuickTerminalPosition::Left,
                size: QuickTerminalSize::Percent(25),
                ..Default::default()
            },
            (1600, 900),
        );
        assert_eq!(left.anchor, Anchor::Left | Anchor::Top | Anchor::Bottom);
        assert_eq!(left.width, 400);
        assert_eq!(left.height, 0);
        assert_eq!(left.initial_logical_size, (400, 900));

        let right = layer_surface_spec(
            &QuickTerminalConfig {
                position: QuickTerminalPosition::Right,
                size: QuickTerminalSize::Pixels(500),
                ..Default::default()
            },
            (1600, 900),
        );
        assert_eq!(right.anchor, Anchor::Right | Anchor::Top | Anchor::Bottom);
        assert_eq!(right.width, 500);
        assert_eq!(right.height, 0);

        let center = layer_surface_spec(
            &QuickTerminalConfig {
                position: QuickTerminalPosition::Center,
                size: QuickTerminalSize::Percent(50),
                ..Default::default()
            },
            (1200, 800),
        );
        assert_eq!(center.anchor, Anchor::empty());
        assert_eq!(center.width, 600);
        assert_eq!(center.height, 400);
    }

    #[test]
    fn test_try_create_returns_none_without_wayland_socket() {
        let prev = std::env::var_os("WAYLAND_DISPLAY");
        // SAFETY: test runs in isolation and restores `WAYLAND_DISPLAY` immediately.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "ultty-nonexistent-wayland-socket-test");
        }
        let result = WaylandLayerSurface::try_create(&QuickTerminalConfig::default(), |_| {});
        // SAFETY: restoring previous environment variable value.
        unsafe {
            match prev {
                Some(val) => std::env::set_var("WAYLAND_DISPLAY", val),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
        }
        assert!(matches!(result, Ok(None)));
    }
}
