//! Tab bar rendering.
//!
//! Builds vertex data for the tab bar background, individual tab backgrounds,
//! active tab indicators, and tab title text.

use super::{Renderer, Rgba, Vertex};
use crate::tabs::TabBarState;

impl Renderer {
    /// Build tab bar vertex data.
    pub(super) fn build_tab_bar_vertices(&self, tab_bar: &TabBarState) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        if !tab_bar.visible {
            return (vertices, indices);
        }

        let height = tab_bar.height();

        // Tab bar background - slightly lighter than terminal background
        let bar_bg = Rgba::new(
            (self.color_scheme.background.r + 0.05).min(1.0),
            (self.color_scheme.background.g + 0.05).min(1.0),
            (self.color_scheme.background.b + 0.05).min(1.0),
            1.0,
        );

        // Draw full tab bar background
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            0.0,
            self.config.width as f32,
            height,
            bar_bg,
        );

        // Draw individual tabs (only when 2+ tabs)
        let show_tabs = tab_bar.tabs.len() > 1;
        for tab in &tab_bar.tabs {
            if !show_tabs {
                continue;
            }
            let tab_bg = if tab.is_active {
                // Active tab - same as terminal background (appears "connected")
                self.color_scheme.background
            } else if tab.is_hovered {
                // Hovered tab - slightly brighter
                Rgba::new(
                    (self.color_scheme.background.r + 0.1).min(1.0),
                    (self.color_scheme.background.g + 0.1).min(1.0),
                    (self.color_scheme.background.b + 0.1).min(1.0),
                    1.0,
                )
            } else {
                // Inactive tab - slightly darker
                Rgba::new(
                    (self.color_scheme.background.r - 0.02).max(0.0),
                    (self.color_scheme.background.g - 0.02).max(0.0),
                    (self.color_scheme.background.b - 0.02).max(0.0),
                    1.0,
                )
            };

            // Tab background with small margin
            let margin = 2.0;
            let tab_height = height - margin * 2.0;
            self.emit_rect(
                &mut vertices,
                &mut indices,
                tab.x + margin,
                margin,
                tab.width - margin * 2.0,
                tab_height,
                tab_bg,
            );

            // Active tab indicator (bottom border)
            if tab.is_active {
                let accent_color = Rgba::new(0.3, 0.5, 0.8, 1.0); // Blue accent
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    tab.x + margin,
                    height - 3.0,
                    tab.width - margin * 2.0,
                    3.0,
                    accent_color,
                );
            }

            // Close button hover background
            let tab_idx = tab_bar
                .tabs
                .iter()
                .position(|t| (t.x - tab.x).abs() < 0.1)
                .unwrap_or(0);
            if tab_bar.hovered_close_button == Some(tab_idx) {
                let close_x = tab.x + tab.width - 12.0 - 12.0;
                let circle_size = 14.0;
                let cx = close_x + 6.0 - circle_size / 2.0;
                let cy = (height - circle_size) / 2.0;
                let hover_bg = Rgba::new(0.7, 0.2, 0.2, 1.0);
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    cx,
                    cy,
                    circle_size,
                    circle_size,
                    hover_bg,
                );
            }
        }

        // New tab "+" button
        {
            use crate::tabs::TabBarState;
            let btn_x = tab_bar.new_tab_button_x;
            let btn_w = TabBarState::NEW_TAB_BUTTON_WIDTH;
            let margin = 2.0;
            let btn_bg = if tab_bar.new_tab_button_hovered {
                Rgba::new(
                    (self.color_scheme.background.r + 0.15).min(1.0),
                    (self.color_scheme.background.g + 0.15).min(1.0),
                    (self.color_scheme.background.b + 0.15).min(1.0),
                    1.0,
                )
            } else {
                Rgba::new(
                    (self.color_scheme.background.r + 0.05).min(1.0),
                    (self.color_scheme.background.g + 0.05).min(1.0),
                    (self.color_scheme.background.b + 0.05).min(1.0),
                    0.0, // Transparent when not hovered
                )
            };
            if tab_bar.new_tab_button_hovered {
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    btn_x + margin,
                    margin,
                    btn_w - margin * 2.0,
                    height - margin * 2.0,
                    btn_bg,
                );
            }
        }

        // Window control buttons (right side): minimize, maximize, close
        {
            use crate::tabs::{WindowControl, WINDOW_BUTTON_WIDTH, WINDOW_CONTROLS_WIDTH};
            let controls_start = tab_bar.total_width - WINDOW_CONTROLS_WIDTH;
            let padding = (WINDOW_CONTROLS_WIDTH - WINDOW_BUTTON_WIDTH * 3.0) / 2.0;
            let margin = 2.0;
            let btn_h = height - margin * 2.0;

            for (i, control) in [
                WindowControl::Minimize,
                WindowControl::Maximize,
                WindowControl::Close,
            ]
            .iter()
            .enumerate()
            {
                let btn_x = controls_start + padding + i as f32 * WINDOW_BUTTON_WIDTH;
                let is_hovered = tab_bar.hovered_window_control == Some(*control);

                if is_hovered {
                    let hover_bg = if *control == WindowControl::Close {
                        Rgba::new(0.8, 0.2, 0.2, 1.0) // Red for close
                    } else {
                        Rgba::new(
                            (self.color_scheme.background.r + 0.15).min(1.0),
                            (self.color_scheme.background.g + 0.15).min(1.0),
                            (self.color_scheme.background.b + 0.15).min(1.0),
                            1.0,
                        )
                    };
                    self.emit_rect(
                        &mut vertices,
                        &mut indices,
                        btn_x,
                        margin,
                        WINDOW_BUTTON_WIDTH,
                        btn_h,
                        hover_bg,
                    );
                }
            }
        }

        // Separator line at bottom of tab bar
        let separator_color = Rgba::new(0.2, 0.2, 0.2, 1.0);
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            height - 1.0,
            self.config.width as f32,
            1.0,
            separator_color,
        );

        (vertices, indices)
    }

    /// Build tab title text vertices.
    pub(super) fn build_tab_title_vertices(
        &mut self,
        tab_bar: &TabBarState,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut all_vertices = Vec::new();
        let mut all_indices = Vec::new();

        if !tab_bar.visible {
            return (all_vertices, all_indices);
        }

        let tab_padding = 12.0; // Match TAB_PADDING from tabs.rs
                                // Center glyphs vertically. Glyph rendering places the baseline at
                                // y + (cell_height - glyph_top). To visually center, offset upward
                                // so the glyph midpoint aligns with the bar midpoint.
        let text_y = (tab_bar.height() - self.cell_height) / 2.0 - self.cell_height * 0.25;

        let show_tabs = tab_bar.tabs.len() > 1;
        for tab in &tab_bar.tabs {
            if !show_tabs {
                continue;
            }
            // Text color: brighter for active tab
            let text_color = if tab.is_active {
                self.color_scheme.foreground
            } else {
                Rgba::new(
                    self.color_scheme.foreground.r * 0.7,
                    self.color_scheme.foreground.g * 0.7,
                    self.color_scheme.foreground.b * 0.7,
                    1.0,
                )
            };

            // Calculate text position and max width
            let text_x = tab.x + tab_padding;
            let max_text_width = tab.width - tab_padding * 2.0 - 16.0; // Leave room for close button

            let (vertices, indices) = self.text_renderer.build_label_vertices(
                &tab.title,
                text_x,
                text_y,
                max_text_width,
                text_color,
            );

            // Merge vertices, adjusting indices
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(vertices);
            all_indices.extend(indices.iter().map(|i| i + base_idx));

            // Close button "x" on the right side of the tab
            let tab_idx = tab_bar.tabs.iter().position(|t| t.x == tab.x).unwrap_or(0);
            let is_close_hovered = tab_bar.hovered_close_button == Some(tab_idx);
            let close_x = tab.x + tab.width - tab_padding - 12.0;
            let close_center_x = close_x + 6.0 - self.cell_width / 2.0;

            let close_color = if is_close_hovered {
                Rgba::new(1.0, 1.0, 1.0, 1.0)
            } else {
                Rgba::new(
                    self.color_scheme.foreground.r * 0.4,
                    self.color_scheme.foreground.g * 0.4,
                    self.color_scheme.foreground.b * 0.4,
                    1.0,
                )
            };
            let (v, idx) = self.text_renderer.build_label_vertices(
                "x",
                close_center_x,
                text_y,
                12.0,
                close_color,
            );
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(v);
            all_indices.extend(idx.iter().map(|i| i + base_idx));
        }

        // Render window control button symbols
        {
            use crate::tabs::{WINDOW_BUTTON_WIDTH, WINDOW_CONTROLS_WIDTH};
            let controls_start = tab_bar.total_width - WINDOW_CONTROLS_WIDTH;
            let padding = (WINDOW_CONTROLS_WIDTH - WINDOW_BUTTON_WIDTH * 3.0) / 2.0;

            let ctrl_color = Rgba::new(
                self.color_scheme.foreground.r * 0.5,
                self.color_scheme.foreground.g * 0.5,
                self.color_scheme.foreground.b * 0.5,
                1.0,
            );

            for (i, symbol) in ["_", "[]", "x"].iter().enumerate() {
                let btn_x = controls_start + padding + i as f32 * WINDOW_BUTTON_WIDTH;
                let center_x = btn_x + WINDOW_BUTTON_WIDTH / 2.0 - self.cell_width / 2.0;
                let (v, idx) = self.text_renderer.build_label_vertices(
                    symbol,
                    center_x,
                    text_y,
                    WINDOW_BUTTON_WIDTH,
                    ctrl_color,
                );
                let base_idx = all_vertices.len() as u32;
                all_vertices.extend(v);
                all_indices.extend(idx.iter().map(|i| i + base_idx));
            }
        }

        // Render "+" for new tab button
        {
            use crate::tabs::TabBarState;
            let btn_x = tab_bar.new_tab_button_x;
            let btn_w = TabBarState::NEW_TAB_BUTTON_WIDTH;
            let center_x = btn_x + btn_w / 2.0 - self.cell_width / 2.0;

            let plus_color = Rgba::new(
                self.color_scheme.foreground.r * 0.6,
                self.color_scheme.foreground.g * 0.6,
                self.color_scheme.foreground.b * 0.6,
                1.0,
            );

            let (vertices, indices) = self
                .text_renderer
                .build_label_vertices("+", center_x, text_y, btn_w, plus_color);
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(vertices);
            all_indices.extend(indices.iter().map(|i| i + base_idx));
        }

        (all_vertices, all_indices)
    }
}
