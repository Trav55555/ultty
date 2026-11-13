//! GPU viewer of a persistent owner. This process owns replicas and a bounded
//! request queue, never PTYs or application-native connections.

pub mod cli;
mod worker;

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use ultty_protocol::{
    Encode, InputEvent, InputType, Key as NativeKey, Modifiers, RegionId, TerminalMessage,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

use crate::attach::transport::AttachmentClient;
use crate::attach::view::SessionView;
use crate::attach::wire::{Command, LayoutCommand};
use crate::config::Config;
use crate::grid::{
    MouseButton as TerminalMouseButton, MouseModifiers, SelectionMode, SelectionPoint,
};
use crate::keys::{self, KeyAction, KeyInput, KeyModes, Mods};
use crate::renderer::{OverlayState, PaneDraw, PaneInfo, Renderer};
use crate::scene::view::NativeRenderSource;

use worker::Delivery;

/// Viewer options; credentials belong to the transport, never window titles.
pub struct Options {
    /// Named host session.
    pub name: String,
    /// Save the first rendered replacement for a bounded smoke gate.
    pub screenshot: Option<PathBuf>,
    /// Optional bounded window lifetime for verification.
    pub exit_after: Option<Duration>,
}

/// Attach a GPU window to an authenticated client. Closing it only detaches.
pub fn run(options: Options, client: AttachmentClient) -> Result<()> {
    let view = SessionView {
        owner: Some(client.owner().clone()),
        ..SessionView::default()
    };
    let events = EventLoop::<()>::with_user_event().build()?;
    let proxy = events.create_proxy();
    let (sender, receiver) = mpsc::sync_channel(128);
    let delivery = Arc::new(Mutex::new(Delivery::default()));
    let closing = Arc::new(AtomicBool::new(false));
    let worker = worker::spawn(
        client,
        receiver,
        Arc::clone(&delivery),
        Arc::clone(&closing),
        move || proxy.send_event(()).is_ok(),
    )?;
    let mut app = Viewer {
        options,
        sender: Some(sender),
        delivery,
        closing: Arc::clone(&closing),
        window: None,
        renderer: None,
        surface: None,
        view,
        controller: false,
        modifiers: ModifiersState::default(),
        pointer: (0.0, 0.0),
        dragging: None,
        held: None,
        last_error: Cell::new(None),
        clipboard: arboard::Clipboard::new().ok(),
        started: Instant::now(),
        last_draw: Instant::now(),
        failure: None,
    };
    let outcome = events.run_app(&mut app);
    closing.store(true, Ordering::Release);
    app.sender.take();
    // The socket deadline bounds the join. No failed mutation is replayed.
    let _ = worker.join();
    outcome?;
    if let Some(error) = app.failure {
        anyhow::bail!(error);
    }
    Ok(())
}

struct Viewer {
    options: Options,
    sender: Option<mpsc::SyncSender<Command>>,
    delivery: Arc<Mutex<Delivery>>,
    closing: Arc<AtomicBool>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    surface: Option<wgpu::Surface<'static>>,
    view: SessionView,
    controller: bool,
    modifiers: ModifiersState,
    pointer: (f32, f32),
    dragging: Option<u64>,
    held: Option<(u64, MouseButton)>,
    last_error: Cell<Option<Instant>>,
    clipboard: Option<arboard::Clipboard>,
    started: Instant,
    last_draw: Instant,
    failure: Option<String>,
}

impl Viewer {
    fn error(&self, message: &str) {
        self.last_error.set(Some(Instant::now()));
        tracing::warn!(%message, "attachment viewer");
        if let Some(window) = &self.window {
            window.set_title(&format!("{} — {message}", self.options.name));
        }
    }
    fn submit(&self, command: Command) {
        if let Some(sender) = &self.sender {
            if let Err(error) = sender.try_send(command) {
                self.error(&format!("request not accepted: {error}"));
            }
        }
    }
    fn focus(&self, pane: u64) {
        if self.controller {
            self.submit(Command::Layout {
                command: LayoutCommand::Focus { pane },
            });
        }
    }
    fn resize_owner(&self) {
        if self.controller {
            if let Some(renderer) = &self.renderer {
                let (cols, rows) = renderer.terminal_dimensions();
                let (width, height) = renderer.cell_dimensions();
                let cell_width = (width.round() as u16).clamp(1, 512);
                let cell_height = (height.round() as u16).clamp(1, 512);
                self.submit(Command::Resize {
                    cols: cols.min(u16::MAX / cell_width),
                    rows: rows.min(u16::MAX / cell_height),
                    cell_width,
                    cell_height,
                });
            }
        }
    }
    fn create(&mut self, events: &ActiveEventLoop) -> Result<()> {
        let config = Config::load();
        let window = Arc::new(
            events.create_window(
                Window::default_attributes()
                    .with_title(format!("{} — attaching", self.options.name))
                    .with_inner_size(winit::dpi::LogicalSize::new(1000.0, 700.0)),
            )?,
        );
        window.set_ime_allowed(true);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            display: Some(Box::new(events.owned_display_handle())),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let size = window.inner_size();
        let mut font = config.font.clone();
        font.size *= window.scale_factor() as f32;
        let mut renderer = pollster::block_on(Renderer::with_config(
            &surface,
            &adapter,
            size.width,
            size.height,
            &config.colors,
            &font,
            false,
        ))?;
        renderer.set_tab_bar_height(crate::tabs::TERMINAL_TOP_PADDING);
        self.window = Some(window);
        self.surface = Some(surface);
        self.renderer = Some(renderer);
        self.submit(Command::Snapshot {
            after_sequence: None,
        });
        Ok(())
    }
    fn receive(&mut self) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let (snapshot, error, controller) = {
            let mut state = self
                .delivery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (state.snapshot.take(), state.error.take(), state.controller)
        };
        let gained = controller && !self.controller;
        self.controller = controller;
        if let Some(snapshot) = snapshot {
            {
                match self.view.replace(snapshot) {
                    Ok(()) => {
                        if let Some(window) = &self.window {
                            let role = if self.controller {
                                "controller"
                            } else {
                                "observer"
                            };
                            if self
                                .last_error
                                .get()
                                .is_none_or(|time| time.elapsed() > Duration::from_secs(5))
                            {
                                window.set_title(&format!(
                                    "{} — {role} — {}",
                                    self.options.name,
                                    self.view.focused().map_or("", |pane| pane.title.as_str())
                                ));
                            }
                            window.request_redraw();
                        }
                    }
                    Err(error) => {
                        self.closing.store(true, Ordering::Release);
                        self.sender.take();
                        self.controller = false;
                        self.error(&format!("invalid snapshot: {error}; reopen to reconnect"));
                        return;
                    }
                }
            }
        }
        if gained {
            self.resize_owner();
        }
        if let Some(error) = error {
            self.error(&error);
        }
    }
    fn draw(&mut self) -> Result<()> {
        let (Some(renderer), Some(surface), Some(window)) =
            (&mut self.renderer, &self.surface, &self.window)
        else {
            return Ok(());
        };
        let texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                let size = window.inner_size();
                renderer.resize(surface, size.width, size.height);
                return Ok(());
            }
            _ => return Ok(()),
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (cw, ch) = renderer.cell_dimensions();
        let focused = self.view.focused().map(|pane| pane.id);
        let panes: Vec<_> = self
            .view
            .panes
            .iter()
            .filter(|pane| Some(pane.tab) == self.view.active_tab)
            .filter_map(|pane| {
                let [x, y, w, h] = pane.rect?;
                Some(PaneDraw {
                    terminal: &pane.terminal,
                    info: PaneInfo::new(x * cw, y * ch, w * cw, h * ch, Some(pane.id) == focused),
                    sessions: pane
                        .native
                        .iter()
                        .map(|native| native as &dyn NativeRenderSource)
                        .collect(),
                })
            })
            .collect();
        renderer.render_panes(&view, &panes, &OverlayState::default());
        if !panes.is_empty() {
            if let Some(path) = self.options.screenshot.take() {
                renderer.capture_panes_screenshot(&panes, &OverlayState::default(), &path)?;
            }
        }
        window.pre_present_notify();
        renderer.present(texture);
        self.last_draw = Instant::now();
        for pane in &mut self.view.panes {
            for native in &mut pane.native {
                native.mark_clean();
            }
        }
        Ok(())
    }
    fn native(&self, session: u64, generation: u64, event: InputEvent) {
        self.submit(Command::NativeInput {
            session,
            generation,
            payload: TerminalMessage::Input(event).encode_to_bytes().to_vec(),
        });
    }
    fn text(&self, text: String, paste: bool) {
        if !self.controller {
            return;
        }
        if text.len() > 64 * 1024 {
            self.error("text exceeds the 64 KiB input limit; nothing was sent");
            return;
        }
        let Some(pane) = self.view.focused() else {
            return;
        };
        if let Some(native) = pane
            .native
            .iter()
            .rev()
            .find(|native| native.is_full_scene())
        {
            self.native(
                native.session_id(),
                native.generation(),
                InputEvent {
                    region_id: native.focused_region().unwrap_or(RegionId::ROOT),
                    event_type: InputType::TextInput { text },
                    x: 0.0,
                    y: 0.0,
                    modifiers: Modifiers::NONE,
                },
            );
        } else {
            let data = if paste && pane.terminal.modes().bracketed_paste {
                format!("\x1b[200~{text}\x1b[201~").into_bytes()
            } else {
                text.into_bytes()
            };
            self.submit(Command::Input {
                pane: pane.id,
                data,
            });
        }
    }
    fn key(&mut self, event: KeyEvent) {
        if event.state == ElementState::Pressed
            && self.modifiers.control_key()
            && self.modifiers.shift_key()
        {
            if let Key::Character(text) = &event.logical_key {
                match text.to_lowercase().as_str() {
                    "c" => {
                        if let Some(text) = self
                            .view
                            .focused()
                            .and_then(|pane| pane.terminal.selected_text())
                        {
                            if let Some(clipboard) = &mut self.clipboard {
                                if let Err(error) = clipboard.set_text(text) {
                                    self.error(&error.to_string());
                                }
                            }
                        }
                        return;
                    }
                    "v" => {
                        if let Some(text) = self
                            .clipboard
                            .as_mut()
                            .and_then(|clipboard| clipboard.get_text().ok())
                        {
                            self.text(text, true);
                        }
                        return;
                    }
                    "t" => {
                        self.submit(Command::ClaimControl);
                        return;
                    }
                    _ => {}
                }
            }
        }
        if !self.controller {
            return;
        }
        if event.state == ElementState::Pressed && self.modifiers.alt_key() {
            if let Key::Character(text) = &event.logical_key {
                if let Ok(index) = text.parse::<usize>() {
                    if let Some(tab) = index
                        .checked_sub(1)
                        .and_then(|index| self.view.tabs.get(index))
                    {
                        self.focus(tab.focused);
                        return;
                    }
                }
            }
        }
        let Some(pane) = self.view.focused() else {
            return;
        };
        let mods = native_modifiers(self.modifiers);
        if let Some(native) = pane
            .native
            .iter()
            .rev()
            .find(|native| native.is_full_scene())
        {
            let Some(key) = native_key(&event.logical_key) else {
                return;
            };
            let event_type = if event.state == ElementState::Pressed {
                InputType::KeyDown {
                    key,
                    modifiers: mods,
                }
            } else {
                InputType::KeyUp {
                    key,
                    modifiers: mods,
                }
            };
            self.native(
                native.session_id(),
                native.generation(),
                InputEvent {
                    region_id: native.focused_region().unwrap_or(RegionId::ROOT),
                    event_type,
                    x: 0.0,
                    y: 0.0,
                    modifiers: mods,
                },
            );
        } else {
            let Some(key) = keys::key_from_winit(
                &event.logical_key,
                &event.key_without_modifiers(),
                event.location,
            ) else {
                return;
            };
            let modes = pane.terminal.modes();
            let shifted = match &event.logical_key {
                Key::Character(text) if self.modifiers.shift_key() => text.chars().next(),
                _ => None,
            };
            let base = match event.physical_key {
                winit::keyboard::PhysicalKey::Code(code) => keys::base_layout_key(code),
                _ => None,
            };
            let data = keys::encode(
                &KeyInput {
                    key,
                    shifted,
                    base,
                    mods: Mods::new(
                        self.modifiers.shift_key(),
                        self.modifiers.alt_key(),
                        self.modifiers.control_key(),
                        self.modifiers.super_key(),
                    ),
                    action: if event.state == ElementState::Released {
                        KeyAction::Release
                    } else if event.repeat {
                        KeyAction::Repeat
                    } else {
                        KeyAction::Press
                    },
                    text: event.text.as_deref(),
                },
                KeyModes {
                    application_cursor: modes.application_cursor_keys,
                    kitty_flags: modes.kitty_keyboard_flags,
                    modify_other_keys: modes.modify_other_keys,
                    linefeed_newline: modes.linefeed_newline,
                },
            );
            if !data.is_empty() {
                self.submit(Command::Input {
                    pane: pane.id,
                    data,
                });
            }
        }
    }
    fn pointer_pane(&self) -> Option<(u64, f32, f32)> {
        let (cw, ch) = self.renderer.as_ref()?.cell_dimensions();
        let x = self.pointer.0 / cw;
        let y = (self.pointer.1 - crate::tabs::TERMINAL_TOP_PADDING) / ch;
        self.view
            .panes
            .iter()
            .filter(|pane| Some(pane.tab) == self.view.active_tab)
            .find_map(|pane| {
                let [px, py, w, h] = pane.rect?;
                (x >= px && x < px + w && y >= py && y < py + h).then_some((
                    pane.id,
                    x - px,
                    y - py,
                ))
            })
    }
    fn captured_pointer(&self) -> Option<(u64, f32, f32)> {
        let id = self.dragging.or(self.held.map(|(id, _)| id));
        if let Some(id) = id {
            let pane = self
                .view
                .panes
                .iter()
                .find(|pane| pane.id == id && Some(pane.tab) == self.view.active_tab)?;
            let [x, y, _, _] = pane.rect?;
            let (cw, ch) = self.renderer.as_ref()?.cell_dimensions();
            return Some((
                id,
                self.pointer.0 / cw - x,
                (self.pointer.1 - crate::tabs::TERMINAL_TOP_PADDING) / ch - y,
            ));
        }
        self.pointer_pane()
    }
    fn native_pointer(&self, id: u64, x: f32, y: f32, event_type: InputType) -> bool {
        if !self.controller || self.modifiers.shift_key() {
            return false;
        }
        let Some(native) = self
            .view
            .panes
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| {
                pane.native
                    .iter()
                    .rev()
                    .find(|native| native.is_full_scene())
            })
        else {
            return false;
        };
        let (region_id, x, y) = native.hit_test(x, y);
        self.native(
            native.session_id(),
            native.generation(),
            InputEvent {
                region_id,
                event_type,
                x,
                y,
                modifiers: native_modifiers(self.modifiers),
            },
        );
        true
    }
    fn vt_pointer(
        &self,
        id: u64,
        x: f32,
        y: f32,
        button: TerminalMouseButton,
        pressed: bool,
        motion: bool,
    ) -> bool {
        if !self.controller || self.modifiers.shift_key() {
            return false;
        }
        let Some(pane) = self.view.panes.iter().find(|pane| pane.id == id) else {
            return false;
        };
        if !pane.terminal.mouse_tracking_enabled() {
            return false;
        }
        let dimensions = pane.terminal.dimensions();
        if let Some(data) = pane.terminal.encode_mouse_event(
            button,
            (x.max(0.0) as u16).min(dimensions.cols.saturating_sub(1)),
            (y.max(0.0) as u16).min(dimensions.rows.saturating_sub(1)),
            pressed,
            motion,
            MouseModifiers::new(
                false,
                self.modifiers.alt_key(),
                self.modifiers.control_key(),
                self.modifiers.super_key(),
            ),
        ) {
            self.submit(Command::Input { pane: id, data });
        }
        true
    }
    fn mouse(&mut self, state: ElementState, button: MouseButton) {
        let point = self.captured_pointer();
        if state == ElementState::Released {
            self.dragging = None;
            self.held = None;
        }
        let Some((id, x, y)) = point else {
            return;
        };
        if state == ElementState::Pressed {
            self.focus(id);
            self.held = Some((id, button));
        }
        let native_button = match button {
            MouseButton::Left => ultty_protocol::MouseButton::Left,
            MouseButton::Right => ultty_protocol::MouseButton::Right,
            MouseButton::Middle => ultty_protocol::MouseButton::Middle,
            _ => return,
        };
        let event = if state == ElementState::Pressed {
            InputType::MouseDown {
                button: native_button,
            }
        } else {
            InputType::MouseUp {
                button: native_button,
            }
        };
        if self.native_pointer(id, x, y, event)
            || self.vt_pointer(
                id,
                x,
                y,
                terminal_button(Some(button)),
                state == ElementState::Pressed,
                false,
            )
        {
            return;
        }
        let Some(pane) = self.view.panes.iter_mut().find(|pane| pane.id == id) else {
            return;
        };
        if button == MouseButton::Left && state == ElementState::Pressed {
            pane.terminal.start_selection(
                SelectionPoint::new(x as i32, y as i32),
                SelectionMode::Normal,
            );
            self.dragging = Some(id);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<()> for Viewer {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(error) = self.create(events) {
                self.failure = Some(error.to_string());
                events.exit();
            }
        }
    }
    fn user_event(&mut self, _events: &ActiveEventLoop, _event: ()) {
        self.receive();
    }
    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        let now = Instant::now();
        let mut deadline = self.options.exit_after.map(|limit| self.started + limit);
        if deadline.is_some_and(|deadline| now >= deadline) {
            events.exit();
            return;
        }
        if self
            .renderer
            .as_ref()
            .is_some_and(Renderer::has_active_animations)
        {
            let tick = self.last_draw + Duration::from_millis(16);
            let tick = if now >= tick {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                now + Duration::from_millis(16)
            } else {
                tick
            };
            deadline = Some(deadline.map_or(tick, |deadline| deadline.min(tick)));
        }
        events.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
    fn window_event(&mut self, events: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => events.exit(),
            WindowEvent::ModifiersChanged(mods) => self.modifiers = mods.state(),
            WindowEvent::KeyboardInput { event, .. } => self.key(event),
            WindowEvent::Ime(Ime::Commit(text)) => self.text(text, false),
            WindowEvent::Resized(size) => {
                if let (Some(renderer), Some(surface)) = (&mut self.renderer, &self.surface) {
                    renderer.resize(surface, size.width, size.height);
                }
                self.resize_owner();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x as f32, position.y as f32);
                if let Some((id, x, y)) = self.captured_pointer() {
                    if self.dragging == Some(id) {
                        if let Some(pane) = self.view.panes.iter_mut().find(|pane| pane.id == id) {
                            pane.terminal
                                .update_selection(SelectionPoint::new(x as i32, y as i32));
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    } else if !self.native_pointer(id, x, y, InputType::MouseMove) {
                        self.vt_pointer(
                            id,
                            x,
                            y,
                            terminal_button(self.held.map(|(_, button)| button)),
                            true,
                            true,
                        );
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse(state, button),
            WindowEvent::Focused(false) => {
                self.dragging = None;
                self.held = None;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(pos) => {
                        ((pos.x / 20.0) as f32, (pos.y / 20.0) as f32)
                    }
                };
                if let Some((id, x, y)) = self.pointer_pane() {
                    if self.native_pointer(
                        id,
                        x,
                        y,
                        InputType::MouseScroll {
                            delta_x: dx,
                            delta_y: dy,
                        },
                    ) {
                        return;
                    }
                    let button = if dy > 0.0 {
                        TerminalMouseButton::ScrollUp
                    } else {
                        TerminalMouseButton::ScrollDown
                    };
                    let tracking = self
                        .view
                        .panes
                        .iter()
                        .find(|pane| pane.id == id)
                        .is_some_and(|pane| pane.terminal.mouse_tracking_enabled());
                    if self.controller && !self.modifiers.shift_key() && tracking {
                        for _ in 0..(dy.abs().ceil() as usize).min(32) {
                            self.vt_pointer(id, x, y, button, true, false);
                        }
                    } else if let Some(pane) = self.view.panes.iter_mut().find(|pane| pane.id == id)
                    {
                        let rows = (dy.abs().ceil() as usize).min(1000) * 3;
                        if dy > 0.0 {
                            pane.terminal.buffer_mut().scroll_up(rows);
                        } else {
                            pane.terminal.buffer_mut().scroll_down(rows);
                        }
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    self.error(&error.to_string());
                }
            }
            _ => {}
        }
    }
}

fn native_modifiers(state: ModifiersState) -> Modifiers {
    let mut mods = Modifiers::NONE;
    for (on, flag) in [
        (state.shift_key(), Modifiers::SHIFT),
        (state.control_key(), Modifiers::CTRL),
        (state.alt_key(), Modifiers::ALT),
        (state.super_key(), Modifiers::META),
    ] {
        if on {
            mods = mods.union(flag);
        }
    }
    mods
}
fn native_key(key: &Key) -> Option<NativeKey> {
    Some(match key {
        Key::Character(text) => NativeKey::Char(text.chars().next()?),
        Key::Named(key) => match key {
            NamedKey::Escape => NativeKey::Escape,
            NamedKey::Enter => NativeKey::Enter,
            NamedKey::Tab => NativeKey::Tab,
            NamedKey::Backspace => NativeKey::Backspace,
            NamedKey::Delete => NativeKey::Delete,
            NamedKey::Insert => NativeKey::Insert,
            NamedKey::Home => NativeKey::Home,
            NamedKey::End => NativeKey::End,
            NamedKey::PageUp => NativeKey::PageUp,
            NamedKey::PageDown => NativeKey::PageDown,
            NamedKey::ArrowUp => NativeKey::Up,
            NamedKey::ArrowDown => NativeKey::Down,
            NamedKey::ArrowLeft => NativeKey::Left,
            NamedKey::ArrowRight => NativeKey::Right,
            NamedKey::F1 => NativeKey::Function(1),
            NamedKey::F2 => NativeKey::Function(2),
            NamedKey::F3 => NativeKey::Function(3),
            NamedKey::F4 => NativeKey::Function(4),
            NamedKey::F5 => NativeKey::Function(5),
            NamedKey::F6 => NativeKey::Function(6),
            NamedKey::F7 => NativeKey::Function(7),
            NamedKey::F8 => NativeKey::Function(8),
            NamedKey::F9 => NativeKey::Function(9),
            NamedKey::F10 => NativeKey::Function(10),
            NamedKey::F11 => NativeKey::Function(11),
            NamedKey::F12 => NativeKey::Function(12),
            _ => return None,
        },
        _ => return None,
    })
}

fn terminal_button(button: Option<MouseButton>) -> TerminalMouseButton {
    match button {
        Some(MouseButton::Left) => TerminalMouseButton::Left,
        Some(MouseButton::Right) => TerminalMouseButton::Right,
        Some(MouseButton::Middle) => TerminalMouseButton::Middle,
        _ => TerminalMouseButton::Release,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attach::model::{PaneSnapshot, SessionSnapshot, TabSnapshot, SNAPSHOT_VERSION};
    use crate::grid::{snapshot::TerminalSnapshot, Dimensions, Terminal};
    use ultty_protocol::control::OwnerRef;

    fn snapshot(sequence: u64) -> SessionSnapshot {
        SessionSnapshot {
            version: SNAPSHOT_VERSION,
            owner: OwnerRef {
                pid: 1,
                epoch: "a".repeat(64),
            },
            sequence,
            viewport: [10, 2],
            cell_pixels: [8, 16],
            active_tab: Some(1),
            tabs: vec![TabSnapshot {
                id: 1,
                title: "retained".into(),
                focused: 2,
                panes: vec![2],
            }],
            panes: vec![PaneSnapshot {
                id: 2,
                tab: 1,
                pid: 1234,
                revision: 0,
                title: "retained".into(),
                status: None,
                rect: Some([0.0, 0.0, 10.0, 2.0]),
                terminal: TerminalSnapshot::capture(&Terminal::new(Dimensions::new(10, 2)))
                    .unwrap(),
                native: Vec::new(),
            }],
        }
    }

    fn viewer() -> Viewer {
        let (sender, _receiver) = mpsc::sync_channel(128);
        Viewer {
            options: Options {
                name: "test".into(),
                screenshot: None,
                exit_after: None,
            },
            sender: Some(sender),
            closing: Arc::new(AtomicBool::new(false)),
            delivery: Arc::new(Mutex::new(Delivery::default())),
            window: None,
            renderer: None,
            surface: None,
            view: SessionView::default(),
            controller: false,
            modifiers: ModifiersState::default(),
            pointer: (0.0, 0.0),
            dragging: None,
            held: None,
            last_error: Cell::new(None),
            clipboard: None,
            started: Instant::now(),
            last_draw: Instant::now(),
            failure: None,
        }
    }

    #[test]
    fn test_invalid_snapshot_retains_last_view_and_permanently_stops_input() {
        for invalid in 0..3 {
            let mut viewer = viewer();
            viewer.view.replace(snapshot(1)).unwrap();
            let mut next = snapshot(2);
            match invalid {
                0 => next.panes[0].terminal.main.cols = 0,
                1 => next.owner.epoch = "b".repeat(64),
                _ => next.sequence = 1,
            }
            {
                let mut delivery = viewer.delivery.lock().unwrap();
                delivery.snapshot = Some(next);
                delivery.controller = true;
            }
            viewer.receive();
            assert!(viewer.closing.load(Ordering::Acquire));
            assert!(viewer.sender.is_none());
            assert!(!viewer.controller);
            assert!(viewer.last_error.get().is_some());
            assert_eq!(viewer.view.sequence, 1);
            assert_eq!(viewer.view.focused().unwrap().pid, 1234);
            // A later valid replacement cannot silently reconnect or re-enable input.
            viewer.delivery.lock().unwrap().snapshot = Some(snapshot(3));
            viewer.receive();
            assert_eq!(viewer.view.sequence, 1);
            assert!(viewer.sender.is_none());
            assert!(!viewer.controller);
        }
    }
}
