//! Terminal application state and main loop.

mod control;
mod input;
use anyhow::Result;
use arboard::Clipboard;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ultty::config::Config;
use ultty::config_watcher::ConfigWatcher;
use ultty::control::ControlServer;
use ultty::event_loop::TerminalEvent;
use ultty::protocol::ProtocolServer;
use ultty::renderer::{OverlayState, Renderer, ScrollbarState};
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
    /// Keyboard shortcuts overlay visible
    shortcuts_visible: bool,
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
            .with_transparent(config.behavior.opacity < 1.0)
            .build(event_loop)?;
        let window = Arc::new(window);

        let size = window.inner_size();

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

        // Create renderer with config
        let mut renderer = pollster::block_on(Renderer::with_config(
            &surface,
            &adapter,
            size.width,
            size.height,
            &config.colors,
            &config.font,
        ))?;

        // Set window opacity
        renderer.set_opacity(config.behavior.opacity);

        // Get terminal dimensions from renderer
        let (cols, rows) = renderer.terminal_dimensions();
        tracing::info!("Terminal size: {}x{}", cols, rows);

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

        // Start native protocol server for TUI apps
        let protocol_server = Self::start_protocol_server();
        let control_server = Self::start_control_server();

        Ok(Self {
            window,
            surface,
            renderer,
            terminal_manager,
            tab_bar,
            last_render: Instant::now(),
            needs_redraw: true,
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
            shortcuts_visible: false,
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
        })
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

    /// Build the list of keyboard shortcuts for display.
    fn build_shortcuts_list(
        keybindings: &ultty::config::KeybindingConfig,
    ) -> Vec<(String, String)> {
        vec![
            (keybindings.copy.clone(), "Copy selection".to_string()),
            (
                keybindings.paste.clone(),
                "Paste from clipboard".to_string(),
            ),
            (keybindings.new_tab.clone(), "New tab".to_string()),
            (keybindings.close_tab.clone(), "Close tab".to_string()),
            (keybindings.next_tab.clone(), "Next tab".to_string()),
            (keybindings.prev_tab.clone(), "Previous tab".to_string()),
            (
                keybindings.split_horizontal.clone(),
                "Split horizontal".to_string(),
            ),
            (
                keybindings.split_vertical.clone(),
                "Split vertical".to_string(),
            ),
            (
                keybindings.focus_pane_left.clone(),
                "Focus pane left".to_string(),
            ),
            (
                keybindings.focus_pane_right.clone(),
                "Focus pane right".to_string(),
            ),
            (
                keybindings.focus_pane_up.clone(),
                "Focus pane up".to_string(),
            ),
            (
                keybindings.focus_pane_down.clone(),
                "Focus pane down".to_string(),
            ),
            (keybindings.close_pane.clone(), "Close pane".to_string()),
            (keybindings.vim_mode.clone(), "Toggle vim mode".to_string()),
            (
                keybindings.reload_config.clone(),
                "Reload config".to_string(),
            ),
            (
                keybindings.show_shortcuts.clone(),
                "Show/hide shortcuts".to_string(),
            ),
            (
                keybindings.cycle_theme.clone(),
                "Cycle color theme".to_string(),
            ),
            (
                keybindings.font_increase.clone(),
                "Increase font size".to_string(),
            ),
            (
                keybindings.font_decrease.clone(),
                "Decrease font size".to_string(),
            ),
            (
                keybindings.font_reset.clone(),
                "Reset font size".to_string(),
            ),
            (
                keybindings.screenshot.clone(),
                "Take screenshot".to_string(),
            ),
            (keybindings.debug_dump.clone(), "Debug dump".to_string()),
        ]
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
            elwt.set_control_flow(ControlFlow::Poll);

            match event {
                Event::WindowEvent { event, .. } => {
                    if app.handle_window_event(event) {
                        elwt.exit();
                    }
                }
                Event::AboutToWait => {
                    if app.update() {
                        elwt.exit();
                    }
                    app.window.request_redraw();
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
        let Some(terminal) = self.terminal() else { return String::new(); };
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

        let Some(terminal) = self.terminal() else { return; };
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

        // Collect row text and hyperlink data first to avoid borrow conflicts
        let (rows, dims, hyperlink_grid, hyperlink_map): (
            Vec<String>,
            _,
            Vec<Vec<u32>>,
            std::collections::HashMap<u32, String>,
        ) = {
            let Some(terminal) = self.terminal() else { return; };
            let dims = terminal.dimensions();
            let rows: Vec<String> = (0..dims.rows)
                .map(|row| {
                    terminal
                        .buffer()
                        .visible_row(row)
                        .map(|r| r.text_trimmed())
                        .unwrap_or_default()
                })
                .collect();

            // Collect hyperlink IDs from visible cells
            let hyperlink_grid: Vec<Vec<u32>> = (0..dims.rows)
                .map(|row| {
                    (0..dims.cols)
                        .map(|col| {
                            terminal
                                .buffer()
                                .grid()
                                .cell(row, col)
                                .map(|c| c.hyperlink_id())
                                .unwrap_or(0)
                        })
                        .collect()
                })
                .collect();

            // Copy hyperlink map
            let hyperlink_map: std::collections::HashMap<u32, String> = terminal
                .all_hyperlinks()
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();

            (rows, dims, hyperlink_grid, hyperlink_map)
        };

        // Detect regex-based URLs
        self.url_detector.detect(rows.len() as u16, |row| {
            rows.get(row as usize).cloned().unwrap_or_default()
        });

        // Detect OSC 8 hyperlinks
        self.url_detector.detect_hyperlinks(
            dims.rows,
            dims.cols,
            |row, col| {
                hyperlink_grid
                    .get(row as usize)
                    .and_then(|r| r.get(col as usize))
                    .copied()
                    .unwrap_or(0)
            },
            |id| hyperlink_map.get(&id).cloned(),
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

    /// Reload configuration from disk.
    fn reload_config(&mut self) {
        let new_config = Config::load();

        // Update color scheme
        self.renderer
            .set_color_scheme(ultty::renderer::ColorScheme::from_config(
                &new_config.colors,
            ));

        // Update opacity
        self.renderer.set_opacity(new_config.behavior.opacity);

        // Update ligatures setting
        self.renderer.set_ligatures(new_config.font.ligatures);

        // Update URL detection setting
        self.url_detector
            .set_enabled(new_config.behavior.url_detection);

        self.config = new_config;
        self.needs_redraw = true;

        tracing::info!("Configuration reloaded");
    }

    /// Change font size (increase or decrease).
    fn change_font_size(&mut self, increase: bool) {
        let new_size = if increase {
            self.renderer.increase_font_size()
        } else {
            self.renderer.decrease_font_size()
        };

        tracing::info!("Font size: {:.1}pt", new_size);

        // Terminal dimensions changed - resize all terminals
        let (cols, rows) = self.renderer.terminal_dimensions();
        if let Err(e) = self.terminal_manager.resize_all(cols, rows) {
            tracing::warn!("Failed to resize terminals after font change: {}", e);
        }

        self.needs_redraw = true;
    }

    /// Reset font size to default.
    fn reset_font_size(&mut self) {
        let default_size = self.config.font.size;
        let new_size = self.renderer.set_font_size(default_size);

        tracing::info!("Font size reset to: {:.1}pt", new_size);

        // Terminal dimensions changed - resize all terminals
        let (cols, rows) = self.renderer.terminal_dimensions();
        if let Err(e) = self.terminal_manager.resize_all(cols, rows) {
            tracing::warn!("Failed to resize terminals after font reset: {}", e);
        }

        self.needs_redraw = true;
    }

    /// Take a screenshot of the terminal.
    fn take_screenshot(&mut self) {
        // Generate filename with timestamp
        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("ultty-screenshot-{}.png", timestamp);

        // Try Pictures directory first, then current directory
        let path = dirs::picture_dir()
            .map(|p| p.join("Screenshots").join(&filename))
            .unwrap_or_else(|| std::path::PathBuf::from(&filename));

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Get terminal reference
        let Some(terminal) = self
            .terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
        else {
            tracing::warn!("Cannot take screenshot: no focused terminal");
            return;
        };

        // Use default overlay state for clean screenshot
        let overlay = OverlayState::default();

        // Capture screenshot
        match self.renderer.capture_screenshot(terminal, &overlay, &path) {
            Ok(()) => {
                tracing::info!("Screenshot saved to {}", path.display());
                // Show notification if possible
                if let Ok(notif) = notify_rust::Notification::new()
                    .summary("Screenshot Saved")
                    .body(&format!("Saved to {}", path.display()))
                    .show()
                {
                    drop(notif);
                }
            }
            Err(e) => {
                tracing::error!("Failed to capture screenshot: {}", e);
            }
        }
    }

    /// Dump terminal state to a file for debugging.
    fn debug_dump_terminal(&self) {
        use std::io::Write;

        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("/tmp/ultty-debug-{}.txt", timestamp);

        let Some(terminal) = self.terminal() else { return; };
        let grid = terminal.grid();
        let dims = grid.dimensions();
        let cursor = grid.cursor();

        let mut output = String::new();

        // Header
        output.push_str("=== Ultty Terminal Debug Dump ===\n");
        output.push_str(&format!("Timestamp: {}\n", timestamp));
        output.push_str(&format!(
            "Dimensions: {}x{} (cols x rows)\n",
            dims.cols, dims.rows
        ));
        output.push_str(&format!(
            "Cursor: row={}, col={}\n",
            cursor.position.row, cursor.position.col
        ));
        output.push_str(&format!(
            "Modes: origin={}, autowrap={}\n",
            terminal.modes().origin_mode,
            terminal.modes().auto_wrap
        ));
        if let Some(region) = grid.scroll_region() {
            output.push_str(&format!(
                "Scroll region: top={}, bottom={}\n",
                region.top, region.bottom
            ));
        }
        output.push_str("\n=== Grid Content ===\n");

        // Dump grid content with row numbers
        for row_idx in 0..dims.rows {
            output.push_str(&format!("{:3}|", row_idx));
            if let Some(row) = grid.row(row_idx) {
                for col_idx in 0..dims.cols {
                    if let Some(cell) = row.get(col_idx) {
                        let ch = cell.char();
                        // Replace control characters with visible representation
                        let display_char = if ch.is_control() || ch == '\0' {
                            '·'
                        } else if ch == ' ' {
                            ' '
                        } else {
                            ch
                        };
                        output.push(display_char);
                    } else {
                        output.push('·');
                    }
                }
            }
            output.push_str("|\n");
        }

        output.push_str("\n=== End of Dump ===\n");

        // Write to file
        match std::fs::File::create(&filename) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(output.as_bytes()) {
                    tracing::error!("Failed to write debug dump: {}", e);
                } else {
                    tracing::info!("Debug dump saved to {}", filename);
                    // Also show notification
                    let _ = notify_rust::Notification::new()
                        .summary("Debug Dump Saved")
                        .body(&format!("Saved to {}", filename))
                        .show();
                }
            }
            Err(e) => {
                tracing::error!("Failed to create debug dump file: {}", e);
            }
        }
    }

    /// Cycle through available color themes.
    fn cycle_theme(&mut self) {
        let theme_names: Vec<String> = self.config.colors.theme_names();
        if theme_names.is_empty() {
            return;
        }

        // Find current theme index
        let current = self.config.colors.theme.as_deref().unwrap_or("");
        let current_idx = theme_names.iter().position(|n| n == current).unwrap_or(0);

        // Move to next theme (wrap around)
        let next_idx = (current_idx + 1) % theme_names.len();
        let next_name = &theme_names[next_idx];

        // Apply the theme
        if let Some(theme) = self.config.colors.get_theme(next_name).cloned() {
            self.config.colors.apply_theme(&theme);
            self.config.colors.theme = Some(next_name.clone());

            // Update renderer
            self.renderer
                .set_color_scheme(ultty::renderer::ColorScheme::from_config(
                    &self.config.colors,
                ));

            self.needs_redraw = true;
            tracing::info!("Switched to theme: {}", theme.name);
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

        if let Some(ref mut server) = self.protocol_server {
            server.set_viewport(size.width as f32, size.height as f32);
            server.broadcast_resize(cols, rows);
        }

        self.needs_redraw = true;
    }

    /// Process terminal events and render. Returns true if should exit.
    fn update(&mut self) -> bool {
        // Check for config file changes (hot-reload)
        if let Some(ref mut watcher) = self.config_watcher {
            if watcher.poll() {
                tracing::info!("Config file changed, reloading...");
                self.reload_config();
            }
        }

        // Poll native protocol server for TUI app messages
        if let Some(ref mut server) = self.protocol_server {
            if let Err(e) = server.poll(Some(Duration::ZERO)) {
                tracing::warn!("Protocol server poll error: {}", e);
            }
            // Tick scroll animations
            if server.update_animations() {
                self.needs_redraw = true;
            }
        }

        if let Some(ref mut server) = self.control_server {
            if let Err(e) = server.poll(Some(Duration::ZERO)) {
                tracing::warn!("Control server poll error: {}", e);
            }
            self.handle_control_requests();
        }

        // Update cursor blink state
        let now = Instant::now();
        if now.duration_since(self.last_cursor_blink) >= CURSOR_BLINK_INTERVAL {
            self.cursor_blink_visible = !self.cursor_blink_visible;
            self.last_cursor_blink = now;
            self.needs_redraw = true;
        }

        // Get the focused terminal ID for title updates
        let focused_id = self.terminal_manager.focused_terminal().map(|t| t.id);

        // Process all terminals
        match self.terminal_manager.process_all() {
            Ok(all_events) => {
                let mut content_changed = false;
                for (terminal_id, events) in all_events {
                    for event in events {
                        match event {
                            TerminalEvent::Redraw => {
                                self.needs_redraw = true;
                                // Only update URL detection for focused terminal
                                if Some(terminal_id) == focused_id {
                                    content_changed = true;
                                }
                            }
                            TerminalEvent::ChildExited { status } => {
                                tracing::info!(
                                    "Terminal {:?} exited with status {}",
                                    terminal_id,
                                    status
                                );
                            }
                            TerminalEvent::Bell => {
                                if Some(terminal_id) == focused_id {
                                    // Visual bell - flash border for 100ms
                                    if self.config.behavior.bell_visual {
                                        self.bell_active_until =
                                            Some(Instant::now() + Duration::from_millis(100));
                                        self.needs_redraw = true;
                                    }
                                    // Audio bell (if enabled)
                                    if self.config.behavior.bell_sound {
                                        // System bell - print BEL character to trigger terminal bell
                                        print!("\x07");
                                    }
                                    tracing::debug!("Bell");
                                }
                            }
                            TerminalEvent::TitleChanged(title) => {
                                // Only set window title from focused terminal
                                if Some(terminal_id) == focused_id {
                                    self.window.set_title(&title);
                                }
                            }
                            TerminalEvent::SetClipboard { clipboard: _, data } => {
                                // Set clipboard content (OSC 52)
                                // Only handle from focused terminal for security
                                if Some(terminal_id) == focused_id {
                                    // If running inside tmux, use passthrough to reach outer terminal
                                    if self.tmux.is_nested() && self.config.tmux.passthrough {
                                        // Send OSC 52 wrapped in tmux passthrough
                                        use base64::Engine;
                                        let encoded =
                                            base64::engine::general_purpose::STANDARD.encode(&data);
                                        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
                                        let passthrough =
                                            TmuxController::wrap_passthrough(osc52.as_bytes());
                                        // Write to stdout to reach outer terminal
                                        use std::io::Write;
                                        let _ = std::io::stdout().write_all(&passthrough);
                                        let _ = std::io::stdout().flush();
                                        tracing::debug!(
                                            "Set clipboard via OSC 52 passthrough: {} chars",
                                            data.len()
                                        );
                                    } else if let Some(ref mut cb) = self.clipboard {
                                        if let Err(e) = cb.set_text(&data) {
                                            tracing::warn!("Failed to set clipboard: {}", e);
                                        } else {
                                            tracing::debug!(
                                                "Set clipboard via OSC 52: {} chars",
                                                data.len()
                                            );
                                        }
                                    }
                                }
                            }
                            TerminalEvent::Notification { title, body } => {
                                // Desktop notification (OSC 9/777)
                                // Only show from focused terminal
                                if Some(terminal_id) == focused_id {
                                    Self::show_notification(&title, &body);
                                }
                            }
                        }
                    }
                }

                // Clean up exited terminals
                self.terminal_manager.cleanup_exited();
                self.update_tab_bar();

                // Check if all terminals have exited
                if self.terminal_manager.all_exited() {
                    tracing::info!("All terminals exited");
                    return true;
                }

                // Update URL detection when content changes
                if content_changed {
                    self.update_url_detection();
                }

                // Check for column mode resize requests (DECCOLM - 132/80 column mode)
                self.handle_column_resize_requests();
            }
            Err(e) => {
                tracing::error!("Terminal error: {}", e);
                return true;
            }
        }

        // Check if bell animation needs to continue
        if let Some(until) = self.bell_active_until {
            if Instant::now() < until {
                self.needs_redraw = true;
            } else {
                // Bell expired, clear it and redraw to remove border
                self.bell_active_until = None;
                self.needs_redraw = true;
            }
        }

        // Render if needed (limit to ~60fps)
        // Suppress rendering during synchronized update (mode 2026) to prevent
        // partial frames, but enforce a 200ms safety timeout to avoid indefinite
        // suppression if the app never sends ESU.
        let now = Instant::now();
        let sync_active = self
            .terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal().modes().synchronized_update)
            .unwrap_or(false);
        let frame_elapsed = now.duration_since(self.last_render);

        if self.needs_redraw
            && frame_elapsed >= Duration::from_millis(16)
            && (!sync_active || frame_elapsed >= Duration::from_millis(200))
        {
            self.render();
            self.last_render = now;
            self.needs_redraw = false;
        }

        false
    }

    /// Render the terminal.
    fn render(&mut self) {
        let pane_count = self.terminal_manager.pane_count();

        let result = if pane_count > 1 {
            self.render_multi_pane()
        } else {
            self.render_single_pane()
        };

        match result {
            Ok(()) => {
                // Clear dirty flags after successful render
                self.terminal_manager.clear_dirty();
            }
            Err(wgpu::SurfaceError::Lost) => {
                let size = self.window.inner_size();
                self.renderer.resize(&self.surface, size.width, size.height);
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                tracing::error!("Out of GPU memory");
            }
            Err(e) => {
                tracing::warn!("Render error: {}", e);
            }
        }

        // Handle auto-dump countdown
        if self.auto_dump_frames > 0 {
            self.auto_dump_frames -= 1;
            if self.auto_dump_frames == 0 {
                tracing::info!("Auto-dump triggered");
                self.debug_dump_terminal();
            }
        }
    }

    /// Render a single terminal pane.
    fn render_single_pane(&mut self) -> Result<(), wgpu::SurfaceError> {
        let Some(terminal) = self
            .terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
        else {
            return Ok(());
        };

        // Calculate scrollbar state
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();
        let scrollbar = ScrollbarState::new(
            buffer.total_lines(),
            dims.rows as usize,
            buffer.scroll_offset(),
        );

        // Check if bell is still active
        let bell_active = self
            .bell_active_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        let overlay = OverlayState {
            copy_mode: if self.copy_mode.is_active() {
                Some(&self.copy_mode)
            } else {
                None
            },
            urls: self.url_detector.urls(),
            ctrl_held: self.modifiers.control_key(),
            tab_bar: if self.tab_bar.visible {
                Some(&self.tab_bar)
            } else {
                None
            },
            scrollbar: if scrollbar.is_visible() {
                Some(scrollbar)
            } else {
                None
            },
            bell_active,
            shortcuts_visible: self.shortcuts_visible,
            keybindings: if self.shortcuts_visible {
                Some(&self.shortcuts_list)
            } else {
                None
            },
            cursor_blink_visible: self.cursor_blink_visible,
        };

        if let Some(ref server) = self.protocol_server {
            if server.session_count() > 0 {
                let search_matches = if self.native_search_active && !self.native_search_matches.is_empty() {
                    Some(self.native_search_matches.as_slice())
                } else {
                    None
                };
                return self.renderer.render_with_native_sessions(
                    &self.surface,
                    terminal,
                    &overlay,
                    server.sessions(),
                    search_matches,
                    self.native_search_index,
                );
            }
        }

        self.renderer
            .render_with_overlay(&self.surface, terminal, &overlay)
    }

    /// Render multiple panes.
    fn render_multi_pane(&mut self) -> Result<(), wgpu::SurfaceError> {
        use ultty::renderer::PaneInfo;
        use ultty::terminal_manager::SplitDirection;

        let size = self.window.inner_size();
        let tab_bar_height = self.tab_bar.height();
        let available_width = size.width as f32;
        let available_height = size.height as f32 - tab_bar_height;

        let terminals = self.terminal_manager.active_tab_terminals();
        let pane_count = terminals.len();

        if pane_count == 0 {
            return Ok(());
        }

        let focused_id = self.terminal_manager.focused_terminal_id();
        let split_direction = self.terminal_manager.split_direction();

        // Calculate pane bounds based on split direction
        let panes: Vec<(&ultty::grid::Terminal, PaneInfo)> = match split_direction {
            SplitDirection::Horizontal => {
                let pane_width = available_width / pane_count as f32;
                terminals
                    .iter()
                    .enumerate()
                    .map(|(i, managed)| {
                        let is_focused = Some(managed.id) == focused_id;
                        let pane = PaneInfo::new(
                            i as f32 * pane_width,
                            0.0,
                            pane_width,
                            available_height,
                            is_focused,
                        );
                        (managed.event_loop.terminal(), pane)
                    })
                    .collect()
            }
            SplitDirection::Vertical => {
                let pane_height = available_height / pane_count as f32;
                terminals
                    .iter()
                    .enumerate()
                    .map(|(i, managed)| {
                        let is_focused = Some(managed.id) == focused_id;
                        let pane = PaneInfo::new(
                            0.0,
                            i as f32 * pane_height,
                            available_width,
                            pane_height,
                            is_focused,
                        );
                        (managed.event_loop.terminal(), pane)
                    })
                    .collect()
            }
        };

        // Calculate scrollbar state from focused terminal
        let scrollbar = self.terminal_manager.focused_terminal().map(|t| {
            let terminal = t.event_loop.terminal();
            let buffer = terminal.buffer();
            let dims = terminal.dimensions();
            ScrollbarState::new(
                buffer.total_lines(),
                dims.rows as usize,
                buffer.scroll_offset(),
            )
        });

        // Check if bell is still active
        let bell_active = self
            .bell_active_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        let overlay = OverlayState {
            copy_mode: if self.copy_mode.is_active() {
                Some(&self.copy_mode)
            } else {
                None
            },
            urls: self.url_detector.urls(),
            ctrl_held: self.modifiers.control_key(),
            tab_bar: if self.tab_bar.visible {
                Some(&self.tab_bar)
            } else {
                None
            },
            scrollbar: scrollbar.filter(|s| s.is_visible()),
            bell_active,
            shortcuts_visible: self.shortcuts_visible,
            keybindings: if self.shortcuts_visible {
                Some(&self.shortcuts_list)
            } else {
                None
            },
            cursor_blink_visible: self.cursor_blink_visible,
        };

        self.renderer.render_panes(&self.surface, &panes, &overlay)
    }

    /// Show a desktop notification.
    fn show_notification(title: &str, body: &str) {
        use notify_rust::Notification;

        let title = if title.is_empty() { "Ultty" } else { title };

        std::thread::spawn({
            let title = title.to_string();
            let body = body.to_string();
            move || {
                if let Err(e) = Notification::new()
                    .summary(&title)
                    .body(&body)
                    .appname("Ultty")
                    .timeout(5000)
                    .show()
                {
                    tracing::warn!("Failed to show notification: {}", e);
                } else {
                    tracing::debug!("Showed notification: {} - {}", title, body);
                }
            }
        });
    }

}
