//! Rendering: single-pane, multi-pane, and frame dispatch.

use std::time::Instant;

use super::gpu::RendererState;
use super::update::pipeline_trace_enabled;
use super::WindowCtx;
use ultty::event_loop::TerminalGuard;
use ultty::protocol::ProtocolServer;
use ultty::renderer::{ImePreedit, OverlayState, PaneDraw, PaneInfo};
use ultty::terminal_manager::{TabId, TerminalId, TerminalManager, WindowKey};

impl WindowCtx<'_> {
    /// Render this window.
    pub(super) fn render(&mut self) {
        let Some(renderer) = self.win.renderer.ready() else {
            self.render_cpu_frame();
            return;
        };
        let palette_caret = renderer.command_palette_caret_cell(&self.win.command_palette);
        self.update_window_corner_radius();
        if self.win.frames_drawn == 0 {
            tracing::debug!(
                target: "ultty::startup",
                window = self.win.key.raw(),
                "drawing the first frame"
            );
        }
        if !self.win.output_shown
            && tracing::enabled!(target: "ultty::startup", tracing::Level::DEBUG)
        {
            let shows_output = self
                .terminal()
                .is_some_and(|terminal| ultty::renderer::shows_output(&terminal));
            self.win.note_output_shown(shows_output, "GPU");
        }
        self.win.frames_drawn = self.win.frames_drawn.saturating_add(1);

        let trace_start = pipeline_trace_enabled().then(Instant::now);
        let pane_count = self.app.runtime.manager.window_pane_count(self.win.key);
        let cursor_cell = self.cursor_cell_rect();
        self.update_ime_cursor_area(super::command_palette::ime_cell(palette_caret, cursor_cell));

        let RendererState::Ready(gpu) = &self.win.renderer else {
            return;
        };
        let (output, suboptimal) = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output) => (output, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(output) => (output, true),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure_surface();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::warn!("Render error: surface texture validation failed");
                return;
            }
        };
        let acquired_at = trace_start.map(|_| Instant::now());

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        if let Some(renderer) = self.win.renderer.ready_mut() {
            renderer.retain_terminal_images(&self.app.runtime.manager.terminal_uids());
        }
        let drawn = if pane_count > 1 {
            self.render_multi_pane(&view, cursor_cell)
        } else {
            self.render_single_pane(&view, cursor_cell)
        };
        let drawn_at = trace_start.map(|_| Instant::now());
        if drawn {
            if let Some(renderer) = self.win.renderer.ready() {
                self.win.window.pre_present_notify();
                renderer.present(output);
            }
        }
        let presented_at = trace_start.map(|_| Instant::now());

        self.app
            .runtime
            .manager
            .clear_window_active_dirty(self.win.key);
        if let Some(active_tab) = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id)
        {
            if let Some(ref mut server) = self.app.runtime.protocol {
                for session in server.tab_sessions_mut(active_tab) {
                    session.clear_render_dirty();
                    session.clear_scroll_dirty();
                }
            }
        }
        if suboptimal {
            self.reconfigure_surface();
        }

        if let (Some(start), Some(acquired), Some(drawn), Some(presented)) =
            (trace_start, acquired_at, drawn_at, presented_at)
        {
            tracing::info!(
                target: "ultty::perf",
                pane_count,
                acquire_us = acquired.duration_since(start).as_micros(),
                draw_us = drawn.duration_since(acquired).as_micros(),
                present_us = presented.duration_since(drawn).as_micros(),
                elapsed_us = start.elapsed().as_micros(),
                "pipeline_render_frame"
            );
        }
        ultty::heap_trim::maybe_trim();
        #[cfg(feature = "heap-stats")]
        ultty::heap_stats::maybe_log();

        if self.app.auto_dump_frames > 0 {
            self.app.auto_dump_frames -= 1;
            if self.app.auto_dump_frames == 0 {
                tracing::info!("Auto-dump triggered");
                self.debug_dump_terminal();
            }
        }
    }

    /// The focused terminal's cursor cell in window pixels, or `None` when
    /// no terminal has focus.
    fn cursor_cell_rect(&self) -> Option<CellRect> {
        let (width, height) = self.win.renderer.cell_dimensions();
        let top = self.win.renderer.tab_bar_height();
        let (origin_x, origin_y) = if self.app.runtime.manager.window_pane_count(self.win.key) > 1 {
            let size = self.win.window.inner_size();
            let focused = self
                .app
                .runtime
                .manager
                .window_focused_terminal_id(self.win.key)?;
            let (_, (x, y, _, _)) = self
                .app
                .runtime
                .manager
                .window_active_pane_rects(self.win.key, size.width as f32, size.height as f32 - top)
                .into_iter()
                .find(|(id, _)| *id == focused)?;
            (x, y + top)
        } else {
            (0.0, top)
        };
        let terminal = self.terminal()?;
        let cursor = terminal.buffer().grid().cursor().position;
        Some(CellRect {
            x: origin_x + f32::from(cursor.col) * width,
            y: origin_y + f32::from(cursor.row) * height,
            width,
            height,
        })
    }

    /// Tell the input method where the cursor is, so its candidate window
    /// opens next to it. Only a change is sent.
    fn update_ime_cursor_area(&mut self, cursor_cell: Option<CellRect>) {
        let Some(cell) = cursor_cell else {
            return;
        };
        let area = [
            cell.x.round() as i32,
            cell.y.round() as i32,
            cell.width.round() as i32,
            cell.height.round() as i32,
        ];
        if self.win.ime_cursor_area == Some(area) {
            return;
        }
        self.win.ime_cursor_area = Some(area);
        self.win.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(area[0], area[1]),
            winit::dpi::PhysicalSize::new(area[2].max(1) as u32, area[3].max(1) as u32),
        );
    }

    /// Configure the surface again for the window's current size.
    fn reconfigure_surface(&mut self) {
        let size = self.win.window.inner_size();
        self.win.renderer.resize(size.width, size.height);
    }

    fn render_single_pane(
        &mut self,
        view: &wgpu::TextureView,
        cursor_cell: Option<CellRect>,
    ) -> bool {
        let Some((pane, tab_id, terminal)) = self
            .app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)
            .map(|t| (t.id, t.tab_id, t.event_loop.terminal()))
        else {
            return false;
        };

        let bell_active = self
            .win
            .bell_active_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        let protocol_status_rows = if self.win.protocol_status_visible {
            Some(self.app.build_protocol_status_rows())
        } else {
            None
        };

        let overlay = OverlayState {
            copy_mode: if self.win.copy_mode.is_active() {
                Some(&self.win.copy_mode)
            } else {
                None
            },
            urls: self.win.url_detector.urls(),
            ctrl_held: self.win.modifiers.control_key(),
            tab_bar: if self.win.tab_bar.visible {
                Some(&self.win.tab_bar)
            } else {
                None
            },
            bell_active,
            shortcuts_visible: self.win.shortcuts_visible,
            keybindings: if self.win.shortcuts_visible {
                Some(&self.app.shortcuts_list)
            } else {
                None
            },
            protocol_status: protocol_status_rows.as_deref(),
            cursor_blink_visible: self.win.cursor_blink.visible(),
            ime_preedit: cursor_cell
                .filter(|_| !self.win.ime_preedit.is_empty())
                .map(|cell| ImePreedit {
                    text: &self.win.ime_preedit,
                    x: cell.x,
                    y: cell.y,
                }),
            command_palette: self
                .win
                .command_palette
                .is_open()
                .then_some(&self.win.command_palette),
        };

        if let Some(ref server) = self.app.runtime.protocol {
            if server
                .pane_or_unbound_sessions_in_tab(tab_id, pane)
                .next()
                .is_some()
            {
                let selection_matches = self.native_transcript_highlights(pane);
                let search_matches = if !selection_matches.is_empty() {
                    Some(selection_matches.as_slice())
                } else if self.win.native_search_active
                    && !self.win.native_search_matches.is_empty()
                {
                    Some(self.win.native_search_matches.as_slice())
                } else {
                    None
                };

                let live_regions: std::collections::HashSet<ultty::renderer::NativeResourceKey> =
                    server
                        .sessions()
                        .flat_map(|s| s.scene().iter_all().map(move |r| (s.session_id(), r.id)))
                        .collect();
                let Some(renderer) = self.win.renderer.ready_mut() else {
                    return false;
                };
                renderer.retain_native_images(&live_regions);

                renderer.render_with_native_sessions(
                    view,
                    &terminal,
                    &overlay,
                    server
                        .pane_or_unbound_sessions_in_tab(tab_id, pane)
                        .map(|session| session as &dyn ultty::scene::view::NativeRenderSource),
                    search_matches,
                    self.win.native_search_index,
                );
                return true;
            }
        }

        let Some(renderer) = self.win.renderer.ready_mut() else {
            return false;
        };
        renderer.render_with_overlay(view, &terminal, &overlay);
        true
    }

    fn render_multi_pane(
        &mut self,
        view: &wgpu::TextureView,
        cursor_cell: Option<CellRect>,
    ) -> bool {
        let active_tab = self
            .app
            .runtime
            .manager
            .window_active_tab(self.win.key)
            .map(|tab| tab.id);
        let locked = lock_window_active_panes(
            &self.app.runtime.manager,
            self.win.key,
            self.win.renderer.surface_size().into(),
            self.win.tab_bar.content_top(),
        );
        if locked.is_empty() {
            return false;
        }

        let mut panes = pane_draws(&locked, active_tab, self.app.runtime.protocol.as_ref());
        for (pane, locked_pane) in panes.iter_mut().zip(&locked) {
            pane.selection = self.native_transcript_highlights(locked_pane.id);
        }

        let bell_active = self
            .win
            .bell_active_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        let protocol_status_rows = if self.win.protocol_status_visible {
            Some(self.app.build_protocol_status_rows())
        } else {
            None
        };

        let overlay = OverlayState {
            copy_mode: if self.win.copy_mode.is_active() {
                Some(&self.win.copy_mode)
            } else {
                None
            },
            urls: self.win.url_detector.urls(),
            ctrl_held: self.win.modifiers.control_key(),
            tab_bar: if self.win.tab_bar.visible {
                Some(&self.win.tab_bar)
            } else {
                None
            },
            bell_active,
            shortcuts_visible: self.win.shortcuts_visible,
            keybindings: if self.win.shortcuts_visible {
                Some(&self.app.shortcuts_list)
            } else {
                None
            },
            protocol_status: protocol_status_rows.as_deref(),
            cursor_blink_visible: self.win.cursor_blink.visible(),
            ime_preedit: cursor_cell
                .filter(|_| !self.win.ime_preedit.is_empty())
                .map(|cell| ImePreedit {
                    text: &self.win.ime_preedit,
                    x: cell.x,
                    y: cell.y,
                }),
            command_palette: self
                .win
                .command_palette
                .is_open()
                .then_some(&self.win.command_palette),
        };

        let Some(renderer) = self.win.renderer.ready_mut() else {
            return false;
        };
        renderer.render_panes(view, &panes, &overlay);
        true
    }
}

/// A pane of the active tab, locked for drawing.
pub(super) struct LockedPane<'a> {
    pub id: TerminalId,
    pub terminal: TerminalGuard<'a>,
    pub info: PaneInfo,
}

/// The locked panes as the renderer draws them, each with the shown native
/// sessions bound to it (ADR 0025).
pub(super) fn pane_draws<'a>(
    locked: &'a [LockedPane<'a>],
    active_tab: Option<TabId>,
    server: Option<&'a ProtocolServer>,
) -> Vec<PaneDraw<'a>> {
    locked
        .iter()
        .map(|pane| PaneDraw {
            terminal: &pane.terminal,
            info: pane.info,
            selection: Vec::new(),
            sessions: match (server, active_tab) {
                (Some(server), Some(tab)) => server
                    .pane_sessions_in_tab(tab, pane.id)
                    .map(|session| session as &dyn ultty::scene::view::NativeRenderSource)
                    .collect(),
                _ => Vec::new(),
            },
        })
        .collect()
}

/// A cell's rectangle in window pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct CellRect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

/// Lock every pane of `window`'s active tab once, paired with its rectangle
/// and focus, as the window draws them and as multi-pane screenshots capture
/// them.
pub(super) fn lock_window_active_panes(
    terminal_manager: &TerminalManager,
    window: WindowKey,
    window_size: winit::dpi::PhysicalSize<u32>,
    tab_bar_height: f32,
) -> Vec<LockedPane<'_>> {
    let terminals = terminal_manager.window_active_tab_terminals(window);
    let focused_id = terminal_manager.window_focused_terminal_id(window);
    terminal_manager
        .window_active_pane_rects(
            window,
            window_size.width as f32,
            window_size.height as f32 - tab_bar_height,
        )
        .into_iter()
        .filter_map(|(id, (x, y, w, h))| {
            let managed = terminals.iter().find(|managed| managed.id == id)?;
            Some(LockedPane {
                id,
                terminal: managed.event_loop.terminal(),
                info: PaneInfo::new(x, y, w, h, Some(id) == focused_id),
            })
        })
        .collect()
}
