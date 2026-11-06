//! Rendering: single-pane, multi-pane, and frame dispatch.

use std::time::Instant;

use super::update::pipeline_trace_enabled;
use super::App;
use ultty::renderer::{OverlayState, ScrollbarState};

impl App {
    /// Render the terminal.
    pub(super) fn render(&mut self) {
        self.update_window_corner_radius();

        let trace_start = pipeline_trace_enabled().then(Instant::now);
        let pane_count = self.terminal_manager.pane_count();

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

        if pane_count > 1 {
            self.render_multi_pane(output);
        } else {
            self.render_single_pane(output);
        }

        // Clear dirty flags after successful render
        self.terminal_manager.clear_dirty();
        // Clear render dirty flags of the sessions that were just
        // drawn so the renderer can reuse cached GPU buffers. Hidden
        // tabs' sessions stay dirty until they are shown.
        if let Some(ref mut server) = self.protocol_server {
            for session in server.active_tab_sessions_mut() {
                session.clear_render_dirty();
                session.clear_scroll_dirty();
            }
        }
        if suboptimal {
            self.reconfigure_surface();
        }

        if let Some(start) = trace_start {
            tracing::info!(
                target: "ultty::perf",
                pane_count,
                elapsed_us = start.elapsed().as_micros(),
                "pipeline_render_frame"
            );
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

    /// Configure the surface again for the window's current size.
    fn reconfigure_surface(&mut self) {
        let size = self.window.inner_size();
        self.renderer.resize(&self.surface, size.width, size.height);
    }

    /// Render a single terminal pane.
    fn render_single_pane(&mut self, output: wgpu::SurfaceTexture) {
        let Some(terminal) = self
            .terminal_manager
            .focused_terminal()
            .map(|t| t.event_loop.terminal())
        else {
            return;
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
            protocol_status: protocol_status_rows.as_deref(),
            cursor_blink_visible: self.cursor_blink_visible,
        };

        // Only the visible tab's native sessions are drawn; a session in a
        // background tab must not paint over the tab the user is looking at.
        if let Some(ref server) = self.protocol_server {
            if server.active_tab_sessions().next().is_some() {
                let search_matches =
                    if self.native_search_active && !self.native_search_matches.is_empty() {
                        Some(self.native_search_matches.as_slice())
                    } else {
                        None
                    };

                // Collect live region IDs (across all tabs, so switching tabs
                // does not evict images of hidden sessions) for cache eviction.
                let live_regions: std::collections::HashSet<ultty_protocol::RegionId> = server
                    .sessions()
                    .flat_map(|s| s.scene().iter_depth_first().map(|r| r.id))
                    .collect();
                self.renderer.retain_native_images(&live_regions);

                return self.renderer.render_with_native_sessions(
                    output,
                    &terminal,
                    &overlay,
                    server.active_tab_sessions(),
                    search_matches,
                    self.native_search_index,
                );
            }
        }

        self.renderer
            .render_with_overlay(output, &terminal, &overlay)
    }

    /// Render multiple panes.
    fn render_multi_pane(&mut self, output: wgpu::SurfaceTexture) {
        use ultty::renderer::PaneInfo;
        use ultty::terminal_manager::SplitDirection;

        let size = self.window.inner_size();
        let tab_bar_height = self.tab_bar.content_top();
        let available_width = size.width as f32;
        let available_height = size.height as f32 - tab_bar_height;

        let terminals = self.terminal_manager.active_tab_terminals();
        let pane_count = terminals.len();

        if pane_count == 0 {
            return;
        }

        let focused_id = self.terminal_manager.focused_terminal_id();
        let split_direction = self.terminal_manager.split_direction();

        // Calculate pane bounds based on split direction and ratios
        let ratios = self.terminal_manager.pane_ratios();
        // One lock per pane, held for this frame. Everything below reads the
        // panes through these guards; locking a pane's terminal again here
        // would deadlock.
        let locked: Vec<(ultty::event_loop::TerminalGuard<'_>, PaneInfo)> = {
            let mut offset = 0.0f32;
            terminals
                .iter()
                .enumerate()
                .map(|(i, managed)| {
                    let ratio = ratios.get(i).copied().unwrap_or(1.0 / pane_count as f32);
                    let is_focused = Some(managed.id) == focused_id;
                    let pane = match split_direction {
                        SplitDirection::Horizontal => {
                            let w = available_width * ratio;
                            let p = PaneInfo::new(offset, 0.0, w, available_height, is_focused);
                            offset += w;
                            p
                        }
                        SplitDirection::Vertical => {
                            let h = available_height * ratio;
                            let p = PaneInfo::new(0.0, offset, available_width, h, is_focused);
                            offset += h;
                            p
                        }
                    };
                    (managed.event_loop.terminal(), pane)
                })
                .collect()
        };

        let panes: Vec<(&ultty::grid::Terminal, PaneInfo)> = locked
            .iter()
            .map(|(terminal, pane)| (&**terminal, *pane))
            .collect();

        // Calculate scrollbar state from the focused pane
        let scrollbar = panes
            .iter()
            .find(|(_, pane)| pane.is_focused)
            .map(|(terminal, _)| {
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
            scrollbar: scrollbar.filter(|s| s.is_visible()),
            bell_active,
            shortcuts_visible: self.shortcuts_visible,
            keybindings: if self.shortcuts_visible {
                Some(&self.shortcuts_list)
            } else {
                None
            },
            protocol_status: protocol_status_rows.as_deref(),
            cursor_blink_visible: self.cursor_blink_visible,
        };

        self.renderer.render_panes(output, &panes, &overlay)
    }
}
