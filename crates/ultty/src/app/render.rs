//! Rendering: single-pane, multi-pane, and frame dispatch.

use std::time::Instant;

use super::App;
use ultty::renderer::{OverlayState, ScrollbarState};

impl App {
    /// Render the terminal.
    pub(super) fn render(&mut self) {
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

                // Collect live region IDs for image cache eviction
                let live_regions: std::collections::HashSet<ultty_protocol::RegionId> = server
                    .sessions()
                    .flat_map(|s| s.scene().iter_depth_first().map(|r| r.id))
                    .collect();
                self.renderer.retain_native_images(&live_regions);

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
}
