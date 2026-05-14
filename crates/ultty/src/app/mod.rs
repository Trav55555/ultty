//! Terminal application state and main loop.

mod actions;
mod control;
mod input;
mod render;
mod update;
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
use ultty::terminal_manager::TerminalManager;
use ultty::tmux::TmuxController;
use ultty::url::UrlDetector;
use ultty::vim::{CopyMode, Selection as VimSelection};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::Event;
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowBuilder};

/// Default window dimensions.
const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;

/// Time window for detecting double/triple clicks.
pub(super) const CLICK_THRESHOLD: Duration = Duration::from_millis(500);

/// Cursor blink interval (how long cursor stays on/off).
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Minimum spacing between rendered frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Maximum time to suppress drawing during synchronized update mode.
const SYNC_UPDATE_TIMEOUT: Duration = Duration::from_millis(200);

/// Maintenance wakeup while PTY and control sockets are not wired into winit wakeups.
const MAINTENANCE_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Faster PTY polling while a frame is dirty but not ready to render.
const PTY_CATCHUP_POLL_INTERVAL: Duration = Duration::from_millis(1);

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
    /// Last render time for frame limiting
    last_render: Instant,
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
    /// Scrollbar dragging state
    scrollbar_dragging: bool,
    /// Pane divider dragging state: (divider_index, last_position)
    divider_dragging: Option<(usize, f32)>,
    /// Keyboard shortcuts overlay visible
    shortcuts_visible: bool,
    /// Native protocol status overlay visible
    protocol_status_visible: bool,
    /// Cached list of keybindings for display
    shortcuts_list: Vec<(String, String)>,
    /// Whether cursor is currently visible in blink cycle
    cursor_blink_visible: bool,
    /// Last time cursor blink state changed
    last_cursor_blink: Instant,
    /// Tmux integration controller
    tmux: TmuxController,
    /// Auto-dump countdown (frames remaining until dump, 0 = disabled)
    auto_dump_frames: u32,
    /// Native protocol server for TUI apps
    protocol_server: Option<ProtocolServer>,
    /// Control server for CLI automation (ulttyctl)
    control_server: Option<ControlServer>,
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
    /// HiDPI scale factor (physical pixels / logical pixels)
    scale_factor: f32,
}

impl App {
    /// Create a new application with window.
    pub fn new(event_loop: &EventLoop<()>) -> Result<Self> {
        // Load configuration
        let config = Config::load();
        tracing::info!("Loaded configuration");

        // Create window with transparency support if opacity < 1
        let window = WindowBuilder::new()
            .with_title("Ultty")
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
            .with_min_inner_size(LogicalSize::new(200.0, 100.0))
            .with_transparent(true)
            .with_decorations(false)
            .build(event_loop)?;
        let window = Arc::new(window);

        let size = window.inner_size();
        let scale_factor = window.scale_factor() as f32;

        // Create wgpu instance
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Create surface
        let surface = instance.create_surface(window.clone())?;

        // Get adapter
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| anyhow::anyhow!("Failed to find suitable GPU adapter"))?;

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

        // Set window opacity
        renderer.set_opacity(config.behavior.opacity);

        // Tab bar is always visible (CSD), set its height before sizing the terminal
        renderer.set_tab_bar_height(ultty::tabs::TAB_BAR_HEIGHT);

        // Get terminal dimensions from renderer
        let (cols, rows) = renderer.terminal_dimensions();
        tracing::info!("Terminal size: {}x{}", cols, rows);

        // Start native protocol server BEFORE spawning shell so child processes
        // inherit ULTTY_SOCKET env var and can use native protocol
        let protocol_server = Self::start_protocol_server();
        let control_server = Self::start_control_server();

        // Create terminal manager - with launch profile if configured
        let terminal_manager = Self::create_terminal_manager(&config, cols, rows)?;

        // Try to create clipboard
        let clipboard = Clipboard::new().ok();
        if clipboard.is_none() {
            tracing::warn!("Failed to initialize clipboard");
        }

        // Create URL detector
        let url_detector = UrlDetector::new(config.behavior.url_detection);

        // Create tab bar state
        let tab_bar = TabBarState::new();

        // Create config watcher for hot-reload
        let config_watcher = ConfigWatcher::new();
        if config_watcher.is_some() {
            tracing::info!("Config hot-reload enabled");
        }

        // Build shortcuts list before moving config
        let shortcuts_list = Self::build_shortcuts_list(&config.keybindings);

        // Initialize tmux controller for passthrough support
        let tmux = TmuxController::new();
        if tmux.is_nested() && config.tmux.auto_detect {
            tracing::info!("Running inside tmux, passthrough mode enabled");
        }

        let mut app = Self {
            window,
            surface,
            renderer,
            terminal_manager,
            tab_bar,
            last_render: Instant::now(),
            needs_redraw: true,
            pending_close: false,
            modifiers: ModifiersState::empty(),
            clipboard,
            mouse_pos: PhysicalPosition::new(0.0, 0.0),
            mouse_pressed: false,
            last_click_time: Instant::now() - CLICK_THRESHOLD * 2,
            click_count: 0,
            last_click_pos: (0, 0),
            config,
            copy_mode: CopyMode::new(),
            url_detector,
            config_watcher,
            bell_active_until: None,
            scrollbar_dragging: false,
            divider_dragging: None,
            shortcuts_visible: false,
            protocol_status_visible: false,
            shortcuts_list,
            cursor_blink_visible: true,
            last_cursor_blink: Instant::now(),
            tmux,
            // Check for auto-dump environment variable (number of frames to wait)
            auto_dump_frames: std::env::var("ULTTY_AUTO_DUMP")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            protocol_server,
            control_server,
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
        };

        // Initialize tab bar state so it renders on first frame
        app.update_tab_bar();

        Ok(app)
    }

    fn start_control_server() -> Option<ControlServer> {
        match ControlServer::new() {
            Ok(server) => {
                tracing::info!("Control server started at {:?}", server.socket_path());
                Some(server)
            }
            Err(e) => {
                tracing::warn!("Failed to start control server: {}", e);
                None
            }
        }
    }

    /// Create terminal manager, optionally using a launch profile or restoring from session.
    fn create_terminal_manager(config: &Config, cols: u16, rows: u16) -> Result<TerminalManager> {
        // Try to restore session if enabled
        if config.behavior.restore_session {
            if let Some(session) = ultty::session::SessionData::load_last() {
                tracing::info!(
                    "Restoring previous session with {} tabs",
                    session.tabs.len()
                );
                match Self::create_manager_from_session(&session, cols, rows) {
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
                return Self::create_manager_from_profile(profile, cols, rows);
            } else {
                tracing::warn!(
                    "Default profile '{}' not found, using default shell",
                    profile_name
                );
            }
        }

        // No profile configured, use default shell
        TerminalManager::new(cols, rows)
            .map_err(|e| anyhow::anyhow!("Failed to create terminal manager: {}", e))
    }

    /// Create terminal manager from a launch profile.
    fn create_manager_from_profile(
        profile: &ultty::config::LaunchProfile,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalManager> {
        // Create an empty manager first
        let mut manager = TerminalManager::new(cols, rows)
            .map_err(|e| anyhow::anyhow!("Failed to create terminal manager: {}", e))?;

        // If profile has tabs, close the default tab and create configured ones
        if !profile.tabs.is_empty() {
            // Close the default tab that was created
            if let Some(tab) = manager.active_tab() {
                let tab_id = tab.id;
                manager.close_tab(tab_id);
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
                    .create_tab_with_command(command, &tab_config.args, working_dir, title)
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
        cols: u16,
        rows: u16,
    ) -> Result<TerminalManager> {
        TerminalManager::from_session(session, cols, rows)
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
        let socket_path = Self::default_socket_path();

        match ProtocolServer::new(&socket_path) {
            Ok(server) => {
                std::env::set_var("ULTTY_SOCKET", server.socket_path());
                tracing::info!(
                    "Native protocol server started at {:?}",
                    server.socket_path()
                );
                Some(server)
            }
            Err(e) => {
                tracing::warn!("Failed to start protocol server: {}", e);
                None
            }
        }
    }

    fn default_socket_path() -> std::path::PathBuf {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            std::path::PathBuf::from(runtime_dir).join("ultty.sock")
        } else {
            std::path::PathBuf::from(format!("/tmp/ultty-{}.sock", std::process::id()))
        }
    }

    /// Run the application.
    pub fn run() -> Result<()> {
        let event_loop = EventLoop::new()?;

        let mut app = App::new(&event_loop)?;

        // Initialize tab bar state
        app.update_tab_bar();

        event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::WaitUntil(
                app.next_update_deadline(Instant::now()),
            ));

            match event {
                Event::WindowEvent { event, .. } => {
                    let should_exit = app.handle_window_event(event);
                    if should_exit {
                        elwt.exit();
                    }
                }
                Event::AboutToWait => {
                    if app.update() {
                        elwt.exit();
                    }
                    elwt.set_control_flow(ControlFlow::WaitUntil(
                        app.next_update_deadline(Instant::now()),
                    ));
                }
                _ => {}
            }
        })?;

        Ok(())
    }

    /// Get the terminal state of the focused terminal.
    fn terminal(&self) -> Option<&ultty::grid::Terminal> {
        self.terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
    }

    /// Get mutable terminal state of the focused terminal.
    fn terminal_mut(&mut self) -> Option<&mut ultty::grid::Terminal> {
        self.terminal_manager
            .focused_terminal_mut()
            .map(|t| t.event_loop.terminal_mut())
    }

    /// Send input to the focused terminal.
    fn send_input(&mut self, data: &[u8]) -> std::io::Result<()> {
        if let Some(terminal) = self.terminal_manager.focused_terminal_mut() {
            terminal.event_loop.send_input(data)
        } else {
            Ok(())
        }
    }

    /// Update the tab bar state from the terminal manager.
    fn update_tab_bar(&mut self) {
        let total_width = self.window.inner_size().width as f32;
        let active_tab_id = self
            .terminal_manager
            .active_tab()
            .map(|t| t.id)
            .unwrap_or_else(|| ultty::terminal_manager::TabId::new(0));

        self.tab_bar
            .update(self.terminal_manager.tabs(), active_tab_id, total_width);

        // Update renderer's tab bar height for dimension calculations
        let new_height = self.tab_bar.height();
        let old_height = self.renderer.tab_bar_height();

        if (new_height - old_height).abs() > 0.1 {
            self.renderer.set_tab_bar_height(new_height);

            // Terminal dimensions may have changed - resize terminals
            let (cols, rows) = self.renderer.terminal_dimensions();
            if let Err(e) = self.terminal_manager.resize_all(cols, rows) {
                tracing::warn!("Failed to resize terminals after tab bar change: {}", e);
            }
        }
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

                        for col in sc..=ec {
                            if let Some(cell) = grid_row.get(col as u16) {
                                text.push(cell.char());
                            }
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
                        text.push_str(&grid_row.text_trimmed());
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
                        for col in *start_col..=*end_col {
                            if let Some(cell) = grid_row.get(col as u16) {
                                text.push(cell.char());
                            }
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
                // Case-insensitive search
                let text_lower = text.to_lowercase();
                let query_lower = query.to_lowercase();
                for (col, _) in text_lower.match_indices(&query_lower) {
                    matches.push((row, col));
                }
            }
        }

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

    /// Check if any native protocol sessions are active.
    fn has_native_sessions(&self) -> bool {
        self.protocol_server
            .as_ref()
            .map(|s| s.session_count() > 0)
            .unwrap_or(false)
    }

    /// Perform search across all native protocol sessions.
    fn perform_native_search(&mut self, query: &str) {
        self.native_search_query = query.to_string();
        self.native_search_matches.clear();
        self.native_search_index = 0;

        if let Some(ref server) = self.protocol_server {
            for session in server.sessions() {
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

        let old_hovered = self.tab_bar.hovered_tab;
        self.tab_bar.set_hover(x, y);

        // Redraw if hover state changed
        if old_hovered != self.tab_bar.hovered_tab {
            self.needs_redraw = true;
        }
    }

    /// Update cursor icon based on URL hover state.
    fn update_resize_cursor(&mut self) {
        let size = self.window.inner_size();
        let x = self.mouse_pos.x as f32;
        let y = self.mouse_pos.y as f32;
        let w = size.width as f32;
        let h = size.height as f32;
        let edge = 10.0;

        let cursor = match (x < edge, x > w - edge, y < edge, y > h - edge) {
            (true, _, true, _) | (_, true, _, true) => CursorIcon::NwseResize,
            (_, true, true, _) | (true, _, _, true) => CursorIcon::NeswResize,
            (true, _, _, _) | (_, true, _, _) => CursorIcon::EwResize,
            (_, _, true, _) | (_, _, _, true) => CursorIcon::NsResize,
            _ => return, // Not near edge, don't override cursor
        };
        self.window.set_cursor_icon(cursor);
    }

    fn update_cursor_for_url(&mut self) {
        // Don't change cursor if hovering over tab bar
        if self.tab_bar.visible && (self.mouse_pos.y as f32) < self.tab_bar.height() {
            self.window.set_cursor_icon(CursorIcon::Default);
            return;
        }

        if !self.url_detector.is_enabled() || !self.modifiers.control_key() {
            self.window.set_cursor_icon(CursorIcon::Default);
            return;
        }

        // Account for tab bar offset
        let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
        let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
        if self.url_detector.has_url_at(col, row) {
            self.window.set_cursor_icon(CursorIcon::Pointer);
        } else {
            self.window.set_cursor_icon(CursorIcon::Default);
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
                        .cell(row, col)
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
    fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Resize renderer
        self.renderer.resize(&self.surface, size.width, size.height);

        // Get new terminal dimensions
        let (cols, rows) = self.renderer.terminal_dimensions();
        tracing::debug!("Resize: {}x{}", cols, rows);

        // Resize all terminals
        if let Err(e) = self.terminal_manager.resize_all(cols, rows) {
            tracing::warn!("Failed to resize terminals: {}", e);
        }

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
}
