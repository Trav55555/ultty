//! Text rendering with cosmic-text and glyph atlas.

use super::atlas::{self, GlyphAtlas, GlyphInfo};
use super::decoration::{strikethrough_quad, underline_quads, DecorationMetrics, Quad};
use super::emoji::{emoji_presentation, pick_emoji_family};
use super::pipeline::TextPipeline;
use super::powerline::is_powerline_triangle;
use super::text_cache::{GridTextCache, GridTextKey};
use super::{ColorScheme, Rgba, Vertex};
use crate::config::{FontConfig, FontMetricsMode};
use crate::grid::{
    Cell, CellFlags, Color, CursorShape, RowView, Selection, Terminal, UnderlineStyle,
};
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, Feature, FeatureTag, FontFeatures, FontSystem,
    Metrics, Shaping, Style, Weight,
};
// Looked up for every drawn cell; SipHash was a third of a scrolling frame.
use foldhash::{HashMap, HashMapExt};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

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

    #[cfg(test)]
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
fn joins_ligature_run(cell: &crate::grid::Cell, fonts: &FontSelection) -> bool {
    let ch = cell.char();
    ch.is_ascii_graphic()
        && fonts.mapped(ch).is_none()
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
fn ligature_run_end(row: &RowView<'_>, start: u16, cols: u16, fonts: &FontSelection) -> u16 {
    let Some(first) = row
        .cell(start)
        .filter(|cell| joins_ligature_run(cell, fonts))
    else {
        return start;
    };
    let key = ligature_key(&first);
    let mut end = start + 1;
    while end < cols {
        let joins = row
            .cell(end)
            .is_some_and(|cell| joins_ligature_run(&cell, fonts) && ligature_key(&cell) == key);
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
/// face it picked tells us whether to emphasize the color (bold) or skew the
/// glyph via [`CacheKeyFlags::FAKE_ITALIC`] (italic).
fn synthesized_styles(style: CellStyle, face: Option<(Weight, Style)>) -> (bool, bool) {
    let has_bold_face = face.is_some_and(|(weight, _)| weight >= Weight::BOLD);
    let has_italic_face = face.is_some_and(|(_, face_style)| face_style != Style::Normal);
    (
        style.bold && !has_bold_face,
        style.italic && !has_italic_face,
    )
}

/// Emphasize a color for cells that asked for bold when no bold face
/// exists: brighter on a dark background, darker on a light one, so bold
/// text stands out from the background more than plain text does, not less.
fn emphasize(color: Rgba, background: Rgba) -> Rgba {
    const LIFT: f32 = 0.3;
    let luma = 0.2126 * background.r + 0.7152 * background.g + 0.0722 * background.b;
    let target = if luma > 0.5 { 0.0 } else { 1.0 };
    Rgba::new(
        color.r + (target - color.r) * LIFT,
        color.g + (target - color.g) * LIFT,
        color.b + (target - color.b) * LIFT,
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

/// Foreground of a cell's glyph: [`cell_foreground`], or the scheme's
/// selection text colour for it when the cell is selected.
fn text_color(cell: &Cell, color_scheme: &ColorScheme, selected: bool) -> Rgba {
    let fg = cell_foreground(cell, color_scheme);
    if selected {
        color_scheme.selection_text(fg)
    } else {
        fg
    }
}

fn is_selected(span: Option<(u16, u16)>, col: u16) -> bool {
    span.is_some_and(|(first, last)| (first..=last).contains(&col))
}

/// The columns of screen row `row` inside `selection`, as (first, last).
/// A selection covers one run of columns on each row.
fn selected_span(selection: &Selection, row: u16, cols: u16) -> Option<(u16, u16)> {
    let mut hits = (0..cols).filter(|&col| selection.contains(col, row));
    let first = hits.next()?;
    let last = hits.next_back().unwrap_or(first);
    Some((first, last))
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
pub(super) fn font_attrs(font_family: &str) -> Attrs<'_> {
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
    /// `font.codepoint_map`: ranges drawn from a given family, with that
    /// family's real faces. Later entries win.
    codepoints: Vec<MappedRange>,
}

#[derive(Debug, Clone, PartialEq)]
struct MappedRange {
    range: std::ops::RangeInclusive<char>,
    family: String,
    face_styles: FaceStyles,
}

/// Whether any installed face is in `family` (names match without regard to
/// case, as the lookups below do).
fn family_installed(db: &cosmic_text::fontdb::Database, family: &str) -> bool {
    db.faces().any(|face| {
        face.families
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(family))
    })
}

/// Whether a family can be drawn from the installed fonts.
type Installed<'a> = &'a dyn Fn(&str) -> bool;

/// `family` if it is installed, empty (the system monospace font, see
/// [`font_attrs`]) otherwise, so a missing font never leaves the grid to
/// cosmic-text's fallback, which may be proportional. Warns once per
/// missing family through `warned`.
fn installed_or_monospace(installed: Installed, family: &str, warned: &mut Vec<String>) -> String {
    if family.is_empty() || installed(family) {
        return family.to_owned();
    }
    if !warned.iter().any(|w| w.eq_ignore_ascii_case(family)) {
        tracing::warn!("font family {family:?} is not installed; using the system monospace font");
        warned.push(family.to_owned());
    }
    String::new()
}

impl FontSelection {
    fn new(db: &cosmic_text::fontdb::Database, config: &FontConfig) -> Self {
        Self::resolve(db, config, &|family| family_installed(db, family))
    }

    /// The selection for `config`, where `installed` says which families
    /// exist. A family that does not is replaced by the system monospace
    /// font.
    fn resolve(
        db: &cosmic_text::fontdb::Database,
        config: &FontConfig,
        installed: Installed,
    ) -> Self {
        let mut warned = Vec::new();
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
        ]
        .map(|family| installed_or_monospace(installed, &family, &mut warned));
        let face_styles = std::array::from_fn(|i| FaceStyles::of_family(db, &families[i]));
        Self {
            families,
            face_styles,
            features: font_features(&crate::config::parse_font_features(&config.features)),
            codepoints: crate::config::parse_codepoint_map(&config.codepoint_map)
                .into_iter()
                .map(|(range, family)| {
                    let family = installed_or_monospace(installed, &family, &mut warned);
                    MappedRange {
                        range,
                        face_styles: FaceStyles::of_family(db, &family),
                        family,
                    }
                })
                .collect(),
        }
    }

    /// The mapped range `ch` falls in, if `font.codepoint_map` has one.
    fn mapped(&self, ch: char) -> Option<&MappedRange> {
        self.codepoints
            .iter()
            .rev()
            .find(|mapped| mapped.range.contains(&ch))
    }

    /// Attributes for `ch`'s style, from its mapped family if it has one.
    fn attrs_for(&self, ch: char, bold: bool, italic: bool) -> Attrs<'_> {
        match self.mapped(ch) {
            Some(mapped) => styled_attrs(&mapped.family, bold, italic, mapped.face_styles)
                .font_features(self.features.clone()),
            None => self.attrs(bold, italic),
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
/// A cluster starting with a character in `font.codepoint_map` is shaped
/// with the mapped family instead, which wins over the emoji family.
/// Anything else, or with no emoji family, uses the configured family.
fn cluster_attrs<'a>(
    text: &str,
    style: CellStyle,
    fonts: &'a FontSelection,
    emoji_family: Option<&'a str>,
) -> (Attrs<'a>, CellStyle) {
    let first = text.chars().next().unwrap_or(' ');
    if fonts.mapped(first).is_some() {
        return (fonts.attrs_for(first, style.bold, style.italic), style);
    }
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

/// Process-wide `cosmic_text::FontSystem` shared across every window and
/// [`FontGrid`] (ADR 0027 slice 2).
///
/// Built once (`FontSystem::new` scans the installed font database); windows
/// reuse this instance rather than re-scanning system fonts.
#[derive(Clone)]
pub struct SharedFontSystem {
    inner: Arc<Mutex<FontSystem>>,
}

impl SharedFontSystem {
    /// Scan the system fonts once and wrap the database for process-wide reuse.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FontSystem::new())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, FontSystem> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for SharedFontSystem {
    fn default() -> Self {
        Self::new()
    }
}

fn normalized_f32_bits(value: f32) -> u32 {
    if value == 0.0 {
        0.0f32.to_bits()
    } else {
        value.to_bits()
    }
}

fn normalize_font_config(config: &FontConfig) -> FontConfig {
    let mut normalized = config.clone();
    normalized.size = crate::config::clamp_font_size(normalized.size);
    normalized.clamp_line_height();
    normalized
}

/// Cache key for a shared [`FontGrid`]: font families, OpenType features,
/// codepoint map, metrics mode, line height, and scale-adjusted physical size.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontGridKey {
    family: String,
    bold_family: String,
    italic_family: String,
    bold_italic_family: String,
    emoji: String,
    features: Vec<String>,
    codepoint_map: Vec<(String, String)>,
    metrics: FontMetricsMode,
    line_height_bits: u32,
    size_bits: u32,
}

impl FontGridKey {
    /// Build a normalized key from a scaled [`FontConfig`].
    pub fn from_config(config: &FontConfig) -> Self {
        let normalized = normalize_font_config(config);
        Self {
            family: normalized.family,
            bold_family: normalized.bold_family,
            italic_family: normalized.italic_family,
            bold_italic_family: normalized.bold_italic_family,
            emoji: normalized.emoji,
            features: normalized.features,
            codepoint_map: normalized
                .codepoint_map
                .into_iter()
                .map(|entry| (entry.range, entry.family))
                .collect(),
            metrics: normalized.metrics,
            line_height_bits: normalized_f32_bits(normalized.line_height),
            size_bits: normalized_f32_bits(normalized.size),
        }
    }
}

struct FontGridState {
    atlas: GlyphAtlas,
    shaping_cache: HashMap<ShapingCacheKey, ShapedGlyph>,
    /// Shaped runs for ligatures; see [`ligature_run_end`].
    run_cache: TextShapeCache<(u16, ShapedGlyph)>,
    /// Shaped multi-codepoint graphemes (combining marks, ZWJ emoji, flags).
    cluster_cache: TextShapeCache<ShapedGlyph>,
}

/// Shared glyph atlas and shaping caches for one `(font config, physical size)`
/// on one GPU device (ADR 0027 slice 2).
struct FontGrid {
    cell_width: f32,
    cell_height: f32,
    /// Configured size multiplied by the display scale.
    font_size: f32,
    metrics: Metrics,
    decoration: DecorationMetrics,
    fonts: FontSelection,
    /// Installed color emoji family that emoji-presentation cells are
    /// shaped with, if any (see [`emoji_presentation`]).
    emoji_family: Option<String>,
    state: Mutex<FontGridState>,
}

impl FontGrid {
    fn from_fonts(
        fonts: &Fonts,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        bind_group_layout: Arc<wgpu::BindGroupLayout>,
        sampler: Arc<wgpu::Sampler>,
    ) -> Self {
        let mut atlas =
            GlyphAtlas::with_shared_resources(device, queue, bind_group_layout, sampler);
        let decoration = measure_decoration_metrics(
            &fonts.font_system,
            &mut atlas,
            &fonts.selection,
            fonts.emoji_family.as_deref(),
            fonts.metrics,
            fonts.cell_width,
            fonts.cell_height,
            fonts.baseline,
        );
        Self {
            cell_width: fonts.cell_width,
            cell_height: fonts.cell_height,
            font_size: fonts.font_size,
            metrics: fonts.metrics,
            decoration,
            fonts: fonts.selection.clone(),
            emoji_family: fonts.emoji_family.clone(),
            state: Mutex::new(FontGridState {
                atlas,
                shaping_cache: HashMap::with_capacity(256),
                run_cache: Default::default(),
                cluster_cache: TextShapeCache::default(),
            }),
        }
    }

    fn state_lock(&self) -> MutexGuard<'_, FontGridState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Per-device pool of ref-counted [`FontGrid`]s keyed by font configuration
/// and physical size (ADR 0027 slice 2).
///
/// Entries are held as [`Weak`] references: a grid stays alive while at least
/// one [`TextRenderer`] references it, and drops its GPU glyph atlas texture
/// automatically when the last window stops using it.
#[derive(Clone)]
pub struct FontGridSet {
    inner: Arc<FontGridSetInner>,
}

struct FontGridSetInner {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    font_system: SharedFontSystem,
    bind_group_layout: Arc<wgpu::BindGroupLayout>,
    sampler: Arc<wgpu::Sampler>,
    grids: Mutex<HashMap<FontGridKey, Weak<FontGrid>>>,
}

impl FontGridSet {
    /// Create a new per-device font grid pool sharing `font_system`.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        font_system: SharedFontSystem,
    ) -> Self {
        let sampler = Arc::new(atlas::create_sampler(&device));
        let bind_group_layout = Arc::new(atlas::create_bind_group_layout(&device));
        Self {
            inner: Arc::new(FontGridSetInner {
                device,
                queue,
                font_system,
                bind_group_layout,
                sampler,
                grids: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The process-wide font system shared by every grid in this pool.
    pub fn font_system(&self) -> &SharedFontSystem {
        &self.inner.font_system
    }

    /// The bind group layout shared by every glyph atlas in this pool.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.inner.bind_group_layout
    }

    /// Number of currently live [`FontGrid`]s (and therefore glyph atlases) in
    /// this device pool.
    pub fn active_grid_count(&self) -> usize {
        let grids = self
            .inner
            .grids
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        grids
            .values()
            .filter(|weak| weak.strong_count() > 0)
            .count()
    }

    fn get_or_create(&self, config: &FontConfig) -> Arc<FontGrid> {
        let key = FontGridKey::from_config(config);
        {
            let grids = self
                .inner
                .grids
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(grid) = grids.get(&key).and_then(Weak::upgrade) {
                return grid;
            }
        }
        let fonts = Fonts::load_with_shared(config, self.inner.font_system.clone());
        let created = Arc::new(FontGrid::from_fonts(
            &fonts,
            Arc::clone(&self.inner.device),
            Arc::clone(&self.inner.queue),
            Arc::clone(&self.inner.bind_group_layout),
            Arc::clone(&self.inner.sampler),
        ));
        self.insert_or_existing(key, created)
    }

    fn get_or_create_from_fonts(&self, fonts: &Fonts) -> Arc<FontGrid> {
        let key = FontGridKey::from_config(&fonts.config);
        {
            let grids = self
                .inner
                .grids
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(grid) = grids.get(&key).and_then(Weak::upgrade) {
                return grid;
            }
        }
        let created = Arc::new(FontGrid::from_fonts(
            fonts,
            Arc::clone(&self.inner.device),
            Arc::clone(&self.inner.queue),
            Arc::clone(&self.inner.bind_group_layout),
            Arc::clone(&self.inner.sampler),
        ));
        self.insert_or_existing(key, created)
    }

    fn insert_or_existing(&self, key: FontGridKey, created: Arc<FontGrid>) -> Arc<FontGrid> {
        let mut grids = self
            .inner
            .grids
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        grids.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = grids.get(&key).and_then(Weak::upgrade) {
            return existing;
        }
        grids.insert(key, Arc::downgrade(&created));
        created
    }
}

pub struct TextRenderer {
    font_grids: FontGridSet,
    grid: Arc<FontGrid>,
    atlas_bind_group: Arc<wgpu::BindGroup>,
    config: FontConfig,
    pipeline: TextPipeline,
    cell_width: f32,
    cell_height: f32,
    /// Configured size multiplied by the display scale.
    font_size: f32,
    metrics: Metrics,
    decoration: DecorationMetrics,
    device: Arc<wgpu::Device>,
    ligatures: bool,
    /// Bumped whenever the fonts, size or features change, so cached row
    /// vertices built before are never reused.
    font_generation: u64,
    /// Reused per run: the run's text, and a copy of its glyphs to draw.
    run_text: String,
    run_glyphs: RunGlyphs,
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

/// Pixels per typographic point at the standard 96 DPI reference (`96 / 72`).
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;

#[derive(Clone, Copy)]
struct FontMetrics {
    pixels_per_unit: f32,
    line_height_ratio: f32,
    line_height_multiplier: f32,
}

impl FontMetrics {
    fn load(config: &FontConfig, font_system: &mut FontSystem, family: &str) -> Self {
        let (pixels_per_unit, line_height_ratio) = match config.metrics {
            FontMetricsMode::Points => (
                POINTS_TO_PIXELS,
                measure_natural_line_ratio(font_system, family),
            ),
            FontMetricsMode::Legacy => (1.0, 1.0),
        };
        Self {
            pixels_per_unit,
            line_height_ratio,
            line_height_multiplier: config.line_height,
        }
    }

    fn at_size(self, scaled_size: f32) -> Metrics {
        let font_size_px = scaled_size * self.pixels_per_unit;
        let line_height_px = (font_size_px * self.line_height_ratio * self.line_height_multiplier)
            .ceil()
            .max(1.0);
        Metrics::new(font_size_px, line_height_px)
    }
}

/// Measure `(ascent - descent + leading) / units_per_em` for the primary face
/// resolved for `font_family`, falling back to `1.0` if unavailable.
fn measure_natural_line_ratio(font_system: &mut FontSystem, font_family: &str) -> f32 {
    let mut buffer = Buffer::new(font_system, Metrics::new(16.0, 16.0));
    buffer.set_text("M", &font_attrs(font_family), Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let Some(font_id) = buffer
        .layout_runs()
        .next()
        .and_then(|run| run.glyphs.first())
        .map(|g| g.font_id)
    else {
        return 1.0;
    };
    let Some(font) = font_system.get_font(font_id, Weight::NORMAL) else {
        return 1.0;
    };
    let m = font.metrics();
    let upem = f32::from(m.units_per_em);
    if upem > 0.0 {
        let natural = (m.ascent - m.descent + m.leading.max(0.0)) / upem;
        if natural.is_finite() && natural > 0.5 {
            return natural;
        }
    }
    1.0
}

/// The fonts text is shaped with and the cell size they give, measured on
/// the CPU. Nothing here needs a GPU device, so startup can size the grid
/// and spawn the shell while the GPU comes up on another thread; the text
/// renderer is then built from these same fonts, so its cell size is the one
/// measured here.
pub struct Fonts {
    font_system: SharedFontSystem,
    config: FontConfig,
    /// Clamped size multiplied by the display scale.
    font_size: f32,
    metrics: Metrics,
    selection: FontSelection,
    emoji_family: Option<String>,
    ligatures: bool,
    cell_width: f32,
    cell_height: f32,
    /// Baseline in px from the cell top.
    baseline: f32,
}

impl Fonts {
    /// Load the system fonts and measure the cell for `font_config`.
    pub fn load(font_config: &FontConfig) -> Self {
        Self::load_with_shared(font_config, SharedFontSystem::new())
    }

    /// Measure the cell for `font_config` using an existing process-wide
    /// [`SharedFontSystem`] without re-scanning system fonts.
    pub fn load_with_shared(font_config: &FontConfig, font_system: SharedFontSystem) -> Self {
        let config = normalize_font_config(font_config);
        let (selection, font_size, metrics, cell_width, cell_height, baseline, emoji_family) = {
            let mut fs = font_system.lock();
            let selection = FontSelection::new(fs.db(), &config);
            let font_size = config.size;
            let font_metrics = FontMetrics::load(&config, &mut fs, selection.family());
            let metrics = font_metrics.at_size(font_size);
            let (cell_width, cell_height, baseline) =
                TextRenderer::measure_cell_with_font(&mut fs, metrics, selection.family());
            let emoji_family = installed_emoji_family(fs.db(), &config.emoji);
            (
                selection,
                font_size,
                metrics,
                cell_width,
                cell_height,
                baseline,
                emoji_family,
            )
        };
        Self {
            font_system,
            ligatures: config.ligatures,
            config,
            font_size,
            metrics,
            selection,
            emoji_family,
            cell_width,
            cell_height,
            baseline,
        }
    }

    /// Shared handle to the process-wide font system backing these fonts.
    pub fn shared_font_system(&self) -> SharedFontSystem {
        self.font_system.clone()
    }

    /// Cell width and height in pixels.
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// The configured size multiplied by the display scale.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// A font system over the same fonts, for the CPU frames. Copies the
    /// database's face list; no font file is read again.
    pub(super) fn font_system_copy(&self) -> FontSystem {
        let fs = self.font_system.lock();
        FontSystem::new_with_locale_and_db(fs.locale().to_owned(), fs.db().clone())
    }

    pub(super) fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// The regular family; empty for the system monospace.
    pub(super) fn family(&self) -> &str {
        self.selection.family()
    }

    /// Baseline in px from the cell top.
    pub(super) fn baseline(&self) -> f32 {
        self.baseline
    }
}

/// Measure the x-height (from the rasterized `x` glyph) and line thickness
/// used to place underline and strikethrough quads.
#[allow(clippy::too_many_arguments)]
fn measure_decoration_metrics(
    font_system: &SharedFontSystem,
    atlas: &mut GlyphAtlas,
    fonts: &FontSelection,
    emoji_family: Option<&str>,
    metrics: Metrics,
    cell_width: f32,
    cell_height: f32,
    baseline: f32,
) -> DecorationMetrics {
    let mut fs = font_system.lock();
    let mut buffer = Buffer::new(&mut fs, metrics);
    buffer.set_size(Some(cell_width * 4.0), Some(cell_height * 2.0));
    let x_height = shape_cell_with_fs(
        &mut fs,
        &mut buffer,
        fonts,
        emoji_family,
        "x",
        CellStyle::default(),
    )
    .and_then(|shaped| atlas.get_glyph(&mut fs, shaped.cache_key))
    .map(|glyph| glyph.top)
    .filter(|top| *top > 0.0)
    .unwrap_or(metrics.font_size * 0.5);
    let thickness = (metrics.font_size / 14.0).round().clamp(1.0, 2.0);

    DecorationMetrics {
        baseline,
        x_height,
        thickness,
    }
}

fn shape_cell_with_fs(
    font_system: &mut FontSystem,
    buffer: &mut Buffer,
    fonts: &FontSelection,
    emoji_family: Option<&str>,
    text: &str,
    style: CellStyle,
) -> Option<ShapedGlyph> {
    shape_text_with_fs(
        font_system,
        buffer,
        fonts,
        emoji_family,
        text,
        style,
        Shaping::Advanced,
    )
    .into_iter()
    .next()
}

fn shape_text_with_fs(
    font_system: &mut FontSystem,
    buffer: &mut Buffer,
    fonts: &FontSelection,
    emoji_family: Option<&str>,
    text: &str,
    style: CellStyle,
    shaping: Shaping,
) -> Vec<ShapedGlyph> {
    let (attrs, style) = cluster_attrs(text, style, fonts, emoji_family);
    buffer.set_text(text, &attrs, shaping, None);
    buffer.shape_until_scroll(font_system, false);

    let Some(run) = buffer.layout_runs().next() else {
        return Vec::new();
    };
    let line_y = run.line_y;
    let mut shaped = Vec::with_capacity(run.glyphs.len());
    let mut base_x = None;
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

impl TextRenderer {
    /// Create a new text renderer with configuration.
    pub fn with_config(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        font_config: &FontConfig,
    ) -> anyhow::Result<Self> {
        Self::with_fonts(
            device,
            queue,
            format,
            uniform_bind_group_layout,
            Fonts::load(font_config),
        )
    }

    /// Create a text renderer drawing with `fonts`, keeping their cell size.
    pub fn with_fonts(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        fonts: Fonts,
    ) -> anyhow::Result<Self> {
        let font_grids = FontGridSet::new(
            Arc::clone(&device),
            Arc::clone(&queue),
            fonts.shared_font_system(),
        );
        Self::with_fonts_and_grid_set(
            device,
            queue,
            format,
            uniform_bind_group_layout,
            font_grids,
            fonts,
        )
    }

    /// Create a text renderer backed by a shared [`FontGridSet`], reusing an
    /// existing [`FontGrid`] when `fonts`'s config already has one.
    pub fn with_fonts_and_grid_set(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        font_grids: FontGridSet,
        fonts: Fonts,
    ) -> anyhow::Result<Self> {
        let config = fonts.config.clone();
        let ligatures = fonts.ligatures;
        let grid = font_grids.get_or_create_from_fonts(&fonts);
        Self::from_grid(
            device,
            queue,
            format,
            uniform_bind_group_layout,
            font_grids,
            grid,
            config,
            ligatures,
        )
    }

    /// Create a text renderer backed by a shared [`FontGridSet`] for
    /// `font_config`, reusing any existing grid without re-measuring fonts.
    pub fn with_grid_set_and_config(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        font_grids: FontGridSet,
        font_config: &FontConfig,
    ) -> anyhow::Result<Self> {
        let config = normalize_font_config(font_config);
        let ligatures = config.ligatures;
        let grid = font_grids.get_or_create(&config);
        Self::from_grid(
            device,
            queue,
            format,
            uniform_bind_group_layout,
            font_grids,
            grid,
            config,
            ligatures,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_grid(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        font_grids: FontGridSet,
        grid: Arc<FontGrid>,
        config: FontConfig,
        ligatures: bool,
    ) -> anyhow::Result<Self> {
        let pipeline = TextPipeline::new(
            &device,
            format,
            uniform_bind_group_layout,
            font_grids.bind_group_layout(),
        )?;

        let atlas_bind_group = grid.state_lock().atlas.bind_group_arc();
        let styled_text_buffer = {
            let mut fs = font_grids.font_system().lock();
            Buffer::new(&mut fs, grid.metrics)
        };

        Ok(Self {
            cell_width: grid.cell_width,
            cell_height: grid.cell_height,
            font_size: grid.font_size,
            metrics: grid.metrics,
            decoration: grid.decoration,
            font_grids,
            grid,
            atlas_bind_group,
            config,
            pipeline,
            device,
            ligatures,
            font_generation: 0,
            run_text: String::new(),
            run_glyphs: Vec::new(),
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

    /// The per-device shared [`FontGridSet`] backing this text renderer.
    pub fn font_grids(&self) -> &FontGridSet {
        &self.font_grids
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

    fn attach_grid(&mut self, grid: Arc<FontGrid>) {
        self.atlas_bind_group = grid.state_lock().atlas.bind_group_arc();
        self.cell_width = grid.cell_width;
        self.cell_height = grid.cell_height;
        self.font_size = grid.font_size;
        self.metrics = grid.metrics;
        self.decoration = grid.decoration;
        self.styled_text_buffer.set_metrics(self.metrics);
        self.grid = grid;
        self.font_generation += 1;
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
    }

    /// Start a frame. Must precede any quad that samples the glyph atlas,
    /// because this is the only point at which atlas slots are evicted.
    pub fn begin_frame(&mut self) {
        {
            let mut state = self.grid.state_lock();
            state.atlas.begin_frame();
            self.atlas_bind_group = state.atlas.bind_group_arc();
        }
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
        self.grid.state_lock().atlas.frame_incomplete()
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
        self.config.ligatures = enabled;
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
    }

    /// Check if ligatures are enabled.
    pub fn ligatures(&self) -> bool {
        self.ligatures
    }

    /// Switch to the families, size, line height, features and emoji font
    /// of `config` without a restart. Switches this window onto the shared
    /// [`FontGrid`] for `config`, keeping other windows' grids untouched.
    /// Returns whether the cell size changed, in which case every grid needs
    /// resizing.
    pub fn apply_font_config(&mut self, config: &FontConfig) -> bool {
        let old_cell = (self.cell_width, self.cell_height);
        self.config = normalize_font_config(config);
        self.ligatures = self.config.ligatures;
        let grid = self.font_grids.get_or_create(&self.config);
        self.attach_grid(grid);
        (self.cell_width, self.cell_height) != old_cell
    }

    /// Get the current size in the selected units after display scaling.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Set the size in the selected units after display scaling and recalculate
    /// cell dimensions by switching this window onto the corresponding shared
    /// [`FontGrid`].
    pub fn set_font_size(&mut self, size: f32) {
        let size = crate::config::clamp_font_size(size);
        self.config.size = size;
        let grid = self.font_grids.get_or_create(&self.config);
        self.attach_grid(grid);
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

        let shaping = self.get_shaping();
        let font_system = self.font_grids.font_system();
        let mut state = self.grid.state_lock();
        let mut fs = font_system.lock();
        let mut buffer = Buffer::new(&mut fs, self.metrics);
        buffer.set_size(Some(10000.0), Some(self.cell_height * 2.0));
        buffer.set_text(text, &self.grid.fonts.attrs(false, false), shaping, None);
        buffer.shape_until_scroll(&mut fs, false);

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

                if let Some(glyph_info) = state.atlas.get_glyph(&mut fs, physical.cache_key) {
                    if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
                        continue;
                    }

                    let gx = (glyph_x + glyph_info.left).round();
                    let gy = (y + run.line_y + physical.y as f32 - glyph_info.top).round();

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
        let attrs = self.grid.fonts.attrs(bold, italic);
        let font_system = self.font_grids.font_system();
        let mut state = self.grid.state_lock();
        let mut fs = font_system.lock();

        self.styled_text_buffer
            .set_text(text, &attrs, shaping, None);
        self.styled_text_buffer.shape_until_scroll(&mut fs, false);

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

                if let Some(glyph_info) = state.atlas.get_glyph(&mut fs, physical.cache_key) {
                    if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
                        continue;
                    }

                    let gx = (glyph_x + glyph_info.left).round();
                    let gy = (y + run.line_y + physical.y as f32 - glyph_info.top).round();

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
            render_pass.set_bind_group(1, self.atlas_bind_group.as_ref(), &[]);
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
        let atlas_epoch = self.grid.state_lock().atlas.epoch();
        cache.begin(
            GridTextKey {
                scheme: color_scheme.clone(),
                cell_size: (cell_width, cell_height),
                origin: (x_offset, y_offset),
                atlas_epoch,
                ligatures: self.ligatures,
                font_generation: self.font_generation,
            },
            dims.rows,
        );
        let block_cursor = block_cursor_cell(terminal, cursor_blink_visible);
        let selection = terminal.selection();
        let grid = Arc::clone(&self.grid);
        let font_system = self.font_grids.font_system().clone();
        let mut grid_state: Option<MutexGuard<'_, FontGridState>> = None;
        let mut buffers: Option<(Buffer, Buffer)> = None;

        for row_idx in 0..dims.rows {
            let Some(row) = term_buffer.visible_row(row_idx) else {
                cache.store_empty(row_idx);
                continue;
            };
            let cursor_col = block_cursor
                .filter(|&(_, cursor_row)| cursor_row == row_idx)
                .map(|(col, _)| col);
            let selected = selection.and_then(|sel| selected_span(sel, row_idx, dims.cols));
            if cache.is_current(row_idx, &row, cursor_col, selected) {
                continue;
            }
            let state = grid_state.get_or_insert_with(|| grid.state_lock());
            let (buffer, run_buffer) = buffers.get_or_insert_with(|| {
                let mut fs = font_system.lock();
                shaping_buffers(&mut fs, self.metrics, cell_width, cell_height, dims.cols)
            });
            let mut vertices = cache.take_vertices(row_idx);
            self.build_row_vertices(
                &grid,
                state,
                &font_system,
                &mut vertices,
                buffer,
                run_buffer,
                &row,
                row_idx,
                cursor_col,
                selected,
                color_scheme,
                (cell_width, cell_height),
                (x_offset, y_offset),
                dims.cols,
            );
            debug_assert_eq!(vertices.len() % 4, 0, "grid geometry is quads");
            cache.store(row_idx, &row, cursor_col, selected, vertices);
        }

        self.frame_quads = cache.vertices().map(<[Vertex]>::len).sum::<usize>() / 4;
        self.grid_caches[draw] = cache;
        draw
    }

    /// Append the glyph and decoration quads of screen row `row_idx`.
    #[allow(clippy::too_many_arguments)]
    fn build_row_vertices(
        &mut self,
        grid: &FontGrid,
        state: &mut FontGridState,
        font_system: &SharedFontSystem,
        vertices: &mut Vec<Vertex>,
        buffer: &mut Buffer,
        run_buffer: &mut Buffer,
        row: &RowView<'_>,
        row_idx: u16,
        cursor_col: Option<u16>,
        selected: Option<(u16, u16)>,
        color_scheme: &ColorScheme,
        (cell_width, cell_height): (f32, f32),
        (x_offset, y_offset): (f32, f32),
        cols: u16,
    ) {
        let block_cursor = cursor_col.map(|col| (col, row_idx));
        let solid_uv = state.atlas.solid_uv();
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
                text_color(&cell, color_scheme, is_selected(selected, col_idx))
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
                let mut end = ligature_run_end(row, col_idx, cols, &grid.fonts);
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
                        self.run_glyphs.clear();
                        if let Some(cached) = state.run_cache.get(style, &self.run_text) {
                            self.run_glyphs.extend_from_slice(cached);
                        } else {
                            let glyphs = {
                                let mut fs = font_system.lock();
                                shape_run_glyphs(
                                    &mut fs,
                                    run_buffer,
                                    &grid.fonts,
                                    &self.run_text,
                                    style,
                                )
                            };
                            self.run_glyphs.extend_from_slice(&glyphs);
                            state.run_cache.insert(style, self.run_text.clone(), glyphs);
                        }
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
                                text_color(&glyph_cell, color_scheme, is_selected(selected, col))
                            };
                            push_glyph_from_atlas(
                                &mut state.atlas,
                                font_system,
                                vertices,
                                shaped,
                                (col, row_idx),
                                (cell_width, cell_height),
                                (x_offset, y_offset),
                                color,
                                color_scheme.background,
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
                cluster_glyphs.clear();
                if let Some(cached) = state.cluster_cache.get(style, &self.cluster_text) {
                    cluster_glyphs.extend_from_slice(cached);
                } else {
                    let shaped = {
                        let mut fs = font_system.lock();
                        shape_text_with_fs(
                            &mut fs,
                            buffer,
                            &grid.fonts,
                            grid.emoji_family.as_deref(),
                            &self.cluster_text,
                            style,
                            Shaping::Advanced,
                        )
                    };
                    cluster_glyphs.extend_from_slice(&shaped);
                    state
                        .cluster_cache
                        .insert(style, self.cluster_text.clone(), shaped);
                }
                if cluster_glyphs.iter().any(|glyph| glyph.unpositioned_mark) {
                    cluster_inks.clear();
                    for glyph in &cluster_glyphs {
                        let ink = state.atlas.get_glyph_with(glyph.cache_key, |swash, key| {
                            swash.get_image_uncached(&mut font_system.lock(), key)
                        });
                        cluster_inks.push(ink);
                    }
                    stack_unpositioned_marks(&mut cluster_glyphs, &cluster_inks);
                }
                &cluster_glyphs
            } else {
                let cache_key_lookup = shaping_cache_key(ch, self.ligatures, style);
                single_glyph = match state.shaping_cache.get(&cache_key_lookup) {
                    Some(&cached) => Some(cached),
                    None => {
                        let mut ch_buf = [0u8; 4];
                        let ch_str = ch.encode_utf8(&mut ch_buf);
                        let shaped = {
                            let mut fs = font_system.lock();
                            shape_cell_with_fs(
                                &mut fs,
                                buffer,
                                &grid.fonts,
                                grid.emoji_family.as_deref(),
                                ch_str,
                                style,
                            )
                        };
                        if let Some(shaped) = shaped {
                            state.shaping_cache.insert(cache_key_lookup, shaped);
                        }
                        shaped
                    }
                };
                single_glyph.as_slice()
            };

            for &shaped in glyphs {
                push_glyph_from_atlas(
                    &mut state.atlas,
                    font_system,
                    vertices,
                    shaped,
                    (col_idx, row_idx),
                    (cell_width, cell_height),
                    (x_offset, y_offset),
                    base_color,
                    color_scheme.background,
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

    /// Append `text` laid out on cells, as the grid is, from `(x, y)` (the
    /// top-left of its first cell), at most `max_cells` cells wide. Each
    /// character is drawn in `color(index)`, its index among `text`'s
    /// characters. Returns the cells used.
    ///
    /// Characters are shaped one at a time through the grid's per-character
    /// shaping cache, so drawing the same text again shapes nothing. For
    /// chrome that redraws every frame while it is shown (the command
    /// palette); a character of no width (a combining mark) is skipped.
    #[allow(clippy::too_many_arguments)]
    pub fn append_cell_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        max_cells: usize,
        mut color: impl FnMut(usize) -> Rgba,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
    ) -> usize {
        let style = CellStyle::default();
        let font_system = self.font_grids.font_system();
        let mut state = self.grid.state_lock();
        let mut buffer: Option<Buffer> = None;
        let mut col = 0usize;
        for (index, ch) in text.chars().enumerate() {
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if width == 0 {
                continue;
            }
            if col + width > max_cells {
                break;
            }
            if ch != ' ' {
                let key = shaping_cache_key(ch, self.ligatures, style);
                let shaped = match state.shaping_cache.get(&key) {
                    Some(&cached) => Some(cached),
                    None => {
                        let mut fs = font_system.lock();
                        let buffer = buffer.get_or_insert_with(|| {
                            let mut buffer = Buffer::new(&mut fs, self.metrics);
                            buffer.set_size(
                                Some(self.cell_width * 4.0),
                                Some(self.cell_height * 2.0),
                            );
                            buffer
                        });
                        let mut ch_buf = [0u8; 4];
                        let shaped = shape_cell_with_fs(
                            &mut fs,
                            buffer,
                            &self.grid.fonts,
                            self.grid.emoji_family.as_deref(),
                            ch.encode_utf8(&mut ch_buf),
                            style,
                        );
                        if let Some(shaped) = shaped {
                            state.shaping_cache.insert(key, shaped);
                        }
                        shaped
                    }
                };
                if let Some(shaped) = shaped {
                    if let Some(glyph) = state.atlas.get_glyph_with(shaped.cache_key, |swash, k| {
                        swash.get_image_uncached(&mut font_system.lock(), k)
                    }) {
                        if glyph.width > 0.0 && glyph.height > 0.0 {
                            let cell = (u16::try_from(col).unwrap_or(u16::MAX), 0);
                            let (gx, gy) = cell_glyph_origin(
                                cell,
                                (self.cell_width, self.cell_height),
                                (x, y),
                                shaped,
                                &glyph,
                            );
                            Self::emit_glyph_quad(vertices, indices, gx, gy, &glyph, color(index));
                        }
                    }
                }
            }
            col += width;
        }
        col
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

        let shaping = self.get_shaping();
        let font_system = self.font_grids.font_system();
        let mut state = self.grid.state_lock();
        let mut fs = font_system.lock();
        let mut buffer = Buffer::new(&mut fs, self.metrics);
        // Set buffer size to allow layout
        buffer.set_size(Some(max_width), Some(self.cell_height * 2.0));
        buffer.set_text(text, &self.grid.fonts.attrs(false, false), shaping, None);
        buffer.shape_until_scroll(&mut fs, false);

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
                        render_char_at_with(
                            &mut fs,
                            &mut state.atlas,
                            &self.grid.fonts,
                            self.metrics,
                            self.cell_width,
                            self.cell_height,
                            shaping,
                            &mut vertices,
                            &mut indices,
                            '…',
                            ellipsis_x,
                            y,
                            color,
                        );
                    }
                    break;
                }

                let physical = glyph.physical((0.0, 0.0), 1.0);

                if let Some(glyph_info) = state.atlas.get_glyph(&mut fs, physical.cache_key) {
                    if glyph_info.width > 0.0 && glyph_info.height > 0.0 {
                        let gx = (x + glyph_info.left + physical.x as f32).round();
                        let gy = (y + run.line_y + physical.y as f32 - glyph_info.top).round();

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

    /// The glyph atlas epoch; see [`super::atlas::GlyphAtlas::epoch`].
    pub fn atlas_epoch(&self) -> u64 {
        self.grid.state_lock().atlas.epoch()
    }

    /// Get the atlas bind group for rendering labels.
    pub fn atlas_bind_group(&self) -> &wgpu::BindGroup {
        &self.atlas_bind_group
    }

    /// Get the text pipeline for rendering labels.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        self.pipeline.pipeline()
    }

    /// Clear the glyph cache.
    pub fn clear_cache(&mut self) {
        {
            let mut state = self.grid.state_lock();
            state.atlas.clear();
            self.atlas_bind_group = state.atlas.bind_group_arc();
        }
        self.grid_caches.iter_mut().for_each(GridTextCache::clear);
    }

    /// Get cache statistics, including the active atlas count in the shared
    /// [`FontGridSet`].
    pub fn cache_stats(&self) -> super::atlas::AtlasStats {
        let mut stats = self.grid.state_lock().atlas.stats();
        stats.atlas_count = self.font_grids.active_grid_count();
        stats
    }
}

fn shaping_buffers(
    font_system: &mut FontSystem,
    metrics: Metrics,
    cell_width: f32,
    cell_height: f32,
    cols: u16,
) -> (Buffer, Buffer) {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(cell_width * 4.0), Some(cell_height * 2.0));
    let mut run_buffer = Buffer::new(font_system, metrics);
    run_buffer.set_wrap(cosmic_text::Wrap::None);
    run_buffer.set_size(
        Some(cell_width * (f32::from(cols) + 1.0)),
        Some(cell_height * 2.0),
    );
    (buffer, run_buffer)
}

#[allow(clippy::too_many_arguments)]
fn push_glyph_from_atlas(
    atlas: &mut GlyphAtlas,
    font_system: &SharedFontSystem,
    vertices: &mut Vec<Vertex>,
    shaped: ShapedGlyph,
    cell: (u16, u16),
    cell_size: (f32, f32),
    render_offset: (f32, f32),
    color: Rgba,
    background: Rgba,
    under_block_cursor: bool,
) {
    let Some(glyph_info) = atlas.get_glyph_with(shaped.cache_key, |swash, key| {
        swash.get_image_uncached(&mut font_system.lock(), key)
    }) else {
        return;
    };
    if glyph_info.width == 0.0 || glyph_info.height == 0.0 {
        return;
    }
    let (x, y) = cell_glyph_origin(cell, cell_size, render_offset, shaped, &glyph_info);
    // Under the cursor the color is already the inverted background;
    // emphasizing it would reduce contrast.
    let color = if shaped.synthetic_bold && !under_block_cursor {
        emphasize(color, background)
    } else {
        color
    };
    TextRenderer::push_glyph_vertices(vertices, x, y, &glyph_info, color);
}

#[allow(clippy::too_many_arguments)]
fn render_char_at_with(
    font_system: &mut FontSystem,
    atlas: &mut GlyphAtlas,
    fonts: &FontSelection,
    metrics: Metrics,
    cell_width: f32,
    cell_height: f32,
    shaping: Shaping,
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    ch: char,
    x: f32,
    y: f32,
    color: Rgba,
) {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(cell_width * 4.0), Some(cell_height * 2.0));
    let mut ch_buf = [0u8; 4];
    buffer.set_text(
        ch.encode_utf8(&mut ch_buf),
        &fonts.attrs(false, false),
        shaping,
        None,
    );
    buffer.shape_until_scroll(font_system, false);

    if let Some(run) = buffer.layout_runs().next() {
        if let Some(glyph) = run.glyphs.first() {
            let physical = glyph.physical((0.0, 0.0), 1.0);
            if let Some(glyph_info) = atlas.get_glyph(font_system, physical.cache_key) {
                if glyph_info.width > 0.0 && glyph_info.height > 0.0 {
                    let gx = (x + glyph_info.left + physical.x as f32).round();
                    let gy = (y + run.line_y + physical.y as f32 - glyph_info.top).round();
                    TextRenderer::emit_glyph_quad(vertices, indices, gx, gy, &glyph_info, color);
                }
            }
        }
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
    fn test_native_text_matches_fresh_glyphs_after_font_changes() {
        let Ok(mut renderer) = pollster::block_on(super::super::Renderer::new_headless(400, 200))
        else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let text = &mut renderer.text_renderer;

        fn assert_matching_glyphs(text: &mut TextRenderer) {
            let color = Rgba::new(1.0, 1.0, 1.0, 1.0);
            // The simple path shapes a fresh buffer, independently of the
            // buffer reused by native labels, rich text and editable text.
            let (fresh, _) = text.build_simple_text_vertices("MM", 0.0, 0.0, color);
            assert_eq!(fresh.len(), 8, "two visible glyph quads must be shaped");
            let expected: Vec<_> = fresh.iter().map(|v| v.position).collect();
            let (styled, _, _) =
                text.build_styled_text_vertices("MM", 0.0, 0.0, color, false, false);
            let (cells, _, _) = text.build_cell_text_vertices("MM", 0.0, 0.0, color, false, false);
            for (path, vertices) in [("styled", styled), ("cell", cells)] {
                let positions: Vec<_> = vertices.iter().map(|v| v.position).collect();
                assert_eq!(
                    positions,
                    expected,
                    "{path} glyph size and cell pitch must agree at {}px",
                    text.font_size()
                );
            }
        }

        assert_matching_glyphs(text);
        text.set_font_size(28.0);
        assert_matching_glyphs(text);
        text.apply_font_config(&FontConfig {
            size: 17.5,
            line_height: 1.25,
            ..FontConfig::default()
        });
        assert_matching_glyphs(text);
        text.set_font_size(14.0);
        assert_matching_glyphs(text);
    }

    #[test]
    fn test_native_text_matches_grid_baseline_for_underscores() {
        let Ok(mut renderer) = pollster::block_on(super::super::Renderer::new_headless(400, 200))
        else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let text = &mut renderer.text_renderer;
        let color = Rgba::new(1.0, 1.0, 1.0, 1.0);
        let y = 24.0;

        for size in [11.0, 14.0] {
            text.set_font_size(size);

            let mut grid_verts = Vec::new();
            let mut grid_idxs = Vec::new();
            text.append_cell_text(
                "ascii_tabs",
                0.0,
                y,
                20,
                |_| color,
                &mut grid_verts,
                &mut grid_idxs,
            );
            let grid_ys: Vec<f32> = grid_verts.iter().map(|v| v.position[1]).collect();
            assert_eq!(
                grid_verts.len(),
                10 * 4,
                "every glyph in ascii_tabs must emit at {size}px"
            );

            let (styled, _, _) =
                text.build_styled_text_vertices("ascii_tabs", 0.0, y, color, false, false);
            let (cells, _, _) =
                text.build_cell_text_vertices("ascii_tabs", 0.0, y, color, false, false);
            let (simple, _) = text.build_simple_text_vertices("ascii_tabs", 0.0, y, color);
            let (label, _) = text.build_label_vertices("ascii_tabs", 0.0, y, 400.0, color);

            for (name, verts) in [
                ("styled", &styled),
                ("cell", &cells),
                ("simple", &simple),
                ("label", &label),
            ] {
                let ys: Vec<f32> = verts.iter().map(|v| v.position[1]).collect();
                assert_eq!(
                    ys, grid_ys,
                    "{name} text baseline must match grid text at {size}px"
                );
            }
        }
    }

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
        // Regular face matched: emphasize and skew.
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
    fn emphasize_brightens_on_a_dark_background_without_overflow() {
        let dark = Rgba::rgb(0.1, 0.1, 0.12);
        let dim = Rgba::new(0.2, 0.4, 0.0, 0.5);
        let bright = emphasize(dim, dark);
        assert!(bright.r > dim.r && bright.g > dim.g && bright.b > dim.b);
        assert_eq!(bright.a, dim.a);

        let white = emphasize(Rgba::rgb(1.0, 1.0, 1.0), dark);
        assert_eq!(white, Rgba::rgb(1.0, 1.0, 1.0));
    }

    #[test]
    fn emphasize_darkens_on_a_light_background() {
        // Brightening dark text on a light theme made synthetic bold paler
        // than plain text.
        let light = Rgba::rgb(0.98, 0.98, 0.98);
        let text = Rgba::rgb(0.2, 0.2, 0.2);
        let bold = emphasize(text, light);
        assert!(bold.r < text.r && bold.g < text.g && bold.b < text.b);

        let black = emphasize(Rgba::rgb(0.0, 0.0, 0.0), light);
        assert_eq!(black, Rgba::rgb(0.0, 0.0, 0.0));
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
        let points = |line_height_multiplier, line_height_ratio| FontMetrics {
            pixels_per_unit: POINTS_TO_PIXELS,
            line_height_ratio,
            line_height_multiplier,
        };
        assert_eq!(points(1.0, 1.0).at_size(12.0).font_size, 16.0);
        assert_eq!(points(1.0, 1.0).at_size(12.0).line_height, 16.0);
        assert_eq!(points(1.5, 1.0).at_size(12.0).line_height, 24.0);
        // Rounded up to whole pixels.
        assert_eq!(points(1.2, 1.0).at_size(12.0).line_height, 20.0);

        let dejavu_ratio = (1901.0 + 483.0) / 2048.0;
        let m = points(1.0, dejavu_ratio).at_size(11.0);
        assert!((m.font_size - 11.0 * 96.0 / 72.0).abs() < 1e-4);
        assert_eq!(m.line_height, 18.0);
    }

    #[test]
    fn test_legacy_metrics_use_scaled_pixels_and_em_line_height() {
        for (scale, expected_height) in [(1.0, 21.0), (1.5, 32.0)] {
            let config = FontConfig {
                metrics: FontMetricsMode::Legacy,
                size: 16.0 * scale,
                line_height: 1.3,
                ..FontConfig::default()
            };
            let fonts = Fonts::load(&config);

            assert_eq!(fonts.metrics().font_size, config.size);
            assert_eq!(fonts.cell_dimensions().1, expected_height);
        }
    }

    #[test]
    fn dejavu_sans_mono_11pt_matches_96_dpi_cell_size() {
        let fonts = Fonts::load(&FontConfig {
            family: "DejaVu Sans Mono".to_string(),
            size: 11.0,
            line_height: 1.0,
            ..FontConfig::default()
        });
        if fonts.family().eq_ignore_ascii_case("DejaVu Sans Mono") {
            let (cw, ch) = fonts.cell_dimensions();
            assert_eq!(cw, 9.0);
            assert!((17.0..=18.0).contains(&ch), "unexpected cell height {ch}");
            assert_eq!(fonts.font_size(), 11.0);
        }
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
            ligature_run_end(&RowView::from(&row), 0, row.width(), &selection("Mono")),
            4,
            "stops at the space"
        );
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 4, row.width(), &selection("Mono")),
            4,
            "a space starts no run"
        );

        let mut row = row_of("x!=y");
        row[2].set_flag(CellFlags::BOLD);
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 0, row.width(), &selection("Mono")),
            2,
            "bold changes the face"
        );

        let mut row = row_of("->中");
        row[2].set_flag(CellFlags::WIDE_CHAR);
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 0, row.width(), &selection("Mono")),
            2,
            "non-ASCII ends the run"
        );

        // A foreground change ends a run, so a ligature never recolors
        // cells the program colored differently. Background does not.
        let mut row = row_of("==>");
        row[1].set_fg(crate::grid::Color::Named(2));
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 0, row.width(), &selection("Mono")),
            1
        );
        let mut row = row_of("==>");
        row[1].set_bg(crate::grid::Color::Named(2));
        assert_eq!(
            ligature_run_end(&RowView::from(&row), 0, row.width(), &selection("Mono")),
            3
        );
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
        // Every family counts as installed: these tests are about what is
        // asked of the font system, not about what is on this machine.
        FontSelection::resolve(&cosmic_text::fontdb::Database::new(), config, &|_| true)
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
    fn a_missing_family_resolves_to_the_system_monospace_font() {
        let config = FontConfig {
            family: "No Such Mono".to_string(),
            bold_family: "Present Bold".to_string(),
            codepoint_map: vec![crate::config::CodepointMapEntry {
                range: "U+E000-U+F8FF".to_string(),
                family: "No Such Symbols".to_string(),
            }],
            ..FontConfig::default()
        };
        let db = cosmic_text::fontdb::Database::new();
        let only_bold = |family: &str| family.eq_ignore_ascii_case("present bold");
        let fonts = FontSelection::resolve(&db, &config, &only_bold);
        assert_eq!(fonts.families[0], "", "regular family is replaced");
        assert_eq!(fonts.families[1], "Present Bold", "an installed one stays");
        assert_eq!(fonts.families[2], "", "italic follows the missing regular");
        assert_eq!(fonts.codepoints[0].family, "");
        assert_eq!(fonts.attrs(false, false).family, Family::Monospace);
    }

    #[test]
    fn a_missing_family_draws_a_uniform_advance_grid() {
        // The default family is missing on a fresh machine. The grid must
        // still be monospace: narrow and wide glyphs advance the same.
        let mut font_system = FontSystem::new();
        if !font_system.db().faces().any(|face| face.monospaced) {
            eprintln!("no monospace font installed; skipped");
            return;
        }
        let missing = "Ultty Test Family That Is Not Installed";
        assert!(!family_installed(font_system.db(), missing));
        let fonts = FontSelection::new(
            font_system.db(),
            &FontConfig {
                family: missing.to_string(),
                ..FontConfig::default()
            },
        );
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        let mut advances = Vec::new();
        for text in ["i", "l", ".", "M", "W", "m"] {
            buffer.set_text(text, &fonts.attrs(false, false), Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut font_system, false);
            let run = buffer.layout_runs().next().expect("a layout run");
            let glyph = run.glyphs.first().expect("a glyph");
            assert!(
                font_system.is_monospace(glyph.font_id),
                "{text:?} was drawn from a proportional face"
            );
            advances.push(glyph.w);
        }
        assert!(
            advances
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 0.01),
            "advances differ: {advances:?}"
        );
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

    fn mapped_config() -> FontConfig {
        config_with(|c| {
            c.codepoint_map = vec![
                crate::config::CodepointMapEntry {
                    range: "U+E000-U+F8FF".to_string(),
                    family: "Symbols".to_string(),
                },
                crate::config::CodepointMapEntry {
                    range: "U+E0A0".to_string(),
                    family: "Powerline Extra".to_string(),
                },
                crate::config::CodepointMapEntry {
                    range: "U+3D".to_string(),
                    family: "Serif".to_string(),
                },
            ];
        })
    }

    #[test]
    fn mapped_codepoints_use_their_family_and_the_last_entry_wins() {
        let fonts = selection_of(&mapped_config());
        let family = |text: &str| {
            cluster_attrs(text, CellStyle::default(), &fonts, None)
                .0
                .family
        };
        assert_eq!(family("\u{E001}"), Family::Name("Symbols"));
        assert_eq!(family("\u{E0A0}"), Family::Name("Powerline Extra"));
        assert_eq!(family("a"), Family::Name("Mono"));
        assert_eq!(family("\u{F8FF}"), Family::Name("Symbols"));
        assert_eq!(family("\u{F900}"), Family::Name("Mono"));
        // The mapping beats the emoji family.
        let (attrs, _) = cluster_attrs(
            "\u{E001}",
            CellStyle::default(),
            &fonts,
            Some("Noto Color Emoji"),
        );
        assert_eq!(attrs.family, Family::Name("Symbols"));
    }

    #[test]
    fn mapped_codepoints_keep_the_cells_style_and_features() {
        let mut config = mapped_config();
        config.features = vec!["-liga".to_string()];
        let fonts = selection_of(&config);
        let bold = CellStyle {
            bold: true,
            italic: false,
        };
        let (attrs, style) = cluster_attrs("\u{E001}", bold, &fonts, None);
        assert_eq!(attrs.weight, Weight::BOLD);
        assert_eq!(style, bold);
        assert_eq!(attrs.font_features.features.len(), 1);
    }

    #[test]
    fn mapped_ascii_does_not_join_ligature_runs() {
        let fonts = selection_of(&mapped_config());
        let row = row_of("a=>b");
        let end =
            |fonts: &FontSelection| ligature_run_end(&RowView::from(&row), 0, row.width(), fonts);
        assert_eq!(end(&selection("Mono")), 4);
        // `=` is mapped to another family: the run stops before it.
        assert_eq!(end(&fonts), 1);
    }

    #[test]
    fn font_grid_key_normalizes_size_and_line_height_and_ignores_ligatures() {
        let base = FontConfig::default();
        let mut with_no_ligs = base.clone();
        with_no_ligs.ligatures = !base.ligatures;
        assert_eq!(
            FontGridKey::from_config(&base),
            FontGridKey::from_config(&with_no_ligs)
        );

        let mut zoomed = base.clone();
        zoomed.size = base.size + 2.0;
        assert_ne!(
            FontGridKey::from_config(&base),
            FontGridKey::from_config(&zoomed)
        );

        let mut huge_a = base.clone();
        huge_a.size = 500.0;
        let mut huge_b = base.clone();
        huge_b.size = 900.0;
        assert_eq!(
            FontGridKey::from_config(&huge_a),
            FontGridKey::from_config(&huge_b),
            "clamped font sizes map to the same FontGridKey"
        );
    }

    #[test]
    fn same_scale_windows_share_one_atlas_and_zoom_is_per_window() {
        let Ok(renderer) = pollster::block_on(super::super::Renderer::new_headless(400, 200))
        else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let device = Arc::clone(renderer.device());
        let queue = Arc::clone(renderer.queue());
        let font_grids = renderer.font_grids().clone();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("test_uniform_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let config = FontConfig::default();
        let mut win1 = TextRenderer::with_grid_set_and_config(
            Arc::clone(&device),
            Arc::clone(&queue),
            format,
            &uniform_layout,
            font_grids.clone(),
            &config,
        )
        .unwrap();
        let mut win2 = TextRenderer::with_grid_set_and_config(
            Arc::clone(&device),
            Arc::clone(&queue),
            format,
            &uniform_layout,
            font_grids.clone(),
            &config,
        )
        .unwrap();

        assert!(
            Arc::ptr_eq(&win1.grid, &win2.grid),
            "same-scale windows must share one FontGrid"
        );
        assert_eq!(
            win1.cache_stats().atlas_count,
            1,
            "atlas count stays 1 for two same-scale windows"
        );
        assert_eq!(win2.cache_stats().atlas_count, 1);

        // Rasterizing glyphs in window 1 populates the shared atlas for window 2.
        let color = Rgba::new(1.0, 1.0, 1.0, 1.0);
        let _ = win1.build_simple_text_vertices("SharedAtlasXYZ", 0.0, 0.0, color);
        let hits_before = win2.cache_stats().cache_hits;
        let misses_before = win2.cache_stats().cache_misses;
        let _ = win2.build_simple_text_vertices("SharedAtlasXYZ", 0.0, 0.0, color);
        let stats_after = win2.cache_stats();
        assert!(
            stats_after.cache_hits > hits_before,
            "window 2 should hit glyphs already rasterized by window 1"
        );
        assert_eq!(
            stats_after.cache_misses, misses_before,
            "window 2 should incur no new atlas misses for already-cached glyphs"
        );

        // Zooming window 2 gives it its own FontGrid without changing window 1.
        let win1_cell_before = win1.cell_dimensions();
        win2.set_font_size(config.size * 2.0);
        assert!(!Arc::ptr_eq(&win1.grid, &win2.grid));
        assert_eq!(win1.cell_dimensions(), win1_cell_before);
        assert!(win2.cell_dimensions().0 > win1_cell_before.0);
        assert_eq!(win1.cache_stats().atlas_count, 2);
        assert_eq!(win2.cache_stats().atlas_count, 2);

        // Resetting window 2's zoom re-attaches to window 1's grid and drops
        // the zoomed atlas automatically.
        win2.set_font_size(config.size);
        assert!(Arc::ptr_eq(&win1.grid, &win2.grid));
        assert_eq!(win1.cache_stats().atlas_count, 1);

        // Dropping a zoomed window also frees its FontGrid.
        win2.set_font_size(config.size * 1.5);
        assert_eq!(win1.cache_stats().atlas_count, 2);
        drop(win2);
        assert_eq!(win1.cache_stats().atlas_count, 1);
    }
}
