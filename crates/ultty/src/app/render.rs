//! Rendering: single-pane, multi-pane, and frame dispatch.

use std::time::Instant;

use super::update::pipeline_trace_enabled;
use super::App;
use ultty::event_loop::TerminalGuard;
use ultty::renderer::{OverlayState, PaneInfo};
use ultty::terminal_manager::{pane_rects, SplitDirection, TerminalManager};

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
        // One lock per pane, held for this frame. Everything below reads the
        // panes through these guards; locking a pane's terminal again here
        // would deadlock.
        let locked = lock_active_panes(
            &self.terminal_manager,
            self.window.inner_size(),
            self.tab_bar.content_top(),
        );
        if locked.is_empty() {
            return;
        }

        let panes: Vec<(&ultty::grid::Terminal, PaneInfo)> = locked
            .iter()
            .map(|(terminal, pane)| (&**terminal, *pane))
            .collect();

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
            cursor_blink_visible: self.cursor_blink_visible,
        };

        self.renderer.render_panes(output, &panes, &overlay)
    }
}

/// Panes of the active tab with their window rectangles and focus, as the
/// window draws them and as multi-pane screenshots capture them.
fn pane_infos(
    direction: SplitDirection,
    ratios: &[f32],
    count: usize,
    focused_index: Option<usize>,
    width: f32,
    height: f32,
) -> Vec<PaneInfo> {
    pane_rects(direction, ratios, count, width, height)
        .into_iter()
        .enumerate()
        .map(|(i, (x, y, w, h))| PaneInfo::new(x, y, w, h, Some(i) == focused_index))
        .collect()
}

/// Lock every pane of the active tab once, paired with its rectangle. The
/// guards must be the only access to these terminals while they live:
/// locking one again would deadlock.
pub(super) fn lock_active_panes(
    terminal_manager: &TerminalManager,
    window_size: winit::dpi::PhysicalSize<u32>,
    tab_bar_height: f32,
) -> Vec<(TerminalGuard<'_>, PaneInfo)> {
    let terminals = terminal_manager.active_tab_terminals();
    let focused_id = terminal_manager.focused_terminal_id();
    let infos = pane_infos(
        terminal_manager.split_direction(),
        &terminal_manager.pane_ratios(),
        terminals.len(),
        terminals.iter().position(|t| Some(t.id) == focused_id),
        window_size.width as f32,
        window_size.height as f32 - tab_bar_height,
    );
    terminals
        .iter()
        .zip(infos)
        .map(|(managed, info)| (managed.event_loop.terminal(), info))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_by_side_panes_split_the_width_and_mark_focus() {
        let infos = pane_infos(
            SplitDirection::Horizontal,
            &[0.25, 0.75],
            2,
            Some(1),
            800.0,
            560.0,
        );
        assert_eq!(infos.len(), 2);
        assert_eq!((infos[0].x, infos[0].y), (0.0, 0.0));
        assert_eq!((infos[0].width, infos[0].height), (200.0, 560.0));
        assert_eq!((infos[1].x, infos[1].y), (200.0, 0.0));
        assert_eq!((infos[1].width, infos[1].height), (600.0, 560.0));
        assert!(!infos[0].is_focused);
        assert!(infos[1].is_focused);
    }

    #[test]
    fn stacked_panes_split_the_height_equally_without_ratios() {
        let infos = pane_infos(SplitDirection::Vertical, &[], 2, Some(0), 800.0, 560.0);
        assert_eq!((infos[0].x, infos[0].y), (0.0, 0.0));
        assert_eq!((infos[0].width, infos[0].height), (800.0, 280.0));
        assert_eq!((infos[1].x, infos[1].y), (0.0, 280.0));
        assert!(infos[0].is_focused && !infos[1].is_focused);
    }

    #[test]
    fn no_focused_index_marks_no_pane_focused() {
        let infos = pane_infos(SplitDirection::Horizontal, &[], 3, None, 900.0, 100.0);
        assert!(infos.iter().all(|p| !p.is_focused));
    }
}
