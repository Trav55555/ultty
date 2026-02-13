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

        // Draw individual tabs
        for tab in &tab_bar.tabs {
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
        let text_y = tab_bar.height() / 2.0 - self.cell_height / 2.0 + 2.0;

        for tab in &tab_bar.tabs {
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
        }

        (all_vertices, all_indices)
    }
}
