//! Text rendering with cosmic-text and glyph atlas.

use super::atlas::{GlyphAtlas, GlyphInfo};
use super::decoration::{strikethrough_quad, underline_quads, DecorationMetrics, Quad};
use super::emoji::{emoji_presentation, pick_emoji_family};
use super::pipeline::TextPipeline;
use super::powerline::is_powerline_triangle;
use super::text_cache::{GridTextCache, GridTextKey};
use super::{ColorScheme, Rgba, Vertex};
use crate::config::FontConfig;
use crate::grid::{Cell, CellFlags, Color, CursorShape, RowView, Terminal, UnderlineStyle};
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, Feature, FeatureTag, FontFeatures, FontSystem,
    Metrics, Shaping, Style, Weight,
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
    /// A zero-width mark the font did not position: it sits at its base's
    /// origin, at the height it was drawn for, so it may collide with the
    /// base (see [`stack_unpositioned_marks`]).
    unpositioned_mark: bool,
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
fn ligature_run_end(row: &RowView<'_>, start: u16, cols: u16) -> u16 {
    let Some(first) = row.cell(start).filter(joins_ligature_run) else {
        return start;
    };
    let key = ligature_key(&first);
    let mut end = start + 1;
    while end < cols {
        let joins = row
            .cell(end)
            .is_some_and(|cell| joins_ligature_run(&cell) && ligature_key(&cell) == key);
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

/// The families and OpenType features text is shaped with: what
/// `font.family`, `bold_family`, `italic_family`, `bold_italic_family` and
/// `features` resolve to against the installed fonts.
#[derive(Debug, Clone, PartialEq)]
struct FontSelection {
    /// Family per [`CellStyle::face`]: regular, bold, italic, bold italic.
    /// A style whose family is not configured holds the regular family, so
    /// its real face is used if it has one and the style is synthesized
    /// otherwise.
    families: [String; 4],
    /// Real bold and italic faces of each entry of `families`.
    face_styles: [FaceStyles; 4],
    features: FontFeatures,
}

impl FontSelection {
    fn new(db: &cosmic_text::fontdb::Database, config: &FontConfig) -> Self {
        let pick = |configured: &String| {
            if configured.is_empty() {
                config.family.clone()
            } else {
                configured.clone()
            }
        };
        let families = [
            config.family.clone(),
            pick(&config.bold_family),
            pick(&config.italic_family),
            pick(&config.bold_italic_family),
        ];
        let face_styles = std::array::from_fn(|i| FaceStyles::of_family(db, &families[i]));
        Self {
            families,
            face_styles,
            features: font_features(&crate::config::parse_font_features(&config.features)),
        }
    }

    /// The regular family, which sets the cell size.
    fn family(&self) -> &str {
        &self.families[0]
    }

    /// Attributes for text in the given style: the style's family, asking
    /// for bold and italic only where that family has the faces, and the
    /// configured features.
    fn attrs(&self, bold: bool, italic: bool) -> Attrs<'_> {
        let face = CellStyle { bold, italic }.face();
        styled_attrs(&self.families[face], bold, italic, self.face_styles[face])
            .font_features(self.features.clone())
    }
}

/// `features` as cosmic-text's list. A later entry for a tag replaces an
/// earlier one, so `["liga", "-liga"]` turns it off.
fn font_features(features: &[crate::config::FontFeature]) -> FontFeatures {
    let mut list = FontFeatures::new();
    for feature in features {
        let tag = FeatureTag::new(&feature.tag);
        list.features.retain(|f| f.tag != tag);
        list.features.push(Feature {
            tag,
            value: feature.value,
        });
    }
    list
}

/// Attributes to shape the grapheme cluster `text` with, and the style the
/// glyphs are drawn in. An emoji-presentation cluster is shaped with
/// `emoji_family` when one is installed, unstyled: color emoji fonts have no
/// bold or italic faces, and a skewed or recolored emoji is no improvement.
/// Anything else, or with no emoji family, uses the configured family.
fn cluster_attrs<'a>(
    text: &str,
    style: CellStyle,
    fonts: &'a FontSelection,
    emoji_family: Option<&'a str>,
) -> (Attrs<'a>, CellStyle) {
    match emoji_family.filter(|_| emoji_presentation(text)) {
        Some(emoji) => (
            Attrs::new().family(Family::Name(emoji)),
            CellStyle::default(),
        ),
        None => (fonts.attrs(style.bold, style.italic), style),
    }
}

/// The color emoji family to shape emoji with (see [`pick_emoji_family`]),
/// warning when the configured `font.emoji` is not installed.
fn installed_emoji_family(db: &cosmic_text::fontdb::Database, configured: &str) -> Option<String> {
    let installed: Vec<&str> = db
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.as_str()))
        .collect();
    let family = pick_emoji_family(configured, &installed);
    if !configured.is_empty() && !family.is_some_and(|f| f.eq_ignore_ascii_case(configured)) {
        tracing::warn!(
            "font.emoji {configured:?} is not installed; using {}",
            family.unwrap_or("the fallback fonts")
        );
    }
    family.map(str::to_owned)
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
    fonts: FontSelection,
    /// Installed color emoji family that emoji-presentation cells are
    /// shaped with, if any (see [`emoji_presentation`]).
    emoji_family: Option<String>,
    ligatures: bool,
    /// Bumped whenever the fonts, size or features change, so cached row
    /// vertices built before are never reused.
    font_generation: u64,
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
    /// Quads in this draw's cached rows, and the quad index list shared by
    /// every draw (all grid geometry is quads).
    frame_quads: usize,
    quad_indices: Vec<u32>,
}

/// One reusable vertex + index buffer pair.
struct GridBuffers {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    /// Quads whose indices `index` already holds. The shared index list
    /// only ever grows, so they stay valid for any draw up to that size.
    indexed_quads: usize,
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
        let fonts = FontSelection::new(font_system.db(), font_config);
        let (cell_width, cell_height, baseline) =
            Self::measure_cell_with_font(&mut font_system, metrics, fonts.family());

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

        let emoji_family = installed_emoji_family(font_system.db(), &font_config.emoji);
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
            fonts,
            emoji_family,
            ligatures: font_config.ligatures,
            font_generation: 0,
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
            frame_quads: 0,
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

    /// Shape `text`, one grapheme cluster, and return every glyph of its
    /// first layout run, each positioned relative to the start of the run.
    /// Emoji are shaped with the color emoji family (see [`cluster_attrs`]).
    fn shape_text(
        &mut self,
        buffer: &mut Buffer,
        text: &str,
        style: CellStyle,
        shaping: Shaping,
    ) -> Vec<ShapedGlyph> {
        let (attrs, style) = cluster_attrs(text, style, &self.fonts, self.emoji_family.as_deref());
        buffer.set_text(text, &attrs, shaping, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let Some(run) = buffer.layout_runs().next() else {
            return Vec::new();
        };
        let line_y = run.line_y;
        let mut shaped = Vec::with_capacity(run.glyphs.len());
        // Origin of the last glyph that advances the pen: the base an
        // unpositioned mark falls back onto.
        let mut base_x = None;
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

            let unpositioned_mark =
                glyph.w == 0.0 && glyph.y_offset == 0.0 && base_x == Some(physical.x);
            if glyph.w != 0.0 {
                base_x = Some(physical.x);
            }

            shaped.push(ShapedGlyph {
                cache_key: physical.cache_key,
                x_offset: physical.x as f32,
                y_offset: physical.y as f32,
                line_y,
                synthetic_bold,
                unpositioned_mark,
            });
        }
        shaped
    }

    /// Shape `text`, one ASCII character per cell, as a single run so the
    /// font can form ligatures across cells. Each glyph comes with the index
    /// of the cell its cluster starts at, and an x offset relative to that
    /// cell. `buffer` must not wrap.
    fn shape_run(&mut self, buffer: &mut Buffer, text: &str, style: CellStyle) -> RunGlyphs {
        shape_run_glyphs(&mut self.font_system, buffer, &self.fonts, text, style)
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
        Self::push_glyph_vertices(vertices, x, y, &glyph_info, color);
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

    /// Upload the vertices of grid draw `draw` (`self.frame_quads` quads)
    /// into the next pooled slot, growing it if needed, and return the slot.
    ///
    /// The rows are copied from the row cache straight into the queue's
    /// staging memory: gathering them into one vector first copied every
    /// vertex of the frame one more time. Indices are uploaded only when the
    /// slot holds fewer than this draw needs.
    fn upload_grid(&mut self, draw: usize) -> usize {
        let quads = self.frame_quads;
        let vertex_len = (quads * 4 * std::mem::size_of::<Vertex>()) as u64;
        let index_len = (quads * 6 * std::mem::size_of::<u32>()) as u64;
        let slot = self.grid_buffers_used;
        self.grid_buffers_used += 1;
        if slot == self.grid_buffers.len() {
            self.grid_buffers.push(GridBuffers {
                vertex: grid_buffer(
                    &self.device,
                    "text_vertex_buffer",
                    wgpu::BufferUsages::VERTEX,
                    vertex_len,
                ),
                index: grid_buffer(
                    &self.device,
                    "text_index_buffer",
                    wgpu::BufferUsages::INDEX,
                    index_len,
                ),
                indexed_quads: 0,
            });
        }
        let buffers = &mut self.grid_buffers[slot];
        if buffers.vertex.size() < vertex_len {
            buffers.vertex = grid_buffer(
                &self.device,
                "text_vertex_buffer",
                wgpu::BufferUsages::VERTEX,
                vertex_len,
            );
        }
        if buffers.index.size() < index_len {
            buffers.index = grid_buffer(
                &self.device,
                "text_index_buffer",
                wgpu::BufferUsages::INDEX,
                index_len,
            );
            buffers.indexed_quads = 0;
        }
        if let Some(size) = wgpu::BufferSize::new(vertex_len) {
            if let Some(mut view) = self.queue.write_buffer_with(&buffers.vertex, 0, size) {
                let mut offset = 0;
                for row in self.grid_caches[draw].vertices() {
                    let bytes: &[u8] = bytemuck::cast_slice(row);
                    view.slice(offset..offset + bytes.len())
                        .copy_from_slice(bytes);
                    offset += bytes.len();
                }
            }
        }
        if buffers.indexed_quads < quads {
            extend_quad_indices(&mut self.quad_indices, quads);
            self.queue.write_buffer(
                &buffers.index,
                0,
                bytemuck::cast_slice(&self.quad_indices[..quads * 6]),
            );
            buffers.indexed_quads = quads;
        }
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

    /// Empty everything keyed by the fonts in use.
    fn drop_font_caches(&mut self) {
        self.atlas.clear();
        self.shaping_cache.clear();
        self.run_cache.clear();
        self.cluster_cache.clear();
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
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
            Self::measure_cell_with_font(&mut self.font_system, self.metrics, self.fonts.family());
        self.cell_width = width;
        self.cell_height = height;

        // Clear caches since metrics changed
        self.font_generation += 1;
        self.drop_font_caches();
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
        buffer.set_text(text, &self.fonts.attrs(false, false), shaping, None);
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
        self.append_text_vertices(text, x, y, fg, bold, italic, false, vertices, indices)
    }

    /// Like [`Self::build_styled_text_vertices`], but laid out on cells as
    /// a terminal does: a grapheme cluster takes the cells of its width by
    /// Unicode (a wide one two) and a combining mark or joiner sequence
    /// shares its cluster's cell, instead of one cell per `char`. Used for
    /// text a caret is placed in.
    pub fn build_cell_text_vertices(
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
        let advance = self.append_text_vertices(
            text,
            x,
            y,
            fg,
            bold,
            italic,
            true,
            &mut vertices,
            &mut indices,
        );
        (vertices, indices, advance)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_text_vertices(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        fg: Rgba,
        bold: bool,
        italic: bool,
        by_cluster: bool,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
    ) -> f32 {
        self.styled_text_buffer
            .set_size(Some(10000.0), Some(self.cell_height * 2.0));

        let shaping = self.get_shaping();
        let attrs = self.fonts.attrs(bold, italic);

        self.styled_text_buffer
            .set_text(text, &attrs, shaping, None);
        self.styled_text_buffer
            .shape_until_scroll(&mut self.font_system, false);

        let clusters = by_cluster.then(|| cluster_columns(text));
        let total_cells = match &clusters {
            Some((_, _, total)) => *total,
            None => {
                fill_byte_to_char_offsets(text, &mut self.byte_to_char_offsets);
                self.byte_to_char_offsets.len()
            }
        };
        let total_advance = total_cells as f32 * self.cell_width;

        for run in self.styled_text_buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let column = match &clusters {
                    Some((starts, columns, _)) => {
                        columns[byte_offset_to_char_index(starts, glyph.start)]
                    }
                    None => byte_offset_to_char_index(&self.byte_to_char_offsets, glyph.start),
                };
                let glyph_x = x + column as f32 * self.cell_width;

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
        let draw = self.build_text_vertices_with_offset(
            terminal,
            color_scheme,
            cell_width,
            cell_height,
            x_offset,
            y_offset,
            cursor_blink_visible,
        );

        if self.frame_quads == 0 {
            return;
        }

        let slot = self.upload_grid(draw);
        let index_count = self.frame_quads * 6;
        let vertex_bytes = (self.frame_quads * 4 * std::mem::size_of::<Vertex>()) as u64;
        let index_bytes = (index_count * std::mem::size_of::<u32>()) as u64;
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

    /// Bring this draw's cached grid rows up to date, rebuilding only the
    /// rows whose content, cursor, or drawing parameters changed since the
    /// same draw in the previous frame. Returns the draw's index into
    /// `grid_caches` and sets `frame_quads` to its number of quads.
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
    ) -> usize {
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
                font_generation: self.font_generation,
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

        self.frame_quads = cache.vertices().map(<[Vertex]>::len).sum::<usize>() / 4;
        self.grid_caches[draw] = cache;
        draw
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
        row: &RowView<'_>,
        row_idx: u16,
        cursor_col: Option<u16>,
        color_scheme: &ColorScheme,
        (cell_width, cell_height): (f32, f32),
        (x_offset, y_offset): (f32, f32),
        cols: u16,
    ) {
        let block_cursor = cursor_col.map(|col| (col, row_idx));
        let solid_uv = self.atlas.solid_uv();
        let mut quads: Vec<Quad> = Vec::new();
        let mut cluster_glyphs: Vec<ShapedGlyph> = Vec::new();
        let mut cluster_inks: Vec<Option<GlyphInfo>> = Vec::new();
        // Cells before this column had their glyphs drawn by a run.
        let mut run_drawn_until = 0u16;

        for col_idx in 0..cols {
            let cell = match row.cell(col_idx) {
                Some(c) => c,
                None => continue,
            };

            if cell.has_flag(CellFlags::HIDDEN) {
                continue;
            }

            let style = CellStyle::from_flags(cell.flags());
            let under_block_cursor = block_cursor == Some((col_idx, row_idx));
            let base_color = if under_block_cursor {
                cursor_glyph_color(&cell, color_scheme)
            } else {
                cell_foreground(&cell, color_scheme)
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
                let line_color = underline_color(&cell, base_color, color_scheme);
                for quad in &quads {
                    Self::emit_solid_quad(vertices, quad, line_color, solid_uv);
                }
                if strikethrough {
                    let quad = strikethrough_quad(cell_rect, self.decoration);
                    Self::emit_solid_quad(vertices, &quad, base_color, solid_uv);
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
                        (col_idx..end).filter_map(|col| row.cell(col).map(|cell| cell.char())),
                    );
                    if could_ligate(&self.run_text) {
                        self.load_run_glyphs(run_buffer, style);
                        let glyphs = std::mem::take(&mut self.run_glyphs);
                        for &(offset, shaped) in &glyphs {
                            let col = col_idx + offset;
                            let Some(glyph_cell) = row.cell(col) else {
                                continue;
                            };
                            let under_cursor = block_cursor == Some((col, row_idx));
                            let color = if under_cursor {
                                cursor_glyph_color(&glyph_cell, color_scheme)
                            } else {
                                cell_foreground(&glyph_cell, color_scheme)
                            };
                            self.push_glyph(
                                vertices,
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
                if cluster_glyphs.iter().any(|glyph| glyph.unpositioned_mark) {
                    cluster_inks.clear();
                    for glyph in &cluster_glyphs {
                        let ink = self.atlas.get_glyph(&mut self.font_system, glyph.cache_key);
                        cluster_inks.push(ink);
                    }
                    stack_unpositioned_marks(&mut cluster_glyphs, &cluster_inks);
                }
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
    /// reserved white block. Grid quads need no indices of their own; the
    /// draw uses the shared quad index list.
    fn emit_solid_quad(vertices: &mut Vec<Vertex>, quad: &Quad, color: Rgba, solid_uv: [f32; 2]) {
        for (position, coverage) in quad.corners.into_iter().zip(quad.coverage) {
            vertices.push(Vertex {
                position,
                tex_coord: solid_uv,
                color: [color.r, color.g, color.b, color.a * coverage],
                is_color: 0,
            });
        }
    }

    /// Emit a quad for a glyph, with its indices.
    fn emit_glyph_quad(
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        glyph: &super::atlas::GlyphInfo,
        color: Rgba,
    ) {
        let base_idx = vertices.len() as u32;
        Self::push_glyph_vertices(vertices, x, y, glyph, color);
        indices.extend_from_slice(&[
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx,
            base_idx + 2,
            base_idx + 3,
        ]);
    }

    /// Push the four vertices of a glyph quad.
    fn push_glyph_vertices(
        vertices: &mut Vec<Vertex>,
        x: f32,
        y: f32,
        glyph: &super::atlas::GlyphInfo,
        color: Rgba,
    ) {
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
        buffer.set_text(text, &self.fonts.attrs(false, false), shaping, None);
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
            &self.fonts.attrs(false, false),
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

    /// The glyph atlas epoch; see [`super::atlas::GlyphAtlas::epoch`].
    pub fn atlas_epoch(&self) -> u64 {
        self.atlas.epoch()
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

/// The start byte and the column of each grapheme cluster of `text`, and
/// the number of columns it takes: what a terminal lays text out by.
fn cluster_columns(text: &str) -> (Vec<usize>, Vec<usize>, usize) {
    use unicode_segmentation::UnicodeSegmentation;

    let mut starts = Vec::new();
    let mut columns = Vec::new();
    let mut column = 0;
    for (start, cluster) in text.grapheme_indices(true) {
        starts.push(start);
        columns.push(column);
        column += crate::scene::caret::grapheme_cells(cluster);
    }
    (starts, columns, column)
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
    fonts: &FontSelection,
    text: &str,
    style: CellStyle,
) -> RunGlyphs {
    let attrs = fonts.attrs(style.bold, style.italic);
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
                unpositioned_mark: false,
            },
        ));
    }
    shaped
}

/// Space left between a stacked mark and the ink below or above it.
const MARK_GAP: f32 = 1.0;

/// A glyph's ink box in pixels: x from the cell origin, y up from the
/// baseline.
#[derive(Clone, Copy)]
struct InkBox {
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
}

impl InkBox {
    fn of(glyph: &ShapedGlyph, ink: &GlyphInfo) -> Option<Self> {
        if ink.width == 0.0 || ink.height == 0.0 {
            return None;
        }
        let left = glyph.x_offset + ink.left;
        let top = ink.top - glyph.y_offset;
        Some(Self {
            left,
            right: left + ink.width,
            bottom: top - ink.height,
            top,
        })
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.bottom < other.top
            && other.bottom < self.top
    }

    fn union(&self, other: &Self) -> Self {
        Self {
            left: self.left.min(other.left),
            right: self.right.max(other.right),
            bottom: self.bottom.min(other.bottom),
            top: self.top.max(other.top),
        }
    }
}

/// Stack the marks of one grapheme that the font left unpositioned, the
/// way shapers place marks for fonts without mark positioning.
///
/// A font with no anchor for a mark on its base (common once the shaper
/// has composed the base with an earlier mark, as e + U+0301 into é)
/// leaves the mark at the base's origin, at the height it was drawn for:
/// a second accent lands on the first. Each such mark whose ink collides
/// with the ink before it moves up above that ink or, if it sits lower
/// than the middle of the base, down below it. `inks` holds each glyph's
/// rasterized ink, `None` where it has none.
fn stack_unpositioned_marks(glyphs: &mut [ShapedGlyph], inks: &[Option<GlyphInfo>]) {
    let mut base_middle = None;
    let mut drawn: Option<InkBox> = None;
    for (glyph, ink) in glyphs.iter_mut().zip(inks) {
        let Some(mut ink_box) = ink.as_ref().and_then(|ink| InkBox::of(glyph, ink)) else {
            continue;
        };
        if let (true, Some(under), Some(middle)) = (glyph.unpositioned_mark, drawn, base_middle) {
            if ink_box.overlaps(&under) {
                let raise = if ink_box.bottom + ink_box.top >= 2.0 * middle {
                    under.top + MARK_GAP - ink_box.bottom
                } else {
                    under.bottom - MARK_GAP - ink_box.top
                };
                glyph.y_offset -= raise;
                ink_box.bottom += raise;
                ink_box.top += raise;
            }
        }
        base_middle.get_or_insert((ink_box.bottom + ink_box.top) / 2.0);
        drawn = Some(drawn.map_or(ink_box, |drawn| drawn.union(&ink_box)));
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
    fn cluster_attrs_shapes_emoji_with_the_emoji_family_unstyled() {
        let bold = CellStyle {
            bold: true,
            italic: false,
        };
        let emoji = Some("Noto Color Emoji");
        let mono = selection("Mono");
        let shape = |text, emoji_family| cluster_attrs(text, bold, &mono, emoji_family);

        let (attrs, style) = shape("\u{1F600}", emoji);
        assert_eq!(attrs.family, Family::Name("Noto Color Emoji"));
        assert_eq!(attrs.weight, Weight::NORMAL, "no bold emoji face exists");
        assert_eq!(style, CellStyle::default(), "nothing to synthesize");

        let (attrs, _) = shape("\u{2764}\u{FE0F}", emoji);
        assert_eq!(attrs.family, Family::Name("Noto Color Emoji"));

        for text in ["\u{2764}", "\u{1F600}\u{FE0E}", "a"] {
            let (attrs, style) = shape(text, emoji);
            assert_eq!(attrs.family, Family::Name("Mono"), "{text:?} is text");
            assert_eq!(attrs.weight, Weight::BOLD);
            assert_eq!(style, bold);
        }

        let (attrs, style) = shape("\u{1F600}", None);
        assert_eq!(
            attrs.family,
            Family::Name("Mono"),
            "without an emoji font, font fallback decides"
        );
        assert_eq!(style, bold);
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

    /// The column a glyph at `byte` of `text` is drawn in on the cell
    /// layout, and the columns in all.
    fn column_of(text: &str, byte: usize) -> (usize, usize) {
        let (starts, columns, total) = cluster_columns(text);
        (columns[byte_offset_to_char_index(&starts, byte)], total)
    }

    #[test]
    fn the_cell_layout_gives_a_wide_character_two_columns() {
        // "你" is bytes 0..3 and "好" 3..6: columns 0 and 2.
        assert_eq!(column_of("你好", 0), (0, 4));
        assert_eq!(column_of("你好", 3), (2, 4));
    }

    #[test]
    fn the_cell_layout_puts_a_combining_mark_in_its_clusters_cell() {
        // "a", "e" + U+0301 (bytes 1..4), "b" at byte 4.
        let text = "ae\u{301}b";
        assert_eq!(column_of(text, 1), (1, 3));
        assert_eq!(column_of(text, 2), (1, 3));
        assert_eq!(column_of(text, 4), (2, 3));
    }

    #[test]
    fn the_cell_layout_keeps_a_joined_emoji_in_one_wide_cluster() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let text = format!("a{family}b");
        let b = text.len() - 1;
        assert_eq!(column_of(&text, 1), (1, 4));
        // Each part of the sequence is drawn at the cluster's column.
        assert_eq!(column_of(&text, 1 + 4 + 3), (1, 4));
        assert_eq!(column_of(&text, b), (3, 4));
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
            unpositioned_mark: false,
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

    fn glyph_at(x_offset: f32, unpositioned_mark: bool) -> ShapedGlyph {
        let (cache_key, _, _) = CacheKey::new(
            cosmic_text::fontdb::ID::dummy(),
            1,
            14.0,
            (0.0, 0.0),
            cosmic_text::fontdb::Weight::NORMAL,
            cosmic_text::CacheKeyFlags::empty(),
        );
        ShapedGlyph {
            cache_key,
            x_offset,
            y_offset: 0.0,
            line_y: 11.0,
            synthetic_bold: false,
            unpositioned_mark,
        }
    }

    /// Ink of a glyph `width` x `height` px, `left` px right of its origin
    /// and `top` px above the baseline.
    fn ink(left: f32, top: f32, width: f32, height: f32) -> Option<super::super::atlas::GlyphInfo> {
        Some(super::super::atlas::GlyphInfo {
            uv: [0.0; 4],
            width,
            height,
            left,
            top,
            is_color: false,
        })
    }

    #[test]
    fn unpositioned_mark_stacks_above_an_accented_base() {
        // "é̃" in Droid Sans Mono: the font composes e + U+0301 into é, whose
        // acute reaches 11 px above the baseline, and has no anchor for a
        // tilde on é, so the tilde stays at its lowercase height (8-11 px)
        // and drew over the acute.
        let mut glyphs = [glyph_at(0.0, false), glyph_at(0.0, true)];
        let inks = [ink(0.0, 11.0, 8.0, 12.0), ink(1.0, 11.0, 6.0, 3.0)];
        stack_unpositioned_marks(&mut glyphs, &inks);

        assert_eq!(glyphs[0].y_offset, 0.0);
        // Raised so its bottom clears the acute by a pixel: 11 + 1 = 8 + 4.
        assert_eq!(glyphs[1].y_offset, -4.0);
        assert_eq!(glyphs[1].x_offset, 0.0);
    }

    #[test]
    fn unpositioned_mark_that_clears_its_base_stays() {
        // "ụ̃": the tilde already sits above the u.
        let mut glyphs = [glyph_at(0.0, false), glyph_at(0.0, true)];
        let inks = [ink(1.0, 8.0, 7.0, 11.0), ink(1.0, 11.0, 6.0, 3.0)];
        stack_unpositioned_marks(&mut glyphs, &inks);
        assert_eq!(glyphs[1].y_offset, 0.0);
    }

    #[test]
    fn unpositioned_mark_below_stacks_under_the_base() {
        // A dot below on a base whose ink already reaches 3 px below the
        // baseline goes under it.
        let mut glyphs = [glyph_at(0.0, false), glyph_at(0.0, true)];
        let inks = [ink(1.0, 8.0, 7.0, 11.0), ink(3.0, -1.0, 2.0, 2.0)];
        stack_unpositioned_marks(&mut glyphs, &inks);
        // Top lowered from -1 to 1 px below the base's bottom (-3): -4.
        assert_eq!(glyphs[1].y_offset, 3.0);
    }

    #[test]
    fn marks_after_an_unpositioned_mark_stack_on_it() {
        let mut glyphs = [
            glyph_at(0.0, false),
            glyph_at(0.0, true),
            glyph_at(0.0, true),
        ];
        let inks = [
            ink(0.0, 11.0, 8.0, 12.0),
            ink(1.0, 11.0, 6.0, 3.0),
            ink(1.0, 11.0, 6.0, 3.0),
        ];
        stack_unpositioned_marks(&mut glyphs, &inks);
        assert_eq!(glyphs[1].y_offset, -4.0);
        // On top of the first tilde, now 12-15 px up.
        assert_eq!(glyphs[2].y_offset, -8.0);
    }

    #[test]
    fn marks_the_font_positioned_are_kept() {
        // A mark the font attached itself is drawn where the font put it,
        // even if its ink touches the base.
        let mut glyphs = [glyph_at(0.0, false), glyph_at(0.0, false)];
        let inks = [ink(0.0, 11.0, 8.0, 12.0), ink(1.0, 11.0, 6.0, 3.0)];
        stack_unpositioned_marks(&mut glyphs, &inks);
        assert_eq!(glyphs[1].y_offset, 0.0);
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
            ligature_run_end(&RowView::from(&row), 0, row.width()),
            4,
            "stops at the space"
        );
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 4, row.width()),
            4,
            "a space starts no run"
        );

        let mut row = row_of("x!=y");
        row[2].set_flag(CellFlags::BOLD);
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 0, row.width()),
            2,
            "bold changes the face"
        );

        let mut row = row_of("->中");
        row[2].set_flag(CellFlags::WIDE_CHAR);
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 0, row.width()),
            2,
            "non-ASCII ends the run"
        );

        // A foreground change ends a run, so a ligature never recolors
        // cells the program colored differently. Background does not.
        let mut row = row_of("==>");
        row[1].set_fg(crate::grid::Color::Named(2));
        assert_eq!(ligature_run_end(&RowView::from(&row), 0, row.width()), 1);
        let mut row = row_of("==>");
        row[1].set_bg(crate::grid::Color::Named(2));
        assert_eq!(ligature_run_end(&RowView::from(&row), 0, row.width()), 3);
    }

    #[test]
    fn only_runs_with_punctuation_are_shaped_together() {
        assert!(could_ligate("a=>b"));
        assert!(could_ligate("!="));
        assert!(!could_ligate("hello123"));
    }

    /// Selection of `family` for every style, with no fonts installed to
    /// narrow what can be requested.
    fn selection(family: &str) -> FontSelection {
        selection_of(&FontConfig {
            family: family.to_string(),
            ..FontConfig::default()
        })
    }

    fn selection_of(config: &FontConfig) -> FontSelection {
        FontSelection::new(&cosmic_text::fontdb::Database::new(), config)
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
            &selection("DejaVu Sans"),
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
            &selection("DejaVu Sans Mono"),
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
                &selection(&family),
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
                        &selection(&family),
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

    fn config_with(f: impl FnOnce(&mut FontConfig)) -> FontConfig {
        let mut config = FontConfig {
            family: "Mono".to_string(),
            ..FontConfig::default()
        };
        f(&mut config);
        config
    }

    #[test]
    fn each_style_uses_its_own_family_when_one_is_set() {
        let fonts = selection_of(&config_with(|c| {
            c.bold_family = "Mono Heavy".to_string();
            c.italic_family = "Mono Slant".to_string();
            c.bold_italic_family = "Mono Heavy Slant".to_string();
        }));
        let family = |bold, italic| fonts.attrs(bold, italic).family;
        assert_eq!(family(false, false), Family::Name("Mono"));
        assert_eq!(family(true, false), Family::Name("Mono Heavy"));
        assert_eq!(family(false, true), Family::Name("Mono Slant"));
        assert_eq!(family(true, true), Family::Name("Mono Heavy Slant"));
        // The style is still requested of the family; it picks its nearest.
        assert_eq!(fonts.attrs(true, true).weight, Weight::BOLD);
        assert_eq!(fonts.attrs(true, true).style, Style::Italic);
    }

    #[test]
    fn unset_style_families_fall_back_to_the_regular_one() {
        let fonts = selection_of(&config_with(|c| c.bold_family = "Mono Heavy".to_string()));
        assert_eq!(fonts.attrs(true, false).family, Family::Name("Mono Heavy"));
        for (bold, italic) in [(false, true), (true, true)] {
            assert_eq!(
                fonts.attrs(bold, italic).family,
                Family::Name("Mono"),
                "bold_italic_family does not borrow bold_family"
            );
        }
    }

    #[test]
    fn selections_differing_in_family_or_features_are_not_equal() {
        // Caches are dropped when the selection changes, so a change in
        // anything that alters shaped glyphs must show up in `==`.
        let base = selection("Mono");
        assert_eq!(base, selection("Mono"));
        assert_ne!(base, selection("Other"));
        assert_ne!(
            base,
            selection_of(&config_with(|c| c.italic_family = "Slant".to_string()))
        );
        let with = |features: &[&str]| {
            selection_of(&config_with(|c| {
                c.features = features.iter().map(|f| f.to_string()).collect();
            }))
        };
        assert_ne!(base, with(&["-liga"]));
        assert_ne!(with(&["ss01"]), with(&["ss01=2"]));
    }

    #[test]
    fn features_reach_the_shaper_with_the_last_entry_winning() {
        let fonts = selection_of(&config_with(|c| {
            c.features = ["calt", "-liga", "ss01", "liga", "bad", "zero=1", "-zero"]
                .map(String::from)
                .into();
        }));
        let list: Vec<_> = fonts
            .attrs(false, false)
            .font_features
            .features
            .iter()
            .map(|f| (f.tag, f.value))
            .collect();
        let tag = |t: &[u8; 4]| FeatureTag::new(t);
        assert_eq!(
            list,
            [
                (tag(b"calt"), 1),
                (tag(b"ss01"), 1),
                (tag(b"liga"), 1),
                (tag(b"zero"), 0),
            ]
        );
        // Every style's attributes carry them.
        assert_eq!(
            fonts.attrs(true, true).font_features,
            fonts.attrs(false, false).font_features
        );
    }

    /// Glyph ids of `text` in `family` shaped with `features`, or `None` when
    /// the family is not installed.
    fn shaped_glyph_ids(family: &str, features: &[&str], text: &str) -> Option<Vec<u16>> {
        let mut font_system = FontSystem::new();
        let installed = font_system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == family));
        if !installed {
            eprintln!("{family} not installed; skipped");
            return None;
        }
        let fonts = selection_of(&config_with(|c| {
            c.family = family.to_string();
            c.features = features.iter().map(|f| f.to_string()).collect();
        }));
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        buffer.set_wrap(cosmic_text::Wrap::None);
        buffer.set_size(Some(1000.0), Some(40.0));
        let attrs = fonts.attrs(false, false);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut font_system, false);
        let run = buffer.layout_runs().next()?;
        Some(run.glyphs.iter().map(|g| g.glyph_id).collect())
    }

    #[test]
    fn a_feature_list_changes_what_the_font_shapes() {
        // DejaVu Sans joins "fi" with `liga`; `-liga` keeps two glyphs.
        let Some(default) = shaped_glyph_ids("DejaVu Sans", &[], "fi") else {
            return;
        };
        let off = shaped_glyph_ids("DejaVu Sans", &["-liga"], "fi").unwrap();
        assert_eq!(default.len(), 1, "fi is one ligature glyph by default");
        assert_eq!(off.len(), 2, "-liga shapes f and i apart");
        // An unrelated feature leaves it alone, and a later `liga` restores it.
        assert_eq!(
            shaped_glyph_ids("DejaVu Sans", &["zero"], "fi").unwrap(),
            default
        );
        assert_eq!(
            shaped_glyph_ids("DejaVu Sans", &["-liga", "liga"], "fi").unwrap(),
            default
        );
    }
}
