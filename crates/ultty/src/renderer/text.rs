//! Text rendering with cosmic-text and glyph atlas.

use super::atlas::GlyphAtlas;
use super::pipeline::TextPipeline;
use super::powerline::is_powerline_triangle;
use super::{ColorScheme, Rgba, Vertex};
use crate::config::FontConfig;
use crate::grid::{Cell, CellFlags, CursorShape, Terminal};
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, Weight,
};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Character plus the shaping-relevant style bits (see [`shaping_cache_key`]).
type ShapingCacheKey = (char, u8);

#[derive(Clone, Copy)]
struct ShapedGlyph {
    cache_key: CacheKey,
    x_offset: f32,
    y_offset: f32,
    line_y: f32,
    /// Bold was requested but the matched face is not bold, so the glyph is
    /// drawn in a brighter color instead.
    synthetic_bold: bool,
}

/// Rendering decisions derived from a cell's attribute flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CellStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    double_underline: bool,
    strikethrough: bool,
}

impl CellStyle {
    fn from_flags(flags: CellFlags) -> Self {
        let double_underline = flags.contains(CellFlags::DOUBLE_UNDERLINE);
        Self {
            bold: flags.contains(CellFlags::BOLD),
            italic: flags.contains(CellFlags::ITALIC),
            underline: double_underline || flags.contains(CellFlags::UNDERLINE),
            double_underline,
            strikethrough: flags.contains(CellFlags::STRIKETHROUGH),
        }
    }

    fn has_decorations(self) -> bool {
        self.underline || self.strikethrough
    }
}

/// Key for the per-character shaping cache. Bold and italic select a
/// different face (or synthesized style), so they must not share entries.
fn shaping_cache_key(ch: char, ligatures: bool, style: CellStyle) -> ShapingCacheKey {
    let bits = (ligatures as u8) | (style.bold as u8) << 1 | (style.italic as u8) << 2;
    (ch, bits)
}

/// Decide which requested styles the matched face cannot provide.
///
/// Returns `(synthetic_bold, fake_italic)`. cosmic-text falls back to the
/// nearest weight/style when the family lacks a bold or italic face, so the
/// face it picked tells us whether to brighten the color (bold) or skew the
/// glyph via [`CacheKeyFlags::FAKE_ITALIC`] (italic).
fn synthesized_styles(style: CellStyle, face: Option<(Weight, Style)>) -> (bool, bool) {
    let has_bold_face = face.is_some_and(|(weight, _)| weight >= Weight::BOLD);
    let has_italic_face = face.is_some_and(|(_, face_style)| face_style != Style::Normal);
    (
        style.bold && !has_bold_face,
        style.italic && !has_italic_face,
    )
}

/// Brighten a color for cells that asked for bold when no bold face exists.
fn brighten(color: Rgba) -> Rgba {
    const LIFT: f32 = 0.3;
    Rgba::new(
        color.r + (1.0 - color.r) * LIFT,
        color.g + (1.0 - color.g) * LIFT,
        color.b + (1.0 - color.b) * LIFT,
        color.a,
    )
}

/// Vertical metrics, in pixels from the cell top, used to place decorations.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DecorationMetrics {
    baseline: f32,
    x_height: f32,
    thickness: f32,
}

/// `(x, y, width, height)` in pixels.
type Rect = (f32, f32, f32, f32);

/// Decoration quads for one cell: underline (single or double) at one pixel
/// below the baseline and strikethrough through the middle of the x-height.
/// Lines are kept inside the cell so they never bleed into the next row.
fn decoration_rects(
    style: CellStyle,
    cell: Rect,
    metrics: DecorationMetrics,
) -> ([Rect; 3], usize) {
    let (cell_x, cell_y, width, height) = cell;
    let t = metrics.thickness;
    let lowest = cell_y + height - t;
    let mut rects = [(0.0, 0.0, 0.0, 0.0); 3];
    let mut count = 0;

    if style.underline {
        let first = (cell_y + metrics.baseline + 1.0).round().min(lowest);
        if style.double_underline {
            let second = (first + 2.0 * t).min(lowest);
            let first = second - 2.0 * t;
            rects[count] = (cell_x, first, width, t);
            rects[count + 1] = (cell_x, second, width, t);
            count += 2;
        } else {
            rects[count] = (cell_x, first, width, t);
            count += 1;
        }
    }

    if style.strikethrough {
        let y = (cell_y + metrics.baseline - metrics.x_height * 0.5 - t * 0.5).round();
        rects[count] = (cell_x, y.clamp(cell_y, lowest), width, t);
        count += 1;
    }

    (rects, count)
}

/// Cell the block cursor is drawn over, if it is drawn this frame.
///
/// Mirrors the visibility rule of the background pass, which paints the
/// cursor block: visible, not in a hidden blink phase, and not scrolled back.
fn block_cursor_cell(terminal: &Terminal, blink_visible: bool) -> Option<(u16, u16)> {
    let cursor = terminal.buffer().grid().cursor();
    let shown = cursor.visible
        && (!cursor.blinking || blink_visible)
        && !terminal.buffer().is_scrolled()
        && cursor.shape == CursorShape::Block;
    shown.then_some((cursor.position.col, cursor.position.row))
}

/// Foreground color of a cell after REVERSE and DIM.
fn cell_foreground(cell: &Cell, color_scheme: &ColorScheme) -> Rgba {
    let fg = if cell.has_flag(CellFlags::REVERSE) {
        color_scheme.resolve(cell.bg(), false)
    } else {
        color_scheme.resolve(cell.fg(), true)
    };

    if cell.has_flag(CellFlags::DIM) {
        Rgba::new(fg.r * 0.65, fg.g * 0.65, fg.b * 0.65, fg.a)
    } else {
        fg
    }
}

/// Color for the glyph under a block cursor: the cell's background, so the
/// character stays legible on top of the cursor block.
fn cursor_glyph_color(cell: &Cell, color_scheme: &ColorScheme) -> Rgba {
    if cell.has_flag(CellFlags::REVERSE) {
        color_scheme.resolve(cell.fg(), true)
    } else {
        color_scheme.resolve(cell.bg(), false)
    }
}

/// Attributes for the configured font family (system monospace when unset).
fn font_attrs(font_family: &str) -> Attrs<'_> {
    if font_family.is_empty() {
        Attrs::new().family(Family::Monospace)
    } else {
        Attrs::new().family(Family::Name(font_family))
    }
}

pub struct TextRenderer {
    font_system: FontSystem,
    atlas: GlyphAtlas,
    pipeline: TextPipeline,
    cell_width: f32,
    cell_height: f32,
    metrics: Metrics,
    decoration: DecorationMetrics,
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

        // Calculate metrics (size clamped so cells can never be 0 px wide)
        let font_size = crate::config::clamp_font_size(font_config.size);
        let line_height = (font_size * font_config.line_height).ceil().max(1.0);
        let metrics = Metrics::new(font_size, line_height);

        // Measure cell dimensions using a sample character
        let font_family = font_config.family.clone();
        let (cell_width, cell_height, baseline) =
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

        let mut renderer = Self {
            font_system,
            atlas,
            pipeline,
            cell_width,
            cell_height,
            metrics,
            decoration: DecorationMetrics {
                baseline,
                x_height: 0.0,
                thickness: 1.0,
            },
            device,
            font_family,
            ligatures: font_config.ligatures,
            shaping_cache: HashMap::with_capacity(256),
            styled_text_buffer,
            byte_to_char_offsets: Vec::with_capacity(256),
        };
        renderer.decoration = renderer.measure_decoration_metrics(baseline);
        Ok(renderer)
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

    /// Measure cell dimensions and baseline (px from the cell top) for a
    /// monospace font.
    fn measure_cell_with_font(
        font_system: &mut FontSystem,
        metrics: Metrics,
        font_family: &str,
    ) -> (f32, f32, f32) {
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_text(font_system, "M", font_attrs(font_family), Shaping::Advanced);

        // Get width and baseline from layout
        let run = buffer.layout_runs().next();
        let width = run
            .as_ref()
            .and_then(|run| run.glyphs.first())
            .map(|g| g.w)
            .unwrap_or(metrics.font_size * 0.6);
        let baseline = run.map(|run| run.line_y).unwrap_or(metrics.font_size * 0.8);

        (
            width.ceil().max(1.0),
            metrics.line_height.ceil().max(1.0),
            baseline,
        )
    }

    /// Measure the x-height (from the rasterized `x` glyph) and line
    /// thickness used to place underline and strikethrough quads.
    fn measure_decoration_metrics(&mut self, baseline: f32) -> DecorationMetrics {
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(
            &mut self.font_system,
            self.cell_width * 4.0,
            self.cell_height * 2.0,
        );
        let x_height = self
            .shape_cell(&mut buffer, "x", CellStyle::default())
            .and_then(|shaped| {
                self.atlas
                    .get_glyph(&mut self.font_system, shaped.cache_key)
            })
            .map(|glyph| glyph.top)
            .filter(|top| *top > 0.0)
            .unwrap_or(self.metrics.font_size * 0.5);
        let thickness = (self.metrics.font_size / 14.0).round().clamp(1.0, 2.0);

        DecorationMetrics {
            baseline,
            x_height,
            thickness,
        }
    }

    /// Shape a single cell's text with the given style.
    ///
    /// Requests a bold/italic face when asked; when the family has none, the
    /// result is flagged for the synthetic fallbacks (brightened color for
    /// bold, skewed glyph for italic).
    fn shape_cell(
        &mut self,
        buffer: &mut Buffer,
        text: &str,
        style: CellStyle,
    ) -> Option<ShapedGlyph> {
        let shaping = self.get_shaping();
        self.shape_text(buffer, text, style, shaping)
            .into_iter()
            .next()
    }

    /// Shape `text` and return every glyph of its first layout run, each
    /// positioned relative to the start of the run.
    fn shape_text(
        &mut self,
        buffer: &mut Buffer,
        text: &str,
        style: CellStyle,
        shaping: Shaping,
    ) -> Vec<ShapedGlyph> {
        let mut attrs = font_attrs(&self.font_family);
        if style.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if style.italic {
            attrs = attrs.style(Style::Italic);
        }
        buffer.set_text(&mut self.font_system, text, attrs, shaping);

        let Some(run) = buffer.layout_runs().next() else {
            return Vec::new();
        };
        let line_y = run.line_y;
        let mut shaped = Vec::with_capacity(run.glyphs.len());
        for glyph in run.glyphs {
            let face = self
                .font_system
                .db()
                .face(glyph.font_id)
                .map(|face| (face.weight, face.style));
            let (synthetic_bold, fake_italic) = synthesized_styles(style, face);

            let mut physical = glyph.physical((0.0, 0.0), 1.0);
            if fake_italic {
                physical.cache_key.flags |= CacheKeyFlags::FAKE_ITALIC;
            }

            shaped.push(ShapedGlyph {
                cache_key: physical.cache_key,
                x_offset: physical.x as f32,
                y_offset: physical.y as f32,
                line_y,
                synthetic_bold,
            });
        }
        shaped
    }

    /// Start a frame. Must precede any quad that samples the glyph atlas,
    /// because this is the only point at which atlas slots are evicted.
    pub fn begin_frame(&mut self) {
        self.atlas.begin_frame();
    }

    /// Whether a glyph was skipped this frame because the atlas was full;
    /// the caller should schedule another frame so it gets drawn.
    pub fn frame_incomplete(&self) -> bool {
        self.atlas.frame_incomplete()
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
        let size = crate::config::clamp_font_size(size);

        // Calculate new metrics with same line height ratio
        let line_height_ratio = self.metrics.line_height / self.metrics.font_size;
        let new_line_height = (size * line_height_ratio).ceil();
        self.metrics = Metrics::new(size, new_line_height);

        // Recalculate cell dimensions
        let (width, height, baseline) =
            Self::measure_cell_with_font(&mut self.font_system, self.metrics, &self.font_family);
        self.cell_width = width;
        self.cell_height = height;

        // Clear caches since metrics changed
        self.atlas.clear();
        self.shaping_cache.clear();
        self.decoration = self.measure_decoration_metrics(baseline);
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
        buffer.set_text(
            &mut self.font_system,
            text,
            font_attrs(&self.font_family),
            shaping,
        );

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
        self.styled_text_buffer
            .set_size(&mut self.font_system, 10000.0, self.cell_height * 2.0);

        let shaping = self.get_shaping();
        let mut attrs = font_attrs(&self.font_family);

        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(Style::Italic);
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
    ///
    /// `cursor_blink_visible` must match what the background pass used to
    /// decide whether the block cursor was painted, so the glyph under it can
    /// be drawn in the inverted color.
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
        cursor_blink_visible: bool,
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
            cursor_blink_visible,
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
        cursor_blink_visible: bool,
        uniform_bind_group: &wgpu::BindGroup,
    ) -> Result<(), wgpu::SurfaceError> {
        let (vertices, indices) = self.build_text_vertices_with_offset(
            terminal,
            color_scheme,
            cell_width,
            cell_height,
            x_offset,
            y_offset,
            cursor_blink_visible,
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

    #[allow(clippy::too_many_arguments)]
    fn build_text_vertices_with_offset(
        &mut self,
        terminal: &Terminal,
        color_scheme: &ColorScheme,
        cell_width: f32,
        cell_height: f32,
        x_offset: f32,
        y_offset: f32,
        cursor_blink_visible: bool,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let term_buffer = terminal.buffer();
        let dims = terminal.dimensions();
        // Estimate ~half of cells have visible glyphs
        let est_glyphs = (dims.rows as usize * dims.cols as usize) / 2;
        let mut vertices = Vec::with_capacity(est_glyphs * 4);
        let mut indices = Vec::with_capacity(est_glyphs * 6);

        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(&mut self.font_system, cell_width * 4.0, cell_height * 2.0);
        let block_cursor = block_cursor_cell(terminal, cursor_blink_visible);
        let solid_uv = self.atlas.solid_uv();

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

                if cell.has_flag(CellFlags::HIDDEN) {
                    continue;
                }

                let style = CellStyle::from_flags(cell.flags());
                let under_block_cursor = block_cursor == Some((col_idx, row_idx));
                let base_color = if under_block_cursor {
                    cursor_glyph_color(cell, color_scheme)
                } else {
                    cell_foreground(cell, color_scheme)
                };

                // Decorations apply to blank cells and wide-char spacers too,
                // so an underlined run stays continuous.
                if style.has_decorations() {
                    let cell_rect = (
                        col_idx as f32 * cell_width + x_offset,
                        row_idx as f32 * cell_height + y_offset,
                        cell_width,
                        cell_height,
                    );
                    let (rects, count) = decoration_rects(style, cell_rect, self.decoration);
                    for rect in &rects[..count] {
                        Self::emit_solid_quad(
                            &mut vertices,
                            &mut indices,
                            *rect,
                            base_color,
                            solid_uv,
                        );
                    }
                }

                let ch = cell.char();

                if ch == ' ' || cell.has_flag(CellFlags::WIDE_CHAR_SPACER) {
                    continue;
                }

                // Skip cell-filling symbols rendered procedurally in the background pass.
                if is_procedural_cell_symbol(ch) {
                    continue;
                }

                // Multi-codepoint graphemes (combining marks, ZWJ emoji,
                // flags) live on the row itself, so history rows render them
                // too. They are always shaped with full shaping so the font's
                // composition rules apply, and every resulting glyph is drawn.
                let cluster_glyphs: Vec<ShapedGlyph>;
                let single_glyph: Option<ShapedGlyph>;
                let glyphs: &[ShapedGlyph] = if let Some(extra) = row.grapheme_extra(col_idx) {
                    let mut text = String::with_capacity(4 * (1 + extra.len()));
                    text.push(ch);
                    text.extend(extra);
                    cluster_glyphs = self.shape_text(&mut buffer, &text, style, Shaping::Advanced);
                    &cluster_glyphs
                } else {
                    let cache_key_lookup = shaping_cache_key(ch, self.ligatures, style);
                    single_glyph = match self.shaping_cache.get(&cache_key_lookup) {
                        Some(&cached) => Some(cached),
                        None => {
                            let shaped = self.shape_cell(&mut buffer, &ch.to_string(), style);
                            if let Some(shaped) = shaped {
                                self.shaping_cache.insert(cache_key_lookup, shaped);
                            }
                            shaped
                        }
                    };
                    single_glyph.as_slice()
                };

                for &shaped in glyphs {
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

                        // Under the cursor the color is already the inverted
                        // background; brightening it would reduce contrast.
                        let fg_color = if shaped.synthetic_bold && !under_block_cursor {
                            brighten(base_color)
                        } else {
                            base_color
                        };

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

    /// Emit a solid quad through the text pipeline by sampling the atlas's
    /// reserved white block.
    fn emit_solid_quad(
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        rect: Rect,
        color: Rgba,
        solid_uv: [f32; 2],
    ) {
        let (x0, y0, width, height) = rect;
        let x1 = x0 + width;
        let y1 = y0 + height;
        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        for position in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
            vertices.push(Vertex {
                position,
                tex_coord: solid_uv,
                color: color_arr,
                is_color: 0,
            });
        }

        indices.extend_from_slice(&[
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx,
            base_idx + 2,
            base_idx + 3,
        ]);
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
        buffer.set_text(
            &mut self.font_system,
            text,
            font_attrs(&self.font_family),
            shaping,
        );

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
        buffer.set_text(
            &mut self.font_system,
            &ch.to_string(),
            font_attrs(&self.font_family),
            shaping,
        );

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
    use crate::grid::Dimensions;

    const METRICS: DecorationMetrics = DecorationMetrics {
        baseline: 14.0,
        x_height: 7.0,
        thickness: 1.0,
    };

    #[test]
    fn cell_style_maps_attribute_flags() {
        assert_eq!(CellStyle::from_flags(CellFlags::NONE), CellStyle::default());

        let style = CellStyle::from_flags(
            CellFlags::BOLD | CellFlags::ITALIC | CellFlags::UNDERLINE | CellFlags::STRIKETHROUGH,
        );
        assert_eq!(
            style,
            CellStyle {
                bold: true,
                italic: true,
                underline: true,
                double_underline: false,
                strikethrough: true,
            }
        );

        let double = CellStyle::from_flags(CellFlags::DOUBLE_UNDERLINE);
        assert!(double.underline && double.double_underline);

        // Flags handled elsewhere (color, visibility) do not affect the style.
        assert_eq!(
            CellStyle::from_flags(CellFlags::REVERSE | CellFlags::DIM | CellFlags::BLINK),
            CellStyle::default()
        );
    }

    #[test]
    fn shaping_cache_key_separates_ligatures_bold_and_italic() {
        let plain = CellStyle::default();
        let bold = CellStyle {
            bold: true,
            ..plain
        };
        let italic = CellStyle {
            italic: true,
            ..plain
        };
        let bold_italic = CellStyle {
            bold: true,
            italic: true,
            ..plain
        };

        let keys = [
            shaping_cache_key('a', false, plain),
            shaping_cache_key('a', true, plain),
            shaping_cache_key('a', false, bold),
            shaping_cache_key('a', false, italic),
            shaping_cache_key('a', false, bold_italic),
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b);
            }
        }

        // Decorations do not change shaping, so they share the entry.
        let underlined = CellStyle {
            underline: true,
            strikethrough: true,
            ..plain
        };
        assert_eq!(
            shaping_cache_key('a', false, underlined),
            shaping_cache_key('a', false, plain)
        );
    }

    #[test]
    fn synthesized_styles_only_when_face_lacks_the_requested_style() {
        let bold_italic = CellStyle {
            bold: true,
            italic: true,
            ..CellStyle::default()
        };

        // Real bold italic face: nothing to synthesize.
        assert_eq!(
            synthesized_styles(bold_italic, Some((Weight::BOLD, Style::Italic))),
            (false, false)
        );
        // Regular face matched: brighten and skew.
        assert_eq!(
            synthesized_styles(bold_italic, Some((Weight::NORMAL, Style::Normal))),
            (true, true)
        );
        // Oblique counts as italic; semibold does not count as bold.
        assert_eq!(
            synthesized_styles(bold_italic, Some((Weight::SEMIBOLD, Style::Oblique))),
            (true, false)
        );
        // Unknown face: assume nothing is available.
        assert_eq!(synthesized_styles(bold_italic, None), (true, true));
        // Nothing requested: never synthesize, whatever the face.
        assert_eq!(
            synthesized_styles(CellStyle::default(), Some((Weight::NORMAL, Style::Normal))),
            (false, false)
        );
    }

    #[test]
    fn brighten_moves_towards_white_without_overflow() {
        let dim = Rgba::new(0.2, 0.4, 0.0, 0.5);
        let bright = brighten(dim);
        assert!(bright.r > dim.r && bright.g > dim.g && bright.b > dim.b);
        assert_eq!(bright.a, dim.a);

        let white = brighten(Rgba::rgb(1.0, 1.0, 1.0));
        assert_eq!(white, Rgba::rgb(1.0, 1.0, 1.0));
    }

    #[test]
    fn decoration_rects_underline_sits_one_pixel_below_baseline() {
        let style = CellStyle {
            underline: true,
            ..CellStyle::default()
        };
        let (rects, count) = decoration_rects(style, (30.0, 40.0, 10.0, 18.0), METRICS);

        assert_eq!(count, 1);
        assert_eq!(rects[0], (30.0, 55.0, 10.0, 1.0));
    }

    #[test]
    fn decoration_rects_strikethrough_crosses_mid_x_height() {
        let style = CellStyle {
            strikethrough: true,
            ..CellStyle::default()
        };
        let (rects, count) = decoration_rects(style, (0.0, 0.0, 10.0, 18.0), METRICS);

        assert_eq!(count, 1);
        // baseline 14 - x_height/2 (3.5) - thickness/2 (0.5) = 10
        assert_eq!(rects[0], (0.0, 10.0, 10.0, 1.0));
    }

    #[test]
    fn decoration_rects_double_underline_stays_inside_the_cell() {
        let style = CellStyle {
            underline: true,
            double_underline: true,
            ..CellStyle::default()
        };
        let (rects, count) = decoration_rects(style, (0.0, 0.0, 10.0, 16.0), METRICS);

        assert_eq!(count, 2);
        // The second line would land at y=17; it is pulled up to the last
        // row (15) and the first line keeps its 2px gap above it.
        assert_eq!(rects[0], (0.0, 13.0, 10.0, 1.0));
        assert_eq!(rects[1], (0.0, 15.0, 10.0, 1.0));
    }

    #[test]
    fn decoration_rects_emits_nothing_for_plain_or_bold_only_cells() {
        let bold = CellStyle {
            bold: true,
            italic: true,
            ..CellStyle::default()
        };
        assert_eq!(decoration_rects(bold, (0.0, 0.0, 10.0, 18.0), METRICS).1, 0);
        assert_eq!(
            decoration_rects(CellStyle::default(), (0.0, 0.0, 10.0, 18.0), METRICS).1,
            0
        );
    }

    #[test]
    fn block_cursor_cell_follows_background_pass_visibility_rules() {
        let mut terminal = Terminal::new(Dimensions::new(8, 3));
        {
            let cursor = terminal.grid_mut().cursor_mut();
            cursor.position.col = 3;
            cursor.position.row = 1;
            cursor.visible = true;
            cursor.blinking = false;
            cursor.shape = CursorShape::Block;
        }
        assert_eq!(block_cursor_cell(&terminal, true), Some((3, 1)));
        assert_eq!(block_cursor_cell(&terminal, false), Some((3, 1)));

        terminal.grid_mut().cursor_mut().blinking = true;
        assert_eq!(block_cursor_cell(&terminal, true), Some((3, 1)));
        assert_eq!(block_cursor_cell(&terminal, false), None);

        terminal.grid_mut().cursor_mut().blinking = false;
        terminal.grid_mut().cursor_mut().shape = CursorShape::Bar;
        assert_eq!(block_cursor_cell(&terminal, true), None);
        terminal.grid_mut().cursor_mut().shape = CursorShape::Underline;
        assert_eq!(block_cursor_cell(&terminal, true), None);

        terminal.grid_mut().cursor_mut().shape = CursorShape::Block;
        terminal.grid_mut().cursor_mut().visible = false;
        assert_eq!(block_cursor_cell(&terminal, true), None);
    }

    #[test]
    fn cursor_glyph_color_is_the_cell_background() {
        use crate::grid::Color;

        let scheme = ColorScheme::dark();
        let cell = Cell::with_attrs('a', Color::Named(1), Color::Named(4), CellFlags::NONE);
        assert_eq!(cursor_glyph_color(&cell, &scheme), scheme.named[4]);
        assert_eq!(cell_foreground(&cell, &scheme), scheme.named[1]);

        let reversed = Cell::with_attrs('a', Color::Named(1), Color::Named(4), CellFlags::REVERSE);
        assert_eq!(cursor_glyph_color(&reversed, &scheme), scheme.named[1]);
        assert_eq!(cell_foreground(&reversed, &scheme), scheme.named[4]);

        let default = Cell::new('a');
        assert_eq!(cursor_glyph_color(&default, &scheme), scheme.background);
    }

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
            synthetic_bold: false,
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
