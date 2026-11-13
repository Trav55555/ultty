//! Rendering: single-pane, multi-pane, and frame dispatch.

use std::time::Instant;

use super::update::pipeline_trace_enabled;
use super::App;
use ultty::event_loop::TerminalGuard;
use ultty::protocol::ProtocolServer;
use ultty::renderer::{ImePreedit, OverlayState, PaneDraw, PaneInfo};
use ultty::terminal_manager::{TerminalId, TerminalManager};

impl App {
    /// Render the terminal.
    pub(super) fn render(&mut self) {
        self.update_window_corner_radius();
        if self.frames_drawn == 0 {
            tracing::debug!(target: "ultty::startup", "drawing the first frame");
        }
        self.frames_drawn = self.frames_drawn.saturating_add(1);

        let trace_start = pipeline_trace_enabled().then(Instant::now);
        let pane_count = self.runtime.manager.pane_count();
        // Where the cursor is, read before the frame is acquired and the
        // terminals are locked for drawing.
        let cursor_cell = self.cursor_cell_rect();
        self.update_ime_cursor_area(cursor_cell);

        // Acquire the frame before locking any terminal: this can block
        // until the next vsync, and the terminals' I/O threads must keep
        // parsing meanwhile.
        let (output, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output) => (output, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(output) => (output, true),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure_surface();
                return;
            }
            // Nothing to draw into this time; the terminals stay dirty, so
            // the next update tries again.
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
        // Tabs and panes close from many places; whatever their images left
        // in the texture cache goes here.
        self.renderer
            .retain_terminal_images(&self.runtime.manager.terminal_uids());
        let drawn = if pane_count > 1 {
            self.render_multi_pane(&view, cursor_cell)
        } else {
            self.render_single_pane(&view, cursor_cell)
        };
        let drawn_at = trace_start.map(|_| Instant::now());
        // Present only now that the drawing above has released every
        // terminal lock: presenting can block for a whole refresh, or for as
        // long as the compositor withholds frame callbacks, and the I/O
        // threads must keep parsing output meanwhile.
        if drawn {
            // On Wayland this asks for a frame callback with this commit;
            // winit holds the next `RedrawRequested` until it arrives. Only
            // call it when a present follows, or no callback ever comes.
            self.window.pre_present_notify();
            self.renderer.present(output);
        }
        let presented_at = trace_start.map(|_| Instant::now());

        // Clear dirty flags after successful render
        self.runtime.manager.clear_dirty();
        // Clear render dirty flags of the sessions that were just
        // drawn so the renderer can reuse cached GPU buffers. Hidden
        // tabs' sessions stay dirty until they are shown.
        if let Some(ref mut server) = self.runtime.protocol {
            for session in server.active_tab_sessions_mut() {
                session.clear_render_dirty();
                session.clear_scroll_dirty();
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

        // Handle auto-dump countdown
        if self.auto_dump_frames > 0 {
            self.auto_dump_frames -= 1;
            if self.auto_dump_frames == 0 {
                tracing::info!("Auto-dump triggered");
                self.debug_dump_terminal();
            }
        }
    }

    /// The focused terminal's cursor cell in window pixels, or `None` when
    /// no terminal has focus.
    fn cursor_cell_rect(&self) -> Option<CellRect> {
        let (width, height) = self.renderer.cell_dimensions();
        let top = self.renderer.tab_bar_height();
        let (origin_x, origin_y) = if self.runtime.manager.pane_count() > 1 {
            let size = self.window.inner_size();
            let focused = self.runtime.manager.focused_terminal_id()?;
            let (_, (x, y, _, _)) = self
                .runtime
                .manager
                .active_pane_rects(size.width as f32, size.height as f32 - top)
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
        if self.ime_cursor_area == Some(area) {
            return;
        }
        self.ime_cursor_area = Some(area);
        self.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(area[0], area[1]),
            winit::dpi::PhysicalSize::new(area[2].max(1) as u32, area[3].max(1) as u32),
        );
    }

    /// Configure the surface again for the window's current size.
    fn reconfigure_surface(&mut self) {
        let size = self.window.inner_size();
        self.renderer.resize(&self.surface, size.width, size.height);
    }

    /// Draw a single terminal pane into `view`, returning whether there
    /// was one to draw. The terminal is locked only for the drawing; the
    /// caller presents.
    fn render_single_pane(
        &mut self,
        view: &wgpu::TextureView,
        cursor_cell: Option<CellRect>,
    ) -> bool {
        let Some((pane, terminal)) = self
            .runtime
            .manager
            .focused_terminal()
            .map(|t| (t.id, t.event_loop.terminal()))
        else {
            return false;
        };

        // Check if bell is still active
        let bell_active = self
            .bell_active_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        let protocol_status_rows = if self.protocol_status_visible {
            Some(self.build_protocol_status_rows())
        } else {
            None
        };

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
            bell_active,
            shortcuts_visible: self.shortcuts_visible,
            keybindings: if self.shortcuts_visible {
                Some(&self.shortcuts_list)
            } else {
                None
            },
            protocol_status: protocol_status_rows.as_deref(),
            cursor_blink_visible: self.cursor_blink.visible(),
            ime_preedit: cursor_cell
                .filter(|_| !self.ime_preedit.is_empty())
                .map(|cell| ImePreedit {
                    text: &self.ime_preedit,
                    x: cell.x,
                    y: cell.y,
                }),
        };

        // Only the visible tab's native sessions are drawn, and only the
        // ones of this pane or not bound yet: a session in a background
        // tab, or bound to a pane that closed under it, must not paint over
        // what the user is looking at.
        if let Some(ref server) = self.runtime.protocol {
            if server.pane_or_unbound_sessions(pane).next().is_some() {
                let selection_matches = self.native_transcript_highlights(pane);
                let search_matches = if !selection_matches.is_empty() {
                    Some(selection_matches.as_slice())
                } else if self.native_search_active && !self.native_search_matches.is_empty() {
                    Some(self.native_search_matches.as_slice())
                } else {
                    None
                };

                // Collect live region IDs (across all tabs and surfaces, so
                // switching tabs does not evict images of hidden sessions)
                // for cache eviction.
                let live_regions: std::collections::HashSet<ultty::renderer::NativeResourceKey> =
                    server
                        .sessions()
                        .flat_map(|s| s.scene().iter_all().map(move |r| (s.session_id(), r.id)))
                        .collect();
                self.renderer.retain_native_images(&live_regions);

                self.renderer.render_with_native_sessions(
                    view,
                    &terminal,
                    &overlay,
                    server
                        .pane_or_unbound_sessions(pane)
                        .map(|session| session as &dyn ultty::scene::view::NativeRenderSource),
                    search_matches,
                    self.native_search_index,
                );
                return true;
            }
        }

        self.renderer.render_with_overlay(view, &terminal, &overlay);
        true
    }

    /// Draw multiple panes into `view`, returning whether there were any
    /// to draw. The panes are locked only for the drawing; the caller
    /// presents.
    fn render_multi_pane(
        &mut self,
        view: &wgpu::TextureView,
        cursor_cell: Option<CellRect>,
    ) -> bool {
        // One lock per pane, held for this frame. Everything below reads the
        // panes through these guards; locking a pane's terminal again here
        // would deadlock.
        let locked = lock_active_panes(
            &self.runtime.manager,
            self.renderer.surface_size().into(),
            self.tab_bar.content_top(),
        );
        if locked.is_empty() {
            return false;
        }

        let mut panes = pane_draws(&locked, self.runtime.protocol.as_ref());
        for (pane, locked_pane) in panes.iter_mut().zip(&locked) {
            pane.selection = self.native_transcript_highlights(locked_pane.id);
        }

        // Check if bell is still active
        let bell_active = self
            .bell_active_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        let protocol_status_rows = if self.protocol_status_visible {
            Some(self.build_protocol_status_rows())
        } else {
            None
        };

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
            bell_active,
            shortcuts_visible: self.shortcuts_visible,
            keybindings: if self.shortcuts_visible {
                Some(&self.shortcuts_list)
            } else {
                None
            },
            protocol_status: protocol_status_rows.as_deref(),
            cursor_blink_visible: self.cursor_blink.visible(),
            ime_preedit: cursor_cell
                .filter(|_| !self.ime_preedit.is_empty())
                .map(|cell| ImePreedit {
                    text: &self.ime_preedit,
                    x: cell.x,
                    y: cell.y,
                }),
        };

        self.renderer.render_panes(view, &panes, &overlay);
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
    server: Option<&'a ProtocolServer>,
) -> Vec<PaneDraw<'a>> {
    locked
        .iter()
        .map(|pane| PaneDraw {
            terminal: &pane.terminal,
            info: pane.info,
            selection: Vec::new(),
            sessions: server.map_or_else(Vec::new, |server| {
                server
                    .pane_sessions(pane.id)
                    .map(|session| session as &dyn ultty::scene::view::NativeRenderSource)
                    .collect()
            }),
        })
        .collect()
}

/// A cell's rectangle in window pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct CellRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// Lock every pane of the active tab once, paired with its rectangle and
/// focus, as the window draws them and as multi-pane screenshots capture
/// them. The rectangles are the split tree's, the same ones each pane's grid
/// is sized to. The guards must be the only access to these terminals while
/// they live: locking one again would deadlock.
pub(super) fn lock_active_panes(
    terminal_manager: &TerminalManager,
    window_size: winit::dpi::PhysicalSize<u32>,
    tab_bar_height: f32,
) -> Vec<LockedPane<'_>> {
    let terminals = terminal_manager.active_tab_terminals();
    let focused_id = terminal_manager.focused_terminal_id();
    terminal_manager
        .active_pane_rects(
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
