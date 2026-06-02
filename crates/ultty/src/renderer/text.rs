//! Text rendering with cosmic-text and glyph atlas.

use super::atlas::GlyphAtlas;
use super::pipeline::TextPipeline;
use super::powerline::is_powerline_triangle;
use super::{ColorScheme, Rgba, Vertex};
use crate::config::FontConfig;
use crate::grid::{CellFlags, Terminal};
use cosmic_text::{Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

type ShapingCacheKey = (char, u8);

#[derive(Clone, Copy)]
struct ShapedGlyph {
    cache_key: CacheKey,
    x_offset: f32,
    y_offset: f32,
    line_y: f32,
}

pub struct TextRenderer {
    font_system: FontSystem,
    atlas: GlyphAtlas,
    pipeline: TextPipeline,
    cell_width: f32,
    cell_height: f32,
    metrics: Metrics,
    device: Arc<wgpu::Device>,
    font_family: String,
    ligatures: bool,
    shaping_cache: HashMap<ShapingCacheKey, ShapedGlyph>,
    styled_text_buffer: Buffer,
    byte_to_char_offsets: Vec<usize>,
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
        let (cell_width, cell_height) =
            Self::measure_cell_with_font(&mut font_system, metrics, &font_family);

        // Create glyph atlas
        let atlas = GlyphAtlas::new(device.clone(), queue);

        // Create text pipeline
        let pipeline = TextPipeline::new(
            &device,
            format,
            uniform_bind_group_layout,
            atlas.bind_group_layout(),
        )?;

        let styled_text_buffer = Buffer::new(&mut font_system, metrics);

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
            shaping_cache: HashMap::with_capacity(256),
            styled_text_buffer,
            byte_to_char_offsets: Vec::with_capacity(256),
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
    fn measure_cell_with_font(
        font_system: &mut FontSystem,
        metrics: Metrics,
        font_family: &str,
    ) -> (f32, f32) {
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

    /// Get the current font size.
    pub fn font_size(&self) -> f32 {
        self.metrics.font_size
    }

    /// Set the font size and recalculate cell dimensions.
    pub fn set_font_size(&mut self, size: f32) {
        // Clamp to reasonable range
        let size = size.clamp(6.0, 72.0);

        // Calculate new metrics with same line height ratio
        let line_height_ratio = self.metrics.line_height / self.metrics.font_size;
        let new_line_height = (size * line_height_ratio).ceil();
        self.metrics = Metrics::new(size, new_line_height);

        // Recalculate cell dimensions
        let (width, height) =
            Self::measure_cell_with_font(&mut self.font_system, self.metrics, &self.font_family);
        self.cell_width = width;
        self.cell_height = height;

        // Clear caches since metrics changed
        self.atlas.clear();
        self.shaping_cache.clear();
    }

    /// Build vertices for a simple text string at a given position.
    pub fn build_simple_text_vertices(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        fg: Rgba,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(&mut self.font_system, 10000.0, self.cell_height * 2.0);

        let shaping = self.get_shaping();
        let attrs = if self.font_family.is_empty() {
            Attrs::new().family(Family::Monospace)
        } else {
            Attrs::new().family(Family::Name(&self.font_family))
        };

        buffer.set_text(&mut self.font_system, text, attrs, shaping);

        // Build a char-index lookup from byte offsets so spaces (which produce
        // no glyphs in cosmic-text) are accounted for in positioning.
        let char_indices: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();

        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                // Map byte offset → character index for grid-aligned placement.
                let char_idx = char_indices
                    .iter()
                    .position(|&b| b == glyph.start)
                    .unwrap_or(0);
                let glyph_x = x + char_idx as f32 * self.cell_width;

                let physical = glyph.physical((0.0, 0.0), 1.0);

                if let Some(glyph_info) = self
                    .atlas
                    .get_glyph(&mut self.font_system, physical.cache_key)
                {
                    if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
                        continue;
                    }

                    let gx = (glyph_x + glyph_info.left).round();
                    let gy = (y + (self.cell_height - glyph_info.top)).round();

                    Self::emit_glyph_quad(&mut vertices, &mut indices, gx, gy, &glyph_info, fg);
                }
            }
        }

        (vertices, indices)
    }

    /// Build vertices for styled text (with bold/italic support).
    /// Returns (vertices, indices, advance_x) where advance_x is the total width.
    pub fn build_styled_text_vertices(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        fg: Rgba,
        bold: bool,
        italic: bool,
    ) -> (Vec<Vertex>, Vec<u32>, f32) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let advance = self.append_styled_text_vertices(
            text,
            x,
            y,
            fg,
            bold,
            italic,
            &mut vertices,
            &mut indices,
        );
        (vertices, indices, advance)
    }

    /// Append vertices for styled text into caller-owned buffers.
    ///
    /// This path reuses the renderer's shaping buffer and byte-offset scratch
    /// map, and emits indices directly into the destination buffers.
    #[allow(clippy::too_many_arguments)]
    pub fn append_styled_text_vertices(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        fg: Rgba,
        bold: bool,
        italic: bool,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
    ) -> f32 {
        use cosmic_text::{Style as CosmicStyle, Weight};

        self.styled_text_buffer
            .set_size(&mut self.font_system, 10000.0, self.cell_height * 2.0);

        let shaping = if self.ligatures {
            Shaping::Advanced
        } else {
            Shaping::Basic
        };
        let mut attrs = if self.font_family.is_empty() {
            Attrs::new().family(Family::Monospace)
        } else {
            Attrs::new().family(Family::Name(&self.font_family))
        };

        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(CosmicStyle::Italic);
        }

        self.styled_text_buffer
            .set_text(&mut self.font_system, text, attrs, shaping);

        fill_byte_to_char_offsets(text, &mut self.byte_to_char_offsets);
        let total_advance = self.byte_to_char_offsets.len() as f32 * self.cell_width;

        for run in self.styled_text_buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let char_idx = byte_offset_to_char_index(&self.byte_to_char_offsets, glyph.start);
                let glyph_x = x + char_idx as f32 * self.cell_width;

                let physical = glyph.physical((0.0, 0.0), 1.0);

                if let Some(glyph_info) = self
                    .atlas
                    .get_glyph(&mut self.font_system, physical.cache_key)
                {
                    if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
                        continue;
                    }

                    let gx = (glyph_x + glyph_info.left).round();
                    let gy = (y + (self.cell_height - glyph_info.top)).round();

                    Self::emit_glyph_quad(vertices, indices, gx, gy, &glyph_info, fg);
                }
            }
        }

        total_advance
    }

    /// Render text from terminal state.
    #[allow(clippy::too_many_arguments)]
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
        self.render_with_offset(
            encoder,
            view,
            terminal,
            color_scheme,
            cell_width,
            cell_height,
            0.0,
            y_offset,
            uniform_bind_group,
        )
    }

    /// Render text from terminal state with X and Y offset.
    #[allow(clippy::too_many_arguments)]
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
        let (vertices, indices) = self.build_text_vertices_with_offset(
            terminal,
            color_scheme,
            cell_width,
            cell_height,
            x_offset,
            y_offset,
        );

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

    fn build_text_vertices_with_offset(
        &mut self,
        terminal: &Terminal,
        color_scheme: &ColorScheme,
        cell_width: f32,
        cell_height: f32,
        x_offset: f32,
        y_offset: f32,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let term_buffer = terminal.buffer();
        let dims = terminal.dimensions();
        // Estimate ~half of cells have visible glyphs
        let est_glyphs = (dims.rows as usize * dims.cols as usize) / 2;
        let mut vertices = Vec::with_capacity(est_glyphs * 4);
        let mut indices = Vec::with_capacity(est_glyphs * 6);

        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(&mut self.font_system, cell_width * 4.0, cell_height * 2.0);
        let shaping = self.get_shaping();

        for row_idx in 0..dims.rows {
            let row = match term_buffer.visible_row(row_idx) {
                Some(r) => r,
                None => continue,
            };

            for col_idx in 0..dims.cols {
                let cell = match row.get(col_idx) {
                    Some(c) => c,
                    None => continue,
                };

                let ch = cell.char();

                if ch == ' ' || cell.has_flag(CellFlags::WIDE_CHAR_SPACER) {
                    continue;
                }

                if cell.has_flag(CellFlags::HIDDEN) {
                    continue;
                }

                let fg_color = if cell.has_flag(CellFlags::REVERSE) {
                    color_scheme.resolve(cell.bg(), false)
                } else {
                    color_scheme.resolve(cell.fg(), true)
                };

                let fg_color = if cell.has_flag(CellFlags::DIM) {
                    Rgba::new(
                        fg_color.r * 0.65,
                        fg_color.g * 0.65,
                        fg_color.b * 0.65,
                        fg_color.a,
                    )
                } else {
                    fg_color
                };

                // Skip cell-filling symbols rendered procedurally in the background pass.
                if is_procedural_cell_symbol(ch) {
                    continue;
                }

                let has_combining = cell.has_flag(CellFlags::HAS_COMBINING);
                let style_key: u8 = if self.ligatures { 1 } else { 0 };
                let cache_key_lookup = (ch, style_key);

                let shaped_glyph = if !has_combining {
                    if let Some(&cached) = self.shaping_cache.get(&cache_key_lookup) {
                        Some(cached)
                    } else {
                        let attrs = if self.font_family.is_empty() {
                            Attrs::new().family(Family::Monospace)
                        } else {
                            Attrs::new().family(Family::Name(&self.font_family))
                        };
                        let ch_str: String = ch.to_string();
                        buffer.set_text(&mut self.font_system, &ch_str, attrs, shaping);

                        if let Some(run) = buffer.layout_runs().next() {
                            if let Some(glyph) = run.glyphs.first() {
                                let physical = glyph.physical((0.0, 0.0), 1.0);
                                let shaped = ShapedGlyph {
                                    cache_key: physical.cache_key,
                                    x_offset: physical.x as f32,
                                    y_offset: physical.y as f32,
                                    line_y: run.line_y,
                                };
                                self.shaping_cache.insert(cache_key_lookup, shaped);
                                Some(shaped)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                } else {
                    let grapheme = terminal
                        .grid()
                        .grapheme(row_idx, col_idx)
                        .unwrap_or_else(|| ch.to_string());
                    let attrs = if self.font_family.is_empty() {
                        Attrs::new().family(Family::Monospace)
                    } else {
                        Attrs::new().family(Family::Name(&self.font_family))
                    };
                    buffer.set_text(&mut self.font_system, &grapheme, attrs, shaping);

                    buffer.layout_runs().next().and_then(|run| {
                        run.glyphs.first().map(|g| {
                            let physical = g.physical((0.0, 0.0), 1.0);
                            ShapedGlyph {
                                cache_key: physical.cache_key,
                                x_offset: physical.x as f32,
                                y_offset: physical.y as f32,
                                line_y: run.line_y,
                            }
                        })
                    })
                };

                if let Some(shaped) = shaped_glyph {
                    if let Some(glyph_info) = self
                        .atlas
                        .get_glyph(&mut self.font_system, shaped.cache_key)
                    {
                        if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
                            continue;
                        }

                        let (x, y) = cell_glyph_origin(
                            (col_idx, row_idx),
                            (cell_width, cell_height),
                            (x_offset, y_offset),
                            shaped,
                            &glyph_info,
                        );

                        Self::emit_glyph_quad(
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

        (vertices, indices)
    }

    /// Emit a quad for a glyph.
    fn emit_glyph_quad(
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        glyph: &super::atlas::GlyphInfo,
        color: Rgba,
    ) {
        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();
        let is_color = if glyph.is_color { 1u32 } else { 0u32 };

        let x0 = x;
        let y0 = y;
        let x1 = x + glyph.width;
        let y1 = y + glyph.height;

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [glyph.uv[0], glyph.uv[1]],
            color: color_arr,
            is_color,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [glyph.uv[2], glyph.uv[1]],
            color: color_arr,
            is_color,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [glyph.uv[2], glyph.uv[3]],
            color: color_arr,
            is_color,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [glyph.uv[0], glyph.uv[3]],
            color: color_arr,
            is_color,
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
        // Set buffer size to allow layout
        buffer.set_size(&mut self.font_system, max_width, self.cell_height * 2.0);
        let shaping = self.get_shaping();

        // Use configured font or system monospace
        let attrs = if self.font_family.is_empty() {
            Attrs::new().family(Family::Monospace)
        } else {
            Attrs::new().family(Family::Name(&self.font_family))
        };

        buffer.set_text(&mut self.font_system, text, attrs, shaping);

        let max_x = x + max_width;

        // Get glyphs from layout
        if let Some(run) = buffer.layout_runs().next() {
            for glyph in run.glyphs.iter() {
                let glyph_start = x + glyph.x;
                let glyph_end = glyph_start + glyph.w;

                // Check if we've exceeded max width
                if glyph_end > max_x {
                    // Add ellipsis if there's room
                    if max_width > self.cell_width {
                        let ellipsis_x = (max_x - self.cell_width).max(x);
                        self.render_char_at(&mut vertices, &mut indices, '…', ellipsis_x, y, color);
                    }
                    break;
                }

                let physical = glyph.physical((0.0, 0.0), 1.0);

                if let Some(glyph_info) = self
                    .atlas
                    .get_glyph(&mut self.font_system, physical.cache_key)
                {
                    if glyph_info.width > 0.0 && glyph_info.height > 0.0 {
                        let gx = (x + glyph_info.left + physical.x as f32).round();
                        let gy =
                            (y + (self.cell_height - glyph_info.top) + physical.y as f32).round();

                        Self::emit_glyph_quad(
                            &mut vertices,
                            &mut indices,
                            gx,
                            gy,
                            &glyph_info,
                            color,
                        );
                    }
                }
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
        // Set buffer size to allow layout
        buffer.set_size(
            &mut self.font_system,
            self.cell_width * 4.0,
            self.cell_height * 2.0,
        );
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

                if let Some(glyph_info) = self
                    .atlas
                    .get_glyph(&mut self.font_system, physical.cache_key)
                {
                    if glyph_info.width > 0.0 && glyph_info.height > 0.0 {
                        let gx = x + glyph_info.left + physical.x as f32;
                        let gy = y + (self.cell_height - glyph_info.top) + physical.y as f32;

                        Self::emit_glyph_quad(vertices, indices, gx, gy, &glyph_info, color);
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

fn is_block_char(ch: char) -> bool {
    matches!(ch, '\u{2580}'..='\u{259F}')
}

fn is_procedural_cell_symbol(ch: char) -> bool {
    is_block_char(ch) || is_powerline_triangle(ch)
}

fn fill_byte_to_char_offsets(text: &str, offsets: &mut Vec<usize>) {
    offsets.clear();
    offsets.extend(text.char_indices().map(|(byte_offset, _)| byte_offset));
}

fn byte_offset_to_char_index(offsets: &[usize], byte_offset: usize) -> usize {
    match offsets.binary_search(&byte_offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    }
}

fn cell_glyph_origin(
    cell: (u16, u16),
    cell_size: (f32, f32),
    render_offset: (f32, f32),
    shaped: ShapedGlyph,
    glyph_info: &super::atlas::GlyphInfo,
) -> (f32, f32) {
    let (col, row) = cell;
    let (cell_width, cell_height) = cell_size;
    let (x_offset, y_offset) = render_offset;

    (
        (col as f32 * cell_width + glyph_info.left + shaped.x_offset + x_offset).round(),
        (row as f32 * cell_height + shaped.line_y + shaped.y_offset - glyph_info.top + y_offset)
            .round(),
    )
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

    #[test]
    fn byte_offset_lookup_maps_utf8_offsets_to_char_indices() {
        let mut offsets = Vec::new();
        fill_byte_to_char_offsets("aé🙂z", &mut offsets);

        assert_eq!(offsets, vec![0, 1, 3, 7]);
        assert_eq!(byte_offset_to_char_index(&offsets, 0), 0);
        assert_eq!(byte_offset_to_char_index(&offsets, 1), 1);
        assert_eq!(byte_offset_to_char_index(&offsets, 3), 2);
        assert_eq!(byte_offset_to_char_index(&offsets, 7), 3);
    }

    #[test]
    fn byte_offset_lookup_uses_previous_char_for_inside_grapheme_offsets() {
        let mut offsets = Vec::new();
        fill_byte_to_char_offsets("aé🙂z", &mut offsets);

        assert_eq!(byte_offset_to_char_index(&offsets, 2), 1);
        assert_eq!(byte_offset_to_char_index(&offsets, 6), 2);
        assert_eq!(byte_offset_to_char_index(&offsets, 99), 3);
    }

    #[test]
    fn powerline_triangles_are_procedural_cell_symbols() {
        assert!(is_procedural_cell_symbol('\u{E0B0}'));
        assert!(is_procedural_cell_symbol('\u{E0B2}'));
        assert!(!is_procedural_cell_symbol('\u{E0B1}'));
        assert!(!is_procedural_cell_symbol('A'));
    }

    #[test]
    fn cell_glyph_origin_applies_shaped_offsets() {
        let (cache_key, _, _) = CacheKey::new(
            cosmic_text::fontdb::ID::dummy(),
            42,
            14.0,
            (0.0, 0.0),
            cosmic_text::CacheKeyFlags::empty(),
        );
        let shaped = ShapedGlyph {
            cache_key,
            x_offset: 3.0,
            y_offset: -2.0,
            line_y: 13.0,
        };
        let glyph = super::super::atlas::GlyphInfo {
            uv: [0.0, 0.0, 1.0, 1.0],
            width: 8.0,
            height: 12.0,
            left: 1.0,
            top: 11.0,
            is_color: false,
        };

        let (x, y) = cell_glyph_origin((2, 3), (10.0, 18.0), (5.0, 7.0), shaped, &glyph);

        assert_eq!(x, 29.0);
        assert_eq!(y, 61.0);
    }

    /// This test verifies that cosmic-text Buffer requires set_size before layout_runs
    /// will return any runs. This was the root cause of the "grey screen" bug where
    /// text wasn't rendering because layout_runs() returned an empty iterator.
    #[test]
    fn cosmic_text_buffer_requires_set_size_for_layout() {
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(14.0, 18.0);

        // Create buffer WITHOUT set_size - this was the bug
        let mut buffer_without_size = Buffer::new(&mut font_system, metrics);
        let attrs = Attrs::new().family(Family::Monospace);
        buffer_without_size.set_text(&mut font_system, "Hello", attrs, Shaping::Advanced);

        // Without set_size, layout_runs() returns empty
        let runs_without_size: Vec<_> = buffer_without_size.layout_runs().collect();
        assert!(
            runs_without_size.is_empty(),
            "Expected no layout runs without set_size (this proves the bug existed)"
        );

        // Create buffer WITH set_size - this is the fix
        let mut buffer_with_size = Buffer::new(&mut font_system, metrics);
        buffer_with_size.set_size(&mut font_system, 200.0, 50.0);
        buffer_with_size.set_text(&mut font_system, "Hello", attrs, Shaping::Advanced);

        // With set_size, layout_runs() returns runs
        let runs_with_size: Vec<_> = buffer_with_size.layout_runs().collect();
        assert!(
            !runs_with_size.is_empty(),
            "Expected layout runs with set_size (the fix works)"
        );

        // Verify we got glyphs
        let glyph_count: usize = runs_with_size.iter().map(|r| r.glyphs.len()).sum();
        assert!(
            glyph_count >= 5, // "Hello" should have at least 5 glyphs
            "Expected at least 5 glyphs for 'Hello', got {}",
            glyph_count
        );
    }
}
