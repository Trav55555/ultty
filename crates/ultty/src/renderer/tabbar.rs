//! Tab bar rendering.
//!
//! Builds vertex data for the tab bar background, individual tab backgrounds,
//! active tab indicators, and tab title text.

use super::{Renderer, Rgba, Vertex};
use crate::tabs::{TabBarState, TAB_ROW_HEIGHT, TITLE_BAR_HEIGHT};

fn lighten(color: Rgba, amount: f32) -> Rgba {
    Rgba::new(
        (color.r + amount).min(1.0),
        (color.g + amount).min(1.0),
        (color.b + amount).min(1.0),
        color.a,
    )
}

fn dim(color: Rgba, amount: f32) -> Rgba {
    Rgba::new(
        (color.r * amount).min(1.0),
        (color.g * amount).min(1.0),
        (color.b * amount).min(1.0),
        color.a,
    )
}

impl Renderer {
    /// Build tab bar vertex data.
    pub(super) fn build_tab_bar_vertices(&self, tab_bar: &TabBarState) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        if !tab_bar.visible {
            return (vertices, indices);
        }

        let height = tab_bar.height();
        let show_tabs = tab_bar.shows_tab_row();
        let bg = self.color_scheme.background;
        let title_bg = lighten(bg, 0.012);
        let tab_row_bg = lighten(bg, 0.035);
        let rail_line = lighten(bg, 0.075);
        let active_tab_bg = lighten(bg, 0.012);
        let inactive_tab_bg = lighten(bg, 0.02);
        let inactive_hover_bg = lighten(bg, 0.055);
        let accent_color = dim(self.color_scheme.cursor, 0.85);
        let control_hover_bg = lighten(bg, 0.07);
        let destructive_hover_bg = Rgba::new(0.55, 0.16, 0.14, 1.0);

        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            0.0,
            self.config.width as f32,
            TITLE_BAR_HEIGHT,
            title_bg,
        );

        if show_tabs {
            self.emit_rect(
                &mut vertices,
                &mut indices,
                0.0,
                TITLE_BAR_HEIGHT,
                self.config.width as f32,
                TAB_ROW_HEIGHT,
                tab_row_bg,
            );
        }

        // The rail/content divider is drawn before tabs so the selected tab can
        // bridge over it and feel attached to the terminal surface.
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            height - 1.0,
            self.config.width as f32,
            1.0,
            rail_line,
        );

        // Draw individual tabs (only when 2+ tabs)
        for tab in &tab_bar.tabs {
            if !show_tabs {
                continue;
            }
            let tab_bg = if tab.is_active {
                active_tab_bg
            } else if tab.is_hovered {
                inactive_hover_bg
            } else {
                inactive_tab_bg
            };

            let margin_x = 1.0;
            let tab_y = TITLE_BAR_HEIGHT + if tab.is_active { 1.0 } else { 3.0 };
            let tab_height = if tab.is_active {
                height - tab_y
            } else {
                height - tab_y - 1.0
            };
            self.emit_rect(
                &mut vertices,
                &mut indices,
                tab.x + margin_x,
                tab_y,
                tab.width - margin_x * 2.0,
                tab_height,
                tab_bg,
            );

            if tab.is_active {
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    tab.x + margin_x,
                    tab_y,
                    tab.width - margin_x * 2.0,
                    2.0,
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
                let target_size = 18.0;
                let cx = close_x + 6.0 - target_size / 2.0;
                let cy = TITLE_BAR_HEIGHT + (TAB_ROW_HEIGHT - target_size) / 2.0;
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    cx,
                    cy,
                    target_size,
                    target_size,
                    destructive_hover_bg,
                );
            }
        }

        // New tab "+" button
        {
            use crate::tabs::TabBarState;
            let btn_x = tab_bar.new_tab_button_x;
            let btn_w = TabBarState::NEW_TAB_BUTTON_WIDTH;
            let margin = 3.0;
            if tab_bar.new_tab_button_hovered {
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    btn_x + margin,
                    margin,
                    btn_w - margin * 2.0,
                    TITLE_BAR_HEIGHT - margin * 2.0,
                    control_hover_bg,
                );
            }
        }

        // Window control buttons (right side): minimize, maximize, close
        {
            use crate::tabs::{WindowControl, WINDOW_BUTTON_WIDTH, WINDOW_CONTROLS_WIDTH};
            let controls_start = tab_bar.total_width - WINDOW_CONTROLS_WIDTH;
            let padding = (WINDOW_CONTROLS_WIDTH - WINDOW_BUTTON_WIDTH * 3.0) / 2.0;
            let margin = 3.0;
            let btn_h = TITLE_BAR_HEIGHT - margin * 2.0;

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
                        destructive_hover_bg
                    } else {
                        control_hover_bg
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
                                // Center text vertically in the tab bar. Place the cell top so the
                                // glyph baseline lands roughly in the vertical middle.
        let tab_text_y = TITLE_BAR_HEIGHT + (TAB_ROW_HEIGHT - self.cell_height) / 2.0;
        let control_text_y = (TITLE_BAR_HEIGHT - self.cell_height) / 2.0;

        let show_tabs = tab_bar.shows_tab_row();
        for tab in &tab_bar.tabs {
            if !show_tabs {
                continue;
            }
            let text_color = if tab.is_active {
                self.color_scheme.foreground
            } else {
                dim(self.color_scheme.foreground, 0.62)
            };

            // Calculate text position and max width
            let text_x = tab.x + tab_padding;
            let max_text_width = tab.width - tab_padding * 2.0 - 16.0; // Leave room for close button

            let (vertices, indices) = self.text_renderer.build_label_vertices(
                &tab.title,
                text_x,
                tab_text_y,
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
                lighten(self.color_scheme.foreground, 0.08)
            } else {
                dim(self.color_scheme.foreground, 0.36)
            };
            let (v, idx) = self.text_renderer.build_label_vertices(
                "×",
                close_center_x,
                tab_text_y,
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

            let ctrl_color = dim(self.color_scheme.foreground, 0.46);

            for (i, symbol) in ["−", "□", "×"].iter().enumerate() {
                let btn_x = controls_start + padding + i as f32 * WINDOW_BUTTON_WIDTH;
                let center_x = btn_x + WINDOW_BUTTON_WIDTH / 2.0 - self.cell_width / 2.0;
                let (v, idx) = self.text_renderer.build_label_vertices(
                    symbol,
                    center_x,
                    control_text_y,
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

            let plus_color = dim(self.color_scheme.foreground, 0.58);

            let (vertices, indices) = self.text_renderer.build_label_vertices(
                "+",
                center_x,
                control_text_y,
                btn_w,
                plus_color,
            );
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(vertices);
            all_indices.extend(indices.iter().map(|i| i + base_idx));
        }

        (all_vertices, all_indices)
    }
}
