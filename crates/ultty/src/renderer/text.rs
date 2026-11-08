//! Text rendering with cosmic-text and glyph atlas.

use super::atlas::GlyphAtlas;
use super::decoration::{strikethrough_quad, underline_quads, DecorationMetrics, Quad};
use super::pipeline::TextPipeline;
use super::powerline::is_powerline_triangle;
use super::text_cache::{GridTextCache, GridTextKey};
use super::{ColorScheme, Rgba, Vertex};
use crate::config::FontConfig;
use crate::grid::{Cell, CellFlags, Color, CursorShape, Row, Terminal, UnderlineStyle};
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, Weight,
};
// Looked up for every drawn cell; SipHash was a third of a scrolling frame.
use foldhash::{HashMap, HashMapExt};
use std::sync::Arc;

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
}

impl CellStyle {
    fn from_flags(flags: CellFlags) -> Self {
        Self {
            bold: flags.contains(CellFlags::BOLD),
            italic: flags.contains(CellFlags::ITALIC),
        }
    }

    /// Index of the face this style selects, in `0..4`.
    fn face(self) -> usize {
        usize::from(self.bold) | usize::from(self.italic) << 1
    }
}

/// Key for the per-character shaping cache. Bold and italic select a
/// different face (or synthesized style), so they must not share entries.
fn shaping_cache_key(ch: char, ligatures: bool, style: CellStyle) -> ShapingCacheKey {
    let bits = (ligatures as u8) | (style.bold as u8) << 1 | (style.italic as u8) << 2;
    (ch, bits)
}

/// Most distinct texts kept per face in a [`TextShapeCache`] before it is
/// cleared.
const MAX_TEXT_CACHE_ENTRIES: usize = 4096;

/// Shaped glyphs by text, one map per face (bold, italic). Keyed by
/// `String` so a lookup borrows the text instead of allocating.
///
/// Shaping a text costs tens of microseconds (font fallback, building a
/// shape plan), so text seen again, as in output scrolling past, must not
/// be shaped again.
struct TextShapeCache<G> {
    faces: [HashMap<String, Vec<G>>; 4],
}

impl<G> Default for TextShapeCache<G> {
    fn default() -> Self {
        Self {
            faces: Default::default(),
        }
    }
}

impl<G> TextShapeCache<G> {
    fn get(&self, style: CellStyle, text: &str) -> Option<&[G]> {
        self.faces[style.face()].get(text).map(Vec::as_slice)
    }

    fn insert(&mut self, style: CellStyle, text: String, glyphs: Vec<G>) {
        let cache = &mut self.faces[style.face()];
        if cache.len() >= MAX_TEXT_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(text, glyphs);
    }

    fn clear(&mut self) {
        self.faces.iter_mut().for_each(HashMap::clear);
    }
}

/// Shaped glyphs of a run, each with the index (from the run's first cell)
/// of the cell its cluster starts at.
type RunGlyphs = Vec<(u16, ShapedGlyph)>;

/// Whether `cell` can be shaped together with its neighbours to form
/// ligatures: a single printable ASCII character with nothing that needs the
/// per-cell path (wide characters, grapheme clusters, hidden text, symbols
/// drawn procedurally).
fn joins_ligature_run(cell: &crate::grid::Cell) -> bool {
    let ch = cell.char();
    ch.is_ascii_graphic()
        && !is_procedural_cell_symbol(ch)
        && !cell.has_flag(CellFlags::WIDE_CHAR)
        && !cell.has_flag(CellFlags::WIDE_CHAR_SPACER)
        && !cell.has_flag(CellFlags::HAS_COMBINING)
        && !cell.has_flag(CellFlags::HIDDEN)
}

/// What decides a glyph's face and drawn color: two cells may share a
/// ligature only if these match. A ligature is one glyph in one color, so
/// joining cells the program colored differently would recolor some.
fn ligature_key(cell: &crate::grid::Cell) -> (bool, bool, crate::grid::Color, bool, bool) {
    let style = CellStyle::from_flags(cell.flags());
    (
        style.bold,
        style.italic,
        cell.fg(),
        cell.has_flag(CellFlags::REVERSE),
        cell.has_flag(CellFlags::DIM),
    )
}

/// The end (exclusive) of the ligature run starting at `start`: consecutive
/// cells that join runs, with the same face and foreground.
fn ligature_run_end(row: &crate::grid::Row, start: u16, cols: u16) -> u16 {
    let Some(first) = row.get(start).filter(|c| joins_ligature_run(c)) else {
        return start;
    };
    let key = ligature_key(first);
    let mut end = start + 1;
    while end < cols {
        let joins = row
            .get(end)
            .is_some_and(|cell| joins_ligature_run(cell) && ligature_key(cell) == key);
        if !joins {
            break;
        }
        end += 1;
    }
    end
}

/// Programming-font ligatures join punctuation (`=>`, `!=`, `->`), so a run
/// of letters and digits gains nothing from run shaping.
fn could_ligate(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_punctuation())
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

/// Color of a cell's underline: its underline color (SGR 58) when set,
/// otherwise `text_color`, the color its glyph is drawn in.
fn underline_color(cell: &Cell, text_color: Rgba, color_scheme: &ColorScheme) -> Rgba {
    match cell.underline_color() {
        Color::Default => text_color,
        color => color_scheme.resolve(color, true),
    }
}

/// The quad index list for `quads` quads drawn from consecutive groups of
/// four vertices, extended in place (indices never change, only grow).
fn extend_quad_indices(indices: &mut Vec<u32>, quads: usize) {
    for quad in indices.len() / 6..quads {
        let base = (quad * 4) as u32;
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
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

/// Which real styled faces the configured family has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FaceStyles {
    bold: bool,
    italic: bool,
}

impl FaceStyles {
    /// Look the family up once. Asking cosmic-text for a style the family
    /// lacks makes it fall back to another family that has it, often a
    /// proportional one (bold DroidSansMono Nerd Font became Noto Sans
    /// Bold), which breaks the grid. A family that is not installed at all
    /// falls back regardless, so all styles stay requestable for it.
    fn of_family(db: &cosmic_text::fontdb::Database, family: &str) -> Self {
        if family.is_empty() {
            return Self {
                bold: true,
                italic: true,
            };
        }
        let mut found = false;
        let mut styles = Self {
            bold: false,
            italic: false,
        };
        for face in db.faces() {
            if face
                .families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
            {
                found = true;
                styles.bold |= face.weight >= Weight::BOLD;
                styles.italic |= face.style != Style::Normal;
            }
        }
        if found {
            styles
        } else {
            Self {
                bold: true,
                italic: true,
            }
        }
    }
}

/// Attributes for text in `family`, requesting bold and italic only when
/// the family has those faces; otherwise its regular face is used and the
/// style is synthesized (see `synthesized_styles`).
fn styled_attrs(family: &str, bold: bool, italic: bool, styles: FaceStyles) -> Attrs<'_> {
    let mut attrs = font_attrs(family);
    if bold && styles.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if italic && styles.italic {
        attrs = attrs.style(Style::Italic);
    }
    attrs
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
    /// Real bold and italic faces available in `font_family`.
    face_styles: FaceStyles,
    ligatures: bool,
    shaping_cache: HashMap<ShapingCacheKey, ShapedGlyph>,
    /// Shaped runs for ligatures; see [`ligature_run_end`].
    run_cache: TextShapeCache<(u16, ShapedGlyph)>,
    /// Reused per run: the run's text, and a copy of its glyphs to draw.
    run_text: String,
    run_glyphs: RunGlyphs,
    /// Shaped multi-codepoint graphemes (combining marks, ZWJ emoji, flags).
    cluster_cache: TextShapeCache<ShapedGlyph>,
    /// Reused per grapheme: its text.
    cluster_text: String,
    styled_text_buffer: Buffer,
    byte_to_char_offsets: Vec<usize>,
    queue: Arc<wgpu::Queue>,
    /// Vertex/index buffers for the grid draws of the current frame, reused
    /// across frames. Each draw in a frame gets its own slot: writes land at
    /// the next submit, so two draws sharing a buffer would both see the
    /// last write.
    grid_buffers: Vec<GridBuffers>,
    grid_buffers_used: usize,
    /// Cached row vertices per grid draw of a frame (one per pane), in draw
    /// order; see [`GridTextCache`].
    grid_caches: Vec<GridTextCache>,
    grid_draws: usize,
    /// This draw's vertices, gathered from the cached rows, and the quad
    /// index list shared by every draw (all grid geometry is quads).
    frame_vertices: Vec<Vertex>,
    quad_indices: Vec<u32>,
}

/// One reusable vertex + index buffer pair.
struct GridBuffers {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
}

/// Smallest buffer the pool allocates, so ordinary frames never regrow.
const MIN_GRID_BUFFER_BYTES: u64 = 64 * 1024;

fn grid_buffer(
    device: &wgpu::Device,
    label: &str,
    usage: wgpu::BufferUsages,
    len: u64,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: len.max(MIN_GRID_BUFFER_BYTES).next_power_of_two(),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Font size and line height for `font_config`. The size is clamped so
/// cells are never 0 px wide; the line height, which is the cell height, is
/// `size * line_height` rounded up to whole pixels.
fn font_metrics(font_config: &FontConfig) -> Metrics {
    let font_size = crate::config::clamp_font_size(font_config.size);
    let line_height = (font_size * font_config.line_height).ceil().max(1.0);
    Metrics::new(font_size, line_height)
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

        let metrics = font_metrics(font_config);

        // Measure cell dimensions using a sample character
        let font_family = font_config.family.clone();
        let (cell_width, cell_height, baseline) =
            Self::measure_cell_with_font(&mut font_system, metrics, &font_family);

        // Create glyph atlas
        let atlas = GlyphAtlas::new(device.clone(), Arc::clone(&queue));

        // Create text pipeline
        let pipeline = TextPipeline::new(
            &device,
            format,
            uniform_bind_group_layout,
            atlas.bind_group_layout(),
        )?;

        let styled_text_buffer = Buffer::new(&mut font_system, metrics);

        let face_styles = FaceStyles::of_family(font_system.db(), &font_family);
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
            face_styles,
            font_family,
            ligatures: font_config.ligatures,
            shaping_cache: HashMap::with_capacity(256),
            run_cache: Default::default(),
            run_text: String::new(),
            run_glyphs: Vec::new(),
            cluster_cache: TextShapeCache::default(),
            cluster_text: String::new(),
            styled_text_buffer,
            byte_to_char_offsets: Vec::with_capacity(256),
            queue,
            grid_buffers: Vec::new(),
            grid_buffers_used: 0,
            grid_caches: Vec::new(),
            grid_draws: 0,
            frame_vertices: Vec::new(),
            quad_indices: Vec::new(),
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
        buffer.set_text("M", &font_attrs(font_family), Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);

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
        buffer.set_size(Some(self.cell_width * 4.0), Some(self.cell_height * 2.0));
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
        // Always full shaping: a single character cannot form a ligature,
        // and basic shaping does no font fallback, so with ligatures off
        // CJK and emoji cells drew as missing-glyph boxes.
        self.shape_text(buffer, text, style, Shaping::Advanced)
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
        let attrs = styled_attrs(
            &self.font_family,
            style.bold,
            style.italic,
            self.face_styles,
        );
        buffer.set_text(text, &attrs, shaping, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

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

    /// Shape `text`, one ASCII character per cell, as a single run so the
    /// font can form ligatures across cells. Each glyph comes with the index
    /// of the cell its cluster starts at, and an x offset relative to that
    /// cell. `buffer` must not wrap.
    fn shape_run(&mut self, buffer: &mut Buffer, text: &str, style: CellStyle) -> RunGlyphs {
        shape_run_glyphs(
            &mut self.font_system,
            buffer,
            &self.font_family,
            self.face_styles,
            text,
            style,
        )
    }

    /// Put the glyphs of the run in `self.run_text` into `self.run_glyphs`,
    /// shaping it only if this face has not seen the text before.
    fn load_run_glyphs(&mut self, buffer: &mut Buffer, style: CellStyle) {
        self.run_glyphs.clear();
        if let Some(cached) = self.run_cache.get(style, &self.run_text) {
            self.run_glyphs.extend_from_slice(cached);
            return;
        }
        let text = std::mem::take(&mut self.run_text);
        let glyphs = self.shape_run(buffer, &text, style);
        self.run_glyphs.extend_from_slice(&glyphs);
        self.run_cache.insert(style, text.clone(), glyphs);
        self.run_text = text;
    }

    /// Put the glyphs of the grapheme in `self.cluster_text` into `out`,
    /// shaping it only if this face has not seen it before. Always full
    /// shaping, so the font's composition rules apply.
    fn load_cluster_glyphs(
        &mut self,
        buffer: &mut Buffer,
        style: CellStyle,
        out: &mut Vec<ShapedGlyph>,
    ) {
        out.clear();
        if let Some(cached) = self.cluster_cache.get(style, &self.cluster_text) {
            out.extend_from_slice(cached);
            return;
        }
        let text = std::mem::take(&mut self.cluster_text);
        let glyphs = self.shape_text(buffer, &text, style, Shaping::Advanced);
        out.extend_from_slice(&glyphs);
        self.cluster_cache.insert(style, text.clone(), glyphs);
        self.cluster_text = text;
    }

    /// Rasterize (or fetch) `shaped` and emit its quad for the cell at
    /// `cell`, in `color` (brightened for synthetic bold unless the cell is
    /// under the block cursor).
    #[allow(clippy::too_many_arguments)]
    fn push_glyph(
        &mut self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        shaped: ShapedGlyph,
        cell: (u16, u16),
        cell_size: (f32, f32),
        render_offset: (f32, f32),
        color: Rgba,
        under_block_cursor: bool,
    ) {
        let Some(glyph_info) = self
            .atlas
            .get_glyph(&mut self.font_system, shaped.cache_key)
        else {
            return;
        };
        if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
            return;
        }
        let (x, y) = cell_glyph_origin(cell, cell_size, render_offset, shaped, &glyph_info);
        // Under the cursor the color is already the inverted background;
        // brightening it would reduce contrast.
        let color = if shaped.synthetic_bold && !under_block_cursor {
            brighten(color)
        } else {
            color
        };
        Self::emit_glyph_quad(vertices, indices, x, y, &glyph_info, color);
    }

    /// Start a frame. Must precede any quad that samples the glyph atlas,
    /// because this is the only point at which atlas slots are evicted.
    pub fn begin_frame(&mut self) {
        self.atlas.begin_frame();
        self.grid_buffers_used = 0;
        // Drop the caches of panes that are gone.
        self.grid_caches.truncate(self.grid_draws.max(1));
        self.grid_draws = 0;
    }

    /// Upload this draw's vertices and indices into the next pooled slot,
    /// growing it if needed, and return the slot.
    fn upload_grid(&mut self, vertices: &[Vertex], indices: &[u32]) -> usize {
        let vertex_bytes: &[u8] = bytemuck::cast_slice(vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(indices);
        let slot = self.grid_buffers_used;
        self.grid_buffers_used += 1;
        if slot == self.grid_buffers.len() {
            self.grid_buffers.push(GridBuffers {
                vertex: grid_buffer(
                    &self.device,
                    "text_vertex_buffer",
                    wgpu::BufferUsages::VERTEX,
                    vertex_bytes.len() as u64,
                ),
                index: grid_buffer(
                    &self.device,
                    "text_index_buffer",
                    wgpu::BufferUsages::INDEX,
                    index_bytes.len() as u64,
                ),
            });
        }
        let buffers = &mut self.grid_buffers[slot];
        if buffers.vertex.size() < vertex_bytes.len() as u64 {
            buffers.vertex = grid_buffer(
                &self.device,
                "text_vertex_buffer",
                wgpu::BufferUsages::VERTEX,
                vertex_bytes.len() as u64,
            );
        }
        if buffers.index.size() < index_bytes.len() as u64 {
            buffers.index = grid_buffer(
                &self.device,
                "text_index_buffer",
                wgpu::BufferUsages::INDEX,
                index_bytes.len() as u64,
            );
        }
        self.queue.write_buffer(&buffers.vertex, 0, vertex_bytes);
        self.queue.write_buffer(&buffers.index, 0, index_bytes);
        slot
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
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
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
        self.run_cache.clear();
        self.cluster_cache.clear();
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
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
        buffer.set_size(Some(10000.0), Some(self.cell_height * 2.0));

        let shaping = self.get_shaping();
        buffer.set_text(text, &font_attrs(&self.font_family), shaping, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

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
            .set_size(Some(10000.0), Some(self.cell_height * 2.0));

        let shaping = self.get_shaping();
        let attrs = styled_attrs(&self.font_family, bold, italic, self.face_styles);

        self.styled_text_buffer
            .set_text(text, &attrs, shaping, None);
        self.styled_text_buffer
            .shape_until_scroll(&mut self.font_system, false);

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
    ) {
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
    ) {
        self.build_text_vertices_with_offset(
            terminal,
            color_scheme,
            cell_width,
            cell_height,
            x_offset,
            y_offset,
            cursor_blink_visible,
        );

        if self.frame_vertices.is_empty() {
            return;
        }

        let vertices = std::mem::take(&mut self.frame_vertices);
        let indices = std::mem::take(&mut self.quad_indices);
        let index_count = vertices.len() / 4 * 6;
        let slot = self.upload_grid(&vertices, &indices[..index_count]);
        let vertex_bytes = std::mem::size_of_val(vertices.as_slice()) as u64;
        let index_bytes = (index_count * std::mem::size_of::<u32>()) as u64;
        self.frame_vertices = vertices;
        self.quad_indices = indices;
        let buffers = &self.grid_buffers[slot];

        // Render pass for text
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // Don't clear - draw over backgrounds
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            render_pass.set_pipeline(self.pipeline.pipeline());
            render_pass.set_bind_group(0, uniform_bind_group, &[]);
            render_pass.set_bind_group(1, self.atlas.bind_group(), &[]);
            render_pass.set_vertex_buffer(0, buffers.vertex.slice(..vertex_bytes));
            render_pass.set_index_buffer(
                buffers.index.slice(..index_bytes),
                wgpu::IndexFormat::Uint32,
            );
            render_pass.draw_indexed(0..index_count as u32, 0, 0..1);
        }
    }

    /// Gather this draw's grid vertices into `frame_vertices` (and make
    /// `quad_indices` long enough for them), rebuilding only the rows whose
    /// content, cursor, or drawing parameters changed since the same draw
    /// in the previous frame.
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
    ) {
        let draw = self.grid_draws;
        self.grid_draws += 1;
        if self.grid_caches.len() <= draw {
            self.grid_caches
                .resize_with(draw + 1, GridTextCache::default);
        }
        let mut cache = std::mem::take(&mut self.grid_caches[draw]);

        let term_buffer = terminal.buffer();
        let dims = terminal.dimensions();
        cache.begin(
            GridTextKey {
                scheme: color_scheme.clone(),
                cell_size: (cell_width, cell_height),
                origin: (x_offset, y_offset),
                atlas_epoch: self.atlas.epoch(),
                ligatures: self.ligatures,
            },
            dims.rows,
        );
        let block_cursor = block_cursor_cell(terminal, cursor_blink_visible);
        let mut buffers: Option<(Buffer, Buffer)> = None;

        for row_idx in 0..dims.rows {
            let Some(row) = term_buffer.visible_row(row_idx) else {
                cache.store_empty(row_idx);
                continue;
            };
            let cursor_col = block_cursor
                .filter(|&(_, cursor_row)| cursor_row == row_idx)
                .map(|(col, _)| col);
            if cache.is_current(row_idx, &row, cursor_col) {
                continue;
            }
            let (buffer, run_buffer) = buffers
                .get_or_insert_with(|| self.shaping_buffers(cell_width, cell_height, dims.cols));
            let mut vertices = cache.take_vertices(row_idx);
            self.build_row_vertices(
                &mut vertices,
                buffer,
                run_buffer,
                &row,
                row_idx,
                cursor_col,
                color_scheme,
                (cell_width, cell_height),
                (x_offset, y_offset),
                dims.cols,
            );
            debug_assert_eq!(vertices.len() % 4, 0, "grid geometry is quads");
            cache.store(row_idx, &row, cursor_col, vertices);
        }

        self.frame_vertices.clear();
        for row in cache.vertices() {
            self.frame_vertices.extend_from_slice(row);
        }
        extend_quad_indices(&mut self.quad_indices, self.frame_vertices.len() / 4);
        self.grid_caches[draw] = cache;
    }

    /// Scratch buffers for shaping single cells and ligature runs (which
    /// span up to a whole row, so that buffer must not wrap).
    fn shaping_buffers(
        &mut self,
        cell_width: f32,
        cell_height: f32,
        cols: u16,
    ) -> (Buffer, Buffer) {
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(Some(cell_width * 4.0), Some(cell_height * 2.0));
        let mut run_buffer = Buffer::new(&mut self.font_system, self.metrics);
        run_buffer.set_wrap(cosmic_text::Wrap::None);
        run_buffer.set_size(
            Some(cell_width * (f32::from(cols) + 1.0)),
            Some(cell_height * 2.0),
        );
        (buffer, run_buffer)
    }

    /// Append the glyph and decoration quads of screen row `row_idx`.
    #[allow(clippy::too_many_arguments)]
    fn build_row_vertices(
        &mut self,
        vertices: &mut Vec<Vertex>,
        buffer: &mut Buffer,
        run_buffer: &mut Buffer,
        row: &Row,
        row_idx: u16,
        cursor_col: Option<u16>,
        color_scheme: &ColorScheme,
        (cell_width, cell_height): (f32, f32),
        (x_offset, y_offset): (f32, f32),
        cols: u16,
    ) {
        // Glyph quads index themselves; the draw uses the shared quad
        // index list instead.
        let mut indices = Vec::new();
        let block_cursor = cursor_col.map(|col| (col, row_idx));
        let solid_uv = self.atlas.solid_uv();
        let mut quads: Vec<Quad> = Vec::new();
        let mut cluster_glyphs: Vec<ShapedGlyph> = Vec::new();
        // Cells before this column had their glyphs drawn by a run.
        let mut run_drawn_until = 0u16;

        for col_idx in 0..cols {
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
            let underline = cell.underline_style();
            let strikethrough = cell.has_flag(CellFlags::STRIKETHROUGH);
            if underline != UnderlineStyle::None || strikethrough {
                let cell_rect = (
                    col_idx as f32 * cell_width + x_offset,
                    row_idx as f32 * cell_height + y_offset,
                    cell_width,
                    cell_height,
                );
                quads.clear();
                underline_quads(underline, cell_rect, self.decoration, &mut quads);
                let line_color = underline_color(cell, base_color, color_scheme);
                for quad in &quads {
                    Self::emit_solid_quad(vertices, &mut indices, quad, line_color, solid_uv);
                }
                if strikethrough {
                    let quad = strikethrough_quad(cell_rect, self.decoration);
                    Self::emit_solid_quad(vertices, &mut indices, &quad, base_color, solid_uv);
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

            if col_idx < run_drawn_until {
                continue;
            }

            // With ligatures on, shape a run of ASCII cells together so
            // the font can join them (`=>`, `!=`). Each glyph is drawn
            // at, and in the color of, the cell its cluster starts at.
            if self.ligatures {
                let mut end = ligature_run_end(row, col_idx, cols);
                // The block cursor's cell is drawn on its own, in the
                // inverted color, so a ligature never straddles it.
                if let Some(cursor_col) = cursor_col {
                    // A run before the cursor stops at it; a cursor at the
                    // run's start leaves it empty, so that cell takes the
                    // per-cell path and the next run starts after it.
                    if (col_idx..end).contains(&cursor_col) {
                        end = cursor_col;
                    }
                }
                if end - col_idx >= 2 {
                    self.run_text.clear();
                    self.run_text.extend(
                        (col_idx..end).filter_map(|col| row.get(col).map(|cell| cell.char())),
                    );
                    if could_ligate(&self.run_text) {
                        self.load_run_glyphs(run_buffer, style);
                        let glyphs = std::mem::take(&mut self.run_glyphs);
                        for &(offset, shaped) in &glyphs {
                            let col = col_idx + offset;
                            let Some(glyph_cell) = row.get(col) else {
                                continue;
                            };
                            let under_cursor = block_cursor == Some((col, row_idx));
                            let color = if under_cursor {
                                cursor_glyph_color(glyph_cell, color_scheme)
                            } else {
                                cell_foreground(glyph_cell, color_scheme)
                            };
                            self.push_glyph(
                                vertices,
                                &mut indices,
                                shaped,
                                (col, row_idx),
                                (cell_width, cell_height),
                                (x_offset, y_offset),
                                color,
                                under_cursor,
                            );
                        }
                        self.run_glyphs = glyphs;
                        run_drawn_until = end;
                        continue;
                    }
                }
            }

            // Multi-codepoint graphemes (combining marks, ZWJ emoji,
            // flags) live on the row itself, so history rows render them
            // too. Every glyph of the grapheme is drawn.
            let single_glyph: Option<ShapedGlyph>;
            let glyphs: &[ShapedGlyph] = if let Some(extra) = row.grapheme_extra(col_idx) {
                self.cluster_text.clear();
                self.cluster_text.push(ch);
                self.cluster_text.extend(extra);
                self.load_cluster_glyphs(buffer, style, &mut cluster_glyphs);
                &cluster_glyphs
            } else {
                let cache_key_lookup = shaping_cache_key(ch, self.ligatures, style);
                single_glyph = match self.shaping_cache.get(&cache_key_lookup) {
                    Some(&cached) => Some(cached),
                    None => {
                        let shaped = self.shape_cell(buffer, &ch.to_string(), style);
                        if let Some(shaped) = shaped {
                            self.shaping_cache.insert(cache_key_lookup, shaped);
                        }
                        shaped
                    }
                };
                single_glyph.as_slice()
            };

            for &shaped in glyphs {
                self.push_glyph(
                    vertices,
                    &mut indices,
                    shaped,
                    (col_idx, row_idx),
                    (cell_width, cell_height),
                    (x_offset, y_offset),
                    base_color,
                    under_block_cursor,
                );
            }
        }
    }

    /// Emit a solid quad through the text pipeline by sampling the atlas's
    /// reserved white block.
    fn emit_solid_quad(
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        quad: &Quad,
        color: Rgba,
        solid_uv: [f32; 2],
    ) {
        let base_idx = vertices.len() as u32;

        for (position, coverage) in quad.corners.into_iter().zip(quad.coverage) {
            vertices.push(Vertex {
                position,
                tex_coord: solid_uv,
                color: [color.r, color.g, color.b, color.a * coverage],
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
        buffer.set_size(Some(max_width), Some(self.cell_height * 2.0));
        let shaping = self.get_shaping();
        buffer.set_text(text, &font_attrs(&self.font_family), shaping, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

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
        buffer.set_size(Some(self.cell_width * 4.0), Some(self.cell_height * 2.0));
        let shaping = self.get_shaping();
        buffer.set_text(
            &ch.to_string(),
            &font_attrs(&self.font_family),
            shaping,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

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
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
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

/// Shape `text`, one ASCII character per cell, as a single run so the font
/// can form ligatures across cells. Each glyph comes with the index of the
/// cell its cluster starts at, and an x offset relative to that cell.
/// `buffer` must not wrap.
fn shape_run_glyphs(
    font_system: &mut FontSystem,
    buffer: &mut Buffer,
    font_family: &str,
    face_styles: FaceStyles,
    text: &str,
    style: CellStyle,
) -> RunGlyphs {
    let attrs = styled_attrs(font_family, style.bold, style.italic, face_styles);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let Some(run) = buffer.layout_runs().next() else {
        return Vec::new();
    };
    let line_y = run.line_y;
    let mut shaped = Vec::with_capacity(run.glyphs.len());
    // Each glyph is placed relative to the first glyph of its cluster, and
    // the cluster at its own cell. Using the run's advances instead drifts
    // whenever the face is narrower or wider than a cell (a fallback face,
    // such as a proportional bold for a font with no bold), opening gaps
    // before the next run and the cursor.
    let mut cluster: Option<(usize, i32)> = None;
    for glyph in run.glyphs {
        let face = font_system
            .db()
            .face(glyph.font_id)
            .map(|face| (face.weight, face.style));
        let (synthetic_bold, fake_italic) = synthesized_styles(style, face);
        let mut physical = glyph.physical((0.0, 0.0), 1.0);
        if fake_italic {
            physical.cache_key.flags |= CacheKeyFlags::FAKE_ITALIC;
        }
        // ASCII only, so byte offsets are cell offsets.
        let cell = glyph.start.min(text.len().saturating_sub(1)) as u16;
        let cluster_x = match cluster {
            Some((start, x)) if start == glyph.start => x,
            _ => {
                cluster = Some((glyph.start, physical.x));
                physical.x
            }
        };
        shaped.push((
            cell,
            ShapedGlyph {
                cache_key: physical.cache_key,
                x_offset: (physical.x - cluster_x) as f32,
                y_offset: physical.y as f32,
                line_y,
                synthetic_bold,
            },
        ));
    }
    shaped
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
            }
        );

        // Flags handled elsewhere (color, visibility, decorations) do not
        // affect the face.
        assert_eq!(
            CellStyle::from_flags(
                CellFlags::REVERSE
                    | CellFlags::DIM
                    | CellFlags::BLINK
                    | CellFlags::DOUBLE_UNDERLINE
            ),
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
    }

    #[test]
    fn text_shape_cache_keeps_each_face_apart() {
        let plain = CellStyle::default();
        let styles = [
            plain,
            CellStyle {
                bold: true,
                ..plain
            },
            CellStyle {
                italic: true,
                ..plain
            },
            CellStyle {
                bold: true,
                italic: true,
            },
        ];
        let mut cache = TextShapeCache::default();
        for (glyph, style) in styles.into_iter().enumerate() {
            cache.insert(style, "é̃".to_string(), vec![glyph]);
        }
        for (glyph, style) in styles.into_iter().enumerate() {
            assert_eq!(cache.get(style, "é̃"), Some(&[glyph][..]));
        }
        assert_eq!(cache.get(plain, "x̧̌"), None);

        cache.clear();
        assert_eq!(cache.get(plain, "é̃"), None);
    }

    #[test]
    fn text_shape_cache_starts_over_when_a_face_fills_up() {
        let plain = CellStyle::default();
        let bold = CellStyle {
            bold: true,
            ..plain
        };
        let mut cache = TextShapeCache::default();
        cache.insert(bold, "kept".to_string(), vec![0u8]);
        for i in 0..MAX_TEXT_CACHE_ENTRIES {
            cache.insert(plain, i.to_string(), vec![0u8]);
        }
        assert_eq!(cache.get(plain, "0"), Some(&[0u8][..]));

        cache.insert(plain, "next".to_string(), vec![1]);
        assert_eq!(cache.get(plain, "0"), None, "the full face starts over");
        assert_eq!(cache.get(plain, "next"), Some(&[1u8][..]));
        assert_eq!(
            cache.get(bold, "kept"),
            Some(&[0u8][..]),
            "other faces stay"
        );
    }

    #[test]
    fn synthesized_styles_only_when_face_lacks_the_requested_style() {
        let bold_italic = CellStyle {
            bold: true,
            italic: true,
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
    fn underline_color_is_the_sgr_58_color_or_the_text_color() {
        let scheme = ColorScheme::dark();
        let text = Rgba::rgb(0.5, 0.5, 0.5);
        let mut cell = Cell::default();
        assert_eq!(underline_color(&cell, text, &scheme), text);

        cell.set_underline_color(Color::Rgb(255, 0, 0));
        assert_eq!(
            underline_color(&cell, text, &scheme),
            scheme.resolve(Color::Rgb(255, 0, 0), true)
        );
    }

    #[test]
    fn quad_indices_grow_in_whole_quads_without_changing() {
        let mut indices = Vec::new();
        extend_quad_indices(&mut indices, 1);
        assert_eq!(indices, [0, 1, 2, 0, 2, 3]);
        extend_quad_indices(&mut indices, 2);
        assert_eq!(indices, [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        extend_quad_indices(&mut indices, 1);
        assert_eq!(indices.len(), 12, "never shrinks");
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
    fn line_height_multiplies_the_cell_height() {
        let font = |line_height| FontConfig {
            size: 14.0,
            line_height,
            ..FontConfig::default()
        };
        assert_eq!(font_metrics(&font(1.0)).line_height, 14.0);
        assert_eq!(font_metrics(&font(1.5)).line_height, 21.0);
        // Rounded up to whole pixels.
        assert_eq!(font_metrics(&font(1.2)).line_height, 17.0);
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
            cosmic_text::fontdb::Weight::NORMAL,
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
    fn row_of(text: &str) -> crate::grid::Row {
        let mut row = crate::grid::Row::new(text.chars().count() as u16 + 4);
        for (col, ch) in text.chars().enumerate() {
            row[col as u16] = crate::grid::Cell::new(ch);
        }
        row
    }

    #[test]
    fn ligature_runs_stop_at_spaces_wide_chars_and_face_changes() {
        let row = row_of("a=>b c");
        assert_eq!(
            ligature_run_end(&row, 0, row.width()),
            4,
            "stops at the space"
        );
        assert_eq!(
            ligature_run_end(&row, 4, row.width()),
            4,
            "a space starts no run"
        );

        let mut row = row_of("x!=y");
        row[2].set_flag(CellFlags::BOLD);
        assert_eq!(
            ligature_run_end(&row, 0, row.width()),
            2,
            "bold changes the face"
        );

        let mut row = row_of("->中");
        row[2].set_flag(CellFlags::WIDE_CHAR);
        assert_eq!(
            ligature_run_end(&row, 0, row.width()),
            2,
            "non-ASCII ends the run"
        );

        // A foreground change ends a run, so a ligature never recolors
        // cells the program colored differently. Background does not.
        let mut row = row_of("==>");
        row[1].set_fg(crate::grid::Color::Named(2));
        assert_eq!(ligature_run_end(&row, 0, row.width()), 1);
        let mut row = row_of("==>");
        row[1].set_bg(crate::grid::Color::Named(2));
        assert_eq!(ligature_run_end(&row, 0, row.width()), 3);
    }

    #[test]
    fn only_runs_with_punctuation_are_shaped_together() {
        assert!(could_ligate("a=>b"));
        assert!(could_ligate("!="));
        assert!(!could_ligate("hello123"));
    }

    const ALL_STYLES: FaceStyles = FaceStyles {
        bold: true,
        italic: true,
    };

    #[test]
    fn styles_the_family_lacks_are_not_requested() {
        let regular_only = FaceStyles {
            bold: false,
            italic: false,
        };
        let attrs = styled_attrs("Some Mono", true, true, regular_only);
        assert_eq!(attrs.weight, Weight::NORMAL);
        assert_eq!(attrs.style, Style::Normal);
        let attrs = styled_attrs("Some Mono", true, true, ALL_STYLES);
        assert_eq!(attrs.weight, Weight::BOLD);
        assert_eq!(attrs.style, Style::Italic);
    }

    #[test]
    fn a_missing_family_keeps_every_style_requestable() {
        let db = cosmic_text::fontdb::Database::new();
        assert_eq!(FaceStyles::of_family(&db, "No Such Family"), ALL_STYLES);
        assert_eq!(FaceStyles::of_family(&db, ""), ALL_STYLES);
    }

    #[test]
    fn a_regular_only_family_is_detected() {
        // DroidSansMono Nerd Font ships a Regular face only; it is the
        // default family, but CI machines may not have it installed.
        let font_system = FontSystem::new();
        let family = "DroidSansMono Nerd Font";
        let installed = font_system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == family));
        if !installed {
            eprintln!("{family} not installed; skipped");
            return;
        }
        let styles = FaceStyles::of_family(font_system.db(), family);
        assert!(!styles.bold, "{family} has no bold face");
    }

    #[test]
    fn run_glyphs_stay_in_their_cells_when_the_font_is_narrower() {
        // A bold prompt in a font with no bold face falls back to another
        // family, often proportional, whose glyphs are narrower than the
        // cell. Each glyph must still start at its own cell; placing them by
        // the font's advances drifted every later glyph left, leaving gaps
        // before the next styled run and the cursor.
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(14.0, 18.0);
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(1000.0), Some(40.0));
        let bold = CellStyle {
            bold: true,
            ..CellStyle::default()
        };
        let glyphs = shape_run_glyphs(
            &mut font_system,
            &mut buffer,
            "DejaVu Sans",
            ALL_STYLES,
            "git:(main)",
            bold,
        );
        assert!(!glyphs.is_empty());
        for (cell, glyph) in &glyphs {
            assert!(
                glyph.x_offset.abs() < 2.0,
                "cell {cell} glyph is {} px off its cell",
                glyph.x_offset
            );
        }
    }

    #[test]
    fn run_glyphs_map_back_to_their_cells() {
        // With a font that forms no ligatures, every cell still gets its
        // own glyph, at its own cell, with a small offset.
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(14.0, 18.0);
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(1000.0), Some(40.0));
        let text = "a=>b!=c";
        let glyphs = shape_run_glyphs(
            &mut font_system,
            &mut buffer,
            "DejaVu Sans Mono",
            ALL_STYLES,
            text,
            CellStyle::default(),
        );
        let cells: Vec<u16> = glyphs.iter().map(|(cell, _)| *cell).collect();
        assert_eq!(cells, (0..text.len() as u16).collect::<Vec<_>>());

        // Offsets only line up with cells in a monospaced face. A machine
        // without the named font (CI runners) falls back to whatever it
        // has, possibly proportional, and then only the mapping is checked.
        let monospaced = glyphs.iter().all(|(_, glyph)| {
            font_system
                .db()
                .face(glyph.cache_key.font_id)
                .is_some_and(|face| face.monospaced)
        });
        if !monospaced {
            eprintln!("no monospaced face matched; checked the cell mapping only");
            return;
        }
        for (cell, glyph) in &glyphs {
            assert!(
                glyph.x_offset.abs() < 1.0,
                "cell {cell} glyph is {} px off its cell",
                glyph.x_offset
            );
        }
    }

    /// Run with a ligature font installed to see what run shaping does:
    /// `cargo test -p ultty --lib probe_ligature_font -- --ignored --nocapture`
    #[test]
    #[ignore = "depends on an installed ligature font"]
    fn probe_ligature_font() {
        let family = std::env::var("ULTTY_LIGATURE_FONT").unwrap_or_else(|_| "Miracode".into());
        let mut font_system = FontSystem::new();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(1000.0), Some(40.0));
        for text in ["=>", "!=", "->", "===", "<=>"] {
            let run: Vec<_> = shape_run_glyphs(
                &mut font_system,
                &mut buffer,
                &family,
                ALL_STYLES,
                text,
                CellStyle::default(),
            )
            .into_iter()
            .map(|(cell, g)| (cell, g.cache_key.glyph_id))
            .collect();
            let alone: Vec<_> = text
                .chars()
                .map(|ch| {
                    shape_run_glyphs(
                        &mut font_system,
                        &mut buffer,
                        &family,
                        ALL_STYLES,
                        &ch.to_string(),
                        CellStyle::default(),
                    )[0]
                    .1
                    .cache_key
                    .glyph_id
                })
                .collect();
            eprintln!("{family} {text:>4}: run {run:?} alone {alone:?}");
        }
    }

    #[test]
    fn sized_cosmic_text_buffer_lays_out_glyphs() {
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(14.0, 18.0);
        let attrs = Attrs::new().family(Family::Monospace);

        let mut buffer_with_size = Buffer::new(&mut font_system, metrics);
        buffer_with_size.set_size(Some(200.0), Some(50.0));
        buffer_with_size.set_text("Hello", &attrs, Shaping::Advanced, None);
        buffer_with_size.shape_until_scroll(&mut font_system, false);

        let runs_with_size: Vec<_> = buffer_with_size.layout_runs().collect();
        assert!(
            !runs_with_size.is_empty(),
            "Expected layout runs from a sized buffer"
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
