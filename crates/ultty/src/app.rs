//! Terminal application state and main loop.

use ultty::config::{Config, KeybindingConfig};
use ultty::config_watcher::ConfigWatcher;
use ultty::event_loop::TerminalEvent;
use ultty::grid::{MouseButton as TerminalMouseButton, SelectionMode, SelectionPoint};
use ultty::renderer::{OverlayState, Renderer, ScrollbarState};
use ultty::tabs::TabBarState;
use ultty::terminal_manager::TerminalManager;
use ultty::tmux::TmuxController;
use ultty::url::UrlDetector;
use ultty::vim::{CopyMode, CopyModeResult, Selection as VimSelection};
use ultty_protocol::{Key as ProtocolKey, Modifiers as ProtocolModifiers};
use anyhow::Result;
use arboard::Clipboard;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowBuilder};

/// Default window dimensions.
const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;

/// Time window for detecting double/triple clicks.
const CLICK_THRESHOLD: Duration = Duration::from_millis(500);

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
        })
    }

    /// Build the list of keyboard shortcuts for display.
    fn build_shortcuts_list(keybindings: &ultty::config::KeybindingConfig) -> Vec<(String, String)> {
        vec![
            (keybindings.copy.clone(), "Copy selection".to_string()),
            (keybindings.paste.clone(), "Paste from clipboard".to_string()),
            (keybindings.new_tab.clone(), "New tab".to_string()),
            (keybindings.close_tab.clone(), "Close tab".to_string()),
            (keybindings.next_tab.clone(), "Next tab".to_string()),
            (keybindings.prev_tab.clone(), "Previous tab".to_string()),
            (keybindings.split_horizontal.clone(), "Split horizontal".to_string()),
            (keybindings.split_vertical.clone(), "Split vertical".to_string()),
            (keybindings.focus_pane_left.clone(), "Focus pane left".to_string()),
            (keybindings.focus_pane_right.clone(), "Focus pane right".to_string()),
            (keybindings.focus_pane_up.clone(), "Focus pane up".to_string()),
            (keybindings.focus_pane_down.clone(), "Focus pane down".to_string()),
            (keybindings.close_pane.clone(), "Close pane".to_string()),
            (keybindings.vim_mode.clone(), "Toggle vim mode".to_string()),
            (keybindings.reload_config.clone(), "Reload config".to_string()),
            (keybindings.show_shortcuts.clone(), "Show/hide shortcuts".to_string()),
            (keybindings.cycle_theme.clone(), "Cycle color theme".to_string()),
        ]
    }

    /// Create terminal manager, optionally using a launch profile or restoring from session.
    fn create_terminal_manager(config: &Config, cols: u16, rows: u16) -> Result<TerminalManager> {
        // Try to restore session if enabled
        if config.behavior.restore_session {
            if let Some(session) = ultty::session::SessionData::load_last() {
                tracing::info!("Restoring previous session with {} tabs", session.tabs.len());
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
                tracing::warn!("Default profile '{}' not found, using default shell", profile_name);
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

                let working_dir = tab_config.working_dir.as_deref()
                    .or(profile.working_dir.as_deref());

                let title = tab_config.title.as_deref();

                manager.create_tab_with_command(
                    command,
                    &tab_config.args,
                    working_dir,
                    title,
                ).map_err(|e| anyhow::anyhow!("Failed to create tab: {}", e))?;

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
    fn terminal(&self) -> &ultty::grid::Terminal {
        self.terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
            .expect("No focused terminal")
    }

    /// Get mutable terminal state of the focused terminal.
    fn terminal_mut(&mut self) -> &mut ultty::grid::Terminal {
        self.terminal_manager
            .focused_terminal_mut()
            .map(|t| t.event_loop.terminal_mut())
            .expect("No focused terminal")
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
        let active_tab_id = self.terminal_manager.active_tab()
            .map(|t| t.id)
            .unwrap_or_else(|| ultty::terminal_manager::TabId::new(0));

        self.tab_bar.update(
            self.terminal_manager.tabs(),
            active_tab_id,
            total_width,
        );

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

    /// Handle window event. Returns true if should exit.
    fn handle_window_event(&mut self, event: WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("Close requested");
                self.save_session();
                return true;
            }
            WindowEvent::Resized(size) => {
                self.handle_resize(size);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = position;
                if self.mouse_pressed {
                    self.handle_mouse_drag();
                } else if self.terminal().track_mouse_motion() {
                    // AnyEvent mode reports motion even without button pressed
                    self.handle_mouse_motion();
                }
                self.update_tab_hover();
                self.update_cursor_for_url();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(button, state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_scroll(delta);
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = true;
            }
            WindowEvent::Focused(focused) => {
                if self.terminal().modes().focus_reporting {
                    // Send focus in/out escape sequence
                    let seq = if focused { b"\x1b[I" } else { b"\x1b[O" };
                    let _ = self.send_input(seq);
                }
            }
            _ => {}
        }
        false
    }

    /// Convert pixel position to terminal cell coordinates.
    fn pixel_to_cell(&self, x: f64, y: f64) -> (u16, u16) {
        let (cell_width, cell_height) = self.renderer.cell_dimensions();
        let col = (x / cell_width as f64).floor().max(0.0) as u16;
        let row = (y / cell_height as f64).floor().max(0.0) as u16;
        (col, row)
    }

    /// Handle mouse button press/release.
    fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        if button != MouseButton::Left
            && button != MouseButton::Middle
            && button != MouseButton::Right
        {
            return;
        }

        let x = self.mouse_pos.x as f32;
        let y = self.mouse_pos.y as f32;

        // Check if mouse tracking is enabled and send event to PTY
        if self.terminal().mouse_tracking_enabled() {
            // Don't intercept UI elements when mouse tracking is on
            if self.tab_bar.visible && y < self.tab_bar.height() {
                // Still allow tab bar interaction
            } else if self.is_in_scrollbar(x, y) {
                // Still allow scrollbar interaction
            } else {
                // Convert to cell coordinates and send to terminal
                let (col, row) =
                    self.pixel_to_cell(self.mouse_pos.x, self.mouse_pos.y - self.tab_bar.height() as f64);
                let term_button = match button {
                    MouseButton::Left => TerminalMouseButton::Left,
                    MouseButton::Middle => TerminalMouseButton::Middle,
                    MouseButton::Right => TerminalMouseButton::Right,
                    _ => return,
                };
                let pressed = state == ElementState::Pressed;
                let term_button = if !pressed {
                    TerminalMouseButton::Release
                } else {
                    term_button
                };
                if let Some(bytes) =
                    self.terminal().encode_mouse_event(term_button, col, row, pressed, false)
                {
                    let _ = self.send_input(&bytes);
                }
                return;
            }
        }

        match state {
            ElementState::Pressed => {
                // Check for tab bar clicks first
                if self.tab_bar.visible && y < self.tab_bar.height() {
                    // Middle click closes tab
                    if button == MouseButton::Middle {
                        if let Some(tab_idx) = self.tab_bar.tab_at_x(x) {
                            if self.terminal_manager.tab_count() > 1 {
                                let tabs = self.terminal_manager.tabs();
                                if let Some(tab) = tabs.get(tab_idx) {
                                    let tab_id = tab.id;
                                    self.terminal_manager.close_tab(tab_id);
                                    tracing::info!("Closed tab {} via middle click", tab_idx);
                                    self.update_tab_bar();
                                    self.needs_redraw = true;
                                }
                            }
                        }
                        return;
                    }

                    // Check for close button click
                    if let Some(tab_idx) = self.tab_bar.close_button_at(x, y) {
                        if self.terminal_manager.tab_count() > 1 {
                            let tabs = self.terminal_manager.tabs();
                            if let Some(tab) = tabs.get(tab_idx) {
                                let tab_id = tab.id;
                                self.terminal_manager.close_tab(tab_id);
                                tracing::info!("Closed tab {} via close button", tab_idx);
                                self.update_tab_bar();
                                self.needs_redraw = true;
                            }
                        }
                        return;
                    }

                    // Click on tab - start drag and switch to it
                    if let Some(tab_idx) = self.tab_bar.tab_at_x(x) {
                        self.tab_bar.start_drag(tab_idx, x);
                        self.terminal_manager.switch_to_tab(tab_idx);
                        self.update_tab_bar();
                        self.needs_redraw = true;
                        tracing::debug!("Started drag on tab {}", tab_idx);
                    }
                    return;
                }

                // Not in tab bar - handle normal clicks
                if button != MouseButton::Left {
                    return;
                }

                // Check for scrollbar click
                if self.is_in_scrollbar(x, y) {
                    self.scrollbar_dragging = true;
                    self.handle_scrollbar_click(y);
                    return;
                }

                let (col, row) = self.pixel_to_cell(self.mouse_pos.x, self.mouse_pos.y - self.tab_bar.height() as f64);

                // Check for Ctrl+Click on URL
                if self.modifiers.control_key() && self.url_detector.is_enabled() {
                    if let Some(url) = self.url_detector.url_at(col, row) {
                        let url_str = url.url.clone();
                        if let Err(e) = UrlDetector::open_url(&url_str) {
                            tracing::warn!("Failed to open URL: {}", e);
                        } else {
                            tracing::info!("Opened URL: {}", url_str);
                        }
                        return;
                    }
                }

                self.mouse_pressed = true;

                // Detect double/triple clicks
                let now = Instant::now();
                let same_cell = (col, row) == self.last_click_pos;
                let quick_click = now.duration_since(self.last_click_time) < CLICK_THRESHOLD;

                if same_cell && quick_click {
                    self.click_count = (self.click_count % 3) + 1;
                } else {
                    self.click_count = 1;
                }

                self.last_click_time = now;
                self.last_click_pos = (col, row);

                // Start selection based on click count and modifiers
                // Alt+drag enables block/rectangular selection
                let mode = if self.modifiers.alt_key() && self.click_count == 1 {
                    SelectionMode::Block
                } else {
                    match self.click_count {
                        1 => SelectionMode::Normal,
                        2 => SelectionMode::Normal, // Word selection handled differently
                        3 => SelectionMode::Line,
                        _ => SelectionMode::Normal,
                    }
                };

                let point = SelectionPoint::new(col as i32, row as i32);

                if self.click_count == 2 && !self.modifiers.alt_key() {
                    // Word selection - find word boundaries (not in block mode)
                    self.select_word(col, row);
                } else {
                    self.terminal_mut().start_selection(point, mode);
                }

                self.needs_redraw = true;
            }
            ElementState::Released => {
                self.mouse_pressed = false;
                self.scrollbar_dragging = false;

                // End tab drag if active
                if self.tab_bar.is_dragging() {
                    self.tab_bar.end_drag();
                    self.needs_redraw = true;
                }

                // Copy selection to clipboard on release (if copy_on_select enabled)
                if self.config.behavior.copy_on_select {
                    if let Some(text) = self.terminal().selected_text() {
                        if !text.is_empty() {
                            self.copy_to_clipboard(&text);
                        }
                    }
                }
            }
        }
    }

    /// Handle mouse drag for selection.
    fn handle_mouse_drag(&mut self) {
        // Handle tab bar dragging
        if self.tab_bar.is_dragging() {
            let x = self.mouse_pos.x as f32;
            if let Some((from, to)) = self.tab_bar.update_drag(x) {
                // Reorder tabs in terminal manager
                if self.terminal_manager.reorder_tabs(from, to) {
                    self.update_tab_bar();
                    tracing::debug!("Reordered tab {} to {}", from, to);
                }
            }
            self.needs_redraw = true;
            return;
        }

        // Handle scrollbar dragging
        if self.scrollbar_dragging {
            self.handle_scrollbar_click(self.mouse_pos.y as f32);
            return;
        }

        // Account for tab bar offset
        let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
        if adjusted_y < 0.0 {
            return; // Dragging in tab bar area
        }

        // Check if motion tracking is enabled
        if self.terminal().track_mouse_motion() && self.mouse_pressed {
            let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
            // When button is held, report as Left button for drag
            if let Some(bytes) =
                self.terminal().encode_mouse_event(TerminalMouseButton::Left, col, row, true, true)
            {
                let _ = self.send_input(&bytes);
            }
            return;
        }

        let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
        let point = SelectionPoint::new(col as i32, row as i32);

        self.terminal_mut().update_selection(point);
        self.needs_redraw = true;
    }

    /// Handle mouse motion without button pressed (for AnyEvent mode).
    fn handle_mouse_motion(&mut self) {
        let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
        if adjusted_y < 0.0 {
            return; // In tab bar area
        }

        let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
        // Motion without button uses Release (3) + motion flag (32) = 35
        if let Some(bytes) =
            self.terminal().encode_mouse_event(TerminalMouseButton::Release, col, row, true, true)
        {
            let _ = self.send_input(&bytes);
        }
    }

    /// Check if a position is within the scrollbar area.
    fn is_in_scrollbar(&self, x: f32, y: f32) -> bool {
        let size = self.window.inner_size();
        let scrollbar_width = 8.0;
        let scrollbar_margin = 2.0;

        // Scrollbar is on the right edge
        let scrollbar_x = size.width as f32 - scrollbar_width - scrollbar_margin;
        let scrollbar_top = self.tab_bar.height() + scrollbar_margin;
        let scrollbar_bottom = size.height as f32 - scrollbar_margin;

        x >= scrollbar_x && y >= scrollbar_top && y <= scrollbar_bottom
    }

    /// Handle scrollbar click/drag - scroll to position.
    fn handle_scrollbar_click(&mut self, y: f32) {
        let size = self.window.inner_size();
        let scrollbar_margin = 2.0;
        let scrollbar_top = self.tab_bar.height() + scrollbar_margin;
        let scrollbar_height = size.height as f32 - self.tab_bar.height() - scrollbar_margin * 2.0;

        // Calculate click position as fraction (0 = top, 1 = bottom)
        let fraction = ((y - scrollbar_top) / scrollbar_height).clamp(0.0, 1.0);

        // Get buffer info
        let terminal = self.terminal();
        let buffer = terminal.buffer();
        let total_lines = buffer.total_lines();
        let viewport_rows = terminal.dimensions().rows as usize;

        if total_lines <= viewport_rows {
            return;
        }

        let max_offset = total_lines - viewport_rows;
        // Invert fraction: top of scrollbar = max offset, bottom = 0
        let target_offset = ((1.0 - fraction) * max_offset as f32).round() as usize;

        // Apply scroll
        let buffer = self.terminal_mut().buffer_mut();
        let current = buffer.scroll_offset();
        if target_offset > current {
            buffer.scroll_up(target_offset - current);
        } else if target_offset < current {
            buffer.scroll_down(current - target_offset);
        }

        self.needs_redraw = true;
    }

    /// Select the word at the given position.
    fn select_word(&mut self, col: u16, row: u16) {
        // First, find word boundaries (immutable borrow)
        let word_bounds = {
            let grid = self.terminal().grid();

            if let Some(grid_row) = grid.row(row) {
                let mut start_col = col;
                let mut end_col = col;

                // Scan left for word start
                while start_col > 0 {
                    if let Some(cell) = grid_row.get(start_col - 1) {
                        if is_word_char(cell.char()) {
                            start_col -= 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                // Scan right for word end
                let cols = grid.dimensions().cols;
                while end_col < cols - 1 {
                    if let Some(cell) = grid_row.get(end_col + 1) {
                        if is_word_char(cell.char()) {
                            end_col += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                Some((start_col, end_col))
            } else {
                None
            }
        };

        // Then set selection (mutable borrow)
        if let Some((start_col, end_col)) = word_bounds {
            let start = SelectionPoint::new(start_col as i32, row as i32);
            let end = SelectionPoint::new(end_col as i32, row as i32);
            self.terminal_mut().start_selection(start, SelectionMode::Normal);
            self.terminal_mut().update_selection(end);
        }
    }

    /// Handle scroll wheel.
    fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y as i32,
            MouseScrollDelta::PixelDelta(pos) => {
                let (_, cell_height) = self.renderer.cell_dimensions();
                (pos.y / cell_height as f64).round() as i32
            }
        };

        if lines == 0 {
            return;
        }

        // Check if mouse tracking is enabled - send scroll events to PTY
        if self.terminal().mouse_tracking_enabled() {
            let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
            let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y.max(0.0));
            let button = if lines > 0 {
                TerminalMouseButton::ScrollUp
            } else {
                TerminalMouseButton::ScrollDown
            };
            let count = lines.unsigned_abs() as usize;
            for _ in 0..count {
                if let Some(bytes) = self.terminal().encode_mouse_event(button, col, row, true, false)
                {
                    let _ = self.send_input(&bytes);
                }
            }
            return;
        }

        let buffer = self.terminal_mut().buffer_mut();

        if lines > 0 {
            buffer.scroll_up(lines as usize);
        } else {
            buffer.scroll_down((-lines) as usize);
        }

        self.needs_redraw = true;
    }

    /// Copy text to clipboard.
    fn copy_to_clipboard(&mut self, text: &str) {
        if let Some(clipboard) = &mut self.clipboard {
            if let Err(e) = clipboard.set_text(text.to_string()) {
                tracing::warn!("Failed to copy to clipboard: {}", e);
            } else {
                tracing::debug!("Copied {} bytes to clipboard", text.len());
            }
        }
    }

    /// Paste text from clipboard.
    fn paste_from_clipboard(&mut self) {
        if let Some(clipboard) = &mut self.clipboard {
            match clipboard.get_text() {
                Ok(text) => {
                    // Check for bracketed paste mode
                    let bracketed = self.terminal().modes().bracketed_paste;
                    if bracketed {
                        let _ = self.send_input(b"\x1b[200~");
                    }

                    if let Err(e) = self.send_input(text.as_bytes()) {
                        tracing::warn!("Failed to paste: {}", e);
                    }

                    if bracketed {
                        let _ = self.send_input(b"\x1b[201~");
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read clipboard: {}", e);
                }
            }
        }
    }

    /// Handle keyboard input.
    fn handle_key(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        // Reset cursor blink on key press (cursor should be visible while typing)
        self.cursor_blink_visible = true;
        self.last_cursor_blink = Instant::now();

        // Check for vim mode toggle (Ctrl+Shift+Space)
        if self.check_keybinding(&event, &self.config.keybindings.vim_mode.clone()) {
            if self.copy_mode.is_active() {
                self.copy_mode.exit();
            } else {
                self.enter_copy_mode();
            }
            self.needs_redraw = true;
            return;
        }

        // Tab keybindings
        if self.check_keybinding(&event, &self.config.keybindings.new_tab.clone()) {
            match self.terminal_manager.create_tab() {
                Ok(tab_id) => {
                    tracing::info!("Created new tab {:?}", tab_id);
                    self.update_tab_bar();
                    self.needs_redraw = true;
                }
                Err(e) => tracing::warn!("Failed to create tab: {}", e),
            }
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.close_tab.clone()) {
            if self.terminal_manager.tab_count() > 1 {
                if let Some(tab) = self.terminal_manager.active_tab() {
                    let tab_id = tab.id;
                    self.terminal_manager.close_tab(tab_id);
                    tracing::info!("Closed tab {:?}", tab_id);
                    self.update_tab_bar();
                    self.needs_redraw = true;
                }
            }
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.next_tab.clone()) {
            self.terminal_manager.next_tab();
            self.update_tab_bar();
            self.needs_redraw = true;
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.prev_tab.clone()) {
            self.terminal_manager.prev_tab();
            self.update_tab_bar();
            self.needs_redraw = true;
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.split_horizontal.clone()) {
            match self.terminal_manager.split_horizontal() {
                Ok(Some(terminal_id)) => {
                    tracing::info!("Split horizontal: new terminal {:?}", terminal_id);
                    self.needs_redraw = true;
                }
                Ok(None) => tracing::warn!("No active tab for split"),
                Err(e) => tracing::warn!("Failed to split: {}", e),
            }
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.split_vertical.clone()) {
            match self.terminal_manager.split_vertical() {
                Ok(Some(terminal_id)) => {
                    tracing::info!("Split vertical: new terminal {:?}", terminal_id);
                    self.needs_redraw = true;
                }
                Ok(None) => tracing::warn!("No active tab for split"),
                Err(e) => tracing::warn!("Failed to split: {}", e),
            }
            return;
        }

        // Pane navigation
        if self.check_keybinding(&event, &self.config.keybindings.focus_pane_right.clone())
            || self.check_keybinding(&event, &self.config.keybindings.focus_pane_down.clone())
        {
            self.terminal_manager.focus_next_pane();
            self.needs_redraw = true;
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.focus_pane_left.clone())
            || self.check_keybinding(&event, &self.config.keybindings.focus_pane_up.clone())
        {
            self.terminal_manager.focus_prev_pane();
            self.needs_redraw = true;
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.close_pane.clone()) {
            if self.terminal_manager.close_focused_pane() {
                tracing::info!("Closed pane");
                self.needs_redraw = true;
            }
            return;
        }

        if self.check_keybinding(&event, &self.config.keybindings.reload_config.clone()) {
            self.reload_config();
            return;
        }

        if self.check_keybinding(&event, &self.config.keybindings.show_shortcuts.clone()) {
            self.shortcuts_visible = !self.shortcuts_visible;
            self.needs_redraw = true;
            return;
        }

        if self.check_keybinding(&event, &self.config.keybindings.cycle_theme.clone()) {
            self.cycle_theme();
            return;
        }

        // Close shortcuts overlay with Escape
        if self.shortcuts_visible {
            if let Key::Named(NamedKey::Escape) = &event.logical_key {
                self.shortcuts_visible = false;
                self.needs_redraw = true;
                return;
            }
            // Block other keys while shortcuts overlay is visible
            return;
        }

        // If in copy mode, route to vim handler
        if self.copy_mode.is_active() {
            self.handle_copy_mode_key(&event);
            return;
        }

        // Clear selection on any key press (except modifiers)
        if !matches!(event.logical_key, Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super)) {
            self.terminal_mut().clear_selection();
        }

        // Handle paste shortcut (Ctrl+Shift+V or Shift+Insert)
        if self.check_keybinding(&event, &self.config.keybindings.paste.clone()) {
            self.paste_from_clipboard();
            return;
        }
        if let Key::Named(NamedKey::Insert) = &event.logical_key {
            if self.modifiers.shift_key() {
                self.paste_from_clipboard();
                return;
            }
        }

        // Handle copy shortcut (Ctrl+Shift+C)
        if self.check_keybinding(&event, &self.config.keybindings.copy.clone()) {
            if let Some(text) = self.terminal().selected_text() {
                self.copy_to_clipboard(&text);
            }
            return;
        }

        // Convert key to terminal input
        let input: Option<&[u8]> = match &event.logical_key {
            Key::Named(named) => match named {
                NamedKey::Enter => Some(b"\r"),
                NamedKey::Backspace => Some(b"\x7f"),
                NamedKey::Tab => Some(b"\t"),
                NamedKey::Escape => Some(b"\x1b"),
                NamedKey::ArrowUp => Some(b"\x1b[A"),
                NamedKey::ArrowDown => Some(b"\x1b[B"),
                NamedKey::ArrowRight => Some(b"\x1b[C"),
                NamedKey::ArrowLeft => Some(b"\x1b[D"),
                NamedKey::Home => Some(b"\x1b[H"),
                NamedKey::End => Some(b"\x1b[F"),
                NamedKey::PageUp => Some(b"\x1b[5~"),
                NamedKey::PageDown => Some(b"\x1b[6~"),
                NamedKey::Insert => Some(b"\x1b[2~"),
                NamedKey::Delete => Some(b"\x1b[3~"),
                NamedKey::F1 => Some(b"\x1bOP"),
                NamedKey::F2 => Some(b"\x1bOQ"),
                NamedKey::F3 => Some(b"\x1bOR"),
                NamedKey::F4 => Some(b"\x1bOS"),
                NamedKey::F5 => Some(b"\x1b[15~"),
                NamedKey::F6 => Some(b"\x1b[17~"),
                NamedKey::F7 => Some(b"\x1b[18~"),
                NamedKey::F8 => Some(b"\x1b[19~"),
                NamedKey::F9 => Some(b"\x1b[20~"),
                NamedKey::F10 => Some(b"\x1b[21~"),
                NamedKey::F11 => Some(b"\x1b[23~"),
                NamedKey::F12 => Some(b"\x1b[24~"),
                _ => None,
            },
            Key::Character(c) => {
                // Handle Ctrl+key combinations
                if self.modifiers.control_key() && !self.modifiers.shift_key() {
                    let ch = c.chars().next().unwrap_or('\0');
                    if ch >= 'a' && ch <= 'z' {
                        let ctrl_char = (ch as u8 - b'a' + 1) as char;
                        if let Err(e) = self.send_input(&[ctrl_char as u8]) {
                            tracing::warn!("Failed to send input: {}", e);
                        }
                        return;
                    }
                }
                None
            }
            _ => None,
        };

        if let Some(bytes) = input {
            if let Err(e) = self.send_input(bytes) {
                tracing::warn!("Failed to send input: {}", e);
            }
        } else if let Key::Character(c) = &event.logical_key {
            // Send regular character input
            if let Err(e) = self.send_input(c.as_bytes()) {
                tracing::warn!("Failed to send input: {}", e);
            }
        }
    }

    /// Check if a key event matches a configured keybinding.
    fn check_keybinding(&self, event: &KeyEvent, binding: &str) -> bool {
        let Some(parsed) = KeybindingConfig::parse_keybinding(binding) else {
            return false;
        };

        // Check modifiers
        if parsed.ctrl != self.modifiers.control_key() {
            return false;
        }
        if parsed.shift != self.modifiers.shift_key() {
            return false;
        }
        if parsed.alt != self.modifiers.alt_key() {
            return false;
        }
        if parsed.super_key != self.modifiers.super_key() {
            return false;
        }

        // Check key
        match &event.logical_key {
            Key::Named(named) => {
                let key_name = match named {
                    NamedKey::Space => "space",
                    NamedKey::Enter => "enter",
                    NamedKey::Tab => "tab",
                    NamedKey::Escape => "escape",
                    NamedKey::Backspace => "backspace",
                    NamedKey::Delete => "delete",
                    NamedKey::Insert => "insert",
                    NamedKey::Home => "home",
                    NamedKey::End => "end",
                    NamedKey::PageUp => "pageup",
                    NamedKey::PageDown => "pagedown",
                    NamedKey::ArrowUp => "up",
                    NamedKey::ArrowDown => "down",
                    NamedKey::ArrowLeft => "left",
                    NamedKey::ArrowRight => "right",
                    _ => return false,
                };
                parsed.key == key_name
            }
            Key::Character(c) => {
                parsed.key == c.to_lowercase()
            }
            _ => false,
        }
    }

    /// Enter vim-style copy mode.
    fn enter_copy_mode(&mut self) {
        let terminal = self.terminal();
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();

        let total_rows = buffer.total_lines();
        let viewport_rows = dims.rows as usize;
        let viewport_cols = dims.cols as usize;

        self.copy_mode.enter(total_rows, viewport_rows, viewport_cols);
        tracing::debug!("Entered copy mode");
    }

    /// Handle key events in copy mode.
    fn handle_copy_mode_key(&mut self, event: &KeyEvent) {
        let protocol_key = self.winit_key_to_protocol(&event.logical_key);
        let protocol_mods = self.winit_mods_to_protocol();

        match self.copy_mode.handle_key(&protocol_key, protocol_mods) {
            CopyModeResult::Handled => {
                self.needs_redraw = true;
            }
            CopyModeResult::Exit => {
                tracing::debug!("Exited copy mode");
                self.needs_redraw = true;
            }
            CopyModeResult::Copy(selection) => {
                if let Some(sel) = selection {
                    let text = self.extract_selection_text(&sel);
                    self.copy_to_clipboard(&text);
                }
                self.needs_redraw = true;
            }
            CopyModeResult::CopyAndExit(selection) => {
                if let Some(sel) = selection {
                    let text = self.extract_selection_text(&sel);
                    self.copy_to_clipboard(&text);
                }
                tracing::debug!("Copied and exited copy mode");
                self.needs_redraw = true;
            }
            CopyModeResult::Search(query) => {
                self.perform_search(&query);
                self.needs_redraw = true;
            }
            CopyModeResult::NotHandled => {
                // Key not handled by copy mode
            }
        }
    }

    /// Convert winit Key to protocol Key.
    fn winit_key_to_protocol(&self, key: &Key) -> ProtocolKey {
        match key {
            Key::Named(named) => match named {
                NamedKey::Escape => ProtocolKey::Escape,
                NamedKey::Enter => ProtocolKey::Enter,
                NamedKey::Tab => ProtocolKey::Tab,
                NamedKey::Backspace => ProtocolKey::Backspace,
                NamedKey::Delete => ProtocolKey::Delete,
                NamedKey::Insert => ProtocolKey::Insert,
                NamedKey::Home => ProtocolKey::Home,
                NamedKey::End => ProtocolKey::End,
                NamedKey::PageUp => ProtocolKey::PageUp,
                NamedKey::PageDown => ProtocolKey::PageDown,
                NamedKey::ArrowUp => ProtocolKey::Up,
                NamedKey::ArrowDown => ProtocolKey::Down,
                NamedKey::ArrowLeft => ProtocolKey::Left,
                NamedKey::ArrowRight => ProtocolKey::Right,
                NamedKey::F1 => ProtocolKey::Function(1),
                NamedKey::F2 => ProtocolKey::Function(2),
                NamedKey::F3 => ProtocolKey::Function(3),
                NamedKey::F4 => ProtocolKey::Function(4),
                NamedKey::F5 => ProtocolKey::Function(5),
                NamedKey::F6 => ProtocolKey::Function(6),
                NamedKey::F7 => ProtocolKey::Function(7),
                NamedKey::F8 => ProtocolKey::Function(8),
                NamedKey::F9 => ProtocolKey::Function(9),
                NamedKey::F10 => ProtocolKey::Function(10),
                NamedKey::F11 => ProtocolKey::Function(11),
                NamedKey::F12 => ProtocolKey::Function(12),
                _ => ProtocolKey::Char('\0'),
            },
            Key::Character(c) => {
                if let Some(ch) = c.chars().next() {
                    ProtocolKey::Char(ch)
                } else {
                    ProtocolKey::Char('\0')
                }
            }
            _ => ProtocolKey::Char('\0'),
        }
    }

    /// Convert winit modifiers to protocol modifiers.
    fn winit_mods_to_protocol(&self) -> ProtocolModifiers {
        let mut mods = ProtocolModifiers::NONE;
        if self.modifiers.control_key() {
            mods = mods.union(ProtocolModifiers::CTRL);
        }
        if self.modifiers.shift_key() {
            mods = mods.union(ProtocolModifiers::SHIFT);
        }
        if self.modifiers.alt_key() {
            mods = mods.union(ProtocolModifiers::ALT);
        }
        mods
    }

    /// Extract text from a vim selection.
    fn extract_selection_text(&self, selection: &VimSelection) -> String {
        let buffer = self.terminal().buffer();
        let mut text = String::new();

        match selection {
            VimSelection::Char { start_row, start_col, end_row, end_col } => {
                for row in *start_row..=*end_row {
                    if let Some(grid_row) = buffer.visible_row(row as u16) {
                        let sc = if row == *start_row { *start_col } else { 0 };
                        let ec = if row == *end_row { *end_col } else { grid_row.width() as usize - 1 };

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
            VimSelection::Block { start_row, start_col, end_row, end_col } => {
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

        let buffer = self.terminal().buffer();
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
        let (rows, dims, hyperlink_grid, hyperlink_map): (Vec<String>, _, Vec<Vec<u32>>, std::collections::HashMap<u32, String>) = {
            let terminal = self.terminal();
            let dims = terminal.dimensions();
            let rows: Vec<String> = (0..dims.rows)
                .map(|row| {
                    terminal.buffer().visible_row(row)
                        .map(|r| r.text_trimmed())
                        .unwrap_or_default()
                })
                .collect();

            // Collect hyperlink IDs from visible cells
            let hyperlink_grid: Vec<Vec<u32>> = (0..dims.rows)
                .map(|row| {
                    (0..dims.cols)
                        .map(|col| {
                            terminal.buffer().grid().cell(row, col)
                                .map(|c| c.hyperlink_id())
                                .unwrap_or(0)
                        })
                        .collect()
                })
                .collect();

            // Copy hyperlink map
            let hyperlink_map: std::collections::HashMap<u32, String> = terminal.all_hyperlinks()
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
                hyperlink_grid.get(row as usize)
                    .and_then(|r| r.get(col as usize))
                    .copied()
                    .unwrap_or(0)
            },
            |id| hyperlink_map.get(&id).cloned(),
        );
    }

    /// Reload configuration from disk.
    fn reload_config(&mut self) {
        let new_config = Config::load();

        // Update color scheme
        self.renderer.set_color_scheme(
            ultty::renderer::ColorScheme::from_config(&new_config.colors)
        );

        // Update opacity
        self.renderer.set_opacity(new_config.behavior.opacity);

        // Update ligatures setting
        self.renderer.set_ligatures(new_config.font.ligatures);

        // Update URL detection setting
        self.url_detector.set_enabled(new_config.behavior.url_detection);

        self.config = new_config;
        self.needs_redraw = true;

        tracing::info!("Configuration reloaded");
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
            self.renderer.set_color_scheme(
                ultty::renderer::ColorScheme::from_config(&self.config.colors)
            );

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
                                tracing::info!("Terminal {:?} exited with status {}", terminal_id, status);
                            }
                            TerminalEvent::Bell => {
                                if Some(terminal_id) == focused_id {
                                    // Visual bell - flash border for 100ms
                                    if self.config.behavior.bell_visual {
                                        self.bell_active_until = Some(Instant::now() + Duration::from_millis(100));
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
                                        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                                        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
                                        let passthrough = TmuxController::wrap_passthrough(osc52.as_bytes());
                                        // Write to stdout to reach outer terminal
                                        use std::io::Write;
                                        let _ = std::io::stdout().write_all(&passthrough);
                                        let _ = std::io::stdout().flush();
                                        tracing::debug!("Set clipboard via OSC 52 passthrough: {} chars", data.len());
                                    } else if let Some(ref mut cb) = self.clipboard {
                                        if let Err(e) = cb.set_text(&data) {
                                            tracing::warn!("Failed to set clipboard: {}", e);
                                        } else {
                                            tracing::debug!("Set clipboard via OSC 52: {} chars", data.len());
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
        let now = Instant::now();
        if self.needs_redraw && now.duration_since(self.last_render) >= Duration::from_millis(16) {
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
    }

    /// Render a single terminal pane.
    fn render_single_pane(&mut self) -> Result<(), wgpu::SurfaceError> {
        let terminal = self.terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
            .expect("No focused terminal");

        // Calculate scrollbar state
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();
        let scrollbar = ScrollbarState::new(
            buffer.total_lines(),
            dims.rows as usize,
            buffer.scroll_offset(),
        );

        // Check if bell is still active
        let bell_active = self.bell_active_until
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

        self.renderer.render_with_overlay(&self.surface, terminal, &overlay)
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
        let scrollbar = self.terminal_manager
            .focused_terminal()
            .map(|t| {
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
        let bell_active = self.bell_active_until
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

/// Check if a character is part of a word (for word selection).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
}
