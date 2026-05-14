//! Overlay rendering.
//!
//! Builds vertex data for vim copy-mode overlays (cursor, selection,
//! search highlights), URL underlines, visual bell border, vim mode
//! indicator, and the keyboard shortcuts modal.

use super::{OverlayState, Renderer, Rgba, Vertex};
use crate::grid::Terminal;
use crate::vim::{CopyMode, Selection as VimSelection};
use ultty_protocol::vim::VimMode;
use wgpu::util::DeviceExt;

impl Renderer {
    /// Build overlay vertex data (vim cursor, selection, URL underlines).
    pub(super) fn build_overlay_vertices(
        &self,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let dims = terminal.grid().dimensions();

        // Render vim copy mode overlays
        if let Some(copy_mode) = overlay.copy_mode {
            if copy_mode.is_active() {
                let (cursor_row, cursor_col) = copy_mode.cursor_position();
                let scroll_offset = copy_mode.scroll_offset();

                // Draw vim cursor (different color from normal cursor)
                let vim_cursor_color = Rgba::new(0.8, 0.4, 0.0, 0.8); // Orange
                let visible_row = cursor_row.saturating_sub(scroll_offset);
                if visible_row < dims.rows as usize {
                    self.emit_quad(
                        &mut vertices,
                        &mut indices,
                        cursor_col as u16,
                        cursor_col as u16 + 1,
                        visible_row as u16,
                        vim_cursor_color,
                    );
                }

                // Draw vim selection
                if let Some(selection) = copy_mode.selection() {
                    let selection_color = Rgba::new(0.4, 0.6, 0.8, 0.4); // Blue highlight
                    self.emit_vim_selection(
                        &mut vertices,
                        &mut indices,
                        &selection,
                        scroll_offset,
                        dims.rows as usize,
                        dims.cols as usize,
                        selection_color,
                    );
                }

                // Draw search match highlights
                let (matches, current_match) = copy_mode.search_matches();
                if !matches.is_empty() {
                    let match_color = Rgba::new(0.8, 0.6, 0.2, 0.4); // Yellow highlight
                    let current_match_color = Rgba::new(1.0, 0.5, 0.0, 0.6); // Bright orange for current

                    for (i, &(match_row, match_col)) in matches.iter().enumerate() {
                        let visible_row = match_row.saturating_sub(scroll_offset);
                        if visible_row < dims.rows as usize {
                            let color = if i == current_match {
                                current_match_color
                            } else {
                                match_color
                            };
                            // Highlight single character for now (could extend to match length)
                            self.emit_quad(
                                &mut vertices,
                                &mut indices,
                                match_col as u16,
                                match_col as u16 + 1,
                                visible_row as u16,
                                color,
                            );
                        }
                    }
                }
            }
        }

        // Render URL underlines when Ctrl is held
        if overlay.ctrl_held && !overlay.urls.is_empty() {
            let underline_color = Rgba::new(0.4, 0.6, 1.0, 0.8); // Light blue
            let underline_height = 2.0;

            for url in overlay.urls {
                if url.row < dims.rows {
                    let x0 = url.start_col as f32 * self.cell_width;
                    let x1 = url.end_col as f32 * self.cell_width;
                    let y = (url.row as f32 + 1.0) * self.cell_height - underline_height
                        + self.tab_bar_height;

                    let base_idx = vertices.len() as u32;
                    let color_arr = underline_color.to_array();

                    vertices.push(Vertex {
                        position: [x0, y],
                        tex_coord: [0.0, 0.0],
                        color: color_arr,
                        is_color: 0,
                    });
                    vertices.push(Vertex {
                        position: [x1, y],
                        tex_coord: [1.0, 0.0],
                        color: color_arr,
                        is_color: 0,
                    });
                    vertices.push(Vertex {
                        position: [x1, y + underline_height],
                        tex_coord: [1.0, 1.0],
                        color: color_arr,
                        is_color: 0,
                    });
                    vertices.push(Vertex {
                        position: [x0, y + underline_height],
                        tex_coord: [0.0, 1.0],
                        color: color_arr,
                        is_color: 0,
                    });

                    indices.push(base_idx);
                    indices.push(base_idx + 1);
                    indices.push(base_idx + 2);
                    indices.push(base_idx);
                    indices.push(base_idx + 2);
                    indices.push(base_idx + 3);
                }
            }
        }

        // Render scrollbar if there's scrollback
        if let Some(scrollbar) = &overlay.scrollbar {
            if scrollbar.is_visible() {
                self.emit_scrollbar(&mut vertices, &mut indices, scrollbar);
            }
        }

        // Render visual bell (border flash)
        if overlay.bell_active {
            self.emit_bell_border(&mut vertices, &mut indices);
        }

        (vertices, indices)
    }

    /// Emit visual bell border vertices (flashing border).
    pub(super) fn emit_bell_border(&self, vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
        let border_width = 4.0;
        let bell_color = Rgba::new(1.0, 0.8, 0.2, 0.8); // Bright yellow/orange

        let width = self.config.width as f32;
        let height = self.config.height as f32;

        // Top border
        self.emit_rect(vertices, indices, 0.0, 0.0, width, border_width, bell_color);
        // Bottom border
        self.emit_rect(
            vertices,
            indices,
            0.0,
            height - border_width,
            width,
            border_width,
            bell_color,
        );
        // Left border
        self.emit_rect(
            vertices,
            indices,
            0.0,
            0.0,
            border_width,
            height,
            bell_color,
        );
        // Right border
        self.emit_rect(
            vertices,
            indices,
            width - border_width,
            0.0,
            border_width,
            height,
            bell_color,
        );
    }

    /// Emit quads for vim selection.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_vim_selection(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        selection: &VimSelection,
        scroll_offset: usize,
        viewport_rows: usize,
        viewport_cols: usize,
        color: Rgba,
    ) {
        match selection {
            VimSelection::Char {
                start_row,
                start_col,
                end_row,
                end_col,
            } => {
                for row in *start_row..=*end_row {
                    let visible_row = row.saturating_sub(scroll_offset);
                    if visible_row >= viewport_rows {
                        continue;
                    }

                    let sc = if row == *start_row { *start_col } else { 0 };
                    let ec = if row == *end_row {
                        *end_col
                    } else {
                        viewport_cols - 1
                    };

                    self.emit_quad(
                        vertices,
                        indices,
                        sc as u16,
                        (ec + 1) as u16,
                        visible_row as u16,
                        color,
                    );
                }
            }
            VimSelection::Line { start_row, end_row } => {
                for row in *start_row..=*end_row {
                    let visible_row = row.saturating_sub(scroll_offset);
                    if visible_row >= viewport_rows {
                        continue;
                    }

                    self.emit_quad(
                        vertices,
                        indices,
                        0,
                        viewport_cols as u16,
                        visible_row as u16,
                        color,
                    );
                }
            }
            VimSelection::Block {
                start_row,
                start_col,
                end_row,
                end_col,
            } => {
                for row in *start_row..=*end_row {
                    let visible_row = row.saturating_sub(scroll_offset);
                    if visible_row >= viewport_rows {
                        continue;
                    }

                    self.emit_quad(
                        vertices,
                        indices,
                        *start_col as u16,
                        (*end_col + 1) as u16,
                        visible_row as u16,
                        color,
                    );
                }
            }
        }
    }

    /// Build vim mode indicator vertices.
    /// Displays the current mode at the bottom-left of the terminal area.
    pub(super) fn build_vim_mode_indicator_vertices(
        &mut self,
        copy_mode: &CopyMode,
    ) -> (Vec<Vertex>, Vec<u32>) {
        if !copy_mode.is_active() {
            return (Vec::new(), Vec::new());
        }

        let mode = copy_mode.vim_mode();
        let text = match mode {
            VimMode::Normal => "-- NORMAL --".to_string(),
            VimMode::Visual => "-- VISUAL --".to_string(),
            VimMode::VisualLine => "-- VISUAL LINE --".to_string(),
            VimMode::VisualBlock => "-- VISUAL BLOCK --".to_string(),
            VimMode::Search => {
                let query = copy_mode.search_query();
                format!("/ {}", query)
            }
            VimMode::Command => ": ".to_string(),
            VimMode::Insert => "-- INSERT --".to_string(),
        };

        // Position at bottom-left with some padding
        let padding = 8.0;
        let x = padding;
        let y = self.config.height as f32 - self.cell_height - padding;

        // Mode indicator background
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Draw a semi-transparent background for the indicator
        let bg_color = Rgba::new(0.1, 0.1, 0.1, 0.9);
        let text_width = text.len() as f32 * self.cell_width * 0.6; // Approximate width
        let bg_width = text_width + padding * 2.0;
        let bg_height = self.cell_height + padding;

        self.emit_rect(
            &mut vertices,
            &mut indices,
            x - padding / 2.0,
            y - padding / 4.0,
            bg_width,
            bg_height,
            bg_color,
        );

        // Build text vertices using the text renderer
        let text_color = match mode {
            VimMode::Normal => Rgba::new(0.5, 0.8, 0.5, 1.0), // Green
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => {
                Rgba::new(0.8, 0.6, 0.2, 1.0) // Orange
            }
            VimMode::Search => Rgba::new(0.5, 0.7, 1.0, 1.0), // Blue
            _ => self.color_scheme.foreground,
        };

        let (text_vertices, text_indices) = self.text_renderer.build_label_vertices(
            &text,
            x,
            y,
            self.config.width as f32 - padding * 2.0,
            text_color,
        );

        // Merge text vertices
        let base_idx = vertices.len() as u32;
        vertices.extend(text_vertices);
        indices.extend(text_indices.iter().map(|i| i + base_idx));

        (vertices, indices)
    }

    /// Build a centered two-column modal overlay.
    pub(super) fn build_rows_overlay_vertices(
        &mut self,
        title: &str,
        rows: &[(String, String)],
        footer: &str,
    ) -> (Vec<Vertex>, Vec<u32>, Vec<Vertex>, Vec<u32>) {
        let mut bg_vertices = Vec::new();
        let mut bg_indices = Vec::new();
        let mut text_vertices = Vec::new();
        let mut text_indices = Vec::new();

        let mut append_text = |mut verts: Vec<Vertex>, inds: Vec<u32>| {
            let base_idx = text_vertices.len() as u32;
            text_vertices.append(&mut verts);
            text_indices.extend(inds.into_iter().map(|i| i + base_idx));
        };

        let padding = 20.0;
        let column_gap = 36.0;
        let line_height = self.cell_height * 1.5;

        // Calculate dimensions
        let max_key_width =
            rows.iter().map(|(k, _)| k.len()).max().unwrap_or(10) as f32 * self.cell_width * 0.9;
        let max_desc_width =
            rows.iter().map(|(_, d)| d.len()).max().unwrap_or(20) as f32 * self.cell_width * 0.9;
        let content_width = max_key_width + max_desc_width + padding * 2.0 + column_gap;
        let content_height = (rows.len() as f32 + 2.0) * line_height + padding * 2.0;

        // Center the modal
        let modal_width = content_width.max(300.0);
        let modal_height = content_height.min(self.config.height as f32 - 100.0);
        let modal_x = (self.config.width as f32 - modal_width) / 2.0;
        let modal_y = (self.config.height as f32 - modal_height) / 2.0;

        // Draw semi-transparent backdrop
        let backdrop_color = Rgba::new(0.0, 0.0, 0.0, 0.7);
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            0.0,
            0.0,
            self.config.width as f32,
            self.config.height as f32,
            backdrop_color,
        );

        // Draw modal background
        let modal_bg = Rgba::new(0.15, 0.15, 0.15, 0.95);
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            modal_x,
            modal_y,
            modal_width,
            modal_height,
            modal_bg,
        );

        // Draw modal border
        let border_color = Rgba::new(0.4, 0.4, 0.4, 1.0);
        let border_width = 2.0;
        // Top border
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            modal_x,
            modal_y,
            modal_width,
            border_width,
            border_color,
        );
        // Bottom border
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            modal_x,
            modal_y + modal_height - border_width,
            modal_width,
            border_width,
            border_color,
        );
        // Left border
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            modal_x,
            modal_y,
            border_width,
            modal_height,
            border_color,
        );
        // Right border
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            modal_x + modal_width - border_width,
            modal_y,
            border_width,
            modal_height,
            border_color,
        );

        // Draw title
        let title_color = Rgba::new(0.9, 0.9, 0.9, 1.0);
        let (title_vertices, title_indices) = self.text_renderer.build_label_vertices(
            title,
            modal_x + padding,
            modal_y + padding,
            modal_width - padding * 2.0,
            title_color,
        );
        append_text(title_vertices, title_indices);

        // Draw separator line under title
        let sep_y = modal_y + padding + line_height;
        self.emit_rect(
            &mut bg_vertices,
            &mut bg_indices,
            modal_x + padding,
            sep_y,
            modal_width - padding * 2.0,
            1.0,
            Rgba::new(0.3, 0.3, 0.3, 1.0),
        );

        // Draw keybindings
        let key_color = Rgba::new(0.6, 0.8, 1.0, 1.0); // Light blue for keys
        let desc_color = Rgba::new(0.8, 0.8, 0.8, 1.0); // Light gray for descriptions

        let start_y = sep_y + line_height * 0.5;
        for (i, (key, desc)) in rows.iter().enumerate() {
            let y = start_y + (i as f32 + 1.0) * line_height;
            if y + line_height > modal_y + modal_height - padding {
                break; // Don't overflow the modal
            }

            // Draw key
            let (key_vertices, key_indices) = self.text_renderer.build_label_vertices(
                key,
                modal_x + padding,
                y,
                max_key_width + padding,
                key_color,
            );
            append_text(key_vertices, key_indices);

            // Draw description
            let (desc_vertices, desc_indices) = self.text_renderer.build_label_vertices(
                desc,
                modal_x + padding + max_key_width + column_gap,
                y,
                max_desc_width + padding,
                desc_color,
            );
            append_text(desc_vertices, desc_indices);
        }

        // Draw footer hint
        let footer_color = Rgba::new(0.5, 0.5, 0.5, 1.0);
        let (footer_vertices, footer_indices) = self.text_renderer.build_label_vertices(
            footer,
            modal_x + padding,
            modal_y + modal_height - padding - line_height * 0.5,
            modal_width - padding * 2.0,
            footer_color,
        );
        append_text(footer_vertices, footer_indices);

        (bg_vertices, bg_indices, text_vertices, text_indices)
    }

    /// Build keyboard shortcuts overlay vertices.
    /// Displays a centered modal with all available keybindings.
    pub(super) fn build_shortcuts_overlay_vertices(
        &mut self,
        keybindings: &[(String, String)],
    ) -> (Vec<Vertex>, Vec<u32>, Vec<Vertex>, Vec<u32>) {
        self.build_rows_overlay_vertices("Keyboard Shortcuts", keybindings, "Press Esc to close")
    }

    /// Build native protocol status overlay vertices.
    pub(super) fn build_protocol_status_overlay_vertices(
        &mut self,
        rows: &[(String, String)],
    ) -> (Vec<Vertex>, Vec<u32>, Vec<Vertex>, Vec<u32>) {
        self.build_rows_overlay_vertices("Native Protocol Status", rows, "Press Esc to close")
    }

    /// Render the keyboard shortcuts overlay.
    pub(super) fn render_shortcuts_overlay(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        keybindings: &[(String, String)],
    ) {
        let (bg_vertices, bg_indices, text_vertices, text_indices) =
            self.build_shortcuts_overlay_vertices(keybindings);
        if bg_vertices.is_empty() && text_vertices.is_empty() {
            return;
        }

        if !bg_vertices.is_empty() && !bg_indices.is_empty() {
            let vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shortcuts_bg_vb"),
                    contents: bytemuck::cast_slice(&bg_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shortcuts_bg_ib"),
                    contents: bytemuck::cast_slice(&bg_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shortcuts_bg_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(self.bg_pipeline.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
        }

        if !text_vertices.is_empty() && !text_indices.is_empty() {
            let vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shortcuts_text_vb"),
                    contents: bytemuck::cast_slice(&text_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shortcuts_text_ib"),
                    contents: bytemuck::cast_slice(&text_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shortcuts_text_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(self.text_renderer.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..text_indices.len() as u32, 0, 0..1);
        }
    }

    /// Render the native protocol status overlay.
    pub(super) fn render_protocol_status_overlay(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        rows: &[(String, String)],
    ) {
        let (bg_vertices, bg_indices, text_vertices, text_indices) =
            self.build_protocol_status_overlay_vertices(rows);
        if bg_vertices.is_empty() && text_vertices.is_empty() {
            return;
        }

        if !bg_vertices.is_empty() && !bg_indices.is_empty() {
            let vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("protocol_status_bg_vb"),
                    contents: bytemuck::cast_slice(&bg_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("protocol_status_bg_ib"),
                    contents: bytemuck::cast_slice(&bg_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("protocol_status_bg_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(self.bg_pipeline.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
        }

        if !text_vertices.is_empty() && !text_indices.is_empty() {
            let vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("protocol_status_text_vb"),
                    contents: bytemuck::cast_slice(&text_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("protocol_status_text_ib"),
                    contents: bytemuck::cast_slice(&text_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("protocol_status_text_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(self.text_renderer.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..text_indices.len() as u32, 0, 0..1);
        }
    }
}
