//! Text rendering with cosmic-text and glyph atlas.

use super::atlas::GlyphAtlas;
use super::pipeline::TextPipeline;
use super::{ColorScheme, Rgba, Vertex};
use crate::config::FontConfig;
use crate::grid::{CellFlags, Terminal};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Text renderer using cosmic-text for shaping and a glyph atlas for caching.
pub struct TextRenderer {
    /// Font system for text shaping
    font_system: FontSystem,
    /// Glyph atlas for caching
    atlas: GlyphAtlas,
    /// Text render pipeline
    pipeline: TextPipeline,
    /// Cell dimensions
    cell_width: f32,
    cell_height: f32,
    /// Font metrics
    metrics: Metrics,
    /// wgpu device
    device: Arc<wgpu::Device>,
    /// Font family name (empty for system monospace)
    font_family: String,
    /// Enable ligatures
    ligatures: bool,
}

impl TextRenderer {
    /// Create a new text renderer with configuration.
    pub fn with_config(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        font_config: &FontConfig,
    ) -> anyhow::Result<Self> {
        // Create font system with system fonts
        let mut font_system = FontSystem::new();

        // Calculate metrics
        let line_height = (font_config.size * font_config.line_height).ceil();
        let metrics = Metrics::new(font_config.size, line_height);

        // Measure cell dimensions using a sample character
        let font_family = font_config.family.clone();
        let (cell_width, cell_height) = Self::measure_cell_with_font(&mut font_system, metrics, &font_family);

        // Create glyph atlas
        let atlas = GlyphAtlas::new(device.clone(), queue);

        // Create text pipeline
        let pipeline = TextPipeline::new(
            &device,
            format,
            uniform_bind_group_layout,
            atlas.bind_group_layout(),
        )?;

        Ok(Self {
            font_system,
            atlas,
            pipeline,
            cell_width,
            cell_height,
            metrics,
            device,
            font_family,
            ligatures: font_config.ligatures,
        })
    }

    /// Create a new text renderer with default font.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        font_size: f32,
    ) -> anyhow::Result<Self> {
        let config = FontConfig {
            size: font_size,
            ..Default::default()
        };
        Self::with_config(device, queue, format, uniform_bind_group_layout, &config)
    }

    /// Measure cell dimensions for a monospace font.
    fn measure_cell_with_font(font_system: &mut FontSystem, metrics: Metrics, font_family: &str) -> (f32, f32) {
        let mut buffer = Buffer::new(font_system, metrics);

        // Use configured font or system monospace
        let attrs = if font_family.is_empty() {
            Attrs::new().family(Family::Monospace)
        } else {
            Attrs::new().family(Family::Name(font_family))
        };

        buffer.set_text(font_system, "M", attrs, Shaping::Advanced);

        // Get width from layout
        let width = buffer
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first())
            .map(|g| g.w)
            .unwrap_or(metrics.font_size * 0.6);

        (width.ceil(), metrics.line_height.ceil())
    }

    /// Get the shaping mode based on ligature settings.
    fn get_shaping(&self) -> Shaping {
        if self.ligatures {
            Shaping::Advanced
        } else {
            Shaping::Basic
        }
    }

    /// Get cell dimensions in pixels.
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Enable or disable ligatures.
    pub fn set_ligatures(&mut self, enabled: bool) {
        self.ligatures = enabled;
    }

    /// Check if ligatures are enabled.
    pub fn ligatures(&self) -> bool {
        self.ligatures
    }

    /// Render text from terminal state.
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        terminal: &Terminal,
        color_scheme: &ColorScheme,
        cell_width: f32,
        cell_height: f32,
        y_offset: f32,
        uniform_bind_group: &wgpu::BindGroup,
    ) -> Result<(), wgpu::SurfaceError> {
        self.render_with_offset(encoder, view, terminal, color_scheme, cell_width, cell_height, 0.0, y_offset, uniform_bind_group)
    }

    /// Render text from terminal state with X and Y offset.
    pub fn render_with_offset(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        terminal: &Terminal,
        color_scheme: &ColorScheme,
        cell_width: f32,
        cell_height: f32,
        x_offset: f32,
        y_offset: f32,
        uniform_bind_group: &wgpu::BindGroup,
    ) -> Result<(), wgpu::SurfaceError> {
        let (vertices, indices) =
            self.build_text_vertices_with_offset(terminal, color_scheme, cell_width, cell_height, x_offset, y_offset);

        if vertices.is_empty() {
            return Ok(());
        }

        // Create vertex/index buffers
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("text_vertex_buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("text_index_buffer"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        // Render pass for text
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // Don't clear - draw over backgrounds
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(self.pipeline.pipeline());
            render_pass.set_bind_group(0, uniform_bind_group, &[]);
            render_pass.set_bind_group(1, self.atlas.bind_group(), &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }

        Ok(())
    }

    /// Build vertex data for text rendering with X and Y offset.
    fn build_text_vertices_with_offset(
        &mut self,
        terminal: &Terminal,
        color_scheme: &ColorScheme,
        cell_width: f32,
        cell_height: f32,
        x_offset: f32,
        y_offset: f32,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let grid = terminal.grid();
        let dims = grid.dimensions();

        // Create a cosmic-text buffer for shaping
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        let shaping = self.get_shaping();

        for row_idx in 0..dims.rows {
            let row = match grid.row(row_idx) {
                Some(r) => r,
                None => continue,
            };

            for col_idx in 0..dims.cols {
                let cell = match row.get(col_idx) {
                    Some(c) => c,
                    None => continue,
                };

                let ch = cell.char();

                // Skip spaces and wide char spacers
                if ch == ' ' || cell.has_flag(CellFlags::WIDE_CHAR_SPACER) {
                    continue;
                }

                // Get the full grapheme (base char + combining chars)
                let grapheme = if cell.has_flag(CellFlags::HAS_COMBINING) {
                    grid.grapheme(row_idx, col_idx).unwrap_or_else(|| ch.to_string())
                } else {
                    ch.to_string()
                };

                // Determine foreground color
                let fg_color = if cell.has_flag(CellFlags::REVERSE) {
                    color_scheme.resolve(cell.bg(), false)
                } else {
                    color_scheme.resolve(cell.fg(), true)
                };

                // Handle dim
                let fg_color = if cell.has_flag(CellFlags::DIM) {
                    Rgba::new(fg_color.r * 0.5, fg_color.g * 0.5, fg_color.b * 0.5, fg_color.a)
                } else {
                    fg_color
                };

                // Skip hidden text
                if cell.has_flag(CellFlags::HIDDEN) {
                    continue;
                }

                // Shape the grapheme (base char + combining chars)
                let attrs = if self.font_family.is_empty() {
                    Attrs::new().family(Family::Monospace)
                } else {
                    Attrs::new().family(Family::Name(&self.font_family))
                };
                buffer.set_text(&mut self.font_system, &grapheme, attrs, shaping);

                // Get the glyph from the layout
                if let Some(run) = buffer.layout_runs().next() {
                    for glyph in run.glyphs.iter() {
                        // Get physical glyph with cache key
                        let physical = glyph.physical((0.0, 0.0), 1.0);

                        // Get or cache the glyph
                        if let Some(glyph_info) =
                            self.atlas.get_glyph(&mut self.font_system, physical.cache_key)
                        {
                            // Skip zero-size glyphs
                            if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
                                continue;
                            }

                            // Calculate position (offset by pane position)
                            let x = col_idx as f32 * cell_width + glyph_info.left + physical.x as f32 + x_offset;
                            let y = row_idx as f32 * cell_height + (cell_height - glyph_info.top) + physical.y as f32 + y_offset;

                            // Emit quad
                            self.emit_glyph_quad(
                                &mut vertices,
                                &mut indices,
                                x,
                                y,
                                &glyph_info,
                                fg_color,
                            );
                        }
                    }
                }
            }
        }

        (vertices, indices)
    }

    /// Emit a quad for a glyph.
    fn emit_glyph_quad(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        glyph: &super::atlas::GlyphInfo,
        color: Rgba,
    ) {
        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        let x0 = x;
        let y0 = y;
        let x1 = x + glyph.width;
        let y1 = y + glyph.height;

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [glyph.uv[0], glyph.uv[1]],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [glyph.uv[2], glyph.uv[1]],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [glyph.uv[2], glyph.uv[3]],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [glyph.uv[0], glyph.uv[3]],
            color: color_arr,
        });

        // Two triangles per quad
        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Build vertices for a text label at a specific position.
    ///
    /// Returns vertices and indices that can be rendered with the text pipeline.
    pub fn build_label_vertices(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        max_width: f32,
        color: Rgba,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        if text.is_empty() {
            return (vertices, indices);
        }

        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        let shaping = self.get_shaping();

        // Use configured font or system monospace
        let attrs = if self.font_family.is_empty() {
            Attrs::new().family(Family::Monospace)
        } else {
            Attrs::new().family(Family::Name(&self.font_family))
        };

        buffer.set_text(&mut self.font_system, text, attrs, shaping);

        let mut current_x = x;
        let max_x = x + max_width;

        // Get glyphs from layout
        if let Some(run) = buffer.layout_runs().next() {
            for glyph in run.glyphs.iter() {
                // Check if we've exceeded max width
                if current_x + glyph.w > max_x {
                    // Add ellipsis if there's room
                    if current_x + self.cell_width < max_x {
                        self.render_char_at(
                            &mut vertices,
                            &mut indices,
                            '…',
                            current_x,
                            y,
                            color,
                        );
                    }
                    break;
                }

                let physical = glyph.physical((0.0, 0.0), 1.0);

                if let Some(glyph_info) = self.atlas.get_glyph(&mut self.font_system, physical.cache_key) {
                    if glyph_info.width > 0.0 && glyph_info.height > 0.0 {
                        let gx = current_x + glyph_info.left + physical.x as f32;
                        let gy = y + (self.cell_height - glyph_info.top) + physical.y as f32;

                        self.emit_glyph_quad(
                            &mut vertices,
                            &mut indices,
                            gx,
                            gy,
                            &glyph_info,
                            color,
                        );
                    }
                }

                current_x += glyph.w;
            }
        }

        (vertices, indices)
    }

    /// Render a single character at a position (helper for ellipsis, etc.).
    fn render_char_at(
        &mut self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        ch: char,
        x: f32,
        y: f32,
        color: Rgba,
    ) {
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        let shaping = self.get_shaping();

        let attrs = if self.font_family.is_empty() {
            Attrs::new().family(Family::Monospace)
        } else {
            Attrs::new().family(Family::Name(&self.font_family))
        };

        buffer.set_text(&mut self.font_system, &ch.to_string(), attrs, shaping);

        if let Some(run) = buffer.layout_runs().next() {
            if let Some(glyph) = run.glyphs.first() {
                let physical = glyph.physical((0.0, 0.0), 1.0);

                if let Some(glyph_info) = self.atlas.get_glyph(&mut self.font_system, physical.cache_key) {
                    if glyph_info.width > 0.0 && glyph_info.height > 0.0 {
                        let gx = x + glyph_info.left + physical.x as f32;
                        let gy = y + (self.cell_height - glyph_info.top) + physical.y as f32;

                        self.emit_glyph_quad(vertices, indices, gx, gy, &glyph_info, color);
                    }
                }
            }
        }
    }

    /// Get the atlas bind group for rendering labels.
    pub fn atlas_bind_group(&self) -> &wgpu::BindGroup {
        self.atlas.bind_group()
    }

    /// Get the text pipeline for rendering labels.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        self.pipeline.pipeline()
    }

    /// Clear the glyph cache.
    pub fn clear_cache(&mut self) {
        self.atlas.clear();
    }

    /// Get cache statistics.
    pub fn cache_stats(&self) -> super::atlas::AtlasStats {
        self.atlas.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_calculation() {
        let metrics = Metrics::new(14.0, 18.0);
        assert_eq!(metrics.font_size, 14.0);
        assert_eq!(metrics.line_height, 18.0);
    }
}
