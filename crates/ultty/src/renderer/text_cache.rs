//! Per-row cache of the terminal grid's text vertices.
//!
//! Building a row's glyph and decoration quads means shaping, atlas lookups
//! and color resolution for every cell. Most frames change a row or two (an
//! echoed key, a ticking status line), so each draw keeps the vertices it
//! built per row and rebuilds only rows whose inputs changed.
//!
//! A row's vertices are reused only when the row's cells and graphemes
//! equal the ones they were built from, and nothing else they depend on
//! changed. Comparing content rather than trusting dirty flags or version
//! counters keeps this correct across everything that swaps what a screen
//! row shows without the grid noticing: scrolling back through history, the
//! alternate screen, and panes that share one renderer.

use super::{ColorScheme, Vertex};
use crate::grid::{Row, RowView};

/// Everything outside a row's own content that its vertices depend on.
/// When any of it changes, every cached row of the draw is stale.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GridTextKey {
    pub scheme: ColorScheme,
    pub cell_size: (f32, f32),
    pub origin: (f32, f32),
    /// [`super::atlas::GlyphAtlas::epoch`]: glyph UVs are only valid within
    /// one epoch.
    pub atlas_epoch: u64,
    pub ligatures: bool,
    /// [`super::text::TextRenderer`]'s font generation: changes with the
    /// families, size and features the vertices were shaped with.
    pub font_generation: u64,
}

/// The cached text vertices of one draw (one terminal or pane).
#[derive(Default)]
pub(super) struct GridTextCache {
    key: Option<GridTextKey>,
    rows: Vec<CachedRow>,
}

#[derive(Default)]
struct CachedRow {
    /// The content the vertices were built from; `None` until built.
    content: Option<Row>,
    /// Column of the block cursor on this row, whose glyph is drawn in an
    /// inverted color.
    cursor_col: Option<u16>,
    vertices: Vec<Vertex>,
}

impl CachedRow {
    fn is_built_from(&self, row: &RowView<'_>, cursor_col: Option<u16>) -> bool {
        self.cursor_col == cursor_col
            && self
                .content
                .as_ref()
                .is_some_and(|content| row.eq_row(content))
    }
}

impl GridTextCache {
    /// Start a draw of `rows` rows with `key`; a changed key drops every
    /// cached row.
    pub fn begin(&mut self, key: GridTextKey, rows: u16) {
        if self.key.as_ref() != Some(&key) {
            self.rows.iter_mut().for_each(|row| row.content = None);
            self.key = Some(key);
        }
        self.rows.resize_with(rows as usize, CachedRow::default);
    }

    /// Forget every row, e.g. after a font change.
    pub fn clear(&mut self) {
        self.key = None;
        self.rows.clear();
    }

    /// Whether row `index`'s cached vertices were built from `row` with the
    /// block cursor at `cursor_col`.
    pub fn is_current(&self, index: u16, row: &RowView<'_>, cursor_col: Option<u16>) -> bool {
        self.rows
            .get(index as usize)
            .is_some_and(|cached| cached.is_built_from(row, cursor_col))
    }

    /// The vertex buffer of row `index`, emptied, to rebuild into and hand
    /// back through [`Self::store`]. Keeps its allocation.
    pub fn take_vertices(&mut self, index: u16) -> Vec<Vertex> {
        let mut vertices = self
            .rows
            .get_mut(index as usize)
            .map(|cached| std::mem::take(&mut cached.vertices))
            .unwrap_or_default();
        vertices.clear();
        vertices
    }

    /// Record that `vertices` were built for row `index` from `row`.
    pub fn store(
        &mut self,
        index: u16,
        row: &RowView<'_>,
        cursor_col: Option<u16>,
        vertices: Vec<Vertex>,
    ) {
        let Some(cached) = self.rows.get_mut(index as usize) else {
            return;
        };
        match &mut cached.content {
            Some(content) => row.copy_into(content),
            None => cached.content = Some(row.to_row()),
        }
        cached.cursor_col = cursor_col;
        cached.vertices = vertices;
    }

    /// Mark row `index` as having nothing to draw (no such row this frame).
    pub fn store_empty(&mut self, index: u16) {
        if let Some(cached) = self.rows.get_mut(index as usize) {
            cached.content = None;
            cached.vertices.clear();
        }
    }

    /// Every row's vertices, top to bottom.
    pub fn vertices(&self) -> impl Iterator<Item = &[Vertex]> {
        self.rows.iter().map(|row| row.vertices.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Cell, Color};

    fn key() -> GridTextKey {
        GridTextKey {
            scheme: ColorScheme::dark(),
            cell_size: (10.0, 20.0),
            origin: (0.0, 0.0),
            atlas_epoch: 0,
            ligatures: true,
            font_generation: 0,
        }
    }

    fn row_of(text: &str) -> Row {
        let mut row = Row::new(8);
        row.write_ascii(0, text.as_bytes(), Cell::default());
        row
    }

    fn vertex() -> Vertex {
        Vertex {
            position: [1.0, 2.0],
            tex_coord: [0.0, 0.0],
            color: [1.0; 4],
            is_color: 0,
        }
    }

    /// A cache with row 0 built from `row`, the cursor elsewhere.
    fn cache_with(row: &Row) -> GridTextCache {
        let mut cache = GridTextCache::default();
        cache.begin(key(), 2);
        cache.store(0, &RowView::from(row), None, vec![vertex()]);
        cache
    }

    #[test]
    fn an_unchanged_row_is_reused() {
        let row = row_of("ls -la");
        let mut cache = cache_with(&row);
        cache.begin(key(), 2);
        assert!(cache.is_current(0, &RowView::from(&row), None));
        let rows: Vec<usize> = cache.vertices().map(<[Vertex]>::len).collect();
        assert_eq!(rows, [1, 0]);
    }

    #[test]
    fn rows_never_built_are_not_current() {
        let row = row_of("ls");
        let cache = cache_with(&row);
        assert!(!cache.is_current(1, &RowView::from(&row), None));
        assert!(!cache.is_current(5, &RowView::from(&row), None));
    }

    #[test]
    fn a_changed_character_or_attribute_rebuilds_the_row() {
        let row = row_of("ls -la");
        let cache = cache_with(&row);

        assert!(!cache.is_current(0, &RowView::from(&row_of("ls -lb")), None));

        let mut recolored = row.clone();
        recolored.get_mut(1).unwrap().set_fg(Color::RED);
        assert!(!cache.is_current(0, &RowView::from(&recolored), None));
    }

    #[test]
    fn a_changed_grapheme_rebuilds_the_row() {
        let mut row = row_of("e");
        row.set_grapheme_extra(0, &['\u{301}']);
        let cache = cache_with(&row);
        assert!(cache.is_current(0, &RowView::from(&row), None));

        let mut other = row_of("e");
        other.set_grapheme_extra(0, &['\u{300}']);
        assert!(!cache.is_current(0, &RowView::from(&other), None));
    }

    #[test]
    fn the_block_cursor_entering_or_leaving_rebuilds_the_row() {
        let row = row_of("ls");
        let cache = cache_with(&row);
        assert!(!cache.is_current(0, &RowView::from(&row), Some(1)));

        let mut cache = GridTextCache::default();
        cache.begin(key(), 1);
        cache.store(0, &RowView::from(&row), Some(1), Vec::new());
        assert!(!cache.is_current(0, &RowView::from(&row), None));
        assert!(!cache.is_current(0, &RowView::from(&row), Some(2)));
        assert!(cache.is_current(0, &RowView::from(&row), Some(1)));
    }

    #[test]
    fn a_changed_key_drops_every_row() {
        let row = row_of("ls");
        let changes: [fn(&mut GridTextKey); 6] = [
            |k| k.scheme = k.scheme.for_screen(true).into_owned(),
            |k| k.cell_size.0 += 1.0,
            |k| k.origin.1 += 20.0,
            |k| k.atlas_epoch += 1,
            |k| k.ligatures = false,
            |k| k.font_generation += 1,
        ];
        for change in changes {
            let mut cache = cache_with(&row);
            let mut changed = key();
            change(&mut changed);
            cache.begin(changed, 2);
            assert!(!cache.is_current(0, &RowView::from(&row), None));
        }
    }

    #[test]
    fn taking_vertices_for_a_rebuild_empties_them() {
        let row = row_of("ls");
        let mut cache = cache_with(&row);
        assert!(cache.take_vertices(0).is_empty());
        assert!(cache.take_vertices(9).is_empty());
    }

    #[test]
    fn an_empty_row_draws_nothing() {
        let row = row_of("ls");
        let mut cache = cache_with(&row);
        cache.store_empty(0);
        assert!(!cache.is_current(0, &RowView::from(&row), None));
        assert!(cache.vertices().all(<[Vertex]>::is_empty));
    }
}
