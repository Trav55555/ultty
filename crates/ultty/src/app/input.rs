//! Input event handling: mouse, keyboard, clipboard, and text selection.

use std::time::Instant;

use super::{App, WindowCtx, CLICK_THRESHOLD};
use ultty::action::Action;
use ultty::command_palette::PaletteKey;
use ultty::config::KeybindingConfig;
use ultty::grid::{
    MouseButton as TerminalMouseButton, MouseModifiers, SelectionMode, SelectionPoint,
};
use ultty::keys::{self, KeyAction, KeyInput, KeyModes, Mods};
use ultty::terminal_manager::SplitDirection;
use ultty::url::UrlDetector;
use ultty::vim::CopyModeResult;
use ultty_protocol::{Key as ProtocolKey, Modifiers as ProtocolModifiers};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

impl WindowCtx<'_> {
    /// Handle a window event for this window. Returns `true` if this window
    /// requested to be closed.
    pub(super) fn handle_window_event(&mut self, event: WindowEvent) -> bool {
        if matches!(event, WindowEvent::Destroyed) {
            tracing::info!("Window {} destroyed", self.win.key.raw());
            return true;
        }
        if let Some(adapter) = self.win.accessibility.as_mut() {
            adapter.process_event(&self.win.window, &event);
        }
        if self.win.pending_close {
            tracing::info!("Close requested for window {}", self.win.key.raw());
            return true;
        }

        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("Close requested for window {}", self.win.key.raw());
                return true;
            }
            WindowEvent::Resized(size) => {
                self.handle_resize(size);
                self.answer_pending_resizes(size);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.handle_scale_factor_changed(scale_factor as f32);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.win.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.win.mouse_pos = position;
                self.win.cursor_inside = true;
                if self.win.reported_button.is_some() {
                    self.report_mouse_drag();
                } else if self.win.mouse_pressed || self.win.tab_bar.is_dragging() {
                    self.handle_mouse_drag();
                } else if self.terminal().is_some_and(|t| t.track_mouse_motion()) {
                    self.handle_mouse_motion();
                }
                self.update_tab_hover();
                if !self.update_resize_cursor() {
                    self.update_cursor_for_url();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.win.cursor_inside = false;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(button, state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                tracing::debug!("WindowEvent::MouseWheel received: delta={:?}", delta);
                self.handle_scroll(delta);
            }
            WindowEvent::Ime(Ime::Enabled) => {
                tracing::debug!("Input method enabled");
            }
            WindowEvent::Ime(Ime::Disabled) => {
                tracing::debug!("Input method disabled");
                self.set_ime_preedit(String::new());
            }
            WindowEvent::Ime(Ime::Preedit(text, _cursor)) => {
                self.set_ime_preedit(text);
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.set_ime_preedit(String::new());
                if self.win.command_palette.is_open() {
                    self.palette_input(PaletteKey::Text(&text));
                    return false;
                }
                if self.forward_text_to_native_session(&text) {
                    return false;
                }
                if let Err(e) = self.send_input(text.as_bytes()) {
                    tracing::warn!("Failed to send IME input: {}", e);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Occluded(occluded) => self.win.set_occluded(occluded),
            WindowEvent::Focused(focused) => {
                if focused {
                    self.win.window.request_user_attention(None);
                } else {
                    self.win.native_mouse_capture.clear();
                    self.cancel_native_transcript_selection();
                }
                if self.terminal().is_some_and(|t| t.modes().focus_reporting) {
                    let seq = if focused { b"\x1b[I" } else { b"\x1b[O" };
                    let _ = self.send_report(seq);
                }
            }
            _ => {}
        }
        self.win.pending_close
    }

    /// Convert pixel position to terminal cell coordinates.
    pub(super) fn pixel_to_cell(&self, x: f64, y: f64) -> (u16, u16) {
        let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
        if cell_width <= 0.0 || cell_height <= 0.0 {
            return (0, 0);
        }
        let col = (x / cell_width as f64).floor().clamp(0.0, u16::MAX as f64) as u16;
        let row = (y / cell_height as f64).floor().clamp(0.0, u16::MAX as f64) as u16;
        (col, row)
    }

    fn mouse_modifiers(&self) -> MouseModifiers {
        MouseModifiers::new(
            self.win.modifiers.shift_key(),
            self.win.modifiers.alt_key(),
            self.win.modifiers.control_key(),
            self.win.modifiers.super_key(),
        )
    }

    fn forward_mouse_to_native_sessions(
        &mut self,
        event_type: ultty_protocol::InputType,
        x: f64,
        y: f64,
    ) -> bool {
        let modifiers = self.winit_mods_to_protocol();
        let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
        let (width, height) = self.win.renderer.terminal_area();
        let target = self.win.native_mouse_capture.route(
            &event_type,
            &self
                .app
                .runtime
                .manager
                .window_active_pane_rects(self.win.key, width, height),
            (x as f32, y as f32),
            (cell_width, cell_height),
        );
        let Some(active_tab) = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id)
        else {
            return false;
        };
        let Some(ref mut server) = self.app.runtime.protocol else {
            return false;
        };
        if server.session_count() == 0 || cell_width <= 0.0 || cell_height <= 0.0 {
            return false;
        }
        let Some((pane, cell_x, cell_y)) = target else {
            return false;
        };
        server.send_mouse_to_tab_pane(active_tab, pane, event_type, cell_x, cell_y, modifiers);
        true
    }

    fn winit_to_protocol_mouse_button(button: MouseButton) -> Option<ultty_protocol::MouseButton> {
        match button {
            MouseButton::Left => Some(ultty_protocol::MouseButton::Left),
            MouseButton::Middle => Some(ultty_protocol::MouseButton::Middle),
            MouseButton::Right => Some(ultty_protocol::MouseButton::Right),
            _ => None,
        }
    }

    fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        if button != MouseButton::Left
            && button != MouseButton::Middle
            && button != MouseButton::Right
        {
            return;
        }

        let x = self.win.mouse_pos.x as f32;
        let y = self.win.mouse_pos.y as f32;

        if self.win.command_palette.is_open() {
            if state == ElementState::Pressed {
                self.close_command_palette();
            }
            return;
        }

        if self.win.tab_bar.tab_menu.is_some() {
            if state == ElementState::Pressed {
                let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
                let picked = self
                    .win
                    .tab_bar
                    .tab_menu_layout(cell_width, cell_height)
                    .and_then(|layout| layout.item_at(x, y))
                    .filter(|_| button == MouseButton::Left)
                    .and_then(|idx| {
                        let menu = self.win.tab_bar.tab_menu.as_ref()?;
                        Some((menu.tab_id, menu.items.get(idx)?.action))
                    });
                self.close_menu();
                if let Some((tab_id, action)) = picked {
                    self.perform_tab_menu_action(tab_id, action);
                }
            }
            return;
        }

        if self.win.tab_bar.menu_open {
            if state == ElementState::Pressed {
                let (cell_width, cell_height) = self.win.renderer.cell_dimensions();
                let item = self
                    .win
                    .tab_bar
                    .menu_layout(cell_width, cell_height)
                    .item_at(x, y)
                    .filter(|_| button == MouseButton::Left);
                self.close_menu();
                if let Some(item) = item {
                    self.perform_menu_item(item);
                }
            }
            return;
        }

        let report_to_program = !self.native_transcript_selection_active()
            && !(state == ElementState::Pressed
                && button == MouseButton::Left
                && self.native_transcript_at_pointer())
            && self.terminal().is_some_and(|t| t.mouse_tracking_enabled())
            && if state == ElementState::Pressed {
                !self.win.modifiers.shift_key()
            } else {
                self.win.reported_button.is_some()
            };
        if report_to_program {
            if self.win.tab_bar.visible && y < self.win.tab_bar.height() {
            } else {
                let (col, row) = self.pixel_to_cell(
                    self.win.mouse_pos.x,
                    self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64,
                );
                let term_button = match button {
                    MouseButton::Left => TerminalMouseButton::Left,
                    MouseButton::Middle => TerminalMouseButton::Middle,
                    MouseButton::Right => TerminalMouseButton::Right,
                    _ => return,
                };
                let pressed = state == ElementState::Pressed;
                let term_button = if pressed {
                    self.win.reported_button = Some(term_button);
                    term_button
                } else {
                    self.win.reported_button.take().unwrap_or(term_button)
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
                if button == MouseButton::Left {
                    if !self.win.tab_bar.is_title_bar_toggle_area(x, y) {
                        self.win.last_title_bar_toggle_click_time =
                            Instant::now() - CLICK_THRESHOLD * 2;
                    }

                    if let Some(direction) = self.resize_direction_at(x, y) {
                        let _ = self.win.window.drag_resize_window(direction);
                        return;
                    }
                }

                if self.win.tab_bar.visible && y < self.win.tab_bar.height() {
                    if button == MouseButton::Left {
                        if let Some(control) = self
                            .win
                            .tab_bar
                            .title_button_at(x, y)
                            .filter(|button| *button != ultty::tabs::TitleButton::NewTab)
                        {
                            use ultty::tabs::TitleButton;
                            match control {
                                TitleButton::Close => {
                                    self.win.pending_close = true;
                                }
                                TitleButton::Maximize => {
                                    self.win
                                        .window
                                        .set_maximized(!self.win.window.is_maximized());
                                }
                                TitleButton::Minimize => {
                                    self.win.window.set_minimized(true);
                                }
                                TitleButton::SplitRight => {
                                    self.split_pane(SplitDirection::Horizontal);
                                }
                                TitleButton::SplitDown => {
                                    self.split_pane(SplitDirection::Vertical);
                                }
                                TitleButton::Menu => {
                                    self.win.tab_bar.menu_open = true;
                                    self.win.tab_bar.hovered_menu_item = None;
                                    self.win.needs_redraw = true;
                                }
                                TitleButton::NewTab => {}
                            }
                            return;
                        }

                        if self.win.tab_bar.is_drag_area(x, y) {
                            if self.win.tab_bar.is_title_bar_toggle_area(x, y) {
                                let now = Instant::now();
                                if now.duration_since(self.win.last_title_bar_toggle_click_time)
                                    < CLICK_THRESHOLD
                                {
                                    self.win
                                        .window
                                        .set_maximized(!self.win.window.is_maximized());
                                    self.win.last_title_bar_toggle_click_time =
                                        now - CLICK_THRESHOLD * 2;
                                    return;
                                }
                                self.win.last_title_bar_toggle_click_time = now;
                            }
                            let _ = self.win.window.drag_window();
                            return;
                        }
                    }

                    let is_tab_row = self.win.tab_bar.shows_tab_row()
                        && y >= self.win.tab_bar.title_bar_height();

                    if button == MouseButton::Middle && is_tab_row {
                        if let Some(tab_idx) = self.win.tab_bar.tab_at_x(x) {
                            if self.app.runtime.manager.window_tab_count(self.win.key) > 1 {
                                let tab_id = self
                                    .app
                                    .runtime
                                    .manager
                                    .window_tabs(self.win.key)
                                    .get(tab_idx)
                                    .map(|tab| tab.id);
                                if let Some(tab_id) = tab_id {
                                    self.freeze_tab_width_after_close();
                                    self.app.runtime.manager.close_tab(tab_id);
                                    tracing::info!("Closed tab {} via middle click", tab_idx);
                                    self.update_tab_bar();
                                    self.win.needs_redraw = true;
                                }
                            }
                        }
                        return;
                    }

                    if let Some(tab_idx) = self.win.tab_bar.close_button_at(x, y) {
                        if self.app.runtime.manager.window_tab_count(self.win.key) > 1 {
                            let tab_id = self
                                .app
                                .runtime
                                .manager
                                .window_tabs(self.win.key)
                                .get(tab_idx)
                                .map(|tab| tab.id);
                            if let Some(tab_id) = tab_id {
                                self.freeze_tab_width_after_close();
                                self.app.runtime.manager.close_tab(tab_id);
                                tracing::info!("Closed tab {} via close button", tab_idx);
                                self.update_tab_bar();
                                self.win.needs_redraw = true;
                            }
                        }
                        return;
                    }

                    if button == MouseButton::Left && self.win.tab_bar.is_new_tab_button_at(x, y) {
                        match self.create_tab() {
                            Ok(_) => {
                                self.clear_tab_width_freeze();
                                tracing::info!("Created new tab via + button");
                                self.update_tab_bar();
                                self.win.needs_redraw = true;
                            }
                            Err(e) => tracing::warn!("Failed to create tab: {}", e),
                        }
                        return;
                    }

                    if is_tab_row {
                        if let Some(tab_idx) = self.win.tab_bar.tab_at_x(x) {
                            if button == MouseButton::Right {
                                self.open_tab_menu(tab_idx, x, y);
                            } else if button == MouseButton::Left {
                                self.win.tab_bar.start_drag(tab_idx, x);
                                self.app
                                    .runtime
                                    .manager
                                    .switch_to_tab_in_window(self.win.key, tab_idx);
                                self.update_tab_bar();
                                self.win.needs_redraw = true;
                                tracing::debug!("Started drag on tab {}", tab_idx);
                            }
                        }
                    }
                    return;
                }

                if button == MouseButton::Left
                    && self.app.runtime.manager.window_pane_count(self.win.key) > 1
                {
                    let content_y = y - self.win.tab_bar.content_top();
                    let (aw, ah) = self.win.renderer.terminal_area();

                    if let Some(divider) = self.app.runtime.manager.window_divider_at(
                        self.win.key,
                        x,
                        content_y,
                        aw,
                        ah,
                        4.0,
                    ) {
                        let along = match divider.direction {
                            SplitDirection::Horizontal => x,
                            SplitDirection::Vertical => content_y,
                        };
                        self.win.divider_dragging = Some((divider, along));
                        return;
                    }

                    if let Some(pane) =
                        self.app
                            .runtime
                            .manager
                            .window_pane_at(self.win.key, x, content_y, aw, ah)
                    {
                        if Some(pane)
                            != self
                                .app
                                .runtime
                                .manager
                                .window_focused_terminal_id(self.win.key)
                        {
                            self.app
                                .runtime
                                .manager
                                .window_focus_terminal(self.win.key, pane);
                            self.win.needs_redraw = true;
                        }
                    }
                }

                if button != MouseButton::Left {
                    return;
                }
                if self.begin_native_transcript_selection() {
                    self.win.mouse_pressed = true;
                    return;
                }
                self.cancel_native_transcript_selection();

                let (col, row) = self.pixel_to_cell(
                    self.win.mouse_pos.x,
                    self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64,
                );

                if let Some(proto_button) = Self::winit_to_protocol_mouse_button(button) {
                    let event_type = ultty_protocol::InputType::MouseDown {
                        button: proto_button,
                    };
                    self.forward_mouse_to_native_sessions(
                        event_type,
                        self.win.mouse_pos.x,
                        self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64,
                    );
                }

                if self.win.modifiers.control_key() && self.win.url_detector.is_enabled() {
                    if let Some(url) = self.win.url_detector.url_at(col, row) {
                        let url_str = url.url.clone();
                        if let Err(e) = UrlDetector::open_url(&url_str) {
                            tracing::warn!("{}", e);
                        } else {
                            tracing::info!("Opened URL: {}", url_str);
                        }
                        return;
                    }
                }

                self.win.mouse_pressed = true;

                let now = Instant::now();
                let same_cell = (col, row) == self.win.last_click_pos;
                let quick_click = now.duration_since(self.win.last_click_time) < CLICK_THRESHOLD;

                if same_cell && quick_click {
                    self.win.click_count = (self.win.click_count % 3) + 1;
                } else {
                    self.win.click_count = 1;
                }

                self.win.last_click_time = now;
                self.win.last_click_pos = (col, row);

                let mode = if self.win.modifiers.alt_key() && self.win.click_count == 1 {
                    SelectionMode::Block
                } else {
                    match self.win.click_count {
                        1 => SelectionMode::Normal,
                        2 => SelectionMode::Normal,
                        3 => SelectionMode::Line,
                        _ => SelectionMode::Normal,
                    }
                };

                let point = SelectionPoint::new(col as i32, row as i32);

                if self.win.click_count == 2 && !self.win.modifiers.alt_key() {
                    self.select_word(col, row);
                } else if let Some(mut t) = self.terminal_mut() {
                    t.start_selection(point, mode);
                }

                self.win.needs_redraw = true;
            }
            ElementState::Released => {
                if button == MouseButton::Left && self.native_transcript_selection_active() {
                    self.end_native_transcript_selection();
                    self.win.mouse_pressed = false;
                    if self.app.config.behavior.copy_on_select {
                        self.native_transcript_copy();
                    }
                    return;
                }
                self.win.mouse_pressed = false;
                self.win.reported_button = None;
                self.win.divider_dragging = None;

                if let Some(proto_button) = Self::winit_to_protocol_mouse_button(button) {
                    let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
                    let event_type = ultty_protocol::InputType::MouseUp {
                        button: proto_button,
                    };
                    self.forward_mouse_to_native_sessions(
                        event_type,
                        self.win.mouse_pos.x,
                        adjusted_y.max(0.0),
                    );
                }

                if let Some(tab_idx) = self.win.tab_bar.dragging_tab {
                    self.win.tab_bar.end_drag();
                    self.win.needs_redraw = true;
                    let win_size = self.win.window.inner_size();
                    if self.app.runtime.manager.window_tab_count(self.win.key) > 1
                        && ultty::tabs::should_detach_tab_drag(
                            (x, y),
                            (win_size.width as f32, win_size.height as f32),
                            !self.win.cursor_inside,
                        )
                    {
                        if let Some(tab_id) = self
                            .app
                            .runtime
                            .manager
                            .window_tabs(self.win.key)
                            .get(tab_idx)
                            .map(|tab| tab.id)
                        {
                            let position = self.win.window.outer_position().ok().map(|origin| {
                                winit::dpi::PhysicalPosition::new(
                                    origin.x + x.round() as i32,
                                    origin.y + y.round() as i32,
                                )
                            });
                            self.app.pending_tab_moves.push(super::PendingTabMove {
                                tab_id,
                                destination: super::MoveDestination::NewWindow,
                                position,
                            });
                        }
                    }
                }

                if self.app.config.behavior.copy_on_select {
                    if let Some(text) = self.terminal().and_then(|t| t.selected_text()) {
                        if !text.is_empty() {
                            self.app.copy_to_clipboard(&text);
                        }
                    }
                }
            }
        }
    }

    fn handle_mouse_drag(&mut self) {
        if self.native_transcript_selection_active() {
            self.update_native_transcript_selection();
            return;
        }
        if self.win.tab_bar.is_dragging() {
            let x = self.win.mouse_pos.x as f32;
            if let Some((from, to)) = self.win.tab_bar.update_drag(x) {
                if self
                    .app
                    .runtime
                    .manager
                    .reorder_window_tabs(self.win.key, from, to)
                {
                    self.update_tab_bar();
                    tracing::debug!("Reordered tab {} to {}", from, to);
                }
            }
            self.win.needs_redraw = true;
            return;
        }

        if let Some((divider, last_pos)) = self.win.divider_dragging {
            let current_pos = match divider.direction {
                SplitDirection::Horizontal => self.win.mouse_pos.x as f32,
                SplitDirection::Vertical => {
                    self.win.mouse_pos.y as f32 - self.win.tab_bar.content_top()
                }
            };
            let delta = current_pos - last_pos;
            if delta.abs() > 1.0 {
                self.app
                    .runtime
                    .manager
                    .window_drag_divider(self.win.key, &divider, delta);
                self.win.divider_dragging = Some((divider, current_pos));
                self.win.needs_redraw = true;
            }
            return;
        }

        let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
        if adjusted_y < 0.0 {
            return;
        }

        self.forward_mouse_to_native_sessions(
            ultty_protocol::InputType::MouseMove,
            self.win.mouse_pos.x,
            adjusted_y,
        );

        let (col, row) = self.pixel_to_cell(self.win.mouse_pos.x, adjusted_y);
        let point = SelectionPoint::new(col as i32, row as i32);

        if let Some(mut t) = self.terminal_mut() {
            t.update_selection(point);
        }
        self.win.needs_redraw = true;
    }

    fn report_mouse_drag(&mut self) {
        let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
        let (col, row) = self.pixel_to_cell(self.win.mouse_pos.x, adjusted_y.max(0.0));
        let Some(button) = self.win.reported_button else {
            return;
        };
        let mods = self.mouse_modifiers();
        if let Some(bytes) = self
            .terminal()
            .and_then(|t| t.encode_mouse_event(button, col, row, true, true, mods))
        {
            let _ = self.send_report(&bytes);
        }
    }

    fn handle_mouse_motion(&mut self) {
        let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
        if adjusted_y < 0.0 {
            return;
        }

        let (col, row) = self.pixel_to_cell(self.win.mouse_pos.x, adjusted_y);
        let mods = self.mouse_modifiers();
        if let Some(bytes) = self.terminal().and_then(|t| {
            t.encode_mouse_event(TerminalMouseButton::Release, col, row, true, true, mods)
        }) {
            let _ = self.send_report(&bytes);
        }
    }

    pub(super) fn close_menu(&mut self) {
        self.win.tab_bar.menu_open = false;
        self.win.tab_bar.hovered_menu_item = None;
        self.win.tab_bar.tab_menu = None;
        self.win.needs_redraw = true;
    }

    fn open_tab_menu(&mut self, tab_idx: usize, x: f32, y: f32) {
        use ultty::tabs::{TabMenuAction, TabMenuItem, TabMenuState};

        let Some(tab_id) = self
            .app
            .runtime
            .manager
            .window_tabs(self.win.key)
            .get(tab_idx)
            .map(|tab| tab.id)
        else {
            return;
        };
        let mut items = Vec::new();
        let tab_count = self.app.runtime.manager.window_tab_count(self.win.key);
        if tab_count > 1 {
            items.push(TabMenuItem {
                label: "Move tab to new window".to_string(),
                action: TabMenuAction::MoveToNewWindow,
            });
        }
        for other in self.app.runtime.manager.window_ids() {
            if other != self.win.key {
                items.push(TabMenuItem {
                    label: format!("Move tab to window {}", other.raw()),
                    action: TabMenuAction::MoveToWindow(other),
                });
            }
        }
        if tab_count > 1 {
            items.push(TabMenuItem {
                label: "Close tab".to_string(),
                action: TabMenuAction::CloseTab,
            });
        }
        if items.is_empty() {
            return;
        }
        self.win.tab_bar.menu_open = false;
        self.win.tab_bar.hovered_menu_item = None;
        self.win.tab_bar.tab_menu = Some(TabMenuState {
            tab_id,
            anchor_x: x,
            anchor_y: y,
            items,
            hovered: None,
        });
        self.win.needs_redraw = true;
    }

    fn perform_tab_menu_action(
        &mut self,
        tab_id: ultty::terminal_manager::TabId,
        action: ultty::tabs::TabMenuAction,
    ) {
        use ultty::tabs::TabMenuAction;

        match action {
            TabMenuAction::MoveToNewWindow => {
                self.app.pending_tab_moves.push(super::PendingTabMove {
                    tab_id,
                    destination: super::MoveDestination::NewWindow,
                    position: None,
                });
            }
            TabMenuAction::MoveToWindow(window) => {
                self.app.pending_tab_moves.push(super::PendingTabMove {
                    tab_id,
                    destination: super::MoveDestination::Existing(window),
                    position: None,
                });
            }
            TabMenuAction::CloseTab => {
                if self.app.runtime.manager.window_tab_count(self.win.key) > 1 {
                    self.freeze_tab_width_after_close();
                    self.app.runtime.manager.close_tab(tab_id);
                    self.update_tab_bar();
                    self.win.needs_redraw = true;
                }
            }
        }
    }

    /// Run a main-menu entry.
    pub(super) fn perform_menu_item(&mut self, item: ultty::tabs::MenuItem) {
        use ultty::tabs::MenuItem;
        match item {
            MenuItem::NewTab => match self.create_tab() {
                Ok(_) => {
                    self.clear_tab_width_freeze();
                    self.update_tab_bar();
                    self.win.needs_redraw = true;
                }
                Err(e) => tracing::warn!("Failed to create tab: {}", e),
            },
            MenuItem::NewWindow => {
                if self.app.config.behavior.window_per_process {
                    let spawned = std::env::current_exe().and_then(|exe| {
                        std::process::Command::new(exe).arg("--new-process").spawn()
                    });
                    if let Err(e) = spawned {
                        tracing::warn!("Failed to open a new window: {}", e);
                    }
                } else {
                    self.app.pending_new_windows += 1;
                }
            }
            MenuItem::SplitRight => self.split_pane(SplitDirection::Horizontal),
            MenuItem::SplitDown => self.split_pane(SplitDirection::Vertical),
            MenuItem::Settings => open_settings(),
            MenuItem::Shortcuts => {
                self.win.shortcuts_visible = true;
                self.win.needs_redraw = true;
            }
            MenuItem::About => {
                App::show_notification("Ultty", &format!("Version {}", env!("CARGO_PKG_VERSION")))
            }
        }
    }

    /// Split the focused pane in this window: side by side (Horizontal) or
    /// stacked (Vertical).
    pub(super) fn split_pane(&mut self, direction: SplitDirection) {
        let result = self
            .app
            .runtime
            .manager
            .split_window_focused(self.win.key, direction);
        match result {
            Ok(Some(terminal_id)) => {
                tracing::info!("Split {:?}: new terminal {:?}", direction, terminal_id);
                self.win.needs_redraw = true;
            }
            Ok(None) => tracing::warn!("No active tab for split"),
            Err(e) => tracing::warn!("Failed to split: {}", e),
        }
    }

    fn select_word(&mut self, col: u16, row: u16) {
        let extra = self.app.config.behavior.selection_word_chars.clone();
        let word_bounds = {
            let Some(terminal) = self.terminal() else {
                return;
            };
            let grid = terminal.grid();

            if let Some(grid_row) = grid.row(row) {
                let mut start_col = col;
                let mut end_col = col;

                while start_col > 0 {
                    if let Some(cell) = grid_row.cell(start_col - 1) {
                        if is_word_char(cell.char(), &extra) {
                            start_col -= 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                let cols = grid.dimensions().cols;
                while end_col < cols - 1 {
                    if let Some(cell) = grid_row.cell(end_col + 1) {
                        if is_word_char(cell.char(), &extra) {
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

    fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        tracing::debug!("handle_scroll: delta={:?}", delta);

        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                self.win.scroll_line_accum += y as f64;
                let lines = self.win.scroll_line_accum.trunc() as i32;
                self.win.scroll_line_accum -= lines as f64;
                self.win.scroll_pixel_accum = 0.0;
                lines
            }
            MouseScrollDelta::PixelDelta(pos) => {
                self.win.scroll_pixel_accum += pos.y;
                self.win.scroll_line_accum = 0.0;
                let (_, cell_height) = self.win.renderer.cell_dimensions();
                let cell_h = (cell_height as f64).max(1.0);
                let lines = (self.win.scroll_pixel_accum / cell_h).trunc() as i32;
                self.win.scroll_pixel_accum -= lines as f64 * cell_h;
                lines
            }
        };

        tracing::debug!(
            "handle_scroll: computed lines={}, pixel_accum={}, line_accum={}",
            lines,
            self.win.scroll_pixel_accum,
            self.win.scroll_line_accum
        );

        if lines == 0 {
            return;
        }

        if self.win.command_palette.is_open() {
            if let Some((key, steps)) = super::command_palette::wheel_key(lines) {
                for _ in 0..steps {
                    self.palette_input(key);
                }
            }
            return;
        }

        if self.terminal().is_some_and(|t| t.mouse_tracking_enabled()) {
            let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
            let (col, row) = self.pixel_to_cell(self.win.mouse_pos.x, adjusted_y.max(0.0));
            let button = if lines > 0 {
                TerminalMouseButton::ScrollUp
            } else {
                TerminalMouseButton::ScrollDown
            };
            let count = lines.unsigned_abs() as usize;
            let mods = self.mouse_modifiers();

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

        {
            let adjusted_y = self.win.mouse_pos.y - self.win.tab_bar.content_top() as f64;
            let delta_y = match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
            };
            self.forward_mouse_to_native_sessions(
                ultty_protocol::InputType::MouseScroll {
                    delta_x: 0.0,
                    delta_y,
                },
                self.win.mouse_pos.x,
                adjusted_y.max(0.0),
            );
        }

        let in_alternate = self.terminal().is_some_and(|t| t.is_alternate_screen());
        let alternate_scroll = self.terminal().is_some_and(|t| t.modes().alternate_scroll);

        if in_alternate && alternate_scroll {
            let app_cursor = self
                .terminal()
                .is_some_and(|t| t.modes().application_cursor_keys);
            let count = lines.unsigned_abs() as usize;
            let arrow: &[u8] = if lines > 0 {
                if app_cursor {
                    b"\x1bOA"
                } else {
                    b"\x1b[A"
                }
            } else if app_cursor {
                b"\x1bOB"
            } else {
                b"\x1b[B"
            };
            for _ in 0..count {
                let _ = self.send_input(arrow);
            }
            return;
        }

        if in_alternate {
            return;
        }

        self.scroll_scrollback(lines);
    }

    fn scroll_scrollback(&mut self, lines: i32) {
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

        self.win.needs_redraw = true;
    }

    fn paste_from_clipboard(&mut self) {
        let text = match self.app.clipboard.as_mut() {
            Some(clipboard) => match clipboard.get_text() {
                Ok(text) => Some(text),
                Err(e) => {
                    tracing::warn!("Failed to read clipboard: {}", e);
                    None
                }
            },
            None => None,
        };
        if let Some(text) = text {
            if self.forward_text_to_native_session(&text) {
                return;
            }
            let bracketed = self.terminal().is_some_and(|t| t.modes().bracketed_paste);
            if let Err(e) = self.send_input(&paste_payload(&text, bracketed)) {
                tracing::warn!("Failed to paste: {}", e);
            }
        }
    }

    fn set_ime_preedit(&mut self, text: String) {
        if self.win.ime_preedit != text {
            self.win.ime_preedit = text;
            self.win.needs_redraw = true;
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if !self.win.ime_preedit.is_empty() {
            return;
        }
        if event.state == ElementState::Released {
            if self.win.command_palette.is_open() {
                return;
            }
            if !self.forward_key_up_to_native_sessions(&event) {
                self.send_key_to_terminal(&event);
            }
            return;
        }

        self.win.cursor_blink.restart(Instant::now());

        if self.win.command_palette.is_open() {
            self.handle_palette_key(&event);
            return;
        }

        for action in self.bound_actions(&event.logical_key) {
            if !action.waits_for_modes() && self.perform_action(action) {
                return;
            }
        }

        if self.win.tab_bar.menu_open || self.win.tab_bar.tab_menu.is_some() {
            if let Key::Named(NamedKey::Escape) = &event.logical_key {
                self.close_menu();
            }
            return;
        }

        if self.win.shortcuts_visible || self.win.protocol_status_visible {
            if let Key::Named(NamedKey::Escape) = &event.logical_key {
                self.win.shortcuts_visible = false;
                self.win.protocol_status_visible = false;
                self.win.needs_redraw = true;
                return;
            }
            return;
        }

        if self.win.copy_mode.is_active() {
            self.handle_copy_mode_key(&event);
            return;
        }

        if !matches!(
            event.logical_key,
            Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super)
        ) {
            if let Some(mut t) = self.terminal_mut() {
                t.clear_selection();
            }
        }

        for action in self.bound_actions(&event.logical_key) {
            if action.waits_for_modes() && self.perform_action(action) {
                return;
            }
        }
        if let Key::Named(NamedKey::Insert) = &event.logical_key {
            if self.win.modifiers.shift_key() {
                self.paste_from_clipboard();
                return;
            }
        }

        if !self.forward_key_to_native_sessions(&event) {
            self.send_key_to_terminal(&event);
        }
    }

    pub(super) fn bound_actions(&self, key: &Key) -> Vec<Action> {
        bound_actions(&self.app.config.keybindings, key, self.win.modifiers)
    }

    pub(super) fn perform_action(&mut self, action: Action) -> bool {
        use ultty::pane_layout::FocusDirection;
        match action {
            Action::CopyMode => {
                if self.win.copy_mode.is_active() {
                    self.win.copy_mode.exit();
                } else {
                    self.enter_copy_mode();
                }
                self.win.needs_redraw = true;
            }
            Action::ScrollPageUp | Action::ScrollPageDown => {
                let screen = self
                    .terminal()
                    .filter(|t| !t.is_alternate_screen())
                    .map(|t| t.dimensions().rows);
                let Some(rows) = screen else {
                    return false;
                };
                let page = i32::from(rows.saturating_sub(1).max(1));
                let up = action == Action::ScrollPageUp;
                self.scroll_scrollback(if up { page } else { -page });
            }
            Action::JumpPrevPrompt | Action::JumpNextPrompt | Action::SelectLastOutput => {
                use ultty::grid::PromptJump;
                let handled = self
                    .terminal_mut()
                    .filter(|t| !t.is_alternate_screen())
                    .map(|mut t| match action {
                        Action::JumpPrevPrompt => t.jump_to_prompt(PromptJump::Previous),
                        Action::JumpNextPrompt => t.jump_to_prompt(PromptJump::Next),
                        _ => t.select_last_output(),
                    });
                let Some(changed) = handled else {
                    return false;
                };
                if changed {
                    self.win.needs_redraw = true;
                }
            }
            Action::Find => {
                if self.has_native_sessions() {
                    self.win.native_search_active = true;
                    self.win.native_search_query.clear();
                    self.win.native_search_matches.clear();
                    self.win.native_search_index = 0;
                }
                self.enter_search_mode();
                self.win.needs_redraw = true;
            }
            Action::NewTab => match self.create_tab() {
                Ok(tab_id) => {
                    self.clear_tab_width_freeze();
                    tracing::info!("Created new tab {:?}", tab_id);
                    self.update_tab_bar();
                    self.win.needs_redraw = true;
                }
                Err(e) => tracing::warn!("Failed to create tab: {}", e),
            },
            Action::CloseTab => {
                if self.app.runtime.manager.window_tab_count(self.win.key) > 1 {
                    if let Some(tab) = self.app.runtime.manager.window_active_tab(self.win.key) {
                        let tab_id = tab.id;
                        self.freeze_tab_width_after_close();
                        self.app.runtime.manager.close_tab(tab_id);
                        tracing::info!("Closed tab {:?}", tab_id);
                        self.update_tab_bar();
                        self.win.needs_redraw = true;
                    }
                }
            }
            Action::NextTab => {
                self.app.runtime.manager.next_tab_in_window(self.win.key);
                self.update_tab_bar();
                self.win.needs_redraw = true;
            }
            Action::PrevTab => {
                self.app.runtime.manager.prev_tab_in_window(self.win.key);
                self.update_tab_bar();
                self.win.needs_redraw = true;
            }
            Action::SplitRight => self.split_pane(SplitDirection::Horizontal),
            Action::SplitDown => self.split_pane(SplitDirection::Vertical),
            Action::FocusPaneLeft
            | Action::FocusPaneRight
            | Action::FocusPaneUp
            | Action::FocusPaneDown => {
                let direction = match action {
                    Action::FocusPaneLeft => FocusDirection::Left,
                    Action::FocusPaneRight => FocusDirection::Right,
                    Action::FocusPaneUp => FocusDirection::Up,
                    _ => FocusDirection::Down,
                };
                let (width, height) = self.win.renderer.terminal_area();
                self.app.runtime.manager.window_focus_direction(
                    self.win.key,
                    direction,
                    width,
                    height,
                );
                self.win.needs_redraw = true;
            }
            Action::ClosePane => {
                if self
                    .app
                    .runtime
                    .manager
                    .close_window_focused_pane(self.win.key)
                {
                    tracing::info!("Closed pane");
                    self.win.needs_redraw = true;
                }
            }
            Action::ReloadConfig => {
                self.app.pending_reload_config = true;
            }
            Action::ShowShortcuts => {
                self.win.shortcuts_visible = !self.win.shortcuts_visible;
                if self.win.shortcuts_visible {
                    self.win.protocol_status_visible = false;
                }
                self.win.needs_redraw = true;
            }
            Action::ProtocolStatus => {
                self.win.protocol_status_visible = !self.win.protocol_status_visible;
                if self.win.protocol_status_visible {
                    self.win.shortcuts_visible = false;
                }
                self.win.needs_redraw = true;
            }
            Action::CommandPalette => self.open_command_palette(),
            Action::CycleTheme => {
                self.app.pending_cycle_theme = true;
            }
            Action::FontIncrease => self.change_font_size(true),
            Action::FontDecrease => self.change_font_size(false),
            Action::FontReset => self.reset_font_size(),
            Action::Screenshot => self.take_screenshot(),
            Action::DebugDump => self.debug_dump_terminal(),
            Action::Paste => self.paste_from_clipboard(),
            Action::Copy => {
                if self.native_transcript_copy() {
                    return true;
                }
                if let Some(text) = self.terminal().and_then(|t| t.selected_text()) {
                    self.app.copy_to_clipboard(&text);
                }
            }
        }
        true
    }

    fn send_key_to_terminal(&mut self, event: &KeyEvent) {
        let bytes = self.encode_key(event);
        if bytes.is_empty() {
            return;
        }
        if let Err(e) = self.send_input(&bytes) {
            tracing::warn!("Failed to send input: {}", e);
        }
    }

    fn encode_key(&self, event: &KeyEvent) -> Vec<u8> {
        let unmodified = event.key_without_modifiers();
        let Some(key) = keys::key_from_winit(&event.logical_key, &unmodified, event.location)
        else {
            return Vec::new();
        };
        let pressed = event.state == ElementState::Pressed;
        let mut mods = Mods::new(
            self.win.modifiers.shift_key(),
            self.win.modifiers.alt_key(),
            self.win.modifiers.control_key(),
            self.win.modifiers.super_key(),
        );
        if let Key::Named(named) = &event.logical_key {
            let own = match named {
                NamedKey::Shift => Some(Mods::SHIFT),
                NamedKey::Alt => Some(Mods::ALT),
                NamedKey::Control => Some(Mods::CTRL),
                NamedKey::Super => Some(Mods::SUPER),
                _ => None,
            };
            if let Some(own) = own {
                mods = if pressed {
                    mods.union(own)
                } else {
                    mods.without(own)
                };
            }
        }
        let shifted = match &event.logical_key {
            Key::Character(s) if mods.contains(Mods::SHIFT) => {
                s.chars().next().filter(|&c| keys::Key::Char(c) != key)
            }
            _ => None,
        };
        let base = match (key, event.physical_key) {
            (keys::Key::Char(_), PhysicalKey::Code(code)) => {
                keys::base_layout_key(code).filter(|&b| keys::Key::Char(b) != key)
            }
            _ => None,
        };
        let action = match (pressed, event.repeat) {
            (false, _) => KeyAction::Release,
            (true, true) => KeyAction::Repeat,
            (true, false) => KeyAction::Press,
        };
        let modes = self
            .terminal()
            .map(|t| {
                let modes = t.modes();
                KeyModes {
                    application_cursor: modes.application_cursor_keys,
                    kitty_flags: modes.kitty_keyboard_flags,
                    modify_other_keys: modes.modify_other_keys,
                    linefeed_newline: modes.linefeed_newline,
                }
            })
            .unwrap_or_default();
        keys::encode(
            &KeyInput {
                key,
                shifted,
                base,
                mods,
                action,
                text: event.text.as_deref(),
            },
            modes,
        )
    }

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
        if self.win.modifiers.shift_key() {
            modifiers = modifiers.union(ProtocolModifiers::SHIFT);
        }
        if self.win.modifiers.control_key() {
            modifiers = modifiers.union(ProtocolModifiers::CTRL);
        }
        if self.win.modifiers.alt_key() {
            modifiers = modifiers.union(ProtocolModifiers::ALT);
        }
        if self.win.modifiers.super_key() {
            modifiers = modifiers.union(ProtocolModifiers::META);
        }

        Some((protocol_key, modifiers))
    }

    fn native_input_target(
        &self,
    ) -> Option<(
        ultty::terminal_manager::TabId,
        ultty::terminal_manager::TerminalId,
        Option<i32>,
    )> {
        let terminal = self
            .app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)?;
        let foreground = terminal.event_loop.foreground_pgrp();
        self.app
            .runtime
            .protocol
            .as_ref()?
            .has_pane_key_subscribers(terminal.tab_id, terminal.id, foreground)
            .then_some((terminal.tab_id, terminal.id, foreground))
    }

    fn forward_text_to_native_session(&mut self, text: &str) -> bool {
        let Some((tab, pane, foreground)) = self.native_input_target() else {
            return false;
        };
        let Some(server) = self.app.runtime.protocol.as_mut() else {
            return false;
        };
        if let Err(error) = server.send_text_input_to_pane(tab, pane, text.to_owned(), foreground) {
            tracing::warn!(%error, "Native text input was refused");
        }
        true
    }

    fn forward_key_to_native_sessions(&mut self, event: &KeyEvent) -> bool {
        let Some((key, modifiers)) = self.convert_key_event(event) else {
            return false;
        };
        let Some((tab, pane, foreground)) = self.native_input_target() else {
            return false;
        };
        let Some(ref mut server) = self.app.runtime.protocol else {
            return false;
        };
        if let Err(error) = server.send_key_to_pane(tab, pane, key, modifiers, foreground) {
            tracing::warn!(%error, "Native key input was refused");
        }
        true
    }

    fn forward_key_up_to_native_sessions(&mut self, event: &KeyEvent) -> bool {
        let Some((key, modifiers)) = self.convert_key_event(event) else {
            return false;
        };
        let Some((tab, pane, foreground)) = self.native_input_target() else {
            return false;
        };
        let Some(ref mut server) = self.app.runtime.protocol else {
            return false;
        };
        if let Err(error) = server.send_key_up_to_pane(tab, pane, key, modifiers, foreground) {
            tracing::warn!(%error, "Native key release was refused");
        }
        true
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

        self.win.search_job = None;
        self.win.copy_mode_pane = self
            .app
            .runtime
            .manager
            .window_focused_terminal_id(self.win.key);
        self.win
            .copy_mode
            .set_search_regex(self.app.config.search.regex);
        self.win
            .copy_mode
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

        self.win.search_job = None;
        self.win.copy_mode_pane = self
            .app
            .runtime
            .manager
            .window_focused_terminal_id(self.win.key);
        self.win
            .copy_mode
            .set_search_regex(self.app.config.search.regex);
        self.win
            .copy_mode
            .enter_search(total_rows, viewport_rows, viewport_cols);
        tracing::debug!("Entered search mode");
    }

    fn handle_copy_mode_key(&mut self, event: &KeyEvent) {
        self.reconcile_copy_mode_pane();
        if !self.win.copy_mode.is_active() {
            return;
        }
        let protocol_key = self.winit_key_to_protocol(&event.logical_key);
        let protocol_mods = self.winit_mods_to_protocol();

        let (cursor_row, _) = self.win.copy_mode.cursor_position();
        let row_text = self
            .terminal()
            .and_then(|t| t.buffer().row_at(cursor_row).map(|row| row.text()))
            .unwrap_or_default();

        match self
            .win
            .copy_mode
            .handle_key_in_row(&protocol_key, protocol_mods, &row_text)
        {
            CopyModeResult::Handled => {
                if self.win.native_search_active && !self.win.native_search_matches.is_empty() {
                    match &protocol_key {
                        ProtocolKey::Char('n') => self.native_search_next(),
                        ProtocolKey::Char('N') => self.native_search_prev(),
                        _ => {}
                    }
                }
                self.win.needs_redraw = true;
            }
            CopyModeResult::Exit => {
                tracing::debug!("Exited copy mode");
                if self.win.native_search_active {
                    self.dismiss_native_search();
                }
                self.win.needs_redraw = true;
            }
            CopyModeResult::Copy(selection) => {
                if let Some(sel) = selection {
                    let text = self.extract_selection_text(&sel);
                    self.app.copy_to_clipboard(&text);
                }
                self.win.needs_redraw = true;
            }
            CopyModeResult::CopyAndExit(selection) => {
                if let Some(sel) = selection {
                    let text = self.extract_selection_text(&sel);
                    self.app.copy_to_clipboard(&text);
                }
                tracing::debug!("Copied and exited copy mode");
                if self.win.native_search_active {
                    self.dismiss_native_search();
                }
                self.win.needs_redraw = true;
            }
            CopyModeResult::Search(query) => {
                self.perform_search(&query);
                self.win.needs_redraw = true;
            }
            CopyModeResult::NotHandled => {}
        }
    }

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

    fn winit_mods_to_protocol(&self) -> ProtocolModifiers {
        let mut mods = ProtocolModifiers::NONE;
        if self.win.modifiers.control_key() {
            mods = mods.union(ProtocolModifiers::CTRL);
        }
        if self.win.modifiers.shift_key() {
            mods = mods.union(ProtocolModifiers::SHIFT);
        }
        if self.win.modifiers.alt_key() {
            mods = mods.union(ProtocolModifiers::ALT);
        }
        mods
    }
}

/// The actions `keys` binds to `key` pressed with `mods`, in
/// [`Action::ALL`] order.
pub(super) fn bound_actions(
    keys: &KeybindingConfig,
    key: &Key,
    mods: ModifiersState,
) -> Vec<Action> {
    Action::ALL
        .into_iter()
        .filter(|action| binding_matches(action.binding(keys), key, mods))
        .collect()
}

/// Whether `key` pressed with `mods` is the keybinding `binding`
/// (`"ctrl+shift+t"`).
fn binding_matches(binding: &str, key: &Key, mods: ModifiersState) -> bool {
    let Some(parsed) = KeybindingConfig::parse_keybinding(binding) else {
        return false;
    };

    // Check modifiers
    if parsed.ctrl != mods.control_key()
        || parsed.shift != mods.shift_key()
        || parsed.alt != mods.alt_key()
        || parsed.super_key != mods.super_key()
    {
        return false;
    }

    // Check key
    match key {
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

/// The bytes a paste of `text` sends to the pty.
///
/// With bracketed paste on, the text is wrapped in `ESC[200~` / `ESC[201~`.
/// Every ESC and C1 control (U+0080-U+009F, which includes the 8-bit CSI)
/// is removed from the text first, so the clipboard can never close the
/// bracket early and have the rest run as typed input. Without bracketed
/// paste the program cannot tell a paste from typing, so newlines become
/// carriage returns, the byte the Enter key sends. Multi-line pastes are not
/// confirmed; a shell without bracketed paste runs each line as it arrives.
fn paste_payload(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut out = Vec::with_capacity(text.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        let mut buf = [0u8; 4];
        for c in text.chars() {
            if c != '\x1b' && !('\u{80}'..='\u{9f}').contains(&c) {
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

fn is_word_char(c: char, extra: &str) -> bool {
    c.is_alphanumeric() || extra.contains(c)
}

/// Open the config file with the desktop's default handler, or its folder
/// when the file does not exist yet.
fn open_settings() {
    let Some(path) = ultty::config::Config::config_path() else {
        tracing::warn!("No config directory");
        return;
    };
    let target = if path.exists() {
        path
    } else {
        match path.parent() {
            Some(dir) => {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    tracing::warn!("Cannot create {}: {}", dir.display(), e);
                    return;
                }
                dir.to_path_buf()
            }
            None => return,
        }
    };
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(&target).spawn() {
        tracing::warn!("Failed to open {}: {}", target.display(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_word_char_alphanumeric() {
        let defaults = ultty::config::DEFAULT_SELECTION_WORD_CHARS;
        // ASCII letters
        assert!(is_word_char('a', defaults));
        assert!(is_word_char('z', defaults));
        assert!(is_word_char('A', defaults));
        assert!(is_word_char('Z', defaults));
        assert!(is_word_char('m', defaults));
        // Digits
        assert!(is_word_char('0', defaults));
        assert!(is_word_char('5', defaults));
        assert!(is_word_char('9', defaults));
    }

    #[test]
    fn is_word_char_special() {
        let defaults = ultty::config::DEFAULT_SELECTION_WORD_CHARS;
        assert!(is_word_char('_', defaults));
        assert!(is_word_char('-', defaults));
        assert!(is_word_char('.', defaults));
    }

    #[test]
    fn is_word_char_space() {
        let defaults = ultty::config::DEFAULT_SELECTION_WORD_CHARS;
        assert!(!is_word_char(' ', defaults));
        assert!(!is_word_char('\t', defaults));
        assert!(!is_word_char('\n', defaults));
    }

    #[test]
    fn is_word_char_punctuation() {
        let defaults = ultty::config::DEFAULT_SELECTION_WORD_CHARS;
        assert!(!is_word_char(',', defaults));
        assert!(!is_word_char(';', defaults));
        assert!(!is_word_char('(', defaults));
        assert!(!is_word_char(')', defaults));
        assert!(!is_word_char(':', defaults));
        assert!(!is_word_char('!', defaults));
        assert!(!is_word_char('@', defaults));
        assert!(!is_word_char('#', defaults));
    }

    #[test]
    fn is_word_char_unicode() {
        let defaults = ultty::config::DEFAULT_SELECTION_WORD_CHARS;
        // Accented Latin characters
        assert!(is_word_char('é', defaults));
        assert!(is_word_char('ñ', defaults));
        assert!(is_word_char('ü', defaults));
        // CJK characters
        assert!(is_word_char('中', defaults));
        assert!(is_word_char('文', defaults));
        // Cyrillic
        assert!(is_word_char('Д', defaults));
    }

    #[test]
    fn test_is_word_char_respects_custom_selection_word_chars() {
        assert!(is_word_char('/', "_-./"));
        assert!(is_word_char(':', "_-.:/"));
        assert!(!is_word_char('.', "_-"));
        assert!(!is_word_char('_', ""));
        assert!(is_word_char('a', ""));
    }

    #[test]
    fn bracketed_paste_cannot_end_the_bracket_early() {
        let payload = paste_payload("x\x1b[201~;touch PWNED\n", true);
        assert_eq!(payload, b"\x1b[200~x[201~;touch PWNED\n\x1b[201~");
    }

    #[test]
    fn bracketed_paste_strips_escape_and_c1_controls() {
        // ESC[200~, a raw ESC, and the 8-bit CSI (U+009B) followed by 201~.
        let payload = paste_payload("a\x1b[200~b\x1bc\u{9b}201~d\u{85}é\tz", true);
        assert_eq!(payload, "\x1b[200~a[200~bc201~dé\tz\x1b[201~".as_bytes());
    }

    #[test]
    fn plain_paste_sends_newlines_as_carriage_returns() {
        assert_eq!(
            paste_payload("one\ntwo\r\nthree", false),
            b"one\rtwo\rthree"
        );
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
}
