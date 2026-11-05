//! Input event handling: mouse, keyboard, clipboard, and text selection.

use std::time::Instant;

use super::{resize_direction_at_window_edge, App, CLICK_THRESHOLD};
use ultty::config::KeybindingConfig;
use ultty::grid::{
    MouseButton as TerminalMouseButton, MouseModifiers, SelectionMode, SelectionPoint,
};
use ultty::tabs::TITLE_BAR_HEIGHT;
use ultty::terminal_manager::SplitDirection;
use ultty::url::UrlDetector;
use ultty::vim::CopyModeResult;
use ultty_protocol::{Key as ProtocolKey, Modifiers as ProtocolModifiers};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, NamedKey};

const SCROLLBAR_WIDTH: f32 = 8.0;
const SCROLLBAR_MARGIN: f32 = 2.0;

impl App {
    /// Handle window event. Returns true if should exit.
    pub(super) fn handle_window_event(&mut self, event: WindowEvent) -> bool {
        // Check for pending close from window control button
        if self.pending_close {
            tracing::info!("Close requested");
            self.save_session();
            return true;
        }

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
                } else if self.terminal().is_some_and(|t| t.track_mouse_motion()) {
                    // AnyEvent mode reports motion even without button pressed
                    self.handle_mouse_motion();
                }
                self.update_tab_hover();
                if !self.update_resize_cursor() {
                    self.update_cursor_for_url();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(button, state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                tracing::debug!("WindowEvent::MouseWheel received: delta={:?}", delta);
                self.handle_scroll(delta);
            }
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                // Forward IME committed text to native protocol sessions as TextInput
                if let Some(ref mut server) = self.protocol_server {
                    if server.session_count() > 0 {
                        server.broadcast_text_input(text.clone());
                    }
                }
                // Also send the text to the terminal PTY
                if let Err(e) = self.send_input(text.as_bytes()) {
                    tracing::warn!("Failed to send IME input: {}", e);
                }
            }
            WindowEvent::RedrawRequested => {
                // Redraw requests are delivery events, not state changes. Dirty state is
                // set by input, PTY output, native protocol updates, timers, and resize.
            }
            WindowEvent::Focused(focused)
                if self.terminal().is_some_and(|t| t.modes().focus_reporting) =>
            {
                // Send focus in/out escape sequence
                let seq = if focused { b"\x1b[I" } else { b"\x1b[O" };
                let _ = self.send_input(seq);
            }
            _ => {}
        }
        false
    }

    /// Convert pixel position to terminal cell coordinates.
    pub(super) fn pixel_to_cell(&self, x: f64, y: f64) -> (u16, u16) {
        let (cell_width, cell_height) = self.renderer.cell_dimensions();
        if cell_width <= 0.0 || cell_height <= 0.0 {
            return (0, 0);
        }
        let col = (x / cell_width as f64).floor().clamp(0.0, u16::MAX as f64) as u16;
        let row = (y / cell_height as f64).floor().clamp(0.0, u16::MAX as f64) as u16;
        (col, row)
    }

    /// Convert winit modifiers to terminal mouse modifiers.
    fn mouse_modifiers(&self) -> MouseModifiers {
        MouseModifiers::new(
            self.modifiers.shift_key(),
            self.modifiers.alt_key(),
            self.modifiers.control_key(),
            self.modifiers.super_key(),
        )
    }

    /// Forward a mouse event to native protocol sessions.
    /// Returns true if any session consumed the event.
    fn forward_mouse_to_native_sessions(
        &mut self,
        event_type: ultty_protocol::InputType,
        cell_x: f32,
        cell_y: f32,
    ) -> bool {
        let modifiers = self.winit_mods_to_protocol();
        if let Some(ref mut server) = self.protocol_server {
            if server.session_count() > 0 {
                server.broadcast_mouse(event_type, cell_x, cell_y, modifiers);
                return true;
            }
        }
        false
    }

    /// Convert winit MouseButton to protocol MouseButton.
    fn winit_to_protocol_mouse_button(button: MouseButton) -> Option<ultty_protocol::MouseButton> {
        match button {
            MouseButton::Left => Some(ultty_protocol::MouseButton::Left),
            MouseButton::Middle => Some(ultty_protocol::MouseButton::Middle),
            MouseButton::Right => Some(ultty_protocol::MouseButton::Right),
            _ => None,
        }
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
        if self.terminal().is_some_and(|t| t.mouse_tracking_enabled()) {
            // Don't intercept UI elements when mouse tracking is on
            if self.tab_bar.visible && y < self.tab_bar.height() {
                // Still allow tab bar interaction
            } else if self.is_in_scrollbar(x, y) {
                // Still allow scrollbar interaction
            } else {
                // Convert to cell coordinates and send to terminal
                let (col, row) = self.pixel_to_cell(
                    self.mouse_pos.x,
                    self.mouse_pos.y - self.tab_bar.height() as f64,
                );
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
                let mods = self.mouse_modifiers();
                if let Some(bytes) = self
                    .terminal()
                    .and_then(|t| t.encode_mouse_event(term_button, col, row, pressed, false, mods))
                {
                    let _ = self.send_input(&bytes);
                }
                return;
            }
        }

        match state {
            ElementState::Pressed => {
                // Edge resize for CSD (no native decorations)
                if button == MouseButton::Left {
                    if !self.tab_bar.is_title_bar_toggle_area(x, y) {
                        self.last_title_bar_toggle_click_time =
                            Instant::now() - CLICK_THRESHOLD * 2;
                    }

                    let size = self.window.inner_size();
                    let w = size.width as f32;
                    let h = size.height as f32;
                    let over_chrome_button = self.tab_bar.visible
                        && (self.tab_bar.is_new_tab_button_at(x, y)
                            || self.tab_bar.window_control_at(x, y).is_some());
                    if !over_chrome_button {
                        if let Some(direction) = resize_direction_at_window_edge(x, y, w, h) {
                            let _ = self.window.drag_resize_window(direction);
                            return;
                        }
                    }
                }

                // Check for tab bar clicks first
                if self.tab_bar.visible && y < self.tab_bar.height() {
                    // Window control buttons
                    if button == MouseButton::Left {
                        if let Some(control) = self.tab_bar.window_control_at(x, y) {
                            use ultty::tabs::WindowControl;
                            match control {
                                WindowControl::Close => {
                                    self.pending_close = true;
                                }
                                WindowControl::Maximize => {
                                    self.window.set_maximized(!self.window.is_maximized());
                                }
                                WindowControl::Minimize => {
                                    self.window.set_minimized(true);
                                }
                            }
                            return;
                        }

                        // Draggable title bar area
                        if self.tab_bar.is_drag_area(x, y) {
                            if self.tab_bar.is_title_bar_toggle_area(x, y) {
                                let now = Instant::now();
                                if now.duration_since(self.last_title_bar_toggle_click_time)
                                    < CLICK_THRESHOLD
                                {
                                    self.window.set_maximized(!self.window.is_maximized());
                                    self.last_title_bar_toggle_click_time =
                                        now - CLICK_THRESHOLD * 2;
                                    return;
                                }
                                self.last_title_bar_toggle_click_time = now;
                            }
                            let _ = self.window.drag_window();
                            return;
                        }
                    }

                    let is_tab_row = self.tab_bar.shows_tab_row() && y >= TITLE_BAR_HEIGHT;

                    // Middle click closes tab
                    if button == MouseButton::Middle && is_tab_row {
                        if let Some(tab_idx) = self.tab_bar.tab_at_x(x) {
                            if self.terminal_manager.tab_count() > 1 {
                                let tabs = self.terminal_manager.tabs();
                                if let Some(tab) = tabs.get(tab_idx) {
                                    let tab_id = tab.id;
                                    self.freeze_tab_width_after_close();
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
                                self.freeze_tab_width_after_close();
                                self.terminal_manager.close_tab(tab_id);
                                tracing::info!("Closed tab {} via close button", tab_idx);
                                self.update_tab_bar();
                                self.needs_redraw = true;
                            }
                        }
                        return;
                    }

                    // Click on "+" new tab button
                    if button == MouseButton::Left && self.tab_bar.is_new_tab_button_at(x, y) {
                        match self.create_tab() {
                            Ok(_) => {
                                self.clear_tab_width_freeze();
                                tracing::info!("Created new tab via + button");
                                self.update_tab_bar();
                                self.needs_redraw = true;
                            }
                            Err(e) => tracing::warn!("Failed to create tab: {}", e),
                        }
                        return;
                    }

                    // Click on tab - start drag and switch to it
                    if is_tab_row {
                        if let Some(tab_idx) = self.tab_bar.tab_at_x(x) {
                            self.tab_bar.start_drag(tab_idx, x);
                            self.terminal_manager.switch_to_tab(tab_idx);
                            self.update_tab_bar();
                            self.needs_redraw = true;
                            tracing::debug!("Started drag on tab {}", tab_idx);
                        }
                    }
                    return;
                }

                // Not in tab bar - handle normal clicks

                // In multi-pane mode, check for divider drag or pane focus
                if button == MouseButton::Left && self.terminal_manager.pane_count() > 1 {
                    let size = self.window.inner_size();
                    let tab_h = self.tab_bar.height();
                    let content_y = y - tab_h;
                    let aw = size.width as f32;
                    let ah = size.height as f32 - tab_h;

                    // Check if clicking on a divider (4px tolerance)
                    let dir = self.terminal_manager.split_direction();
                    let (pos, total) = match dir {
                        SplitDirection::Horizontal => (x, aw),
                        SplitDirection::Vertical => (content_y, ah),
                    };
                    if let Some(div_idx) =
                        self.terminal_manager.divider_at_position(pos, total, 4.0)
                    {
                        self.divider_dragging = Some((div_idx, pos));
                        return;
                    }

                    // Otherwise, focus the clicked pane
                    if let Some(idx) = self.terminal_manager.pane_at_position(x, content_y, aw, ah)
                    {
                        if idx != self.terminal_manager.focused_pane_index() {
                            self.terminal_manager.focus_pane(idx);
                            self.needs_redraw = true;
                        }
                    }
                }

                if button != MouseButton::Left {
                    return;
                }

                // Check for scrollbar click
                if self.is_in_scrollbar(x, y) {
                    self.scrollbar_dragging = true;
                    self.handle_scrollbar_click(y);
                    return;
                }

                let (col, row) = self.pixel_to_cell(
                    self.mouse_pos.x,
                    self.mouse_pos.y - self.tab_bar.height() as f64,
                );

                // Forward to native protocol sessions
                if let Some(proto_button) = Self::winit_to_protocol_mouse_button(button) {
                    let event_type = ultty_protocol::InputType::MouseDown {
                        button: proto_button,
                    };
                    self.forward_mouse_to_native_sessions(event_type, col as f32, row as f32);
                }

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
                } else if let Some(mut t) = self.terminal_mut() {
                    t.start_selection(point, mode);
                }

                self.needs_redraw = true;
            }
            ElementState::Released => {
                self.mouse_pressed = false;
                self.scrollbar_dragging = false;
                self.divider_dragging = None;

                // Forward MouseUp to native protocol sessions
                if let Some(proto_button) = Self::winit_to_protocol_mouse_button(button) {
                    let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
                    let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y.max(0.0));
                    let event_type = ultty_protocol::InputType::MouseUp {
                        button: proto_button,
                    };
                    self.forward_mouse_to_native_sessions(event_type, col as f32, row as f32);
                }

                // End tab drag if active
                if self.tab_bar.is_dragging() {
                    self.tab_bar.end_drag();
                    self.needs_redraw = true;
                }

                // Copy selection to clipboard on release (if copy_on_select enabled)
                if self.config.behavior.copy_on_select {
                    if let Some(text) = self.terminal().and_then(|t| t.selected_text()) {
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

        // Handle pane divider dragging
        if let Some((div_idx, last_pos)) = self.divider_dragging {
            let size = self.window.inner_size();
            let tab_h = self.tab_bar.height();
            let dir = self.terminal_manager.split_direction();
            let (current_pos, total) = match dir {
                SplitDirection::Horizontal => (self.mouse_pos.x as f32, size.width as f32),
                SplitDirection::Vertical => {
                    (self.mouse_pos.y as f32 - tab_h, size.height as f32 - tab_h)
                }
            };
            let delta = current_pos - last_pos;
            if delta.abs() > 1.0 {
                self.terminal_manager.resize_pane(div_idx, delta, total);
                self.divider_dragging = Some((div_idx, current_pos));
                self.needs_redraw = true;
            }
            return;
        }

        // Account for tab bar offset
        let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
        if adjusted_y < 0.0 {
            return; // Dragging in tab bar area
        }

        // Forward mouse move to native protocol sessions
        {
            let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
            self.forward_mouse_to_native_sessions(
                ultty_protocol::InputType::MouseMove,
                col as f32,
                row as f32,
            );
        }

        // Check if motion tracking is enabled
        if self.terminal().is_some_and(|t| t.track_mouse_motion()) && self.mouse_pressed {
            let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
            // When button is held, report as Left button for drag
            let mods = self.mouse_modifiers();
            if let Some(bytes) = self.terminal().and_then(|t| {
                t.encode_mouse_event(TerminalMouseButton::Left, col, row, true, true, mods)
            }) {
                let _ = self.send_input(&bytes);
            }
            return;
        }

        let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y);
        let point = SelectionPoint::new(col as i32, row as i32);

        if let Some(mut t) = self.terminal_mut() {
            t.update_selection(point);
        }
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
        let mods = self.mouse_modifiers();
        if let Some(bytes) = self.terminal().and_then(|t| {
            t.encode_mouse_event(TerminalMouseButton::Release, col, row, true, true, mods)
        }) {
            let _ = self.send_input(&bytes);
        }
    }

    /// Check if a position is within the scrollbar area.
    fn is_in_scrollbar(&self, x: f32, y: f32) -> bool {
        let size = self.window.inner_size();
        is_in_scrollbar_hitbox(
            x,
            y,
            size.width as f32,
            size.height as f32,
            self.tab_bar.height(),
        )
    }

    /// Handle scrollbar click/drag - scroll to position.
    fn handle_scrollbar_click(&mut self, y: f32) {
        let size = self.window.inner_size();
        let Some(terminal) = self.terminal() else {
            return;
        };
        let total_lines = terminal.buffer().total_lines();
        let viewport_rows = terminal.dimensions().rows as usize;
        drop(terminal);

        let Some(target_offset) = scrollbar_target_offset(
            y,
            size.height as f32,
            self.tab_bar.height(),
            total_lines,
            viewport_rows,
        ) else {
            return;
        };

        // Apply scroll
        let Some(mut t) = self.terminal_mut() else {
            return;
        };
        let buffer = t.buffer_mut();
        let current = buffer.scroll_offset();
        if target_offset > current {
            buffer.scroll_up(target_offset - current);
        } else if target_offset < current {
            buffer.scroll_down(current - target_offset);
        }
        drop(t);

        self.needs_redraw = true;
    }

    /// Select the word at the given position.
    fn select_word(&mut self, col: u16, row: u16) {
        // First, find word boundaries (immutable borrow)
        let word_bounds = {
            let Some(terminal) = self.terminal() else {
                return;
            };
            let grid = terminal.grid();

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
            if let Some(mut t) = self.terminal_mut() {
                t.start_selection(start, SelectionMode::Normal);
            }
            if let Some(mut t) = self.terminal_mut() {
                t.update_selection(end);
            }
        }
    }

    /// Handle scroll wheel.
    fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        tracing::debug!("handle_scroll: delta={:?}", delta);

        // winit scroll delta: positive y = scroll UP (wheel rolled away from user)
        // This should reveal content ABOVE (scroll up in terminal terms)
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                self.scroll_line_accum += y as f64;
                let lines = self.scroll_line_accum.trunc() as i32;
                self.scroll_line_accum -= lines as f64;
                self.scroll_pixel_accum = 0.0;
                lines
            }
            MouseScrollDelta::PixelDelta(pos) => {
                // Accumulate pixel deltas to avoid dropping small scroll events
                // (common on Wayland/HiDPI where each event is a few pixels)
                self.scroll_pixel_accum += pos.y;
                self.scroll_line_accum = 0.0;
                let (_, cell_height) = self.renderer.cell_dimensions();
                let cell_h = (cell_height as f64).max(1.0);
                let lines = (self.scroll_pixel_accum / cell_h).trunc() as i32;
                self.scroll_pixel_accum -= lines as f64 * cell_h;
                lines
            }
        };

        tracing::debug!(
            "handle_scroll: computed lines={}, pixel_accum={}, line_accum={}",
            lines,
            self.scroll_pixel_accum,
            self.scroll_line_accum
        );

        if lines == 0 {
            return;
        }

        // Check if mouse tracking is enabled - send scroll events to PTY
        if self.terminal().is_some_and(|t| t.mouse_tracking_enabled()) {
            let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
            let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y.max(0.0));
            // Terminal scroll: positive lines = scroll up (button 64)
            // Button 64 = scroll up, Button 65 = scroll down
            let button = if lines > 0 {
                TerminalMouseButton::ScrollUp
            } else {
                TerminalMouseButton::ScrollDown
            };
            let count = lines.unsigned_abs() as usize;
            let mods = self.mouse_modifiers();

            // Debug: log scroll events
            let mouse_mode = self.terminal().map(|t| t.modes().mouse_mode);
            tracing::debug!(
                "Mouse scroll: lines={}, button={:?}, pos=({},{}), mode={:?}",
                lines,
                button,
                col,
                row,
                mouse_mode
            );

            for _ in 0..count {
                if let Some(bytes) = self
                    .terminal()
                    .and_then(|t| t.encode_mouse_event(button, col, row, true, false, mods))
                {
                    tracing::debug!(
                        "Sending scroll bytes: {:?}",
                        String::from_utf8_lossy(&bytes)
                    );
                    let _ = self.send_input(&bytes);
                }
            }
            return;
        }

        // Forward scroll to native protocol sessions
        {
            let adjusted_y = self.mouse_pos.y - self.tab_bar.height() as f64;
            let (col, row) = self.pixel_to_cell(self.mouse_pos.x, adjusted_y.max(0.0));
            let delta_y = match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
            };
            self.forward_mouse_to_native_sessions(
                ultty_protocol::InputType::MouseScroll {
                    delta_x: 0.0,
                    delta_y,
                },
                col as f32,
                row as f32,
            );
        }

        // Check if in alternate screen with alternate scroll mode
        // In this case, send arrow keys instead of scrolling
        let in_alternate = self.terminal().is_some_and(|t| t.is_alternate_screen());
        let alternate_scroll = self.terminal().is_some_and(|t| t.modes().alternate_scroll);

        if in_alternate && alternate_scroll {
            // Send arrow keys for scrolling in alternate screen
            // Check DECCKM mode to send correct escape sequence
            let app_cursor = self
                .terminal()
                .is_some_and(|t| t.modes().application_cursor_keys);
            let count = lines.unsigned_abs() as usize;
            let arrow: &[u8] = if lines > 0 {
                if app_cursor {
                    b"\x1bOA"
                } else {
                    b"\x1b[A"
                } // Up
            } else if app_cursor {
                b"\x1bOB" // Down (application mode)
            } else {
                b"\x1b[B" // Down (normal mode)
            };
            for _ in 0..count {
                let _ = self.send_input(arrow);
            }
            return;
        }

        // In alternate screen without alternate_scroll, ignore scroll
        if in_alternate {
            return;
        }

        // Normal mode: scroll the scrollback buffer
        let Some(mut t) = self.terminal_mut() else {
            return;
        };
        let buffer = t.buffer_mut();
        let offset_before = buffer.scroll_offset();

        tracing::debug!(
            "Scrollback: lines={}, offset_before={}",
            lines,
            offset_before
        );

        if lines > 0 {
            buffer.scroll_up(lines as usize);
        } else {
            buffer.scroll_down((-lines) as usize);
        }
        drop(t);

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
                    let bracketed = self.terminal().is_some_and(|t| t.modes().bracketed_paste);
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
        if event.state == ElementState::Released {
            self.forward_key_up_to_native_sessions(&event);
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

        // Check for find/search (Ctrl+Shift+F)
        if self.check_keybinding(&event, &self.config.keybindings.find.clone()) {
            // If native sessions are active, also search their scene trees
            if self.has_native_sessions() {
                self.native_search_active = true;
                self.native_search_query.clear();
                self.native_search_matches.clear();
                self.native_search_index = 0;
            }
            self.enter_search_mode();
            self.needs_redraw = true;
            return;
        }

        // Tab keybindings
        if self.check_keybinding(&event, &self.config.keybindings.new_tab.clone()) {
            match self.create_tab() {
                Ok(tab_id) => {
                    self.clear_tab_width_freeze();
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
                    self.freeze_tab_width_after_close();
                    self.terminal_manager.close_tab(tab_id);
                    self.renderer.clear_image_cache();
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

        // Pane navigation — direction-aware based on split orientation
        {
            let dir = self.terminal_manager.split_direction();
            let right =
                self.check_keybinding(&event, &self.config.keybindings.focus_pane_right.clone());
            let down =
                self.check_keybinding(&event, &self.config.keybindings.focus_pane_down.clone());
            let left =
                self.check_keybinding(&event, &self.config.keybindings.focus_pane_left.clone());
            let up = self.check_keybinding(&event, &self.config.keybindings.focus_pane_up.clone());

            let next = match dir {
                SplitDirection::Horizontal => right,
                SplitDirection::Vertical => down,
            };
            let prev = match dir {
                SplitDirection::Horizontal => left,
                SplitDirection::Vertical => up,
            };
            if next {
                self.terminal_manager.focus_next_pane();
                self.needs_redraw = true;
                return;
            }
            if prev {
                self.terminal_manager.focus_prev_pane();
                self.needs_redraw = true;
                return;
            }
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
            if self.shortcuts_visible {
                self.protocol_status_visible = false;
            }
            self.needs_redraw = true;
            return;
        }

        if self.check_keybinding(&event, &self.config.keybindings.protocol_status.clone()) {
            self.protocol_status_visible = !self.protocol_status_visible;
            if self.protocol_status_visible {
                self.shortcuts_visible = false;
            }
            self.needs_redraw = true;
            return;
        }

        if self.check_keybinding(&event, &self.config.keybindings.cycle_theme.clone()) {
            self.cycle_theme();
            return;
        }

        // Font size shortcuts
        if self.check_keybinding(&event, &self.config.keybindings.font_increase.clone()) {
            self.change_font_size(true);
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.font_decrease.clone()) {
            self.change_font_size(false);
            return;
        }
        if self.check_keybinding(&event, &self.config.keybindings.font_reset.clone()) {
            self.reset_font_size();
            return;
        }

        // Screenshot shortcut
        if self.check_keybinding(&event, &self.config.keybindings.screenshot.clone()) {
            self.take_screenshot();
            return;
        }

        // Debug dump shortcut
        if self.check_keybinding(&event, &self.config.keybindings.debug_dump.clone()) {
            self.debug_dump_terminal();
            return;
        }

        // Close modal overlays with Escape
        if self.shortcuts_visible || self.protocol_status_visible {
            if let Key::Named(NamedKey::Escape) = &event.logical_key {
                self.shortcuts_visible = false;
                self.protocol_status_visible = false;
                self.needs_redraw = true;
                return;
            }
            // Block other keys while a modal overlay is visible.
            return;
        }

        // If in copy mode, route to vim handler
        if self.copy_mode.is_active() {
            self.handle_copy_mode_key(&event);
            return;
        }

        // Clear selection on any key press (except modifiers)
        if !matches!(
            event.logical_key,
            Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super)
        ) {
            if let Some(mut t) = self.terminal_mut() {
                t.clear_selection();
            }
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
            if let Some(text) = self.terminal().and_then(|t| t.selected_text()) {
                self.copy_to_clipboard(&text);
            }
            return;
        }

        // Check if application cursor keys mode is enabled (DECCKM)
        let app_cursor = self
            .terminal()
            .is_some_and(|t| t.modes().application_cursor_keys);

        // Compute xterm modifier code: Shift=2, Alt=3, Shift+Alt=4, Ctrl=5,
        // Shift+Ctrl=6, Alt+Ctrl=7, Shift+Alt+Ctrl=8
        let modifier_code = {
            let mut m = 0u8;
            if self.modifiers.shift_key() {
                m |= 1;
            }
            if self.modifiers.alt_key() {
                m |= 2;
            }
            if self.modifiers.control_key() {
                m |= 4;
            }
            if m > 0 {
                m + 1
            } else {
                0
            }
        };

        // Helper: send a dynamically formatted sequence and return early
        macro_rules! send_modified {
            ($self:expr, $seq:expr) => {{
                if let Err(e) = $self.send_input($seq.as_bytes()) {
                    tracing::warn!("Failed to send input: {}", e);
                }
                $self.forward_key_to_native_sessions(&event);
                return;
            }};
        }

        // Convert key to terminal input
        let input: Option<&[u8]> = match &event.logical_key {
            Key::Named(named) => match named {
                NamedKey::Space => Some(b" "),
                NamedKey::Enter => Some(b"\r"),
                NamedKey::Backspace => Some(b"\x7f"),
                NamedKey::Tab => Some(b"\t"),
                NamedKey::Escape => Some(b"\x1b"),

                // Arrow keys: modified → CSI 1;{mod} X, unmodified → SS3 X or CSI X
                NamedKey::ArrowUp
                | NamedKey::ArrowDown
                | NamedKey::ArrowRight
                | NamedKey::ArrowLeft => {
                    let suffix = match named {
                        NamedKey::ArrowUp => 'A',
                        NamedKey::ArrowDown => 'B',
                        NamedKey::ArrowRight => 'C',
                        _ => 'D',
                    };
                    if modifier_code > 0 {
                        send_modified!(self, format!("\x1b[1;{}{}", modifier_code, suffix));
                    }
                    Some(if app_cursor {
                        match suffix {
                            'A' => &b"\x1bOA"[..],
                            'B' => &b"\x1bOB"[..],
                            'C' => &b"\x1bOC"[..],
                            _ => &b"\x1bOD"[..],
                        }
                    } else {
                        match suffix {
                            'A' => &b"\x1b[A"[..],
                            'B' => &b"\x1b[B"[..],
                            'C' => &b"\x1b[C"[..],
                            _ => &b"\x1b[D"[..],
                        }
                    })
                }

                // Home/End: modified → CSI 1;{mod} H/F
                NamedKey::Home | NamedKey::End => {
                    let suffix = if matches!(named, NamedKey::Home) {
                        'H'
                    } else {
                        'F'
                    };
                    if modifier_code > 0 {
                        send_modified!(self, format!("\x1b[1;{}{}", modifier_code, suffix));
                    }
                    Some(match (app_cursor, suffix) {
                        (true, 'H') => &b"\x1bOH"[..],
                        (true, _) => &b"\x1bOF"[..],
                        (false, 'H') => &b"\x1b[H"[..],
                        (false, _) => &b"\x1b[F"[..],
                    })
                }

                // Tilde-encoded keys: modified → CSI {n};{mod} ~
                NamedKey::PageUp | NamedKey::PageDown | NamedKey::Insert | NamedKey::Delete => {
                    let n = match named {
                        NamedKey::Insert => 2,
                        NamedKey::Delete => 3,
                        NamedKey::PageUp => 5,
                        _ => 6,
                    };
                    if modifier_code > 0 {
                        send_modified!(self, format!("\x1b[{};{}~", n, modifier_code));
                    }
                    match n {
                        2 => Some(&b"\x1b[2~"[..]),
                        3 => Some(&b"\x1b[3~"[..]),
                        5 => Some(&b"\x1b[5~"[..]),
                        _ => Some(&b"\x1b[6~"[..]),
                    }
                }

                // F1-F4: modified → CSI 1;{mod} P/Q/R/S
                NamedKey::F1 | NamedKey::F2 | NamedKey::F3 | NamedKey::F4 => {
                    let suffix = match named {
                        NamedKey::F1 => 'P',
                        NamedKey::F2 => 'Q',
                        NamedKey::F3 => 'R',
                        _ => 'S',
                    };
                    if modifier_code > 0 {
                        send_modified!(self, format!("\x1b[1;{}{}", modifier_code, suffix));
                    }
                    match suffix {
                        'P' => Some(&b"\x1bOP"[..]),
                        'Q' => Some(&b"\x1bOQ"[..]),
                        'R' => Some(&b"\x1bOR"[..]),
                        _ => Some(&b"\x1bOS"[..]),
                    }
                }

                // F5-F12: modified → CSI {n};{mod} ~
                NamedKey::F5
                | NamedKey::F6
                | NamedKey::F7
                | NamedKey::F8
                | NamedKey::F9
                | NamedKey::F10
                | NamedKey::F11
                | NamedKey::F12 => {
                    let n = match named {
                        NamedKey::F5 => 15,
                        NamedKey::F6 => 17,
                        NamedKey::F7 => 18,
                        NamedKey::F8 => 19,
                        NamedKey::F9 => 20,
                        NamedKey::F10 => 21,
                        NamedKey::F11 => 23,
                        _ => 24,
                    };
                    if modifier_code > 0 {
                        send_modified!(self, format!("\x1b[{};{}~", n, modifier_code));
                    }
                    match n {
                        15 => Some(&b"\x1b[15~"[..]),
                        17 => Some(&b"\x1b[17~"[..]),
                        18 => Some(&b"\x1b[18~"[..]),
                        19 => Some(&b"\x1b[19~"[..]),
                        20 => Some(&b"\x1b[20~"[..]),
                        21 => Some(&b"\x1b[21~"[..]),
                        23 => Some(&b"\x1b[23~"[..]),
                        _ => Some(&b"\x1b[24~"[..]),
                    }
                }

                _ => None,
            },
            Key::Character(c) => {
                let ch = c.chars().next().unwrap_or('\0');
                // Handle Ctrl+key combinations
                if self.modifiers.control_key()
                    && !self.modifiers.shift_key()
                    && ch.is_ascii_alphabetic()
                {
                    let ctrl_char = (ch.to_ascii_lowercase() as u8 - b'a' + 1) as char;
                    if let Err(e) = self.send_input(&[ctrl_char as u8]) {
                        tracing::warn!("Failed to send input: {}", e);
                    }
                    self.forward_key_to_native_sessions(&event);
                    return;
                }
                // Handle Alt+key combinations (ESC prefix)
                if self.modifiers.alt_key() {
                    let mut buf = [0u8; 5]; // ESC + up to 4 UTF-8 bytes
                    buf[0] = 0x1b;
                    let encoded = ch.encode_utf8(&mut buf[1..]);
                    let len = 1 + encoded.len();
                    if let Err(e) = self.send_input(&buf[..len]) {
                        tracing::warn!("Failed to send input: {}", e);
                    }
                    self.forward_key_to_native_sessions(&event);
                    return;
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
            if let Err(e) = self.send_input(c.as_bytes()) {
                tracing::warn!("Failed to send input: {}", e);
            }
        }

        self.forward_key_to_native_sessions(&event);
    }

    /// Convert a winit key event into protocol key + modifiers.
    fn convert_key_event(&self, event: &KeyEvent) -> Option<(ProtocolKey, ProtocolModifiers)> {
        let protocol_key = match &event.logical_key {
            Key::Named(named) => match named {
                NamedKey::Escape => Some(ProtocolKey::Escape),
                NamedKey::Enter => Some(ProtocolKey::Enter),
                NamedKey::Tab => Some(ProtocolKey::Tab),
                NamedKey::Backspace => Some(ProtocolKey::Backspace),
                NamedKey::Delete => Some(ProtocolKey::Delete),
                NamedKey::Insert => Some(ProtocolKey::Insert),
                NamedKey::Home => Some(ProtocolKey::Home),
                NamedKey::End => Some(ProtocolKey::End),
                NamedKey::PageUp => Some(ProtocolKey::PageUp),
                NamedKey::PageDown => Some(ProtocolKey::PageDown),
                NamedKey::ArrowUp => Some(ProtocolKey::Up),
                NamedKey::ArrowDown => Some(ProtocolKey::Down),
                NamedKey::ArrowLeft => Some(ProtocolKey::Left),
                NamedKey::ArrowRight => Some(ProtocolKey::Right),
                NamedKey::Space => Some(ProtocolKey::Char(' ')),
                NamedKey::F1 => Some(ProtocolKey::Function(1)),
                NamedKey::F2 => Some(ProtocolKey::Function(2)),
                NamedKey::F3 => Some(ProtocolKey::Function(3)),
                NamedKey::F4 => Some(ProtocolKey::Function(4)),
                NamedKey::F5 => Some(ProtocolKey::Function(5)),
                NamedKey::F6 => Some(ProtocolKey::Function(6)),
                NamedKey::F7 => Some(ProtocolKey::Function(7)),
                NamedKey::F8 => Some(ProtocolKey::Function(8)),
                NamedKey::F9 => Some(ProtocolKey::Function(9)),
                NamedKey::F10 => Some(ProtocolKey::Function(10)),
                NamedKey::F11 => Some(ProtocolKey::Function(11)),
                NamedKey::F12 => Some(ProtocolKey::Function(12)),
                _ => None,
            },
            Key::Character(c) => c.chars().next().map(ProtocolKey::Char),
            _ => None,
        }?;

        let mut modifiers = ProtocolModifiers::NONE;
        if self.modifiers.shift_key() {
            modifiers = modifiers.union(ProtocolModifiers::SHIFT);
        }
        if self.modifiers.control_key() {
            modifiers = modifiers.union(ProtocolModifiers::CTRL);
        }
        if self.modifiers.alt_key() {
            modifiers = modifiers.union(ProtocolModifiers::ALT);
        }
        if self.modifiers.super_key() {
            modifiers = modifiers.union(ProtocolModifiers::META);
        }

        Some((protocol_key, modifiers))
    }

    fn forward_key_to_native_sessions(&mut self, event: &KeyEvent) {
        let Some((key, modifiers)) = self.convert_key_event(event) else {
            return;
        };
        let Some(ref mut server) = self.protocol_server else {
            return;
        };
        if server.session_count() > 0 {
            server.broadcast_key(key, modifiers);
        }
    }

    fn forward_key_up_to_native_sessions(&mut self, event: &KeyEvent) {
        let Some((key, modifiers)) = self.convert_key_event(event) else {
            return;
        };
        let Some(ref mut server) = self.protocol_server else {
            return;
        };
        if server.session_count() > 0 {
            server.broadcast_key_up(key, modifiers);
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
                let event_key = c.to_lowercase();
                parsed.key == event_key
                    || (parsed.key == "?" && event_key == "/")
                    || (parsed.key == "/" && event_key == "?")
            }
            _ => false,
        }
    }

    fn enter_copy_mode(&mut self) {
        let Some(terminal) = self.terminal() else {
            return;
        };
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();

        let total_rows = buffer.total_lines();
        let viewport_rows = dims.rows as usize;
        let viewport_cols = dims.cols as usize;
        drop(terminal);

        self.copy_mode
            .enter(total_rows, viewport_rows, viewport_cols);
        tracing::debug!("Entered copy mode");
    }

    fn enter_search_mode(&mut self) {
        let Some(terminal) = self.terminal() else {
            return;
        };
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();

        let total_rows = buffer.total_lines();
        let viewport_rows = dims.rows as usize;
        let viewport_cols = dims.cols as usize;
        drop(terminal);

        self.copy_mode
            .enter_search(total_rows, viewport_rows, viewport_cols);
        tracing::debug!("Entered search mode");
    }

    fn handle_copy_mode_key(&mut self, event: &KeyEvent) {
        let protocol_key = self.winit_key_to_protocol(&event.logical_key);
        let protocol_mods = self.winit_mods_to_protocol();

        // Word motions need the text of the row the copy-mode cursor is on.
        let (cursor_row, _) = self.copy_mode.cursor_position();
        let row_text = self
            .terminal()
            .and_then(|t| t.buffer().row_at(cursor_row).map(|row| row.text()))
            .unwrap_or_default();

        match self
            .copy_mode
            .handle_key_in_row(&protocol_key, protocol_mods, &row_text)
        {
            CopyModeResult::Handled => {
                // If native search is active, mirror n/N navigation
                if self.native_search_active && !self.native_search_matches.is_empty() {
                    // Check if the key was n or N by looking at the key
                    match &protocol_key {
                        ProtocolKey::Char('n') => self.native_search_next(),
                        ProtocolKey::Char('N') => self.native_search_prev(),
                        _ => {}
                    }
                }
                self.needs_redraw = true;
            }
            CopyModeResult::Exit => {
                tracing::debug!("Exited copy mode");
                if self.native_search_active {
                    self.dismiss_native_search();
                }
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
                if self.native_search_active {
                    self.dismiss_native_search();
                }
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
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
}

fn is_in_scrollbar_hitbox(
    x: f32,
    y: f32,
    window_width: f32,
    window_height: f32,
    tab_bar_height: f32,
) -> bool {
    let scrollbar_x = window_width - SCROLLBAR_WIDTH - SCROLLBAR_MARGIN;
    let scrollbar_top = tab_bar_height + SCROLLBAR_MARGIN;
    let scrollbar_bottom = window_height - SCROLLBAR_MARGIN;

    x >= scrollbar_x && y >= scrollbar_top && y <= scrollbar_bottom
}

fn scrollbar_target_offset(
    y: f32,
    window_height: f32,
    tab_bar_height: f32,
    total_lines: usize,
    viewport_rows: usize,
) -> Option<usize> {
    if total_lines <= viewport_rows {
        return None;
    }

    let scrollbar_top = tab_bar_height + SCROLLBAR_MARGIN;
    let scrollbar_height = window_height - tab_bar_height - SCROLLBAR_MARGIN * 2.0;
    if scrollbar_height <= 0.0 {
        return None;
    }

    let fraction = ((y - scrollbar_top) / scrollbar_height).clamp(0.0, 1.0);
    let max_offset = total_lines - viewport_rows;
    Some(((1.0 - fraction) * max_offset as f32).round() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_word_char_alphanumeric() {
        // ASCII letters
        assert!(is_word_char('a'));
        assert!(is_word_char('z'));
        assert!(is_word_char('A'));
        assert!(is_word_char('Z'));
        assert!(is_word_char('m'));
        // Digits
        assert!(is_word_char('0'));
        assert!(is_word_char('5'));
        assert!(is_word_char('9'));
    }

    #[test]
    fn is_word_char_special() {
        assert!(is_word_char('_'));
        assert!(is_word_char('-'));
        assert!(is_word_char('.'));
    }

    #[test]
    fn is_word_char_space() {
        assert!(!is_word_char(' '));
        assert!(!is_word_char('\t'));
        assert!(!is_word_char('\n'));
    }

    #[test]
    fn is_word_char_punctuation() {
        assert!(!is_word_char(','));
        assert!(!is_word_char(';'));
        assert!(!is_word_char('('));
        assert!(!is_word_char(')'));
        assert!(!is_word_char(':'));
        assert!(!is_word_char('!'));
        assert!(!is_word_char('@'));
        assert!(!is_word_char('#'));
    }

    #[test]
    fn is_word_char_unicode() {
        // Accented Latin characters
        assert!(is_word_char('é'));
        assert!(is_word_char('ñ'));
        assert!(is_word_char('ü'));
        // CJK characters
        assert!(is_word_char('中'));
        assert!(is_word_char('文'));
        // Cyrillic
        assert!(is_word_char('Д'));
    }

    #[test]
    fn click_threshold_is_reasonable() {
        assert!(
            CLICK_THRESHOLD >= Duration::from_millis(100),
            "CLICK_THRESHOLD ({:?}) should be at least 100ms",
            CLICK_THRESHOLD
        );
        assert!(
            CLICK_THRESHOLD <= Duration::from_millis(1000),
            "CLICK_THRESHOLD ({:?}) should be at most 1000ms",
            CLICK_THRESHOLD
        );
    }

    #[test]
    fn scrollbar_hitbox_covers_right_edge_track() {
        let window_width = 1000.0;
        let window_height = 700.0;
        let tab_bar_height = 30.0;
        let track_left = window_width - SCROLLBAR_WIDTH - SCROLLBAR_MARGIN;
        let track_top = tab_bar_height + SCROLLBAR_MARGIN;
        let track_bottom = window_height - SCROLLBAR_MARGIN;

        assert!(is_in_scrollbar_hitbox(
            track_left,
            track_top,
            window_width,
            window_height,
            tab_bar_height
        ));
        assert!(is_in_scrollbar_hitbox(
            window_width - 1.0,
            track_bottom,
            window_width,
            window_height,
            tab_bar_height
        ));
    }

    #[test]
    fn scrollbar_hitbox_rejects_points_outside_track() {
        let window_width = 1000.0;
        let window_height = 700.0;
        let tab_bar_height = 30.0;
        let track_left = window_width - SCROLLBAR_WIDTH - SCROLLBAR_MARGIN;
        let track_top = tab_bar_height + SCROLLBAR_MARGIN;
        let track_bottom = window_height - SCROLLBAR_MARGIN;

        assert!(!is_in_scrollbar_hitbox(
            track_left - 1.0,
            track_top,
            window_width,
            window_height,
            tab_bar_height
        ));
        assert!(!is_in_scrollbar_hitbox(
            window_width - 1.0,
            track_top - 1.0,
            window_width,
            window_height,
            tab_bar_height
        ));
        assert!(!is_in_scrollbar_hitbox(
            window_width - 1.0,
            track_bottom + 1.0,
            window_width,
            window_height,
            tab_bar_height
        ));
    }

    #[test]
    fn scrollbar_click_maps_top_middle_and_bottom_to_offsets() {
        let window_height = 700.0;
        let tab_bar_height = 30.0;
        let total_lines = 100;
        let viewport_rows = 20;
        let track_top = tab_bar_height + SCROLLBAR_MARGIN;
        let track_height = window_height - tab_bar_height - SCROLLBAR_MARGIN * 2.0;
        let track_middle = track_top + track_height / 2.0;
        let track_bottom = window_height - SCROLLBAR_MARGIN;

        assert_eq!(
            scrollbar_target_offset(
                track_top,
                window_height,
                tab_bar_height,
                total_lines,
                viewport_rows
            ),
            Some(80)
        );
        assert_eq!(
            scrollbar_target_offset(
                track_middle,
                window_height,
                tab_bar_height,
                total_lines,
                viewport_rows
            ),
            Some(40)
        );
        assert_eq!(
            scrollbar_target_offset(
                track_bottom,
                window_height,
                tab_bar_height,
                total_lines,
                viewport_rows
            ),
            Some(0)
        );
    }

    #[test]
    fn scrollbar_click_mapping_clamps_and_skips_non_overflowing_content() {
        let window_height = 700.0;
        let tab_bar_height = 30.0;

        assert_eq!(
            scrollbar_target_offset(0.0, window_height, tab_bar_height, 100, 20),
            Some(80)
        );
        assert_eq!(
            scrollbar_target_offset(900.0, window_height, tab_bar_height, 100, 20),
            Some(0)
        );
        assert_eq!(
            scrollbar_target_offset(32.0, window_height, tab_bar_height, 20, 20),
            None
        );
    }
}
