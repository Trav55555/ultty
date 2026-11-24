//! Terminal application state and main loop.

mod accessibility;
mod actions;
mod blink;
mod command_palette;
mod control;
mod control_events;
mod control_resize;
mod control_wait;
mod cpu_frames;
mod gpu;
mod input;
mod native_panes;
mod native_selection;
mod pacing;
mod render;
mod update;
mod wakeup;
use anyhow::Result;
use arboard::Clipboard;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ultty::config::{Config, ShellIntegration, TerminalConfig};
use ultty::config_watcher::ConfigWatcher;
use ultty::control::ControlServer;
use ultty::protocol::ProtocolServer;
use ultty::tabs::TabBarState;
use ultty::terminal_manager::{TabId, TerminalId, TerminalManager, WindowKey};
use ultty::url::UrlDetector;
use ultty::vim::{CopyMode, Selection as VimSelection};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

/// The window's title until the focused pane sets one.
const WINDOW_TITLE: &str = "Ultty";

/// Longest a copy-mode search holds the terminal lock per frame.
const SEARCH_SLICE: Duration = Duration::from_millis(2);

/// A copy-mode scan and its completed matches belong to one pane lifetime.
fn reconcile_copy_mode_pane(
    owner: &mut Option<TerminalId>,
    job: &mut Option<ultty::vim::search::SearchJob>,
    mode: &mut CopyMode,
    focused: Option<TerminalId>,
) -> bool {
    if owner.is_none() || *owner == focused {
        return false;
    }
    *owner = None;
    *job = None;
    mode.set_search_matches(Vec::new());
    mode.set_search_status(ultty::vim::SearchStatus::Idle);
    mode.exit();
    true
}

/// Default window dimensions.
const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;

/// An explicit startup choice, overriding automatic session restoration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupCommand {
    /// Run a program with unchanged argument boundaries.
    Exec { program: String, args: Vec<String> },
    /// Launch the named configured profile.
    Profile(String),
    /// Restore a named layout into fresh shells in a new window.
    Workspace(String),
}

/// Time window for detecting double/triple clicks.
pub(super) const CLICK_THRESHOLD: Duration = Duration::from_millis(500);

// Client-side resize zones, in logical pixels (scaled by the window's
// scale factor). Corners are the most-wanted and most-missed target, so they
// are generous: the edge band counts as a corner for CORNER_ARM along each
// edge, and a CORNER_SQUARE at the very corner reaches further in.

/// Width of the resize band just inside each window edge.
const WINDOW_RESIZE_EDGE_BAND: f32 = 8.0;

/// Maximum width of the resize band inside the left and right edges where
/// they overlap the terminal cell grid, in logical pixels. Kept narrow (and
/// further capped to a quarter of `cell_width`) so clicks in column 0 start
/// text selection instead of resizing the window.
const WINDOW_RESIZE_GRID_SIDE_BAND: f32 = 2.0;

/// Resize band outside the window edge, for compositors that report
/// pointer positions there.
const WINDOW_RESIZE_OUTER_BAND: f32 = 8.0;

/// How far along each edge from a corner the edge band resizes diagonally.
const WINDOW_RESIZE_CORNER_ARM: f32 = 32.0;

/// Side of the square at each corner that resizes diagonally.
const WINDOW_RESIZE_CORNER_SQUARE: f32 = 14.0;

/// Rounded corner radius for floating client-side decorated windows.
const WINDOW_CORNER_RADIUS: f32 = 10.0;

/// Tolerance for recognizing compositor-tiled half/quarter window sizes.
const TILED_WINDOW_SIZE_TOLERANCE_PX: u32 = 48;

/// Minimum spacing between rendered frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How soon after the user's input PTY output still counts as its echo and
/// is drawn without waiting out [`FRAME_INTERVAL`]; see `should_render`.
const INPUT_ECHO_WINDOW: Duration = Duration::from_millis(50);

/// Maximum time to suppress drawing during synchronized update mode.
const SYNC_UPDATE_TIMEOUT: Duration = Duration::from_millis(200);

/// Maintenance tick while native sessions (keepalives, animations) need
/// checking. Nothing else polls: without one, an idle window sleeps until
/// input, output, a socket or a timer it set (such as a `wait-for`
/// deadline) wakes it.
const MAINTENANCE_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Faster PTY polling while a frame is dirty but not ready to render.
const PTY_CATCHUP_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Duration to keep tab widths stable after a tab close.
const TAB_CLOSE_WIDTH_FREEZE_DURATION: Duration = Duration::from_secs(3);

/// Most frequent best-effort tab title refresh (foreground program and
/// directory) while panes produce output.
const TAB_TITLE_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

/// User events sent to the winit application loop.
#[derive(Debug)]
enum AppEvent {
    /// At least one runtime fd is readable and should be drained on the UI thread.
    RuntimeReadable,
    /// A request from assistive technology, for [`ultty::a11y::Adapter`].
    Accessibility(accesskit_winit::Event),
    /// SIGTERM, SIGHUP or SIGINT: exit as if the window were closed.
    ShutdownSignal,
    /// The GPU setup finished, or failed; see `gpu`.
    Renderer(gpu::Handoff),
}

impl From<accesskit_winit::Event> for AppEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

#[derive(Debug, Clone, Copy)]
struct TabWidthFreeze {
    width: f32,
    until: Instant,
}

/// Process-wide application state shared across every open window (ADR 0027).
struct ProcessState {
    proxy: EventLoopProxy<AppEvent>,
    shared_gpu: Option<gpu::SharedGpu>,
    runtime: ultty::runtime::SessionRuntime,
    session_file: Option<std::path::PathBuf>,
    config: Config,
    config_watcher: Option<ConfigWatcher>,
    clipboard: Option<Clipboard>,
    shortcuts_list: Vec<(String, String)>,
    control_server: Option<ControlServer>,
    pending_control_waits: Vec<control_wait::PendingControlWait>,
    pending_override_pushes: Vec<control::PendingOverridePush>,
    pending_semantic_actions: Vec<control::PendingSemanticAction>,
    pending_resizes: Vec<(WindowKey, control_resize::PendingResize)>,
    next_override_request: u32,
    published_panes: Vec<ultty::terminal_manager::TerminalId>,
    socket_wakeup: wakeup::SocketWakeup,
    auto_dump_frames: u32,
    pending_new_windows: usize,
    pending_reload_config: bool,
    pending_cycle_theme: bool,
}

/// Per-window surface, renderer, accessibility bridge, and UI state (ADR 0027).
struct WindowState {
    key: WindowKey,
    accessibility: Option<ultty::a11y::Adapter>,
    renderer: gpu::RendererState,
    cpu_frames: Option<cpu_frames::CpuFrames>,
    window: Arc<Window>,
    output_shown: bool,
    tab_bar: TabBarState,
    tab_width_freeze: Option<TabWidthFreeze>,
    last_render: Instant,
    input_sent_at: Option<Instant>,
    output_since_input: bool,
    last_tab_title_refresh: Instant,
    tab_title_activity: Option<Instant>,
    needs_redraw: bool,
    frame_pacing: pacing::FramePacing,
    pending_close: bool,
    modifiers: ModifiersState,
    mouse_pos: PhysicalPosition<f64>,
    mouse_pressed: bool,
    native_selection: native_selection::NativeSelectionState,
    reported_button: Option<ultty::grid::MouseButton>,
    last_click_time: Instant,
    click_count: u8,
    last_click_pos: (u16, u16),
    last_title_bar_toggle_click_time: Instant,
    copy_mode: CopyMode,
    search_job: Option<ultty::vim::search::SearchJob>,
    copy_mode_pane: Option<TerminalId>,
    url_detector: UrlDetector,
    bell_active_until: Option<Instant>,
    divider_dragging: Option<(ultty::pane_layout::Divider, f32)>,
    shortcuts_visible: bool,
    command_palette: ultty::command_palette::CommandPalette,
    protocol_status_visible: bool,
    cursor_blink: blink::CursorBlink,
    frames_drawn: u32,
    native_mouse_capture: native_panes::MouseCapture,
    native_search_matches: Vec<ultty::scene::SearchMatch>,
    native_search_active: bool,
    native_search_query: String,
    native_search_index: usize,
    url_row_buf: Vec<String>,
    url_hyperlink_grid: Vec<Vec<u32>>,
    url_hyperlink_map: std::collections::HashMap<u32, String>,
    scroll_pixel_accum: f64,
    scroll_line_accum: f64,
    scale_factor: f32,
    min_window_size: Option<PhysicalSize<u32>>,
    launched_at: Instant,
    ime_preedit: String,
    ime_cursor_area: Option<[i32; 4]>,
}

/// Disjoint mutable view pairing process-wide state with one open window.
struct WindowCtx<'a> {
    app: &'a mut ProcessState,
    win: &'a mut WindowState,
}

/// Terminal application owning one shared runtime and any number of windows.
pub struct App {
    shared: ProcessState,
    windows: BTreeMap<WindowKey, WindowState>,
    window_ids: HashMap<WindowId, WindowKey>,
    focused_window: Option<WindowKey>,
}

impl WindowState {
    fn create(
        key: WindowKey,
        event_loop: &ActiveEventLoop,
        event_proxy: EventLoopProxy<AppEvent>,
        config: &Config,
        shared_gpu: Option<&gpu::SharedGpu>,
    ) -> Result<Self> {
        let create_start = Instant::now();
        let attributes = Window::default_attributes()
            .with_title(WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
            .with_min_inner_size(LogicalSize::new(200.0, 100.0))
            .with_transparent(true)
            .with_decorations(false)
            .with_visible(!config.behavior.accessibility);
        #[cfg(all(unix, not(target_os = "macos")))]
        let attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(
            attributes, "ultty", "ultty",
        );
        let window = Arc::new(event_loop.create_window(attributes)?);
        tracing::debug!(
            target: "ultty::startup",
            window = key.raw(),
            "window created"
        );
        let size = window.inner_size();
        let scale_factor = window.scale_factor() as f32;

        let paced_by_frame_callbacks = {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            window
                .window_handle()
                .is_ok_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)))
        };

        let mut scaled_font = config.font.clone();
        scaled_font.size *= scale_factor;
        let fonts = ultty::renderer::Fonts::load(&scaled_font);
        tracing::debug!(
            target: "ultty::startup",
            window = key.raw(),
            "cell metrics ready"
        );

        let mut tab_bar = TabBarState::new();
        tab_bar.scale = scale_factor;
        tab_bar.title_letters = 0;

        let mut cpu_frames = None;
        let mut renderer = if let Some(shared) = shared_gpu {
            match gpu::setup_with_shared(
                shared,
                Arc::clone(&window),
                size,
                &config.colors,
                fonts,
                paced_by_frame_callbacks,
            ) {
                gpu::SharedGpuSetup::Ready(mut gpu) => {
                    gpu.renderer.set_opacity(config.behavior.opacity);
                    gpu.renderer.set_tab_bar_height(tab_bar.content_top());
                    tracing::debug!(
                        target: "ultty::startup",
                        window = key.raw(),
                        elapsed_ms = create_start.elapsed().as_millis(),
                        "shared GPU window ready"
                    );
                    gpu::RendererState::Ready(gpu)
                }
                gpu::SharedGpuSetup::Failed(error) => return Err(error),
                gpu::SharedGpuSetup::Incompatible(fonts) => {
                    tracing::info!(
                        "Window {} is incompatible with shared GPU adapter; creating dedicated GPU setup",
                        key.raw()
                    );
                    Self::start_async_gpu(
                        key,
                        event_loop,
                        &window,
                        size,
                        config,
                        *fonts,
                        paced_by_frame_callbacks,
                        event_proxy.clone(),
                        &mut cpu_frames,
                    )?
                }
            }
        } else {
            Self::start_async_gpu(
                key,
                event_loop,
                &window,
                size,
                config,
                fonts,
                paced_by_frame_callbacks,
                event_proxy.clone(),
                &mut cpu_frames,
            )?
        };
        renderer.set_tab_bar_height(tab_bar.content_top());

        let accessibility = config.behavior.accessibility.then(|| {
            let adapter =
                ultty::a11y::Adapter::new(event_loop, &window, WINDOW_TITLE, event_proxy.clone());
            window.set_visible(true);
            tracing::debug!(
                target: "ultty::startup",
                window = key.raw(),
                "accessibility adapter ready, window shown"
            );
            adapter
        });
        window.set_ime_allowed(true);

        Ok(Self {
            key,
            window,
            accessibility,
            renderer,
            cpu_frames,
            output_shown: false,
            tab_bar,
            tab_width_freeze: None,
            last_render: Instant::now(),
            input_sent_at: None,
            output_since_input: false,
            last_tab_title_refresh: Instant::now() - TAB_TITLE_REFRESH_INTERVAL,
            tab_title_activity: None,
            needs_redraw: true,
            frame_pacing: pacing::FramePacing::default(),
            pending_close: false,
            modifiers: ModifiersState::empty(),
            mouse_pos: PhysicalPosition::new(0.0, 0.0),
            mouse_pressed: false,
            native_selection: native_selection::NativeSelectionState::default(),
            reported_button: None,
            last_click_time: Instant::now() - CLICK_THRESHOLD * 2,
            click_count: 0,
            last_click_pos: (0, 0),
            last_title_bar_toggle_click_time: Instant::now() - CLICK_THRESHOLD * 2,
            copy_mode: CopyMode::new(),
            search_job: None,
            copy_mode_pane: None,
            url_detector: UrlDetector::new(config.behavior.url_detection),
            bell_active_until: None,
            divider_dragging: None,
            shortcuts_visible: false,
            command_palette: Default::default(),
            protocol_status_visible: false,
            cursor_blink: blink::CursorBlink::new(),
            frames_drawn: 0,
            native_mouse_capture: native_panes::MouseCapture::default(),
            native_search_matches: Vec::new(),
            native_search_active: false,
            native_search_query: String::new(),
            native_search_index: 0,
            url_row_buf: Vec::new(),
            url_hyperlink_grid: Vec::new(),
            url_hyperlink_map: std::collections::HashMap::new(),
            scroll_pixel_accum: 0.0,
            scroll_line_accum: 0.0,
            scale_factor,
            min_window_size: None,
            launched_at: Instant::now(),
            ime_preedit: String::new(),
            ime_cursor_area: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn start_async_gpu(
        key: WindowKey,
        event_loop: &ActiveEventLoop,
        window: &Arc<Window>,
        size: PhysicalSize<u32>,
        config: &Config,
        fonts: ultty::renderer::Fonts,
        paced_by_frame_callbacks: bool,
        event_proxy: EventLoopProxy<AppEvent>,
        cpu_frames: &mut Option<cpu_frames::CpuFrames>,
    ) -> Result<gpu::RendererState> {
        *cpu_frames = cpu_frames::enabled(config.behavior.cpu_first_frames)
            .then(|| {
                cpu_frames::CpuFrames::new(event_loop.owned_display_handle(), Arc::clone(window))
                    .inspect_err(|error| tracing::warn!("No CPU first frames: {error:#}"))
                    .ok()
            })
            .flatten();
        let (fonts_sender, fonts_receiver) = std::sync::mpsc::channel();
        gpu::spawn_setup(
            key,
            event_loop.owned_display_handle(),
            Arc::clone(window),
            size,
            config.colors.clone(),
            fonts_receiver,
            paced_by_frame_callbacks,
            event_proxy,
        )?;
        if let Some(cpu) = cpu_frames.as_mut() {
            cpu.set_fonts(&fonts, &config.colors);
            tracing::debug!(
                target: "ultty::startup",
                window = key.raw(),
                "CPU frame fonts ready"
            );
        }
        let renderer =
            gpu::RendererState::Starting(gpu::Starting::new(&fonts, size, config.behavior.opacity));
        let _ = fonts_sender.send(fonts);
        Ok(renderer)
    }
}

impl ProcessState {
    /// Save the current session, unless this process keeps none.
    pub(super) fn save_session(&self) {
        save_session(&self.runtime.manager, self.session_file.as_deref());
    }

    /// Create a new tab in `window` whose shell is wired to its own protocol socket.
    pub(super) fn create_tab_in_window(&mut self, window: WindowKey) -> std::io::Result<TabId> {
        self.runtime
            .manager
            .create_tab_in_window(window, &mut |tab_id| {
                tab_shell_env(&mut self.runtime.protocol, &self.config.terminal, tab_id)
            })
    }

    /// Create a new tab running `command` in `window`, wired to its own protocol socket.
    pub(super) fn create_tab_with_command_in_window(
        &mut self,
        window: WindowKey,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> std::io::Result<TabId> {
        self.runtime.manager.create_tab_with_command_in_window(
            window,
            command,
            args,
            working_dir,
            title,
            &mut |tab_id| tab_shell_env(&mut self.runtime.protocol, &self.config.terminal, tab_id),
        )
    }

    /// Keep the protocol server's view of tabs in step with the terminal manager.
    pub(super) fn sync_protocol_tabs(&mut self) {
        self.runtime.sync_tabs();
    }

    /// Socket fds the wakeup bridge watches.
    pub(super) fn refresh_runtime_wakeup_fds(&mut self) {
        let mut readable = Vec::new();
        let mut writable = Vec::new();
        if let Some(server) = self.runtime.protocol.as_ref() {
            readable.extend(server.readiness_fds());
        }
        if let Some(server) = self.control_server.as_ref() {
            readable.extend(server.readiness_fds());
            writable.extend(server.writable_fds());
        }
        self.socket_wakeup.set_fds(readable, writable);
    }

    /// Copy text to the system clipboard.
    pub(super) fn copy_to_clipboard(&mut self, text: &str) {
        if let Some(clipboard) = &mut self.clipboard {
            if let Err(e) = clipboard.set_text(text.to_string()) {
                tracing::warn!("Failed to copy to clipboard: {}", e);
            } else {
                tracing::debug!("Copied {} bytes to clipboard", text.len());
            }
        }
    }
}

impl App {
    /// Create a new application with its initial window.
    fn new(
        event_loop: &ActiveEventLoop,
        event_proxy: EventLoopProxy<AppEvent>,
        startup_command: Option<&StartupCommand>,
        config: Config,
    ) -> Result<Self> {
        tracing::info!("Loaded configuration");

        let primary_key = WindowKey::PRIMARY;
        let first_window =
            WindowState::create(primary_key, event_loop, event_proxy.clone(), &config, None)?;
        let (cols, rows) = first_window.renderer.terminal_dimensions();
        tracing::info!("Terminal size: {}x{}", cols, rows);

        Self::sweep_stale_runtime_files();
        let mut protocol_server = Self::start_protocol_server();
        if let Some(server) = protocol_server.as_mut() {
            server.set_viewport(f32::from(cols), f32::from(rows));
        }
        let control_server = Self::start_control_server();

        let mut terminal_manager = Self::create_terminal_manager(
            &config,
            cols,
            rows,
            &mut protocol_server,
            startup_command,
        )?;
        tracing::debug!(target: "ultty::startup", "shell started");
        terminal_manager.set_theme_colors(config.colors.theme_colors());
        terminal_manager.set_scrollback_lines(config.behavior.scrollback_lines);
        terminal_manager.set_image_storage_limit(config.images.storage_limit_bytes());
        terminal_manager.set_close_on_exit(config.behavior.close_on_exit);
        terminal_manager.set_inherit_working_directory(config.behavior.inherit_working_directory);
        let pty_proxy = std::sync::Mutex::new(event_proxy.clone());
        let published_panes = control_events::open_panes(&terminal_manager)
            .into_iter()
            .map(|(pane, _)| pane)
            .collect();
        let mut runtime =
            ultty::runtime::SessionRuntime::new(terminal_manager, protocol_server, (cols, rows))?;
        runtime.set_viewer_present(true);
        runtime.set_viewer_waker(Some(std::sync::Arc::new(move || {
            if let Ok(proxy) = pty_proxy.lock() {
                let _ = proxy.send_event(AppEvent::RuntimeReadable);
            }
        })));
        let socket_proxy = event_proxy.clone();
        let socket_wakeup = wakeup::SocketWakeup::new(move || {
            socket_proxy.send_event(AppEvent::RuntimeReadable).is_ok()
        });

        let clipboard = Clipboard::new().ok();
        if clipboard.is_none() {
            tracing::warn!("Failed to initialize clipboard");
        }

        let watcher_proxy = event_proxy.clone();
        let config_watcher = ConfigWatcher::new(move || {
            let _ = watcher_proxy.send_event(AppEvent::RuntimeReadable);
        });
        if config_watcher.is_some() {
            tracing::info!("Config hot-reload enabled");
        }

        let shortcuts_list = ProcessState::build_shortcuts_list(&config.keybindings);
        let first_winit_id = first_window.window.id();

        let shared = ProcessState {
            proxy: event_proxy,
            shared_gpu: None,
            runtime,
            session_file: session_file(
                startup_command,
                ultty::session::SessionData::last_session_path(),
            ),
            config,
            config_watcher,
            clipboard,
            shortcuts_list,
            control_server,
            pending_control_waits: Vec::new(),
            pending_override_pushes: Vec::new(),
            pending_semantic_actions: Vec::new(),
            pending_resizes: Vec::new(),
            next_override_request: 1,
            published_panes,
            socket_wakeup,
            auto_dump_frames: std::env::var("ULTTY_AUTO_DUMP")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            pending_new_windows: 0,
            pending_reload_config: false,
            pending_cycle_theme: false,
        };

        let mut windows = BTreeMap::new();
        windows.insert(primary_key, first_window);
        let mut window_ids = HashMap::new();
        window_ids.insert(first_winit_id, primary_key);

        let mut app = Self {
            shared,
            windows,
            window_ids,
            focused_window: Some(primary_key),
        };

        app.with_window(primary_key, |ctx| {
            ctx.update_tab_bar();
        });
        app.shared.refresh_runtime_wakeup_fds();

        Ok(app)
    }

    /// Run `f` with a disjoint mutable view of `key`'s window and process state.
    fn with_window<R>(
        &mut self,
        key: WindowKey,
        f: impl FnOnce(&mut WindowCtx<'_>) -> R,
    ) -> Option<R> {
        let win = self.windows.get_mut(&key)?;
        let mut ctx = WindowCtx {
            app: &mut self.shared,
            win,
        };
        Some(f(&mut ctx))
    }

    /// The window targeted by window-unqualified control commands.
    pub(super) fn target_window_key(&self) -> Option<WindowKey> {
        self.focused_window
            .filter(|key| self.windows.contains_key(key))
            .or_else(|| self.windows.keys().next().copied())
    }

    /// Open a new window in the running process, sharing the GPU device when
    /// compatible (ADR 0027).
    pub(super) fn open_new_window(&mut self, event_loop: &ActiveEventLoop) -> Result<WindowKey> {
        let start = Instant::now();
        let (cols, rows) = self
            .target_window_key()
            .and_then(|key| self.windows.get(&key))
            .map(|win| win.renderer.terminal_dimensions())
            .unwrap_or((80, 24));
        let (key, _tab_id) =
            self.shared
                .runtime
                .manager
                .create_window(cols, rows, &mut |tab_id| {
                    tab_shell_env(
                        &mut self.shared.runtime.protocol,
                        &self.shared.config.terminal,
                        tab_id,
                    )
                })?;
        let win = match WindowState::create(
            key,
            event_loop,
            self.shared.proxy.clone(),
            &self.shared.config,
            self.shared.shared_gpu.as_ref(),
        ) {
            Ok(win) => win,
            Err(error) => {
                self.shared.runtime.manager.close_window(key);
                self.shared.sync_protocol_tabs();
                return Err(error);
            }
        };
        let winit_id = win.window.id();
        self.windows.insert(key, win);
        self.window_ids.insert(winit_id, key);
        self.focused_window = Some(key);
        self.shared.runtime.manager.set_focused_window(key);
        self.shared.runtime.set_viewer_present(true);
        self.with_window(key, |ctx| {
            ctx.update_tab_bar();
            ctx.relayout_terminals();
            ctx.win.needs_redraw = true;
        });
        tracing::info!(
            target: "ultty::startup",
            window = key.raw(),
            windows = self.windows.len(),
            elapsed_ms = start.elapsed().as_millis(),
            "opened in-process window"
        );
        Ok(key)
    }

    /// Close one window. Returns `true` when the application event loop should
    /// exit (last window closed on Linux; macOS stays alive with zero windows).
    pub(super) fn close_window(&mut self, key: WindowKey) -> bool {
        let is_last = self.windows.len() <= 1;
        if is_last {
            self.shared.save_session();
        }
        if let Some(win) = self.windows.remove(&key) {
            self.window_ids.remove(&win.window.id());
        }
        self.shared
            .pending_resizes
            .retain(|(window_key, _)| *window_key != key);
        self.shared.runtime.manager.close_window(key);

        if self.focused_window == Some(key) {
            self.focused_window = self.windows.keys().next().copied();
            if let Some(next) = self.focused_window {
                self.shared.runtime.manager.set_focused_window(next);
            }
        }
        if self.windows.is_empty() {
            self.shared.runtime.set_viewer_present(false);
        }
        self.shared.sync_protocol_tabs();
        self.shared.refresh_runtime_wakeup_fds();

        self.windows.is_empty() && cfg!(not(target_os = "macos"))
    }

    fn drain_pending_process_actions(&mut self, event_loop: &ActiveEventLoop) {
        if self.shared.pending_reload_config {
            self.shared.pending_reload_config = false;
            self.reload_config();
        }
        if self.shared.pending_cycle_theme {
            self.shared.pending_cycle_theme = false;
            self.cycle_theme();
        }
        while self.shared.pending_new_windows > 0 {
            self.shared.pending_new_windows -= 1;
            if let Err(error) = self.open_new_window(event_loop) {
                tracing::warn!("Failed to open new window: {error:#}");
            }
        }
    }

    fn handle_window_event_for(
        &mut self,
        event_loop: &ActiveEventLoop,
        key: WindowKey,
        event: WindowEvent,
    ) -> bool {
        if matches!(event, WindowEvent::Focused(true)) {
            self.focused_window = Some(key);
            self.shared.runtime.manager.set_focused_window(key);
            self.shared.sync_protocol_tabs();
        }
        let close = self
            .with_window(key, |ctx| ctx.handle_window_event(event))
            .unwrap_or(false);
        self.drain_pending_process_actions(event_loop);
        if close {
            return self.close_window(key);
        }
        false
    }

    fn start_control_server() -> Option<ControlServer> {
        let server = match ControlServer::new() {
            Ok(server) => server,
            Err(e) => {
                tracing::warn!("Failed to start control server: {}", e);
                return None;
            }
        };

        // The server fails closed without a token file, so a token we cannot
        // write means nobody can use the socket; say so loudly.
        let token_path = server.token_path().to_path_buf();
        match control::write_auth_token(&token_path) {
            Ok(_) => tracing::info!(
                "Control server started at {:?} (auth token in {:?})",
                server.socket_path(),
                token_path
            ),
            Err(e) => tracing::error!(
                "Control server started at {:?} but the auth token could not be written to {:?}: {}; all control requests will be rejected",
                server.socket_path(),
                token_path,
                e
            ),
        }
        Some(server)
    }

    /// Create terminal manager, optionally using a launch profile or restoring from session.
    ///
    /// A `startup_command` (`ultty -e`) replaces both: the window opens with
    /// one tab running it, as `ulttyctl new-tab COMMAND` would.
    ///
    /// Every tab created here gets a listening socket on `protocol_server`
    /// before its shell is spawned.
    fn create_terminal_manager(
        config: &Config,
        cols: u16,
        rows: u16,
        protocol_server: &mut Option<ProtocolServer>,
        startup_command: Option<&StartupCommand>,
    ) -> Result<TerminalManager> {
        if let Some(command) = startup_command {
            return match command {
                StartupCommand::Exec { program, args } => {
                    TerminalManager::with_command_tab(cols, rows, program, args, &mut |tab_id| {
                        tab_shell_env(protocol_server, &config.terminal, tab_id)
                    })
                    .map_err(|e| anyhow::anyhow!("Failed to start '{}': {}", program, e))
                }
                StartupCommand::Profile(name) => {
                    let profile = config
                        .profiles
                        .profiles
                        .get(name)
                        .ok_or_else(|| anyhow::anyhow!("Unknown launch profile '{name}'"))?;
                    Self::create_manager_from_profile(
                        profile,
                        &config.terminal,
                        cols,
                        rows,
                        protocol_server,
                    )
                }
                StartupCommand::Workspace(name) => {
                    let session = ultty::session::SessionData::load_named(name)?;
                    Self::create_manager_from_session(
                        &session,
                        &config.terminal,
                        cols,
                        rows,
                        protocol_server,
                    )
                }
            };
        }

        // Try to restore session if enabled
        if config.behavior.restore_session {
            if let Some(session) = ultty::session::SessionData::load_last() {
                tracing::info!(
                    "Restoring previous session with {} tabs",
                    session.tabs.len()
                );
                match Self::create_manager_from_session(
                    &session,
                    &config.terminal,
                    cols,
                    rows,
                    protocol_server,
                ) {
                    Ok(manager) => return Ok(manager),
                    Err(e) => {
                        tracing::warn!("Failed to restore session: {}, using default", e);
                    }
                }
            }
        }

        // Check if a default profile is configured
        if let Some(ref profile_name) = config.profiles.default {
            if let Some(profile) = config.profiles.profiles.get(profile_name) {
                tracing::info!("Using launch profile: {}", profile_name);
                return Self::create_manager_from_profile(
                    profile,
                    &config.terminal,
                    cols,
                    rows,
                    protocol_server,
                );
            } else {
                tracing::warn!(
                    "Default profile '{}' not found, using default shell",
                    profile_name
                );
            }
        }

        // No profile configured, use default shell
        TerminalManager::with_tab_env(cols, rows, &mut |tab_id| {
            tab_shell_env(protocol_server, &config.terminal, tab_id)
        })
        .map_err(|e| anyhow::anyhow!("Failed to create terminal manager: {}", e))
    }

    /// Create terminal manager from a launch profile.
    fn create_manager_from_profile(
        profile: &ultty::config::LaunchProfile,
        terminal: &TerminalConfig,
        cols: u16,
        rows: u16,
        protocol_server: &mut Option<ProtocolServer>,
    ) -> Result<TerminalManager> {
        // Validate the complete profile before spawning any process.
        for tab in &profile.tabs {
            for pane in &tab.panes {
                profile_split_direction(pane.split.as_deref())?;
            }
        }
        if profile.tabs.is_empty() {
            return TerminalManager::with_tab_env(cols, rows, &mut |tab_id| {
                tab_shell_env(protocol_server, terminal, tab_id)
            })
            .map_err(|e| anyhow::anyhow!("Failed to create terminal manager: {}", e));
        }
        let mut manager = TerminalManager::empty(cols, rows);
        {
            // Get default shell for tabs without explicit command
            let default_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

            // Create tabs from profile
            for tab_config in &profile.tabs {
                let command = tab_config.command.as_deref().unwrap_or(&default_shell);

                let working_dir = tab_config
                    .working_dir
                    .as_deref()
                    .or(profile.working_dir.as_deref());

                let title = tab_config.title.as_deref();

                manager
                    .create_tab_with_command(
                        command,
                        &tab_config.args,
                        working_dir,
                        title,
                        &mut |tab_id| tab_shell_env(protocol_server, terminal, tab_id),
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to create tab: {}", e))?;

                // Each configured pane splits the previous one, in configuration
                // order. This flat format does not describe arbitrary trees.
                let mut target = manager
                    .focused_terminal_id()
                    .ok_or_else(|| anyhow::anyhow!("Profile tab has no pane"))?;
                for pane in &tab_config.panes {
                    target = manager
                        .split_pane_with_command(
                            target,
                            profile_split_direction(pane.split.as_deref())?,
                            pane.command.as_deref().unwrap_or(&default_shell),
                            &pane.args,
                            pane.working_dir.as_deref().or(working_dir),
                        )?
                        .ok_or_else(|| anyhow::anyhow!("Profile split target disappeared"))?;
                }

                tracing::info!(
                    "Created tab: {} (command: {})",
                    title.unwrap_or("unnamed"),
                    command
                );
            }
        }

        Ok(manager)
    }

    /// Create terminal manager from a saved session.
    fn create_manager_from_session(
        session: &ultty::session::SessionData,
        terminal: &TerminalConfig,
        cols: u16,
        rows: u16,
        protocol_server: &mut Option<ProtocolServer>,
    ) -> Result<TerminalManager> {
        TerminalManager::from_session(session, cols, rows, &mut |tab_id| {
            tab_shell_env(protocol_server, terminal, tab_id)
        })
        .map_err(|e| anyhow::anyhow!("Failed to restore session: {}", e))
    }

    fn sweep_stale_runtime_files() {
        let mut dirs: Vec<_> = [
            ultty::control::paths::control_dir(),
            ultty::platform::socket_dir(),
        ]
        .into_iter()
        .filter_map(Result::ok)
        .collect();
        dirs.dedup();
        for dir in &dirs {
            ultty::platform::runtime_files::sweep_stale_runtime_files(dir);
        }
    }

    fn start_protocol_server() -> Option<ProtocolServer> {
        let socket_dir = match ultty::platform::socket_dir() {
            Ok(dir) => dir,
            Err(e) => {
                tracing::warn!("Failed to resolve protocol socket directory: {}", e);
                return None;
            }
        };

        match ProtocolServer::new(&socket_dir) {
            Ok(server) => {
                tracing::info!(
                    "Native protocol server started; per-tab sockets in {:?}",
                    server.socket_dir()
                );
                Some(server)
            }
            Err(e) => {
                tracing::warn!("Failed to start protocol server: {}", e);
                None
            }
        }
    }

    /// Run the application. The first tab runs `startup_command`, or the
    /// configured shell, profile or restored session when there is none.
    pub fn run(startup_command: Option<StartupCommand>, config: Config) -> Result<()> {
        let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
        let signal_proxy = event_loop.create_proxy();
        if let Err(e) = ultty::platform::shutdown::on_shutdown_signal(move || {
            let _ = signal_proxy.send_event(AppEvent::ShutdownSignal);
        }) {
            tracing::warn!(
                "Failed to catch termination signals; one will kill Ultty without cleanup: {}",
                e
            );
        }
        let mut handler = AppHandler {
            proxy: event_loop.create_proxy(),
            startup_command,
            config: Some(config),
            app: None,
            init_error: None,
        };
        event_loop.run_app(&mut handler)?;
        handler.init_error.map_or(Ok(()), Err)
    }

    /// Run pending terminal work, then sleep until the next deadline.
    fn update_and_schedule(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_pending_process_actions(event_loop);
        if self.update() {
            event_loop.exit();
        }
        self.shared.socket_wakeup.rearm();
        event_loop.set_control_flow(control_flow(self.next_update_deadline(Instant::now())));
    }
}

impl WindowCtx<'_> {
    /// Create a new tab in this window whose shell is wired to its own protocol socket.
    pub(super) fn create_tab(&mut self) -> std::io::Result<TabId> {
        self.app.create_tab_in_window(self.win.key)
    }

    /// Get the terminal state of this window's focused terminal.
    pub(super) fn terminal(&self) -> Option<ultty::event_loop::TerminalGuard<'_>> {
        self.app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)
            .map(|t| t.event_loop.terminal())
    }

    /// Get mutable terminal state of this window's focused terminal.
    pub(super) fn terminal_mut(&mut self) -> Option<ultty::event_loop::TerminalGuard<'_>> {
        self.app
            .runtime
            .manager
            .window_focused_terminal_mut(self.win.key)
            .map(|t| t.event_loop.terminal_mut())
    }

    /// Send input to this window's focused terminal, returning a scrolled-back
    /// viewport to the live bottom so typed input is immediately visible.
    pub(super) fn send_input(&mut self, data: &[u8]) -> std::io::Result<()> {
        if let Some(terminal) = self
            .app
            .runtime
            .manager
            .window_focused_terminal_mut(self.win.key)
        {
            let mut scrolled_back = false;
            {
                let mut term = terminal.event_loop.terminal_mut();
                if !term.is_alternate_screen() && term.buffer().is_scrolled() {
                    term.buffer_mut().scroll_to_bottom();
                    scrolled_back = true;
                }
            }
            if scrolled_back {
                self.win.needs_redraw = true;
            }
            self.win.input_sent_at = Some(Instant::now());
            self.win.output_since_input = false;
            terminal.event_loop.send_input(data)
        } else {
            Ok(())
        }
    }

    /// Send a report the terminal generates on its own (mouse motion, focus).
    pub(super) fn send_report(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self
            .app
            .runtime
            .manager
            .window_focused_terminal_mut(self.win.key)
        {
            Some(terminal) => terminal.event_loop.send_input(data),
            None => Ok(()),
        }
    }

    fn update_title_location(&mut self) {
        let cwd = self
            .app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)
            .and_then(control::terminal_working_directory);
        let home = std::env::var("HOME").ok();
        self.win.tab_bar.title_path = cwd
            .as_deref()
            .map(|dir| ultty::tabs::display_path(dir, home.as_deref()))
            .unwrap_or_default();
        self.win.tab_bar.title_branch = cwd
            .as_deref()
            .and_then(|dir| ultty::tabs::git_branch(std::path::Path::new(dir)));
        self.update_min_window_size();
    }

    fn update_min_window_size(&mut self) {
        let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
        let (width, height) = self.win.tab_bar.min_window_size(cell_width, cell_height);
        let size = PhysicalSize::new(width as u32, height as u32);
        if self.win.min_window_size != Some(size) {
            self.win.min_window_size = Some(size);
            self.win.window.set_min_inner_size(Some(size));
        }
    }

    pub(super) fn update_tab_bar(&mut self) {
        self.app.sync_protocol_tabs();
        let now = Instant::now();
        self.win.last_tab_title_refresh = now;
        self.win.tab_title_activity =
            update::title_activity_after_refresh(self.win.tab_title_activity, now);

        let total_width = self.win.renderer.surface_size().0 as f32;
        let active_tab_id = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|t| t.id)
            .unwrap_or_else(|| TabId::new(0));

        let tab_width_override = self.active_tab_width_freeze().map(|freeze| freeze.width);
        let tab_titles = self
            .app
            .runtime
            .manager
            .window_tab_display_titles(self.win.key);
        let window_tabs = self.app.runtime.manager.window_tabs(self.win.key);

        self.win.tab_bar.update_with_tab_refs_and_width_override(
            &window_tabs,
            &tab_titles,
            active_tab_id,
            total_width,
            tab_width_override,
        );
        self.update_title_location();
        self.update_tab_statuses();

        let new_height = self.win.tab_bar.content_top();
        let old_height = self.win.renderer.tab_bar_height();

        if (new_height - old_height).abs() > 0.1 {
            self.win.renderer.set_tab_bar_height(new_height);
            self.relayout_terminals();
        }

        self.app.refresh_runtime_wakeup_fds();
    }

    fn update_tab_statuses(&mut self) {
        let manager = &self.app.runtime.manager;
        let window_tabs = manager.window_tabs(self.win.key);
        for (state, tab) in self.win.tab_bar.tabs.iter_mut().zip(window_tabs) {
            state.status = tab
                .terminals
                .iter()
                .filter_map(|&pane| manager.terminal(pane)?.shown_status())
                .max_by_key(|status| status.urgency())
                .cloned();
        }
        self.win.tab_bar.title_status = manager
            .window_focused_terminal(self.win.key)
            .and_then(|terminal| terminal.shown_status().cloned());
    }

    pub(super) fn freeze_tab_width_after_close(&mut self) {
        let Some(width) = self.win.tab_bar.tabs.first().map(|tab| tab.width) else {
            return;
        };

        self.win.tab_width_freeze = Some(TabWidthFreeze {
            width,
            until: Instant::now() + TAB_CLOSE_WIDTH_FREEZE_DURATION,
        });
    }

    pub(super) fn clear_tab_width_freeze(&mut self) {
        self.win.tab_width_freeze = None;
    }

    fn active_tab_width_freeze(&self) -> Option<TabWidthFreeze> {
        self.win
            .tab_width_freeze
            .filter(|freeze| Instant::now() < freeze.until)
    }

    pub(super) fn expire_tab_width_freeze(&mut self, now: Instant) {
        if self
            .win
            .tab_width_freeze
            .is_some_and(|freeze| now >= freeze.until)
        {
            self.win.tab_width_freeze = None;
            self.update_tab_bar();
            self.win.needs_redraw = true;
        }
    }

    pub(super) fn extract_selection_text(&self, selection: &VimSelection) -> String {
        let Some(terminal) = self.terminal() else {
            return String::new();
        };
        extract_vim_selection_text(terminal.buffer(), selection)
    }

    pub(super) fn perform_search(&mut self, query: &str) {
        use ultty::vim::search::{Matcher, SearchJob, SearchOptions};
        use ultty::vim::SearchStatus;

        self.win.search_job = None;
        if query.is_empty() {
            return;
        }
        let options = SearchOptions {
            regex: self.win.copy_mode.search_regex(),
            ..self.app.config.search
        };
        match Matcher::new(query, options) {
            Err(error) => {
                self.win.copy_mode.begin_search();
                self.win
                    .copy_mode
                    .set_search_status(SearchStatus::Invalid(error.to_string()));
            }
            Ok(matcher) => {
                let job = self
                    .terminal()
                    .map(|terminal| SearchJob::new(matcher, terminal.buffer()));
                if job.is_some() {
                    self.win.search_job = job;
                    self.win.copy_mode.begin_search();
                }
            }
        }

        if self.win.native_search_active {
            self.perform_native_search(query);
        }
    }

    pub(super) fn advance_search(&mut self) {
        use ultty::vim::SearchStatus;

        self.reconcile_copy_mode_pane();
        if !self.win.copy_mode.is_active() {
            self.win.search_job = None;
            self.win.copy_mode_pane = None;
            return;
        }
        let Some(mut job) = self.win.search_job.take() else {
            if !self.win.copy_mode.search_matches().0.is_empty() {
                let evicted = self.terminal().map(|t| t.buffer().evicted_lines());
                if let Some(evicted) = evicted {
                    let before = self.win.copy_mode.search_matches().0.len();
                    self.win.copy_mode.rebase_search(evicted);
                    self.win.needs_redraw |= self.win.copy_mode.search_matches().0.len() != before;
                }
            }
            return;
        };
        let Some(terminal) = self.terminal() else {
            return;
        };
        let buffer = terminal.buffer();
        let done = job.step(buffer, SEARCH_SLICE);
        let percent = job.percent_done(buffer);
        drop(terminal);
        if done {
            let (matches, evicted) = job.into_matches();
            tracing::debug!("Search finished: {} matches", matches.len());
            self.win.copy_mode.finish_search(matches, evicted);
        } else {
            self.win
                .copy_mode
                .set_search_status(SearchStatus::Searching(percent));
            self.win.search_job = Some(job);
        }
        self.win.needs_redraw = true;
    }

    pub(super) fn reconcile_copy_mode_pane(&mut self) {
        self.win.needs_redraw |= reconcile_copy_mode_pane(
            &mut self.win.copy_mode_pane,
            &mut self.win.search_job,
            &mut self.win.copy_mode,
            self.app
                .runtime
                .manager
                .window_focused_terminal_id(self.win.key),
        );
    }

    pub(super) fn has_native_sessions(&self) -> bool {
        let Some(active_tab) = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id)
        else {
            return false;
        };
        self.app
            .runtime
            .protocol
            .as_ref()
            .is_some_and(|s| s.tab_sessions(active_tab).next().is_some())
    }

    fn perform_native_search(&mut self, query: &str) {
        self.win.native_search_query = query.to_string();
        self.win.native_search_matches.clear();
        self.win.native_search_index = 0;

        if let (Some(ref server), Some(active_tab)) = (
            &self.app.runtime.protocol,
            self.app
                .runtime
                .manager
                .window_active_tab(self.win.key)
                .map(|tab| tab.id),
        ) {
            for session in server.tab_sessions(active_tab) {
                let matches = session.scene().search(query);
                self.win.native_search_matches.extend(matches);
            }
        }

        tracing::debug!(
            "Native search '{}': found {} matches",
            query,
            self.win.native_search_matches.len()
        );
    }

    pub(super) fn native_search_next(&mut self) {
        if self.win.native_search_matches.is_empty() {
            return;
        }
        self.win.native_search_index =
            (self.win.native_search_index + 1) % self.win.native_search_matches.len();
        self.win.needs_redraw = true;
    }

    pub(super) fn native_search_prev(&mut self) {
        if self.win.native_search_matches.is_empty() {
            return;
        }
        if self.win.native_search_index == 0 {
            self.win.native_search_index = self.win.native_search_matches.len() - 1;
        } else {
            self.win.native_search_index -= 1;
        }
        self.win.needs_redraw = true;
    }

    pub(super) fn dismiss_native_search(&mut self) {
        self.win.native_search_active = false;
        self.win.native_search_matches.clear();
        self.win.native_search_query.clear();
        self.win.native_search_index = 0;
        self.win.needs_redraw = true;
    }

    pub(super) fn update_tab_hover(&mut self) {
        if !self.win.tab_bar.visible {
            return;
        }

        let x = self.win.mouse_pos.x as f32;
        let y = self.win.mouse_pos.y as f32;

        let old_hovered_tab = self.win.tab_bar.hovered_tab;
        let old_new_tab_hovered = self.win.tab_bar.new_tab_button_hovered;
        let old_window_control = self.win.tab_bar.hovered_title_button;
        let old_close_button = self.win.tab_bar.hovered_close_button;
        self.win.tab_bar.set_hover(x, y);
        let old_menu_item = self.win.tab_bar.hovered_menu_item;
        if self.win.tab_bar.menu_open {
            let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
            self.win.tab_bar.hovered_menu_item = self
                .win
                .tab_bar
                .menu_layout(cell_width, cell_height)
                .item_at(x, y);
            if self.win.tab_bar.hovered_menu_item != old_menu_item {
                self.win.needs_redraw = true;
            }
        }

        if old_hovered_tab != self.win.tab_bar.hovered_tab
            || old_new_tab_hovered != self.win.tab_bar.new_tab_button_hovered
            || old_window_control != self.win.tab_bar.hovered_title_button
            || old_close_button != self.win.tab_bar.hovered_close_button
        {
            self.win.needs_redraw = true;
        }
    }

    pub(super) fn resize_direction_at(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        let size = self.win.window.inner_size();
        let monitor_size = self
            .win
            .window
            .current_monitor()
            .map(|monitor| monitor.size());
        if window_should_use_square_corners(size, monitor_size, self.win.window.is_maximized()) {
            return None;
        }
        let (cell_width, _) = self.win.renderer.cell_dimensions();
        resize_direction_at_window_edge(
            x,
            y,
            size.width as f32,
            size.height as f32,
            self.win.window.scale_factor() as f32,
            self.win.tab_bar.content_top(),
            cell_width,
        )
    }

    pub(super) fn update_resize_cursor(&mut self) -> bool {
        let x = self.win.mouse_pos.x as f32;
        let y = self.win.mouse_pos.y as f32;
        let Some(direction) = self.resize_direction_at(x, y) else {
            return false;
        };

        let cursor = match direction {
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
            ResizeDirection::West | ResizeDirection::East => CursorIcon::EwResize,
            ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        };
        self.win.window.set_cursor(cursor);
        true
    }

    pub(super) fn update_cursor_for_url(&mut self) {
        if self.win.tab_bar.visible && (self.win.mouse_pos.y as f32) < self.win.tab_bar.height() {
            self.win.window.set_cursor(CursorIcon::Default);
            return;
        }

        if !self.win.url_detector.is_enabled() || !self.win.modifiers.control_key() {
            self.win.window.set_cursor(CursorIcon::Default);
            return;
        }

        let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
        let (col, row) = self.pixel_to_cell(self.win.mouse_pos.x, adjusted_y);
        if self.win.url_detector.has_url_at(col, row) {
            self.win.window.set_cursor(CursorIcon::Pointer);
        } else {
            self.win.window.set_cursor(CursorIcon::Default);
        }
    }

    pub(super) fn update_url_detection(&mut self) {
        if !self.win.url_detector.is_enabled() {
            return;
        }

        self.win.url_row_buf.clear();
        self.win.url_hyperlink_grid.clear();
        self.win.url_hyperlink_map.clear();

        let Some(focused) = self
            .app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)
        else {
            return;
        };
        let terminal = focused.event_loop.terminal();
        let dims = terminal.dimensions();

        self.win.url_row_buf.reserve(dims.rows as usize);
        for row in 0..dims.rows {
            self.win.url_row_buf.push(
                terminal
                    .buffer()
                    .visible_row(row)
                    .map(|r| r.text_trimmed())
                    .unwrap_or_default(),
            );
        }

        self.win.url_hyperlink_grid.reserve(dims.rows as usize);
        for row in 0..dims.rows {
            let mut row_links = Vec::with_capacity(dims.cols as usize);
            for col in 0..dims.cols {
                row_links.push(
                    terminal
                        .buffer()
                        .grid()
                        .cell(col, row)
                        .map(|c| c.hyperlink_id())
                        .unwrap_or(0),
                );
            }
            self.win.url_hyperlink_grid.push(row_links);
        }

        for (k, v) in terminal.all_hyperlinks().iter() {
            self.win.url_hyperlink_map.insert(*k, v.clone());
        }

        let rows = &self.win.url_row_buf;
        self.win.url_detector.detect(rows.len() as u16, |row| {
            rows.get(row as usize).cloned().unwrap_or_default()
        });

        let grid = &self.win.url_hyperlink_grid;
        let map = &self.win.url_hyperlink_map;
        self.win.url_detector.detect_hyperlinks(
            dims.rows,
            dims.cols,
            |row, col| {
                grid.get(row as usize)
                    .and_then(|r| r.get(col as usize))
                    .copied()
                    .unwrap_or(0)
            },
            |id| map.get(&id).cloned(),
        );
    }

    pub(super) fn handle_column_resize_requests(&mut self) {
        if let Some(terminal) = self
            .app
            .runtime
            .manager
            .window_focused_terminal_mut(self.win.key)
        {
            if let Some(columns) = terminal
                .event_loop
                .terminal_mut()
                .take_column_resize_request()
            {
                let (cell_width, _) = self.win.renderer.cell_dimensions();
                let new_width = (columns as f32 * cell_width) as u32;
                let current_height = self.win.window.inner_size().height;

                let new_size = PhysicalSize::new(new_width, current_height);
                if self.win.window.request_inner_size(new_size).is_some() {
                    tracing::info!(
                        "Requested window resize to {} columns ({} pixels wide)",
                        columns,
                        new_width
                    );
                }

                self.win.needs_redraw = true;
            }
        }
    }

    pub(super) fn relayout_terminals(&mut self) {
        let (width, height) = self.win.renderer.terminal_area();
        let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
        let area = ultty::terminal_manager::PaneArea {
            width,
            height,
            cell_width,
            cell_height,
        };
        if let Err(e) = self
            .app
            .runtime
            .manager
            .layout_window_panes(self.win.key, area)
        {
            tracing::warn!("Failed to resize terminals: {}", e);
        }
    }

    pub(super) fn handle_scale_factor_changed(&mut self, scale_factor: f32) {
        if scale_factor <= 0.0 || (scale_factor - self.win.scale_factor).abs() < f32::EPSILON {
            return;
        }
        let font_size = self.win.renderer.font_size() / self.win.scale_factor * scale_factor;
        tracing::info!(
            "Scale factor {} -> {}, font size {:.1}pt",
            self.win.scale_factor,
            scale_factor,
            font_size / scale_factor
        );
        self.win.scale_factor = scale_factor;
        self.win.tab_bar.scale = scale_factor;
        self.win.renderer.set_font_size(font_size);
        self.update_tab_bar();
        self.relayout_terminals();
        self.win.needs_redraw = true;
    }

    pub(super) fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.win.renderer.resize(size.width, size.height);
        self.update_window_corner_radius();
        self.clear_tab_width_freeze();
        self.update_tab_bar();

        let (cols, rows) = self.win.renderer.terminal_dimensions();
        tracing::debug!("Resize: {}x{}", cols, rows);

        self.relayout_terminals();

        if self.win.copy_mode.is_active() {
            let total_rows = self
                .terminal()
                .map(|t| t.buffer().history_len() + t.dimensions().rows as usize)
                .unwrap_or(rows as usize);
            self.win
                .copy_mode
                .resize(total_rows, rows as usize, cols as usize);
        }

        self.sync_native_viewports();
        self.win.needs_redraw = true;
    }

    pub(super) fn update_window_corner_radius(&mut self) {
        let size = self.win.window.inner_size();
        let monitor_size = self
            .win
            .window
            .current_monitor()
            .map(|monitor| monitor.size());
        let square_corners =
            window_should_use_square_corners(size, monitor_size, self.win.window.is_maximized());
        let radius = if square_corners {
            0.0
        } else {
            WINDOW_CORNER_RADIUS
        };
        if let Some(renderer) = self.win.renderer.ready_mut() {
            renderer.set_window_corner_radius(radius);
        }
    }
}

fn window_should_use_square_corners(
    window_size: PhysicalSize<u32>,
    monitor_size: Option<PhysicalSize<u32>>,
    is_maximized: bool,
) -> bool {
    if is_maximized {
        return true;
    }

    let Some(monitor_size) = monitor_size else {
        return false;
    };

    is_tiled_window_size(window_size, monitor_size)
}

fn is_tiled_window_size(window_size: PhysicalSize<u32>, monitor_size: PhysicalSize<u32>) -> bool {
    if window_size.width == 0
        || window_size.height == 0
        || monitor_size.width == 0
        || monitor_size.height == 0
    {
        return false;
    }

    let width_full = near_scaled_dimension(window_size.width, monitor_size.width, 1);
    let width_half = near_scaled_dimension(window_size.width, monitor_size.width, 2);
    let height_full = near_scaled_dimension(window_size.height, monitor_size.height, 1);
    let height_half = near_scaled_dimension(window_size.height, monitor_size.height, 2);

    (width_full || width_half) && (height_full || height_half)
}

fn near_scaled_dimension(value: u32, target: u32, divisor: u32) -> bool {
    let scaled = value as i64 * divisor as i64;
    let target = target as i64;
    let tolerance = TILED_WINDOW_SIZE_TOLERANCE_PX as i64 * divisor as i64;

    (scaled - target).abs() <= tolerance
}

/// Extract text from a copy-mode selection using absolute buffer row indices
/// (`0..buffer.total_lines()`).
fn extract_vim_selection_text(buffer: &ultty::grid::Buffer, selection: &VimSelection) -> String {
    match selection {
        VimSelection::Char {
            start_row,
            start_col,
            end_row,
            end_col,
        } => buffer.copy_text(*start_row..=*end_row, *start_col as u16, *end_col as u16),
        VimSelection::Line { start_row, end_row } => {
            buffer.copy_text(*start_row..=*end_row, 0, u16::MAX)
        }
        VimSelection::Block {
            start_row,
            start_col,
            end_row,
            end_col,
        } => {
            let mut text = String::new();
            for row in *start_row..=*end_row {
                if let Some(grid_row) = buffer.row_at(row) {
                    if start_col <= end_col {
                        text.push_str(
                            grid_row
                                .copy_text(*start_col as u16, *end_col as u16)
                                .trim_end(),
                        );
                    }
                    if row < *end_row {
                        text.push('\n');
                    }
                }
            }
            text
        }
    }
}

/// The resize direction for a pointer at physical (x, y) in a window of
/// physical size (width, height), or None away from the edges.
///
/// Below `grid_top`, the cell grid sits flush against the left edge (and
/// within any fractional-cell slack of the right edge), so when
/// `cell_width > 0` the interior left and right bands are narrowed to leave
/// column 0 and the last column clickable for text selection.
fn resize_direction_at_window_edge(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
    grid_top: f32,
    cell_width: f32,
) -> Option<ResizeDirection> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let band = WINDOW_RESIZE_EDGE_BAND * scale;
    let outer = WINDOW_RESIZE_OUTER_BAND * scale;
    let arm = WINDOW_RESIZE_CORNER_ARM * scale;
    let square = WINDOW_RESIZE_CORNER_SQUARE * scale;

    if x < -outer || y < -outer || x > width + outer || y > height + outer {
        return None;
    }
    // Distance inward from each edge (negative outside the window).
    let from_left = x;
    let from_right = width - x;
    let from_top = y;
    let from_bottom = height - y;

    let (left_band, right_band) = if y >= grid_top && cell_width > 0.0 {
        let side = (WINDOW_RESIZE_GRID_SIDE_BAND * scale)
            .min(cell_width * 0.25)
            .max(1.0);
        let right_slack = (width - (width / cell_width).floor() * cell_width).max(0.0);
        (side, band.min(right_slack + side))
    } else {
        (band, band)
    };

    let on_left = from_left <= left_band;
    let on_right = from_right <= right_band;
    let on_top = from_top <= band;
    let on_bottom = from_bottom <= band;
    let near_corner = |on_a: bool, a: f32, on_b: bool, b: f32| {
        (on_a && b <= arm) || (on_b && a <= arm) || (a <= square && b <= square)
    };

    if near_corner(on_left, from_left, on_top, from_top) {
        Some(ResizeDirection::NorthWest)
    } else if near_corner(on_right, from_right, on_top, from_top) {
        Some(ResizeDirection::NorthEast)
    } else if near_corner(on_left, from_left, on_bottom, from_bottom) {
        Some(ResizeDirection::SouthWest)
    } else if near_corner(on_right, from_right, on_bottom, from_bottom) {
        Some(ResizeDirection::SouthEast)
    } else if on_left {
        Some(ResizeDirection::West)
    } else if on_right {
        Some(ResizeDirection::East)
    } else if on_top {
        Some(ResizeDirection::North)
    } else if on_bottom {
        Some(ResizeDirection::South)
    } else {
        None
    }
}

/// Shell environment for a new tab: `TERM` (from `terminal.term`), the opt-out
/// for shell integration (from `terminal.shell_integration`) and the
/// tab's own native protocol socket.
///
/// Opens the tab's listener on `server` before the shell is spawned so the
/// shell (and every app it launches) sees a socket that already exists. If
/// the listener cannot be opened the tab still works, without native
/// protocol support, and apps fall back to ANSI rendering.
fn tab_shell_env(
    server: &mut Option<ProtocolServer>,
    terminal: &TerminalConfig,
    tab_id: TabId,
) -> std::io::Result<ultty::terminal_manager::TabEnv> {
    let mut env = vec![("TERM".into(), terminal.term.as_str().into())];
    if terminal.shell_integration == ShellIntegration::Off {
        env.push((ultty::pty::shell_integration::ENV_MODE.into(), "off".into()));
    }
    let Some(server) = server.as_mut() else {
        return Ok(env);
    };
    match server.open_tab(tab_id) {
        Ok(path) => env.push(("ULTTY_SOCKET".into(), path.into_os_string())),
        Err(e) => {
            tracing::warn!(
                "Failed to open protocol socket for tab {}: {}; tab runs without native protocol",
                tab_id.raw(),
                e
            );
        }
    }
    Ok(env)
}

/// Sleep until `deadline`, or with none, until an event arrives.
fn control_flow(deadline: Option<Instant>) -> ControlFlow {
    deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
}

/// The file a window saves its session to on exit: the user's last session,
/// except for a window started with `ultty -e`, which neither restores nor
/// saves one.
fn session_file(
    startup_command: Option<&StartupCommand>,
    last_session: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    match startup_command {
        Some(StartupCommand::Exec { .. }) => None,
        Some(StartupCommand::Profile(_)) | None => last_session,
        Some(StartupCommand::Workspace(name)) => ultty::session::SessionData::named_path(name).ok(),
    }
}

/// Configured panes split their predecessor; omitted direction means side by side.
fn profile_split_direction(value: Option<&str>) -> Result<ultty::terminal_manager::SplitDirection> {
    use ultty::terminal_manager::SplitDirection;

    match value {
        None | Some("horizontal") => Ok(SplitDirection::Horizontal),
        Some("vertical") => Ok(SplitDirection::Vertical),
        Some(value) => anyhow::bail!("Unknown profile split direction '{value}'"),
    }
}

/// Write `manager`'s tabs to `file`; with no file, write nothing.
fn save_session(manager: &TerminalManager, file: Option<&std::path::Path>) {
    let Some(file) = file else {
        return;
    };
    match manager.to_session_data().save(file) {
        Ok(()) => tracing::info!("Session saved"),
        Err(e) => tracing::warn!("Failed to save session: {}", e),
    }
}

/// Drives [`App`] from winit. The window, and so the app, is created on the
/// first `resumed`, as winit 0.30 requires.
struct AppHandler {
    proxy: EventLoopProxy<AppEvent>,
    /// What the first tab runs instead of the shell (`ultty -e`).
    startup_command: Option<StartupCommand>,
    /// Configuration loaded in `main` before the event loop and any threads
    /// start; taken once on the first `resumed`.
    config: Option<Config>,
    app: Option<App>,
    /// A startup failure, returned from [`App::run`] after the loop exits.
    init_error: Option<anyhow::Error>,
}

impl ApplicationHandler<AppEvent> for AppHandler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(app) = self.app.as_mut() {
            if cfg!(target_os = "macos") && app.windows.is_empty() {
                if let Err(error) = app.open_new_window(event_loop) {
                    tracing::warn!("Failed to reopen window: {error:#}");
                }
            }
            return;
        }
        let config = self.config.take().unwrap_or_else(Config::load);
        match App::new(
            event_loop,
            self.proxy.clone(),
            self.startup_command.as_ref(),
            config,
        ) {
            Ok(mut app) => {
                let primary = WindowKey::PRIMARY;
                app.with_window(primary, |ctx| {
                    ctx.update_tab_bar();
                    ctx.relayout_terminals();
                });
                event_loop.set_control_flow(control_flow(app.next_update_deadline(Instant::now())));
                self.app = Some(app);
            }
            Err(error) => {
                self.init_error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(app) = self.app.as_mut() {
            let Some(&key) = app.window_ids.get(&window_id) else {
                return;
            };
            if app.handle_window_event_for(event_loop, key, event) {
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::RuntimeReadable => {
                if let Some(app) = self.app.as_mut() {
                    app.update_and_schedule(event_loop);
                }
            }
            AppEvent::Accessibility(event) => {
                if let Some(app) = self.app.as_mut() {
                    if let Some(&key) = app.window_ids.get(&event.window_id) {
                        app.with_window(key, |ctx| {
                            ctx.handle_accessibility_event(event.window_event);
                        });
                    }
                    app.update_and_schedule(event_loop);
                }
            }
            AppEvent::ShutdownSignal => {
                if let Some(app) = self.app.as_ref() {
                    app.shared.save_session();
                }
                event_loop.exit();
            }
            AppEvent::Renderer(gpu::Handoff { window_key, result }) => {
                let Some(app) = self.app.as_mut() else {
                    return;
                };
                match result {
                    Ok(gpu) => {
                        app.shared
                            .shared_gpu
                            .get_or_insert_with(|| gpu.shared.clone());
                        app.with_window(window_key, |ctx| {
                            ctx.install_renderer(gpu);
                        });
                        app.update_and_schedule(event_loop);
                    }
                    Err(error) => {
                        if app.windows.len() <= 1 {
                            self.init_error = Some(error);
                            event_loop.exit();
                        } else {
                            tracing::error!(
                                "GPU setup failed for window {}: {error:#}",
                                window_key.raw()
                            );
                            if app.close_window(window_key) {
                                event_loop.exit();
                            }
                        }
                    }
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(app) = self.app.as_mut() {
            app.update_and_schedule(event_loop);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use winit::dpi::PhysicalSize;
    use winit::window::ResizeDirection;

    use super::{
        extract_vim_selection_text, resize_direction_at_window_edge,
        window_should_use_square_corners, MAINTENANCE_POLL_INTERVAL,
    };

    #[test]
    fn test_switching_panes_cancels_a_partial_search_before_another_buffer_is_read() {
        use ultty::grid::{Buffer, Dimensions};
        use ultty::terminal_manager::TerminalId;
        use ultty::vim::search::{Matcher, SearchJob, SearchOptions};
        use ultty::vim::CopyMode;

        let buffer = Buffer::with_scrollback(Dimensions::new(80, 200), 400);
        let mut scan = SearchJob::new(
            Matcher::new("needle", SearchOptions::default()).unwrap(),
            &buffer,
        );
        assert!(!scan.step(&buffer, Duration::ZERO));
        let first = TerminalId::new(1);
        let second = TerminalId::new(2);
        let mut owner = Some(first);
        let mut pending = Some(scan);
        let mut mode = CopyMode::new();
        mode.enter(200, 24, 80);
        mode.begin_search();

        assert!(!super::reconcile_copy_mode_pane(
            &mut owner,
            &mut pending,
            &mut mode,
            Some(first)
        ));
        assert!(pending.is_some());
        assert!(super::reconcile_copy_mode_pane(
            &mut owner,
            &mut pending,
            &mut mode,
            Some(second)
        ));
        assert!(pending.is_none());
        assert!(owner.is_none());
        assert!(!mode.is_active());
        assert!(mode.search_matches().0.is_empty());
    }

    #[test]
    fn test_switching_or_closing_panes_discards_completed_search_matches() {
        use ultty::terminal_manager::TerminalId;
        use ultty::vim::search::SearchMatch;
        use ultty::vim::CopyMode;

        let first = TerminalId::new(1);
        for focused in [Some(TerminalId::new(2)), None] {
            let mut owner = Some(first);
            let mut pending = None;
            let mut mode = CopyMode::new();
            mode.enter(200, 24, 80);
            mode.finish_search(vec![SearchMatch::at(150, 7)], 0);
            assert!(!super::reconcile_copy_mode_pane(
                &mut owner,
                &mut pending,
                &mut mode,
                Some(first)
            ));
            assert_eq!(mode.search_matches().0, [SearchMatch::at(150, 7)]);
            assert!(super::reconcile_copy_mode_pane(
                &mut owner,
                &mut pending,
                &mut mode,
                focused
            ));
            assert!(mode.search_matches().0.is_empty());
            assert!(!mode.is_active());
            assert!(owner.is_none());
        }
    }

    #[test]
    fn a_window_started_with_a_command_leaves_the_saved_session_alone() {
        use super::{save_session, session_file, StartupCommand};
        use ultty::terminal_manager::{no_tab_env, TerminalManager};

        let dir = tempfile::tempdir().unwrap();
        let last = dir.path().join("ultty/sessions/last.json");
        std::fs::create_dir_all(last.parent().unwrap()).unwrap();
        std::fs::write(&last, "the user's tabs").unwrap();

        let command = StartupCommand::Exec {
            program: "sleep".to_owned(),
            args: vec!["5".to_owned()],
        };
        let manager =
            TerminalManager::with_command_tab(80, 24, "sleep", &["5".to_owned()], &mut no_tab_env)
                .unwrap();
        let file = session_file(Some(&command), Some(last.clone()));
        save_session(&manager, file.as_deref());
        assert_eq!(std::fs::read_to_string(&last).unwrap(), "the user's tabs");

        // A window started without one saves over it, as before.
        let file = session_file(None, Some(last.clone()));
        save_session(&manager, file.as_deref());
        let saved = std::fs::read_to_string(&last).unwrap();
        assert!(saved.contains("\"tabs\""), "{saved}");
    }

    #[test]
    fn test_named_profile_spawns_configured_panes_with_exact_arguments() {
        use ultty::config::{Config, LaunchProfile, PaneConfig, TabConfig};

        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        let arguments = |file: &std::path::Path, value: &str| {
            vec![
                "-c".to_owned(),
                "printf '%s\\n' \"$2\" > \"$1\"; exec sleep 30".to_owned(),
                "profile-test".to_owned(),
                file.to_string_lossy().into_owned(),
                value.to_owned(),
            ]
        };
        let profile = LaunchProfile {
            tabs: vec![TabConfig {
                command: Some("/bin/sh".to_owned()),
                args: arguments(&first, "first argument with spaces"),
                panes: vec![PaneConfig {
                    command: Some("/bin/sh".to_owned()),
                    args: arguments(&second, "literal $HOME; 'quote'"),
                    split: Some("vertical".to_owned()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut config = Config::default();
        config.behavior.restore_session = true;
        config
            .profiles
            .profiles
            .insert("remote".to_owned(), profile);
        let choice = super::StartupCommand::Profile("remote".to_owned());
        let manager =
            super::App::create_terminal_manager(&config, 80, 24, &mut None, Some(&choice)).unwrap();
        assert_eq!(manager.tab_count(), 1);
        // The configured command is the first child: no throwaway shell/job.
        assert_eq!(manager.tabs()[0].terminals[0].raw(), 0);
        assert_eq!(manager.pane_count(), 2);
        let rects = manager.active_pane_rects(800.0, 600.0);
        assert_eq!(rects[0].1, (0.0, 0.0, 800.0, 300.0));
        assert_eq!(rects[1].1, (0.0, 300.0, 800.0, 300.0));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&first).ok().as_deref()
            != Some("first argument with spaces\n")
            || std::fs::read_to_string(&second).ok().as_deref() != Some("literal $HOME; 'quote'\n")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "profile children did not run"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(first).unwrap(),
            "first argument with spaces\n"
        );
        assert_eq!(
            std::fs::read_to_string(second).unwrap(),
            "literal $HOME; 'quote'\n"
        );
    }

    #[test]
    fn test_profile_validation_precedes_spawning_and_restoration() {
        use ultty::config::{Config, LaunchProfile, PaneConfig, TabConfig, TerminalConfig};

        let mut config = Config::default();
        config.behavior.restore_session = true;
        let missing = super::StartupCommand::Profile("missing".to_owned());
        let result =
            super::App::create_terminal_manager(&config, 80, 24, &mut None, Some(&missing));
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("Unknown launch profile"));
        let profile = LaunchProfile {
            tabs: vec![TabConfig {
                panes: vec![PaneConfig {
                    split: Some("typo".to_owned()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = super::App::create_manager_from_profile(
            &profile,
            &TerminalConfig::default(),
            80,
            24,
            &mut None,
        );
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("Unknown profile split direction"));
    }

    #[test]
    fn maintenance_poll_interval_is_not_frame_cadence() {
        assert_eq!(MAINTENANCE_POLL_INTERVAL, Duration::from_millis(250));
    }

    fn feed_terminal(term: &mut ultty::grid::Terminal, bytes: &[u8]) {
        let mut parser = ultty_vt::ansi::AnsiParser::new();
        let mut handler = ultty_vt::grid::TerminalHandler::new(term);
        parser.feed(bytes, |action| handler.handle(action));
    }

    #[test]
    fn extract_vim_selection_reads_scrollback_and_screen_by_absolute_row() {
        use ultty::grid::{Dimensions, Terminal};
        use ultty::vim::Selection as VimSelection;

        let mut term = Terminal::new(Dimensions::new(20, 4));
        for i in 0..10 {
            let line = format!("row-{i:02}-abcdefgh\r\n");
            feed_terminal(&mut term, line.as_bytes());
        }
        let buf = term.buffer();
        assert!(buf.history_len() >= 6);

        let screen_first = buf.history_len();
        assert_eq!(
            extract_vim_selection_text(
                buf,
                &VimSelection::Line {
                    start_row: screen_first,
                    end_row: screen_first,
                }
            ),
            format!("row-{screen_first:02}-abcdefgh")
        );
        assert_eq!(
            extract_vim_selection_text(
                buf,
                &VimSelection::Line {
                    start_row: 1,
                    end_row: 2,
                }
            ),
            "row-01-abcdefgh\nrow-02-abcdefgh"
        );
        assert_eq!(
            extract_vim_selection_text(
                buf,
                &VimSelection::Char {
                    start_row: screen_first,
                    start_col: 4,
                    end_row: screen_first,
                    end_col: 9,
                }
            ),
            format!("{screen_first:02}-abc")
        );
        assert_eq!(
            extract_vim_selection_text(
                buf,
                &VimSelection::Block {
                    start_row: screen_first,
                    start_col: 0,
                    end_row: screen_first + 1,
                    end_col: 5,
                }
            ),
            format!("row-{screen_first:02}\nrow-{:02}", screen_first + 1)
        );
    }

    #[test]
    fn extract_vim_selection_joins_soft_wrapped_lines() {
        use ultty::grid::{Dimensions, Terminal};
        use ultty::vim::Selection as VimSelection;

        let mut term = Terminal::new(Dimensions::new(10, 4));
        for i in 0..6 {
            feed_terminal(&mut term, format!("hdr-{i}\r\n").as_bytes());
        }
        feed_terminal(&mut term, b"0123456789abcdef\r\n");
        let buf = term.buffer();
        let wrap_start = buf.total_lines() - 3;
        assert_eq!(
            extract_vim_selection_text(
                buf,
                &VimSelection::Line {
                    start_row: wrap_start,
                    end_row: wrap_start + 1,
                }
            ),
            "0123456789abcdef"
        );
    }

    fn hit(x: f32, y: f32, scale: f32) -> Option<ResizeDirection> {
        resize_direction_at_window_edge(x, y, 800.0, 600.0, scale, 0.0, 0.0)
    }

    fn hit_grid(
        x: f32,
        y: f32,
        scale: f32,
        grid_top: f32,
        cell_width: f32,
    ) -> Option<ResizeDirection> {
        resize_direction_at_window_edge(x, y, 800.0, 600.0, scale, grid_top, cell_width)
    }

    #[test]
    fn resize_edges_cover_a_band_inside_and_outside_the_window() {
        assert_eq!(hit(8.0, 300.0, 1.0), Some(ResizeDirection::West));
        assert_eq!(hit(-8.0, 300.0, 1.0), Some(ResizeDirection::West));
        assert_eq!(hit(792.0, 300.0, 1.0), Some(ResizeDirection::East));
        assert_eq!(hit(400.0, 0.0, 1.0), Some(ResizeDirection::North));
        assert_eq!(hit(400.0, 599.0, 1.0), Some(ResizeDirection::South));
        assert_eq!(hit(9.0, 300.0, 1.0), None);
        assert_eq!(hit(-9.0, 300.0, 1.0), None);
        assert_eq!(hit(400.0, 300.0, 1.0), None);
    }

    #[test]
    fn resize_corners_reach_along_both_edges() {
        // 32 px along the top edge from the top-left corner is still diagonal.
        assert_eq!(hit(32.0, 4.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(4.0, 32.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(33.0, 4.0, 1.0), Some(ResizeDirection::North));
        // The corner square reaches past the edge band.
        assert_eq!(hit(14.0, 14.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(15.0, 15.0, 1.0), None);
        // Every corner, including outside the window.
        assert_eq!(hit(780.0, 2.0, 1.0), Some(ResizeDirection::NorthEast));
        assert_eq!(hit(2.0, 580.0, 1.0), Some(ResizeDirection::SouthWest));
        assert_eq!(hit(790.0, 590.0, 1.0), Some(ResizeDirection::SouthEast));
        assert_eq!(hit(806.0, 606.0, 1.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn resize_zones_scale_with_the_display() {
        // At 2x, the 8 px logical band is 16 physical pixels.
        assert_eq!(hit(16.0, 300.0, 2.0), Some(ResizeDirection::West));
        assert_eq!(hit(16.0, 300.0, 1.0), None);
        assert_eq!(hit(64.0, 8.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(hit(28.0, 28.0, 2.0), Some(ResizeDirection::NorthWest));
    }

    #[test]
    fn resize_side_band_leaves_column_zero_clickable_in_grid() {
        assert_eq!(
            hit_grid(0.0, 300.0, 1.0, 34.0, 7.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            hit_grid(1.0, 300.0, 1.0, 34.0, 7.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            hit_grid(-4.0, 300.0, 1.0, 34.0, 7.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(hit_grid(2.0, 300.0, 1.0, 34.0, 7.0), None);
        assert_eq!(hit_grid(3.5, 300.0, 1.0, 34.0, 7.0), None);
        assert_eq!(hit_grid(6.0, 300.0, 1.0, 34.0, 7.0), None);
        assert_eq!(
            hit_grid(8.0, 10.0, 1.0, 34.0, 7.0),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(hit_grid(794.5, 300.0, 1.0, 34.0, 7.0), None);
        assert_eq!(
            hit_grid(799.0, 300.0, 1.0, 34.0, 7.0),
            Some(ResizeDirection::East)
        );
        assert_eq!(
            hit_grid(3.0, 300.0, 2.0, 68.0, 14.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(hit_grid(7.0, 300.0, 2.0, 68.0, 14.0), None);
        assert_eq!(
            hit_grid(2.0, 595.0, 1.0, 34.0, 7.0),
            Some(ResizeDirection::SouthWest)
        );
        assert_eq!(hit_grid(3.5, 575.0, 1.0, 34.0, 7.0), None);
    }

    #[test]
    fn square_corners_for_maximized_windows() {
        assert!(window_should_use_square_corners(
            PhysicalSize::new(800, 600),
            None,
            true
        ));
    }

    #[test]
    fn square_corners_for_half_and_quarter_monitor_sizes() {
        let monitor = Some(PhysicalSize::new(1920, 1080));

        assert!(window_should_use_square_corners(
            PhysicalSize::new(960, 1080),
            monitor,
            false
        ));
        assert!(window_should_use_square_corners(
            PhysicalSize::new(1920, 540),
            monitor,
            false
        ));
        assert!(window_should_use_square_corners(
            PhysicalSize::new(960, 540),
            monitor,
            false
        ));
    }

    #[test]
    fn square_corners_tolerate_os_panel_offsets() {
        assert!(window_should_use_square_corners(
            PhysicalSize::new(960, 1032),
            Some(PhysicalSize::new(1920, 1080)),
            false
        ));
    }

    #[test]
    fn rounded_corners_for_freeform_window_sizes() {
        assert!(!window_should_use_square_corners(
            PhysicalSize::new(1000, 700),
            Some(PhysicalSize::new(1920, 1080)),
            false
        ));
        assert!(!window_should_use_square_corners(
            PhysicalSize::new(1000, 700),
            None,
            false
        ));
    }
}
