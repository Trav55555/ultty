//! Terminal application state and main loop.

mod actions;
mod blink;
mod control;
mod control_events;
mod control_wait;
mod input;
mod render;
mod update;
mod wakeup;
use anyhow::Result;
use arboard::Clipboard;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ultty::config::Config;
use ultty::config_watcher::ConfigWatcher;
use ultty::control::ControlServer;
use ultty::protocol::ProtocolServer;
use ultty::renderer::Renderer;
use ultty::tabs::TabBarState;
use ultty::terminal_manager::{TabId, TerminalManager};
use ultty::url::UrlDetector;
use ultty::vim::{CopyMode, Selection as VimSelection};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

/// Default window dimensions.
const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;

/// Time window for detecting double/triple clicks.
pub(super) const CLICK_THRESHOLD: Duration = Duration::from_millis(500);

// Client-side resize zones, in logical pixels (scaled by the window's
// scale factor). Corners are the most-wanted and most-missed target, so they
// are generous: the edge band counts as a corner for CORNER_ARM along each
// edge, and a CORNER_SQUARE at the very corner reaches further in.

/// Width of the resize band just inside each window edge.
const WINDOW_RESIZE_EDGE_BAND: f32 = 8.0;

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
#[derive(Debug, Clone, Copy)]
enum AppEvent {
    /// At least one runtime fd is readable and should be drained on the UI thread.
    RuntimeReadable,
}

#[derive(Debug, Clone, Copy)]
struct TabWidthFreeze {
    width: f32,
    until: Instant,
}

/// Terminal application.
pub struct App {
    /// Window
    window: Arc<Window>,
    /// wgpu surface
    surface: wgpu::Surface<'static>,
    /// GPU renderer
    renderer: Renderer,
    /// Terminal manager (handles multiple tabs/panes)
    terminal_manager: TerminalManager,
    /// Tab bar state for rendering
    tab_bar: TabBarState,
    /// Temporarily fixed tab width after a close, so close buttons stay aligned.
    tab_width_freeze: Option<TabWidthFreeze>,
    /// Last render time for frame limiting
    last_render: Instant,
    /// When input was last sent to the focused terminal, until a frame has
    /// drawn the output that followed it.
    input_sent_at: Option<Instant>,
    /// The focused terminal produced output after `input_sent_at`.
    output_since_input: bool,
    /// Last time best-effort tab display titles were refreshed.
    last_tab_title_refresh: Instant,
    /// The latest PTY output not yet followed by a settled tab title
    /// refresh; see `update::title_activity_after_refresh`.
    tab_title_activity: Option<Instant>,
    /// Whether we need to redraw
    needs_redraw: bool,
    /// Pending close request (from window control button).
    pending_close: bool,
    /// Current modifier state
    modifiers: ModifiersState,
    /// Clipboard
    clipboard: Option<Clipboard>,
    /// Current mouse position in pixels
    mouse_pos: PhysicalPosition<f64>,
    /// Whether left mouse button is pressed (for selection drag)
    mouse_pressed: bool,
    /// Last click time for double/triple click detection
    last_click_time: Instant,
    /// Click count (1=single, 2=double, 3=triple)
    click_count: u8,
    /// Last click position for multi-click detection
    last_click_pos: (u16, u16),
    /// Last click time in the title band's maximize/restore toggle area.
    last_title_bar_toggle_click_time: Instant,
    /// Configuration
    config: Config,
    /// Vim-style copy mode
    copy_mode: CopyMode,
    /// URL detector
    url_detector: UrlDetector,
    /// Config file watcher for hot-reload
    config_watcher: Option<ConfigWatcher>,
    /// Visual bell state (time when bell was triggered, None if not active)
    bell_active_until: Option<Instant>,
    /// Pane divider dragging state: (divider_index, last_position)
    divider_dragging: Option<(ultty::pane_layout::Divider, f32)>,
    /// Keyboard shortcuts overlay visible
    shortcuts_visible: bool,
    /// Native protocol status overlay visible
    protocol_status_visible: bool,
    /// Cached list of keybindings for display
    shortcuts_list: Vec<(String, String)>,
    /// The focused cursor's blink phase.
    cursor_blink: blink::CursorBlink,
    /// Auto-dump countdown (frames remaining until dump, 0 = disabled)
    auto_dump_frames: u32,
    /// Frames drawn so far (saturating); marks the first in the startup log.
    frames_drawn: u32,
    /// Native protocol server for TUI apps
    protocol_server: Option<ProtocolServer>,
    /// Control server for CLI automation (ulttyctl)
    control_server: Option<ControlServer>,
    /// Control wait requests that are checked after terminal output is drained.
    pending_control_waits: Vec<control_wait::PendingControlWait>,
    /// Panes subscribers were last told about, for pane opened/closed events.
    published_panes: Vec<ultty::terminal_manager::TerminalId>,
    /// Cross-thread wakeups from PTY readiness into winit.
    socket_wakeup: wakeup::SocketWakeup,
    /// Search matches for native protocol regions
    native_search_matches: Vec<ultty::scene::SearchMatch>,
    /// Whether native search overlay is active
    native_search_active: bool,
    /// Current search query for native sessions
    native_search_query: String,
    /// Current highlighted match index
    native_search_index: usize,
    /// Reusable buffer for URL detection row text (avoids per-frame allocation)
    url_row_buf: Vec<String>,
    /// Reusable buffer for URL detection hyperlink grid
    url_hyperlink_grid: Vec<Vec<u32>>,
    /// Reusable buffer for URL detection hyperlink map
    url_hyperlink_map: std::collections::HashMap<u32, String>,
    /// Accumulated pixel scroll delta for smooth scrolling on Wayland/HiDPI
    scroll_pixel_accum: f64,
    scroll_line_accum: f64,
    /// HiDPI scale factor (physical pixels / logical pixels); kept in step
    /// with the window by `WindowEvent::ScaleFactorChanged`.
    scale_factor: f32,
    /// The minimum inner size last sent to the window.
    min_window_size: Option<PhysicalSize<u32>>,
    /// When the window opened; drives the title's typing animation.
    launched_at: Instant,
    /// Text the input method is composing (empty when it is not). While
    /// it is non-empty, keys belong to the input method.
    ime_preedit: String,
    /// The cursor rectangle last given to the input method, in window
    /// pixels, so it is only sent again when the cursor moves.
    ime_cursor_area: Option<[i32; 4]>,
}

impl App {
    /// Create a new application with window.
    fn new(event_loop: &ActiveEventLoop, event_proxy: EventLoopProxy<AppEvent>) -> Result<Self> {
        // Load configuration
        let config = Config::load();
        tracing::info!("Loaded configuration");

        // Create window with transparency support if opacity < 1
        let attributes = Window::default_attributes()
            .with_title("Ultty")
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
            .with_min_inner_size(LogicalSize::new(200.0, 100.0))
            .with_transparent(true)
            .with_decorations(false);
        // The Wayland app id and X11 WM_CLASS: desktops match it to
        // ultty.desktop (StartupWMClass=ultty) to show Ultty's icon and name.
        // Wayland had no app id at all, so GNOME showed a generic icon.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(
            attributes, "ultty", "ultty",
        );
        let window = event_loop.create_window(attributes)?;
        let window = Arc::new(window);
        tracing::debug!(target: "ultty::startup", "window created");
        // winit leaves input methods off; without this, fcitx and ibus
        // users cannot type Chinese, Japanese or Korean.
        window.set_ime_allowed(true);

        let size = window.inner_size();
        let scale_factor = window.scale_factor() as f32;

        // Vulkan (or the platform's primary API) first, GL only if that
        // finds no adapter. An instance with every backend brings up GL as
        // well, which loads and initializes the EGL driver even when Vulkan
        // is then chosen: startup time and resident memory for nothing.
        let (surface, adapter) = match request_gpu(event_loop, &window, wgpu::Backends::PRIMARY) {
            Ok(gpu) => gpu,
            Err(primary) => {
                tracing::info!("No primary GPU backend ({primary:#}); trying GL");
                request_gpu(event_loop, &window, wgpu::Backends::SECONDARY)
                    .map_err(|gl| anyhow::anyhow!("{primary:#}; with GL: {gl:#}"))?
            }
        };
        tracing::debug!(target: "ultty::startup", "GPU adapter ready");

        // Scale font size to physical pixels so cosmic-text metrics match
        // the orthographic projection (which uses physical window dimensions).
        let mut scaled_font = config.font.clone();
        scaled_font.size *= scale_factor;

        // Create renderer with config
        let mut renderer = pollster::block_on(Renderer::with_config(
            &surface,
            &adapter,
            size.width,
            size.height,
            &config.colors,
            &scaled_font,
        ))?;

        tracing::debug!(target: "ultty::startup", "renderer ready");
        // Set window opacity
        renderer.set_opacity(config.behavior.opacity);

        // Create tab bar state at the window's scale. It is always visible
        // (CSD), so set its height before sizing the terminal.
        let mut tab_bar = TabBarState::new();
        tab_bar.scale = scale_factor;
        // The title types itself out at launch, starting from `->>>_`.
        tab_bar.title_letters = 0;
        renderer.set_tab_bar_height(tab_bar.content_top());

        // Get terminal dimensions from renderer
        let (cols, rows) = renderer.terminal_dimensions();
        tracing::info!("Terminal size: {}x{}", cols, rows);

        // Start the native protocol server BEFORE spawning shells: each tab
        // gets its own listening socket, and the tab's shell is told about it
        // through ULTTY_SOCKET at spawn time.
        let mut protocol_server = Self::start_protocol_server();
        let control_server = Self::start_control_server();

        // Create terminal manager - with launch profile if configured
        let mut terminal_manager =
            Self::create_terminal_manager(&config, cols, rows, &mut protocol_server)?;
        tracing::debug!(target: "ultty::startup", "shell started");
        terminal_manager.set_theme_colors(config.colors.theme_colors());
        terminal_manager.set_scrollback_lines(config.behavior.scrollback_lines);
        terminal_manager.set_close_on_exit(config.behavior.close_on_exit);
        // Each terminal's I/O thread parses on its own and pokes the UI
        // through this proxy when there is something new to draw.
        let pty_proxy = std::sync::Mutex::new(event_proxy.clone());
        let published_panes = control_events::open_panes(&terminal_manager)
            .into_iter()
            .map(|(pane, _)| pane)
            .collect();
        terminal_manager.set_waker(std::sync::Arc::new(move || {
            if let Ok(proxy) = pty_proxy.lock() {
                let _ = proxy.send_event(AppEvent::RuntimeReadable);
            }
        }));
        let socket_proxy = event_proxy.clone();
        let socket_wakeup = wakeup::SocketWakeup::new(move || {
            socket_proxy.send_event(AppEvent::RuntimeReadable).is_ok()
        });

        // Try to create clipboard
        let clipboard = Clipboard::new().ok();
        if clipboard.is_none() {
            tracing::warn!("Failed to initialize clipboard");
        }

        // Create URL detector
        let url_detector = UrlDetector::new(config.behavior.url_detection);

        // Create config watcher for hot-reload
        let config_watcher = ConfigWatcher::new(move || {
            let _ = event_proxy.send_event(AppEvent::RuntimeReadable);
        });
        if config_watcher.is_some() {
            tracing::info!("Config hot-reload enabled");
        }

        // Build shortcuts list before moving config
        let shortcuts_list = Self::build_shortcuts_list(&config.keybindings);

        let mut app = Self {
            window,
            surface,
            renderer,
            terminal_manager,
            tab_bar,
            tab_width_freeze: None,
            last_render: Instant::now(),
            input_sent_at: None,
            output_since_input: false,
            last_tab_title_refresh: Instant::now() - TAB_TITLE_REFRESH_INTERVAL,
            tab_title_activity: None,
            needs_redraw: true,
            pending_close: false,
            modifiers: ModifiersState::empty(),
            clipboard,
            mouse_pos: PhysicalPosition::new(0.0, 0.0),
            mouse_pressed: false,
            last_click_time: Instant::now() - CLICK_THRESHOLD * 2,
            click_count: 0,
            last_click_pos: (0, 0),
            last_title_bar_toggle_click_time: Instant::now() - CLICK_THRESHOLD * 2,
            config,
            copy_mode: CopyMode::new(),
            url_detector,
            config_watcher,
            bell_active_until: None,
            divider_dragging: None,
            shortcuts_visible: false,
            protocol_status_visible: false,
            shortcuts_list,
            cursor_blink: blink::CursorBlink::new(),
            // Check for auto-dump environment variable (number of frames to wait)
            frames_drawn: 0,
            auto_dump_frames: std::env::var("ULTTY_AUTO_DUMP")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            protocol_server,
            control_server,
            pending_control_waits: Vec::new(),
            published_panes,
            socket_wakeup,
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
            ime_preedit: String::new(),
            ime_cursor_area: None,
            launched_at: Instant::now(),
        };

        // Initialize tab bar state so it renders on first frame
        app.update_tab_bar();
        app.refresh_runtime_wakeup_fds();

        Ok(app)
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
    /// Every tab created here gets a listening socket on `protocol_server`
    /// before its shell is spawned.
    fn create_terminal_manager(
        config: &Config,
        cols: u16,
        rows: u16,
        protocol_server: &mut Option<ProtocolServer>,
    ) -> Result<TerminalManager> {
        // Try to restore session if enabled
        if config.behavior.restore_session {
            if let Some(session) = ultty::session::SessionData::load_last() {
                tracing::info!(
                    "Restoring previous session with {} tabs",
                    session.tabs.len()
                );
                match Self::create_manager_from_session(
                    &session,
                    &config.terminal.term,
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
                    &config.terminal.term,
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
            tab_shell_env(protocol_server, &config.terminal.term, tab_id)
        })
        .map_err(|e| anyhow::anyhow!("Failed to create terminal manager: {}", e))
    }

    /// Create terminal manager from a launch profile.
    fn create_manager_from_profile(
        profile: &ultty::config::LaunchProfile,
        term: &str,
        cols: u16,
        rows: u16,
        protocol_server: &mut Option<ProtocolServer>,
    ) -> Result<TerminalManager> {
        // Create an empty manager first
        let mut manager = TerminalManager::with_tab_env(cols, rows, &mut |tab_id| {
            tab_shell_env(protocol_server, term, tab_id)
        })
        .map_err(|e| anyhow::anyhow!("Failed to create terminal manager: {}", e))?;

        // If profile has tabs, close the default tab and create configured ones
        if !profile.tabs.is_empty() {
            // Close the default tab that was created
            if let Some(tab) = manager.active_tab() {
                let tab_id = tab.id;
                manager.close_tab(tab_id);
                if let Some(server) = protocol_server.as_mut() {
                    server.close_tab(tab_id);
                }
            }

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
                        &mut |tab_id| tab_shell_env(protocol_server, term, tab_id),
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to create tab: {}", e))?;

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
        term: &str,
        cols: u16,
        rows: u16,
        protocol_server: &mut Option<ProtocolServer>,
    ) -> Result<TerminalManager> {
        TerminalManager::from_session(session, cols, rows, &mut |tab_id| {
            tab_shell_env(protocol_server, term, tab_id)
        })
        .map_err(|e| anyhow::anyhow!("Failed to restore session: {}", e))
    }

    /// Save the current session.
    fn save_session(&self) {
        let session = self.terminal_manager.to_session_data();
        if let Err(e) = session.save_as_last() {
            tracing::warn!("Failed to save session: {}", e);
        } else {
            tracing::info!("Session saved");
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

    /// Create a new tab whose shell is wired to its own protocol socket.
    fn create_tab(&mut self) -> std::io::Result<TabId> {
        self.terminal_manager.create_tab_with_env(&mut |tab_id| {
            tab_shell_env(
                &mut self.protocol_server,
                &self.config.terminal.term,
                tab_id,
            )
        })
    }

    /// Create a new tab running `command`, wired to its own protocol socket.
    fn create_tab_with_command(
        &mut self,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> std::io::Result<TabId> {
        self.terminal_manager.create_tab_with_command(
            command,
            args,
            working_dir,
            title,
            &mut |tab_id| {
                tab_shell_env(
                    &mut self.protocol_server,
                    &self.config.terminal.term,
                    tab_id,
                )
            },
        )
    }

    /// Keep the protocol server's view of tabs in step with the terminal
    /// manager: the visible tab owns keyboard input and clipboard access, and
    /// tabs that no longer exist lose their socket and sessions.
    fn sync_protocol_tabs(&mut self) {
        let Some(server) = self.protocol_server.as_mut() else {
            return;
        };
        let tabs = self.terminal_manager.tabs();
        let closed: Vec<TabId> = server
            .open_tabs()
            .filter(|open| !tabs.iter().any(|tab| tab.id == *open))
            .collect();
        for tab_id in closed {
            server.close_tab(tab_id);
        }
        server.set_active_tab(self.terminal_manager.active_tab().map(|tab| tab.id));
    }

    /// Run the application.
    pub fn run() -> Result<()> {
        let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
        let mut handler = AppHandler {
            proxy: event_loop.create_proxy(),
            app: None,
            init_error: None,
        };
        event_loop.run_app(&mut handler)?;
        handler.init_error.map_or(Ok(()), Err)
    }

    /// Run pending terminal work, then sleep until the next deadline.
    fn update_and_schedule(&mut self, event_loop: &ActiveEventLoop) {
        if self.update() {
            event_loop.exit();
        }
        // The sockets have been drained; let them wake the loop again.
        self.socket_wakeup.rearm();
        event_loop.set_control_flow(control_flow(self.next_update_deadline(Instant::now())));
    }

    /// Get the terminal state of the focused terminal.
    fn terminal(&self) -> Option<ultty::event_loop::TerminalGuard<'_>> {
        self.terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
    }

    /// Process group in the foreground of the focused pane's PTY: the only
    /// program whose native sessions receive what the user types.
    fn focused_foreground_pgrp(&self) -> Option<i32> {
        self.terminal_manager
            .focused_terminal()
            .and_then(|terminal| terminal.event_loop.foreground_pgrp())
    }

    /// Get mutable terminal state of the focused terminal.
    fn terminal_mut(&mut self) -> Option<ultty::event_loop::TerminalGuard<'_>> {
        self.terminal_manager
            .focused_terminal_mut()
            .map(|t| t.event_loop.terminal_mut())
    }

    /// Send input to the focused terminal.
    fn send_input(&mut self, data: &[u8]) -> std::io::Result<()> {
        if let Some(terminal) = self.terminal_manager.focused_terminal_mut() {
            self.input_sent_at = Some(Instant::now());
            self.output_since_input = false;
            terminal.event_loop.send_input(data)
        } else {
            Ok(())
        }
    }

    /// Send a report the terminal generates on its own (mouse motion,
    /// focus changes). Unlike [`Self::send_input`] it does not mark the
    /// output that follows as an echo to draw at once: a moving mouse in
    /// vim or htop would otherwise force a frame for every motion event.
    fn send_report(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self.terminal_manager.focused_terminal_mut() {
            Some(terminal) => terminal.event_loop.send_input(data),
            None => Ok(()),
        }
    }

    /// Update the tab bar state from the terminal manager.
    ///
    /// This is the single hook every tab change goes through, so it also
    /// syncs the protocol server's tab bookkeeping.
    /// Show the focused pane's directory and git branch in the title band.
    fn update_title_location(&mut self) {
        let cwd = self
            .terminal_manager
            .focused_terminal()
            .and_then(control::terminal_working_directory);
        let home = std::env::var("HOME").ok();
        self.tab_bar.title_path = cwd
            .as_deref()
            .map(|dir| ultty::tabs::display_path(dir, home.as_deref()))
            .unwrap_or_default();
        self.tab_bar.title_branch = cwd
            .as_deref()
            .and_then(|dir| ultty::tabs::git_branch(std::path::Path::new(dir)));
        self.update_min_window_size();
    }

    /// Keep the window from shrinking below what the title and three rows
    /// need. Only tells the window when the size actually changes.
    fn update_min_window_size(&mut self) {
        let (cell_width, cell_height) = self.renderer.cell_dimensions();
        let (width, height) = self.tab_bar.min_window_size(cell_width, cell_height);
        let size = PhysicalSize::new(width as u32, height as u32);
        if self.min_window_size != Some(size) {
            self.min_window_size = Some(size);
            self.window.set_min_inner_size(Some(size));
        }
    }

    fn update_tab_bar(&mut self) {
        self.sync_protocol_tabs();
        let now = Instant::now();
        self.last_tab_title_refresh = now;
        self.tab_title_activity =
            update::title_activity_after_refresh(self.tab_title_activity, now);

        let total_width = self.renderer.surface_size().0 as f32;
        let active_tab_id = self
            .terminal_manager
            .active_tab()
            .map(|t| t.id)
            .unwrap_or_else(|| TabId::new(0));

        let tab_width_override = self.active_tab_width_freeze().map(|freeze| freeze.width);
        let tab_titles = self.terminal_manager.tab_display_titles();

        self.tab_bar.update_with_titles_and_tab_width_override(
            self.terminal_manager.tabs(),
            &tab_titles,
            active_tab_id,
            total_width,
            tab_width_override,
        );
        self.update_title_location();
        self.update_tab_statuses();

        // Update renderer's tab bar height for dimension calculations
        let new_height = self.tab_bar.content_top();
        let old_height = self.renderer.tab_bar_height();

        if (new_height - old_height).abs() > 0.1 {
            self.renderer.set_tab_bar_height(new_height);

            // The area under the bar changed: re-size every pane's grid.
            self.relayout_terminals();
        }

        self.refresh_runtime_wakeup_fds();
    }

    /// Show each tab's most urgent pane status, and the focused pane's in
    /// the title band.
    fn update_tab_statuses(&mut self) {
        let manager = &self.terminal_manager;
        for (state, tab) in self.tab_bar.tabs.iter_mut().zip(manager.tabs()) {
            state.status = tab
                .terminals
                .iter()
                .filter_map(|&pane| manager.terminal(pane)?.status.as_ref())
                .max_by_key(|status| status.urgency())
                .cloned();
        }
        self.tab_bar.title_status = manager
            .focused_terminal()
            .and_then(|terminal| terminal.status.clone());
    }

    fn freeze_tab_width_after_close(&mut self) {
        let Some(width) = self.tab_bar.tabs.first().map(|tab| tab.width) else {
            return;
        };

        self.tab_width_freeze = Some(TabWidthFreeze {
            width,
            until: Instant::now() + TAB_CLOSE_WIDTH_FREEZE_DURATION,
        });
    }

    fn clear_tab_width_freeze(&mut self) {
        self.tab_width_freeze = None;
    }

    fn active_tab_width_freeze(&self) -> Option<TabWidthFreeze> {
        self.tab_width_freeze
            .filter(|freeze| Instant::now() < freeze.until)
    }

    fn expire_tab_width_freeze(&mut self, now: Instant) {
        if self
            .tab_width_freeze
            .is_some_and(|freeze| now >= freeze.until)
        {
            self.tab_width_freeze = None;
            self.update_tab_bar();
            self.needs_redraw = true;
        }
    }

    /// Socket fds the wakeup bridge watches. PTYs are not among them: each
    /// terminal's I/O thread wakes the UI itself.
    fn refresh_runtime_wakeup_fds(&mut self) {
        let mut readable = Vec::new();
        let mut writable = Vec::new();
        if let Some(server) = self.protocol_server.as_ref() {
            readable.extend(server.readiness_fds());
        }
        if let Some(server) = self.control_server.as_ref() {
            readable.extend(server.readiness_fds());
            writable.extend(server.writable_fds());
        }
        self.socket_wakeup.set_fds(readable, writable);
    }

    /// Extract text from a vim selection.
    fn extract_selection_text(&self, selection: &VimSelection) -> String {
        let Some(terminal) = self.terminal() else {
            return String::new();
        };
        let buffer = terminal.buffer();
        let mut text = String::new();

        match selection {
            VimSelection::Char {
                start_row,
                start_col,
                end_row,
                end_col,
            } => {
                for row in *start_row..=*end_row {
                    if let Some(grid_row) = buffer.visible_row(row as u16) {
                        let sc = if row == *start_row { *start_col } else { 0 };
                        let ec = if row == *end_row {
                            *end_col
                        } else {
                            grid_row.width() as usize - 1
                        };

                        if sc <= ec {
                            text.push_str(&grid_row.copy_text(sc as u16, ec as u16));
                        }
                        if row < *end_row {
                            text.push('\n');
                        }
                    }
                }
            }
            VimSelection::Line { start_row, end_row } => {
                for row in *start_row..=*end_row {
                    if let Some(grid_row) = buffer.visible_row(row as u16) {
                        text.push_str(grid_row.copy_text(0, grid_row.width()).trim_end());
                        if row < *end_row {
                            text.push('\n');
                        }
                    }
                }
            }
            VimSelection::Block {
                start_row,
                start_col,
                end_row,
                end_col,
            } => {
                for row in *start_row..=*end_row {
                    if let Some(grid_row) = buffer.visible_row(row as u16) {
                        if start_col <= end_col {
                            text.push_str(&grid_row.copy_text(*start_col as u16, *end_col as u16));
                        }
                        if row < *end_row {
                            text.push('\n');
                        }
                    }
                }
            }
        }

        text
    }

    /// Perform search in scrollback.
    fn perform_search(&mut self, query: &str) {
        if query.is_empty() {
            return;
        }

        let Some(terminal) = self.terminal() else {
            return;
        };
        let buffer = terminal.buffer();
        let mut matches = Vec::new();

        let total_lines = buffer.total_lines();
        for row in 0..total_lines {
            if let Some(grid_row) = buffer.row_at(row) {
                let text = grid_row.text_trimmed();
                // Case-insensitive search; columns are char indices into the
                // original row, not byte offsets into a lowercased copy.
                let query_lower = query.to_lowercase();
                for found in control_wait::find_ignore_case(&text, &query_lower) {
                    matches.push((row, found.char_col));
                }
            }
        }

        drop(terminal);

        tracing::debug!("Search '{}': found {} matches", query, matches.len());
        let has_matches = !matches.is_empty();
        self.copy_mode.set_search_matches(matches);

        // Jump to first match if any
        if has_matches {
            // Move cursor to first match
            // The copy_mode's goto_next_match will handle positioning
        }

        // Also search native sessions if active
        if self.native_search_active {
            self.perform_native_search(query);
        }
    }

    /// Check if the visible tab has any native protocol sessions.
    fn has_native_sessions(&self) -> bool {
        self.protocol_server
            .as_ref()
            .is_some_and(|s| s.active_tab_sessions().next().is_some())
    }

    /// Perform search across the visible tab's native protocol sessions.
    fn perform_native_search(&mut self, query: &str) {
        self.native_search_query = query.to_string();
        self.native_search_matches.clear();
        self.native_search_index = 0;

        if let Some(ref server) = self.protocol_server {
            for session in server.active_tab_sessions() {
                let matches = session.scene().search(query);
                self.native_search_matches.extend(matches);
            }
        }

        tracing::debug!(
            "Native search '{}': found {} matches",
            query,
            self.native_search_matches.len()
        );
    }

    /// Navigate to the next native search match.
    fn native_search_next(&mut self) {
        if self.native_search_matches.is_empty() {
            return;
        }
        self.native_search_index =
            (self.native_search_index + 1) % self.native_search_matches.len();
        self.needs_redraw = true;
    }

    /// Navigate to the previous native search match.
    fn native_search_prev(&mut self) {
        if self.native_search_matches.is_empty() {
            return;
        }
        if self.native_search_index == 0 {
            self.native_search_index = self.native_search_matches.len() - 1;
        } else {
            self.native_search_index -= 1;
        }
        self.needs_redraw = true;
    }

    /// Dismiss native search.
    fn dismiss_native_search(&mut self) {
        self.native_search_active = false;
        self.native_search_matches.clear();
        self.native_search_query.clear();
        self.native_search_index = 0;
        self.needs_redraw = true;
    }

    /// Update tab bar hover state.
    fn update_tab_hover(&mut self) {
        if !self.tab_bar.visible {
            return;
        }

        let x = self.mouse_pos.x as f32;
        let y = self.mouse_pos.y as f32;

        let old_hovered_tab = self.tab_bar.hovered_tab;
        let old_new_tab_hovered = self.tab_bar.new_tab_button_hovered;
        let old_window_control = self.tab_bar.hovered_title_button;
        let old_close_button = self.tab_bar.hovered_close_button;
        self.tab_bar.set_hover(x, y);
        let old_menu_item = self.tab_bar.hovered_menu_item;
        if self.tab_bar.menu_open {
            let (cell_width, cell_height) = self.renderer.cell_dimensions();
            self.tab_bar.hovered_menu_item = self
                .tab_bar
                .menu_layout(cell_width, cell_height)
                .item_at(x, y);
            if self.tab_bar.hovered_menu_item != old_menu_item {
                self.needs_redraw = true;
            }
        }

        // Redraw if hover state changed
        if old_hovered_tab != self.tab_bar.hovered_tab
            || old_new_tab_hovered != self.tab_bar.new_tab_button_hovered
            || old_window_control != self.tab_bar.hovered_title_button
            || old_close_button != self.tab_bar.hovered_close_button
        {
            self.needs_redraw = true;
        }
    }

    /// The client-side resize zone under physical (x, y), if any. Resizing
    /// wins over the chrome buttons in the edge band, so every corner can
    /// be grabbed; a maximized or screen-filling window has no zones, which
    /// keeps its screen-corner close button a full target.
    pub(super) fn resize_direction_at(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        let size = self.window.inner_size();
        let monitor_size = self.window.current_monitor().map(|monitor| monitor.size());
        if window_should_use_square_corners(size, monitor_size, self.window.is_maximized()) {
            return None;
        }
        resize_direction_at_window_edge(
            x,
            y,
            size.width as f32,
            size.height as f32,
            self.window.scale_factor() as f32,
        )
    }

    /// Update cursor icon when hovering a client-side resize zone.
    fn update_resize_cursor(&mut self) -> bool {
        let x = self.mouse_pos.x as f32;
        let y = self.mouse_pos.y as f32;
        let Some(direction) = self.resize_direction_at(x, y) else {
            return false;
        };

        let cursor = match direction {
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
            ResizeDirection::West | ResizeDirection::East => CursorIcon::EwResize,
            ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        };
        self.window.set_cursor(cursor);
        true
    }

    fn update_cursor_for_url(&mut self) {
        // Don't change cursor if hovering over tab bar
        if self.tab_bar.visible && (self.mouse_pos.y as f32) < self.tab_bar.height() {
            self.window.set_cursor(CursorIcon::Default);
            return;
        }

        if !self.url_detector.is_enabled() || !self.modifiers.control_key() {
            self.window.set_cursor(CursorIcon::Default);
            return;
        }

        // Account for tab bar offset
        let adjusted_y = self.mouse_pos.y - self.tab_bar.content_top() as f64;
        let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
        if self.url_detector.has_url_at(col, row) {
            self.window.set_cursor(CursorIcon::Pointer);
        } else {
            self.window.set_cursor(CursorIcon::Default);
        }
    }

    /// Update URL detection when terminal content changes.
    fn update_url_detection(&mut self) {
        if !self.url_detector.is_enabled() {
            return;
        }

        // Reuse buffers: clear and refill instead of allocating new Vecs each frame
        self.url_row_buf.clear();
        self.url_hyperlink_grid.clear();
        self.url_hyperlink_map.clear();

        // Access terminal_manager directly (not self.terminal()) to enable
        // disjoint field borrows — Rust can see url_row_buf/url_hyperlink_grid/
        // url_hyperlink_map are separate from terminal_manager.
        let Some(focused) = self.terminal_manager.focused_terminal() else {
            return;
        };
        let terminal = focused.event_loop.terminal();
        let dims = terminal.dimensions();

        // Fill row text buffer
        self.url_row_buf.reserve(dims.rows as usize);
        for row in 0..dims.rows {
            self.url_row_buf.push(
                terminal
                    .buffer()
                    .visible_row(row)
                    .map(|r| r.text_trimmed())
                    .unwrap_or_default(),
            );
        }

        // Fill hyperlink grid buffer
        self.url_hyperlink_grid.reserve(dims.rows as usize);
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
            self.url_hyperlink_grid.push(row_links);
        }

        // Fill hyperlink map buffer
        for (k, v) in terminal.all_hyperlinks().iter() {
            self.url_hyperlink_map.insert(*k, v.clone());
        }

        // Detect regex-based URLs
        let rows = &self.url_row_buf;
        self.url_detector.detect(rows.len() as u16, |row| {
            rows.get(row as usize).cloned().unwrap_or_default()
        });

        // Detect OSC 8 hyperlinks
        let grid = &self.url_hyperlink_grid;
        let map = &self.url_hyperlink_map;
        self.url_detector.detect_hyperlinks(
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

    /// Handle pending column resize requests (DECCOLM - 132/80 column mode).
    ///
    /// Some legacy applications use CSI ?3h to switch to 132-column mode
    /// and CSI ?3l to switch back to 80-column mode. This resizes the window
    /// to accommodate the requested column count.
    fn handle_column_resize_requests(&mut self) {
        // Check if focused terminal has a pending resize request
        if let Some(terminal) = self.terminal_manager.focused_terminal_mut() {
            if let Some(columns) = terminal
                .event_loop
                .terminal_mut()
                .take_column_resize_request()
            {
                // Calculate new window size based on column count
                let (cell_width, _) = self.renderer.cell_dimensions();
                let new_width = (columns as f32 * cell_width) as u32;
                let current_height = self.window.inner_size().height;

                // Request window resize
                let new_size = PhysicalSize::new(new_width, current_height);
                if self.window.request_inner_size(new_size).is_some() {
                    tracing::info!(
                        "Requested window resize to {} columns ({} pixels wide)",
                        columns,
                        new_width
                    );
                }

                self.needs_redraw = true;
            }
        }
    }

    /// Handle window resize.
    /// Size every pane's terminal, in every tab, to the space it is drawn
    /// in. Splits, pane closes and divider drags re-lay out from the same
    /// area inside the terminal manager.
    pub(super) fn relayout_terminals(&mut self) {
        let (width, height) = self.renderer.terminal_area();
        let (cell_width, cell_height) = self.renderer.cell_dimensions();
        let area = ultty::terminal_manager::PaneArea {
            width,
            height,
            cell_width,
            cell_height,
        };
        if let Err(e) = self.terminal_manager.layout_panes(area) {
            tracing::warn!("Failed to resize terminals: {}", e);
        }
    }

    /// The window moved to a display with another scale factor, or the
    /// display's scaling changed. Rescales the chrome and the font; winit
    /// follows this with a `Resized` event that resizes the surface.
    fn handle_scale_factor_changed(&mut self, scale_factor: f32) {
        if scale_factor <= 0.0 || (scale_factor - self.scale_factor).abs() < f32::EPSILON {
            return;
        }
        // Keep any zoom the user applied: the font stays the same size in
        // logical points.
        let font_size = self.renderer.font_size() / self.scale_factor * scale_factor;
        tracing::info!(
            "Scale factor {} -> {}, font size {:.1}px",
            self.scale_factor,
            scale_factor,
            font_size
        );
        self.scale_factor = scale_factor;
        self.tab_bar.scale = scale_factor;
        self.renderer.set_font_size(font_size);
        self.update_tab_bar();
        // Cell size changed: re-size every pane's grid.
        self.relayout_terminals();
        self.needs_redraw = true;
    }

    fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Resize renderer
        self.renderer.resize(&self.surface, size.width, size.height);
        self.update_window_corner_radius();
        self.clear_tab_width_freeze();
        self.update_tab_bar();

        // Get new terminal dimensions
        let (cols, rows) = self.renderer.terminal_dimensions();
        tracing::debug!("Resize: {}x{}", cols, rows);

        // Size each pane's terminal to its own share of the window.
        self.relayout_terminals();

        // Clamp copy mode cursor/scroll if active
        if self.copy_mode.is_active() {
            let total_rows = self
                .terminal()
                .map(|t| t.buffer().history_len() + t.dimensions().rows as usize)
                .unwrap_or(rows as usize);
            self.copy_mode
                .resize(total_rows, rows as usize, cols as usize);
        }

        if let Some(ref mut server) = self.protocol_server {
            server.set_viewport(cols as f32, rows as f32);
            server.broadcast_resize(cols, rows);
        }

        self.needs_redraw = true;
    }

    fn update_window_corner_radius(&mut self) {
        let size = self.window.inner_size();
        let monitor_size = self.window.current_monitor().map(|monitor| monitor.size());
        let square_corners =
            window_should_use_square_corners(size, monitor_size, self.window.is_maximized());
        let radius = if square_corners {
            0.0
        } else {
            WINDOW_CORNER_RADIUS
        };
        self.renderer.set_window_corner_radius(radius);
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

/// The resize direction for a pointer at physical (x, y) in a window of
/// physical size (width, height), or None away from the edges.
fn resize_direction_at_window_edge(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
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

    let on_edge = |d: f32| d <= band;
    let near_corner = |a: f32, b: f32| {
        (on_edge(a) && b <= arm) || (on_edge(b) && a <= arm) || (a <= square && b <= square)
    };

    if near_corner(from_left, from_top) {
        Some(ResizeDirection::NorthWest)
    } else if near_corner(from_right, from_top) {
        Some(ResizeDirection::NorthEast)
    } else if near_corner(from_left, from_bottom) {
        Some(ResizeDirection::SouthWest)
    } else if near_corner(from_right, from_bottom) {
        Some(ResizeDirection::SouthEast)
    } else if on_edge(from_left) {
        Some(ResizeDirection::West)
    } else if on_edge(from_right) {
        Some(ResizeDirection::East)
    } else if on_edge(from_top) {
        Some(ResizeDirection::North)
    } else if on_edge(from_bottom) {
        Some(ResizeDirection::South)
    } else {
        None
    }
}

/// Shell environment for a new tab: `TERM` (from `terminal.term`) and the
/// tab's own native protocol socket.
///
/// Opens the tab's listener on `server` before the shell is spawned so the
/// shell (and every app it launches) sees a socket that already exists. If
/// the listener cannot be opened the tab still works, without native
/// protocol support, and apps fall back to ANSI rendering.
fn tab_shell_env(
    server: &mut Option<ProtocolServer>,
    term: &str,
    tab_id: TabId,
) -> std::io::Result<ultty::terminal_manager::TabEnv> {
    let mut env = vec![("TERM".into(), term.into())];
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

/// A surface for `window` and an adapter that can draw to it, from an
/// instance limited to `backends`.
fn request_gpu(
    event_loop: &ActiveEventLoop,
    window: &Arc<Window>,
    backends: wgpu::Backends,
) -> Result<(wgpu::Surface<'static>, wgpu::Adapter)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        // GL presents through the display connection (Wayland needs it).
        display: Some(Box::new(event_loop.owned_display_handle())),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance.create_surface(Arc::clone(window))?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .map_err(|e| anyhow::anyhow!("Failed to find suitable GPU adapter: {e}"))?;
    Ok((surface, adapter))
}

/// Sleep until `deadline`, or with none, until an event arrives.
fn control_flow(deadline: Option<Instant>) -> ControlFlow {
    deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
}

/// Drives [`App`] from winit. The window, and so the app, is created on the
/// first `resumed`, as winit 0.30 requires.
struct AppHandler {
    proxy: EventLoopProxy<AppEvent>,
    app: Option<App>,
    /// A startup failure, returned from [`App::run`] after the loop exits.
    init_error: Option<anyhow::Error>,
}

impl ApplicationHandler<AppEvent> for AppHandler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }
        match App::new(event_loop, self.proxy.clone()) {
            Ok(mut app) => {
                app.update_tab_bar();
                app.relayout_terminals();
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
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(app) = self.app.as_mut() {
            if app.handle_window_event(event) {
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
        resize_direction_at_window_edge, window_should_use_square_corners,
        MAINTENANCE_POLL_INTERVAL,
    };

    #[test]
    fn maintenance_poll_interval_is_not_frame_cadence() {
        assert_eq!(MAINTENANCE_POLL_INTERVAL, Duration::from_millis(250));
    }

    fn hit(x: f32, y: f32, scale: f32) -> Option<ResizeDirection> {
        resize_direction_at_window_edge(x, y, 800.0, 600.0, scale)
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
