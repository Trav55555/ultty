//! Native protocol session rendering.
//!
//! Renders native protocol scene trees (backgrounds, text, widgets, images)
//! for sessions using the ultty-protocol native UI framework.

use super::{DeferredImageRender, Renderer, Vertex};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod borders;
mod cache;
mod geometry;
mod images;
mod widgets;

/// Word-wrap a line to fit within `max_cols` columns.
/// Breaks at spaces when possible, falls back to hard character breaks.
fn wrap_line(line: &str, max_cols: usize) -> Vec<String> {
    fn skip_spaces(line: &str, mut byte_idx: usize) -> usize {
        while byte_idx < line.len() {
            match line[byte_idx..].chars().next() {
                Some(' ') => byte_idx += ' '.len_utf8(),
                _ => break,
            }
        }
        byte_idx
    }

    if max_cols == 0 {
        return vec![line.to_string()];
    }

    if line.width() <= max_cols {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut row_start = 0usize;

    while row_start < line.len() {
        let mut row_end = row_start;
        let mut used_cols = 0usize;
        let mut break_at = None;

        for (rel_idx, ch) in line[row_start..].char_indices() {
            let ch_start = row_start + rel_idx;
            let ch_width = ch.width().unwrap_or(0);
            if used_cols + ch_width > max_cols {
                break;
            }
            used_cols += ch_width;
            if ch == ' ' {
                break_at = Some(ch_start);
            }
            row_end = ch_start + ch.len_utf8();
        }

        if row_end == line.len() {
            result.push(line[row_start..].to_string());
            break;
        }

        if row_end == row_start {
            let ch = line[row_start..]
                .chars()
                .next()
                .expect("row_start always within bounds");
            let next = row_start + ch.len_utf8();
            result.push(line[row_start..next].to_string());
            row_start = skip_spaces(line, next);
            continue;
        }

        match break_at {
            Some(space_idx) => {
                result.push(line[row_start..space_idx].to_string());
                row_start = skip_spaces(line, space_idx + 1);
            }
            None => {
                result.push(line[row_start..row_end].to_string());
                row_start = skip_spaces(line, row_end);
            }
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
}

fn next_display_width_chunk_end(segment: &str, byte_pos: usize, cols_left: usize) -> Option<usize> {
    if cols_left == 0 || byte_pos >= segment.len() {
        return None;
    }

    let mut chunk_end = byte_pos;
    let mut used_cols = 0usize;
    let mut previous_was_joiner = false;

    for (offset, ch) in segment[byte_pos..].char_indices() {
        let ch_end = byte_pos + offset + ch.len_utf8();
        let ch_width = ch.width().unwrap_or(0);
        let force_include = chunk_end == byte_pos
            || previous_was_joiner
            || ch_width == 0
            || ch == '\u{200d}'
            || is_variation_selector(ch);

        if used_cols + ch_width > cols_left && !force_include {
            break;
        }

        used_cols += ch_width;
        chunk_end = ch_end;
        previous_was_joiner = ch == '\u{200d}';
    }

    (chunk_end > byte_pos).then_some(chunk_end)
}

fn is_variation_selector(ch: char) -> bool {
    matches!(ch, '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
}

/// Where an inline surface is drawn: its root, its height in rows, and
/// the pixel row its top is at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceOrigin {
    pub id: ultty_protocol::RegionId,
    pub rows: u16,
    pub top: f32,
}

/// Vertices of native regions, built before they are uploaded.
struct NativeVertices {
    bg_vertices: Vec<Vertex>,
    bg_indices: Vec<u32>,
    text_vertices: Vec<Vertex>,
    text_indices: Vec<u32>,
    deferred_images: Vec<DeferredImageRender>,
}

#[allow(clippy::too_many_arguments)]
fn append_clipped_glyphs(
    verts: &[Vertex],
    idxs: &[u32],
    clip_min_x: f32,
    clip_min_y: f32,
    clip_max_x: f32,
    clip_max_y: f32,
    out_verts: &mut Vec<Vertex>,
    out_idxs: &mut Vec<u32>,
) {
    for quad in idxs.as_chunks::<6>().0 {
        let first = quad[0] as usize;
        let v0 = &verts[first];
        let v2 = &verts[first + 2];
        let glyph_left = v0.position[0];
        let glyph_top = v0.position[1];
        let glyph_right = v2.position[0];
        let glyph_bottom = v2.position[1];

        if glyph_right <= clip_min_x
            || glyph_left >= clip_max_x
            || glyph_bottom <= clip_min_y
            || glyph_top >= clip_max_y
        {
            continue;
        }

        let base = out_verts.len() as u32;
        out_idxs.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        out_verts.extend_from_slice(&verts[first..first + 4]);
    }
}

/// The colour region's background is filled with: its own `bg`, or for a
/// surface root without one the terminal's background, so the fallback text
/// drawn under a surface never shows through it.
fn region_background(
    bg: Option<super::color::Rgba>,
    is_surface_root: bool,
    terminal_background: super::color::Rgba,
) -> Option<super::color::Rgba> {
    bg.or(is_surface_root.then_some(terminal_background))
}

/// Where the placed inline surfaces of `sessions` show on `terminal`'s
/// screen, whose first row is at pixel `grid_top` and whose rows are
/// `cell_height` high; see [`Renderer::visible_surfaces`].
fn visible_surfaces(
    terminal: &crate::grid::Terminal,
    sessions: &[&crate::protocol::NativeSession],
    grid_top: f32,
    cell_height: f32,
) -> Vec<(u64, SurfaceOrigin)> {
    crate::protocol::pairing::surfaces_on_screen(terminal, sessions.iter().copied())
        .into_iter()
        .map(|surface| {
            (
                surface.session_id,
                SurfaceOrigin {
                    id: surface.id,
                    rows: surface.rows,
                    top: grid_top + surface.row as f32 * cell_height,
                },
            )
        })
        .collect()
}

impl Renderer {
    fn next_capacity(prev: usize, estimate: usize, per_region: usize) -> usize {
        let baseline = estimate.saturating_mul(per_region);
        if prev == 0 {
            return baseline;
        }

        // Grow smoothly from prior usage but avoid carrying over huge transient spikes forever.
        let growth_target = prev.saturating_add(prev / 4);
        let max_retained = baseline.saturating_mul(4);
        growth_target.min(max_retained)
    }

    /// Where the placed inline surfaces of `sessions` show on `terminal`'s
    /// screen, by session id, in drawing order. Surfaces with no row on the
    /// screen, and all of them while the alternate screen shows, are left
    /// out.
    pub(crate) fn visible_surfaces(
        &self,
        terminal: &crate::grid::Terminal,
        sessions: &[&crate::protocol::NativeSession],
    ) -> Vec<(u64, SurfaceOrigin)> {
        visible_surfaces(terminal, sessions, self.tab_bar_height, self.cell_height)
    }

    /// Draw the native scenes of the visible tab's `sessions` over
    /// `terminal`: every session's full-scene tree, with the `search`
    /// matches and the index of the current one, when any is full-scene;
    /// else the `surfaces` (from [`Renderer::visible_surfaces`]) over the
    /// grid.
    pub(crate) fn draw_native_scenes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        terminal: &crate::grid::Terminal,
        sessions: &[&crate::protocol::NativeSession],
        surfaces: &[(u64, SurfaceOrigin)],
        search: Option<(&[crate::scene::SearchMatch], usize)>,
    ) {
        if sessions.iter().any(|session| session.is_full_scene()) {
            let (matches, current) = search.map_or((None, 0), |(m, i)| (Some(m), i));
            for session in sessions {
                self.render_native_session(encoder, view, session, matches, current);
            }
            return;
        }
        let dims = terminal.dimensions();
        let pane = (
            0.0,
            self.tab_bar_height,
            f32::from(dims.cols) * self.cell_width,
            self.tab_bar_height + f32::from(dims.rows) * self.cell_height,
        );
        for session in sessions {
            let origins: Vec<SurfaceOrigin> = surfaces
                .iter()
                .filter(|(session_id, _)| *session_id == session.session_id())
                .map(|&(_, origin)| origin)
                .collect();
            if !origins.is_empty() {
                self.render_native_surfaces(encoder, view, session, &origins, pane);
            }
        }
    }

    /// Vertex buffers for `region_count` regions, sized from the last
    /// frame's use.
    fn native_vertices_for(&self, region_count: usize) -> NativeVertices {
        let est = region_count.saturating_mul(4);
        NativeVertices {
            bg_vertices: Vec::with_capacity(Self::next_capacity(self.prev_bg_count.0, est, 4)),
            bg_indices: Vec::with_capacity(Self::next_capacity(self.prev_bg_count.1, est, 6)),
            text_vertices: Vec::with_capacity(Self::next_capacity(self.prev_text_count.0, est, 4)),
            text_indices: Vec::with_capacity(Self::next_capacity(self.prev_text_count.1, est, 6)),
            deferred_images: Vec::new(),
        }
    }

    /// Upload, draw and cache `vertices`, then draw their images.
    fn draw_native_vertices(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertices: NativeVertices,
    ) {
        self.upload_draw_and_cache_native_buffers(
            encoder,
            view,
            &vertices.bg_vertices,
            &vertices.bg_indices,
            &vertices.text_vertices,
            &vertices.text_indices,
        );
        self.render_deferred_native_images(encoder, view, &vertices.deferred_images);
    }

    /// Render a native protocol session's full-scene tree (backgrounds,
    /// text, widgets, images) over the whole pane.
    pub fn render_native_session(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        session: &crate::protocol::NativeSession,
        search_matches: Option<&[crate::scene::SearchMatch]>,
        current_match_index: usize,
    ) {
        let scene = session.scene();
        let mut vertices = self.native_vertices_for(scene.iter_depth_first().count());
        let origin = (0.0, self.tab_bar_height);

        let needs_scroll_rebuild =
            session.is_scroll_dirty() || self.scroll_adjustments_cache.is_empty();

        if needs_scroll_rebuild {
            geometry::rebuild_scroll_geometry(
                scene,
                ultty_protocol::RegionId::ROOT,
                |id| session.scroll_offset(id),
                self.cell_width,
                self.cell_height,
                origin,
                None,
                &mut self.scroll_adjustments_cache,
                &mut self.clip_rects_cache,
            );
        }

        let scroll_adjustments = std::mem::take(&mut self.scroll_adjustments_cache);
        let clip_rects = std::mem::take(&mut self.clip_rects_cache);
        self.build_tree_vertices(
            session,
            ultty_protocol::RegionId::ROOT,
            origin,
            &scroll_adjustments,
            &clip_rects,
            search_matches,
            current_match_index,
            &mut vertices,
        );
        self.scroll_adjustments_cache = scroll_adjustments;
        self.clip_rects_cache = clip_rects;

        self.draw_native_vertices(encoder, view, vertices);
    }

    /// Render `session`'s placed inline surfaces (ADR 0020): each at its
    /// pixel `top` and the pane's left edge, clipped to its own rows and to
    /// `pane`, the grid area as `(min_x, min_y, max_x, max_y)` in pixels.
    pub fn render_native_surfaces(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        session: &crate::protocol::NativeSession,
        surfaces: &[SurfaceOrigin],
        pane: (f32, f32, f32, f32),
    ) {
        let scene = session.scene();
        let mut vertices = self.native_vertices_for(scene.len());
        let mut scroll_adjustments = geometry::ScrollAdjustments::new();
        let mut clip_rects = geometry::ClipRects::new();
        for surface in surfaces {
            let origin = (pane.0, surface.top);
            let rows = (
                origin.0,
                origin.1,
                pane.2,
                origin.1 + f32::from(surface.rows) * self.cell_height,
            );
            let bounds = (
                rows.0.max(pane.0),
                rows.1.max(pane.1),
                rows.2.min(pane.2),
                rows.3.min(pane.3),
            );
            geometry::rebuild_scroll_geometry(
                scene,
                surface.id,
                |id| session.scroll_offset(id),
                self.cell_width,
                self.cell_height,
                origin,
                Some(bounds),
                &mut scroll_adjustments,
                &mut clip_rects,
            );
            self.build_tree_vertices(
                session,
                surface.id,
                origin,
                &scroll_adjustments,
                &clip_rects,
                None,
                0,
                &mut vertices,
            );
        }
        self.draw_native_vertices(encoder, view, vertices);
    }

    /// Append the vertices of the tree under `root`, laid out from pixel
    /// `origin`, with the scroll adjustments and clip rects of its regions.
    #[allow(clippy::too_many_arguments)]
    fn build_tree_vertices(
        &mut self,
        session: &crate::protocol::NativeSession,
        root: ultty_protocol::RegionId,
        (origin_x, origin_y): (f32, f32),
        scroll_adjustments: &geometry::ScrollAdjustments,
        clip_rects: &geometry::ClipRects,
        search_matches: Option<&[crate::scene::SearchMatch]>,
        current_match_index: usize,
        vertices: &mut NativeVertices,
    ) {
        use ultty_protocol::Content;

        let scene = session.scene();
        let NativeVertices {
            bg_vertices,
            bg_indices,
            text_vertices,
            text_indices,
            deferred_images,
        } = vertices;

        // Build a set of matched region IDs for quick lookup
        let matched_regions: std::collections::HashSet<ultty_protocol::RegionId> = search_matches
            .map(|matches| matches.iter().map(|m| m.region_id).collect())
            .unwrap_or_default();

        for region in scene.iter_tree(root) {
            if region.id == ultty_protocol::RegionId::ROOT {
                continue;
            }

            let layout = &region.computed;
            if layout.width <= 0.0 || layout.height <= 0.0 {
                continue;
            }

            let (sadj_x, sadj_y) = scroll_adjustments
                .get(&region.id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let x = ((layout.x - sadj_x) * self.cell_width + origin_x).round();
            let y = ((layout.y - sadj_y) * self.cell_height + origin_y).round();
            let w = (layout.width * self.cell_width).round();
            let h = (layout.height * self.cell_height).round();

            let cx = ((layout.content_x - sadj_x) * self.cell_width + origin_x).round();
            let cy = ((layout.content_y - sadj_y) * self.cell_height + origin_y).round();
            let cw = layout.content_width;

            // Use inherited clip rect (intersected with overflow ancestors)
            let (clip_min_x, clip_min_y, clip_max_x, clip_max_y) = clip_rects
                .get(&region.id)
                .copied()
                .unwrap_or((x, y, x + w, y + h));

            // Skip entirely clipped regions
            if clip_min_x >= clip_max_x || clip_min_y >= clip_max_y {
                continue;
            }

            // Clip bg quad to inherited clip rect
            let bg_x0 = x.max(clip_min_x);
            let bg_y0 = y.max(clip_min_y);
            let bg_x1 = (x + w).min(clip_max_x);
            let bg_y1 = (y + h).min(clip_max_y);

            let background = region_background(
                region
                    .style
                    .bg
                    .as_ref()
                    .map(|bg| self.color_scheme.resolve_protocol_color(bg)),
                scene.surface(region.id).is_some(),
                self.color_scheme.background,
            );
            if let Some(color) = background {
                if bg_x0 < bg_x1 && bg_y0 < bg_y1 {
                    let base_idx = bg_vertices.len() as u32;

                    bg_vertices.push(Vertex {
                        position: [bg_x0, bg_y0],
                        tex_coord: [0.0, 0.0],
                        color: [color.r, color.g, color.b, color.a * region.style.opacity],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [bg_x1, bg_y0],
                        tex_coord: [1.0, 0.0],
                        color: [color.r, color.g, color.b, color.a * region.style.opacity],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [bg_x1, bg_y1],
                        tex_coord: [1.0, 1.0],
                        color: [color.r, color.g, color.b, color.a * region.style.opacity],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [bg_x0, bg_y1],
                        tex_coord: [0.0, 1.0],
                        color: [color.r, color.g, color.b, color.a * region.style.opacity],
                        is_color: 0,
                    });

                    bg_indices.extend_from_slice(&[
                        base_idx,
                        base_idx + 1,
                        base_idx + 2,
                        base_idx,
                        base_idx + 2,
                        base_idx + 3,
                    ]);
                }
            }

            // Render border quads (also clipped)
            self.generate_border_quads(
                &region.style.border,
                &region.style.border_color,
                &region.style.fg,
                region.style.opacity,
                bg_x0,
                bg_y0,
                bg_x1 - bg_x0,
                bg_y1 - bg_y0,
                bg_vertices,
                bg_indices,
            );

            match &region.content {
                Content::Text(text_content) => {
                    let resolved = region
                        .style
                        .fg
                        .as_ref()
                        .map(|c| self.color_scheme.resolve_protocol_color(c))
                        .unwrap_or(self.color_scheme.foreground);
                    let fg_color = super::color::Rgba::new(
                        resolved.r,
                        resolved.g,
                        resolved.b,
                        resolved.a * region.style.opacity,
                    );

                    let (cell_w, cell_h) = self.text_renderer.cell_dimensions();
                    let max_cols = cw as usize;
                    let mut visual_line = 0usize;
                    let mut cursor_line = 0usize;
                    let mut cursor_col = 0usize;

                    for line in text_content.text.split('\n') {
                        let mut chars = line.chars();
                        let single_char = match (chars.next(), chars.next()) {
                            (Some(ch), None) => Some(ch),
                            _ => None,
                        };
                        let fill_width = max_cols;
                        let render_text = if let Some(ch) = single_char.filter(|_| fill_width > 1) {
                            ch.to_string().repeat(fill_width)
                        } else {
                            line.to_string()
                        };

                        if render_text.is_empty() {
                            cursor_line = visual_line;
                            cursor_col = 0;
                            visual_line += 1;
                            continue;
                        }

                        let wrapped = wrap_line(&render_text, max_cols);
                        for (sub_idx, sub_line) in wrapped.iter().enumerate() {
                            if sub_idx + 1 == wrapped.len() {
                                cursor_line = visual_line;
                                cursor_col = sub_line.chars().count();
                            }

                            let line_y = cy + visual_line as f32 * cell_h;
                            if line_y + cell_h <= clip_min_y || line_y >= clip_max_y {
                                visual_line += 1;
                                continue;
                            }

                            let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                                sub_line,
                                cx,
                                line_y,
                                fg_color,
                                region.style.bold,
                                region.style.italic,
                            );

                            append_clipped_glyphs(
                                &verts,
                                &idxs,
                                clip_min_x,
                                clip_min_y,
                                clip_max_x,
                                clip_max_y,
                                text_vertices,
                                text_indices,
                            );
                            visual_line += 1;
                        }
                    }

                    if session.is_streaming(region.id) {
                        let blink_on = (std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                            / 500)
                            .is_multiple_of(2);

                        if blink_on {
                            if max_cols > 0 && cursor_col >= max_cols {
                                cursor_line += cursor_col / max_cols;
                                cursor_col %= max_cols;
                            }

                            let cursor_x = cx + cursor_col as f32 * cell_w;
                            let cursor_y = cy + cursor_line as f32 * cell_h;
                            let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                                "▊",
                                cursor_x,
                                cursor_y,
                                fg_color,
                                region.style.bold,
                                region.style.italic,
                            );
                            append_clipped_glyphs(
                                &verts,
                                &idxs,
                                clip_min_x,
                                clip_min_y,
                                clip_max_x,
                                clip_max_y,
                                text_vertices,
                                text_indices,
                            );
                        }
                    }
                }
                Content::RichText(rich_text) => {
                    let (cell_w, cell_h) = self.text_renderer.cell_dimensions();
                    let max_cols = cw as usize;
                    let mut cursor_x = cx;
                    let mut line_idx = 0usize;

                    for span in &rich_text.spans {
                        let resolved = span
                            .style
                            .fg
                            .as_ref()
                            .map(|c| self.color_scheme.resolve_protocol_color(c))
                            .unwrap_or_else(|| {
                                region
                                    .style
                                    .fg
                                    .as_ref()
                                    .map(|c| self.color_scheme.resolve_protocol_color(c))
                                    .unwrap_or(self.color_scheme.foreground)
                            });
                        let fg_color = super::color::Rgba::new(
                            resolved.r,
                            resolved.g,
                            resolved.b,
                            resolved.a * region.style.opacity,
                        );

                        let bold = span.style.bold || region.style.bold;
                        let italic = span.style.italic || region.style.italic;

                        if max_cols == 0 {
                            let span_y = cy + line_idx as f32 * cell_h;
                            let (verts, idxs, advance) =
                                self.text_renderer.build_styled_text_vertices(
                                    &span.text, cursor_x, span_y, fg_color, bold, italic,
                                );
                            append_clipped_glyphs(
                                &verts,
                                &idxs,
                                clip_min_x,
                                clip_min_y,
                                clip_max_x,
                                clip_max_y,
                                text_vertices,
                                text_indices,
                            );
                            cursor_x += advance;
                            continue;
                        }

                        // Split span text on newlines, render each segment separately
                        for (seg_idx, segment) in span.text.split('\n').enumerate() {
                            if seg_idx > 0 {
                                cursor_x = cx;
                                line_idx += 1;
                            }

                            let mut byte_pos = 0usize;

                            while byte_pos < segment.len() {
                                let cols_used = ((cursor_x - cx) / cell_w).round() as usize;
                                let cols_left = max_cols.saturating_sub(cols_used);

                                if cols_left == 0 {
                                    cursor_x = cx;
                                    line_idx += 1;
                                    continue;
                                }

                                let Some(chunk_end) =
                                    next_display_width_chunk_end(segment, byte_pos, cols_left)
                                else {
                                    break;
                                };
                                let chunk = &segment[byte_pos..chunk_end];

                                let span_y = cy + line_idx as f32 * cell_h;
                                let (verts, idxs, advance) =
                                    self.text_renderer.build_styled_text_vertices(
                                        chunk, cursor_x, span_y, fg_color, bold, italic,
                                    );
                                append_clipped_glyphs(
                                    &verts,
                                    &idxs,
                                    clip_min_x,
                                    clip_min_y,
                                    clip_max_x,
                                    clip_max_y,
                                    text_vertices,
                                    text_indices,
                                );
                                cursor_x += advance;
                                byte_pos = chunk_end;

                                if byte_pos < segment.len() {
                                    cursor_x = cx;
                                    line_idx += 1;
                                }
                            }
                        }
                    }
                }
                Content::Widget(widget) => {
                    self.render_widget_content(
                        widget,
                        region.id,
                        cx,
                        cy,
                        w,
                        h,
                        &region.style,
                        bg_vertices,
                        bg_indices,
                        text_vertices,
                        text_indices,
                        deferred_images,
                    );
                }
                Content::None => {}
            }

            // Draw search highlight rectangles for matched regions
            if matched_regions.contains(&region.id) {
                if let Some(matches) = search_matches {
                    for (match_idx, m) in matches
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.region_id == region.id)
                    {
                        let char_w = self.cell_width;

                        // For RichText, offset by the character count of preceding spans
                        let span_offset_chars = if let Some(span_idx) = m.span_index {
                            if let Content::RichText(rt) = &region.content {
                                rt.spans
                                    .iter()
                                    .take(span_idx)
                                    .map(|s| s.text.chars().count())
                                    .sum::<usize>()
                            } else {
                                0
                            }
                        } else {
                            0
                        };

                        let highlight_x = x + (span_offset_chars + m.char_offset) as f32 * char_w;
                        let highlight_w = m.length as f32 * char_w;
                        // Active match: brighter orange; others: dim yellow
                        let highlight_color = if match_idx == current_match_index {
                            [1.0_f32, 0.6, 0.0, 0.6]
                        } else {
                            [1.0_f32, 0.8, 0.0, 0.35]
                        };

                        let base_idx = bg_vertices.len() as u32;
                        bg_vertices.push(Vertex {
                            position: [highlight_x, y],
                            tex_coord: [0.0, 0.0],
                            color: highlight_color,
                            is_color: 0,
                        });
                        bg_vertices.push(Vertex {
                            position: [highlight_x + highlight_w, y],
                            tex_coord: [1.0, 0.0],
                            color: highlight_color,
                            is_color: 0,
                        });
                        bg_vertices.push(Vertex {
                            position: [highlight_x + highlight_w, y + h],
                            tex_coord: [1.0, 1.0],
                            color: highlight_color,
                            is_color: 0,
                        });
                        bg_vertices.push(Vertex {
                            position: [highlight_x, y + h],
                            tex_coord: [0.0, 1.0],
                            color: highlight_color,
                            is_color: 0,
                        });
                        bg_indices.extend_from_slice(&[
                            base_idx,
                            base_idx + 1,
                            base_idx + 2,
                            base_idx,
                            base_idx + 2,
                            base_idx + 3,
                        ]);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod surfaces {
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        use ultty_protocol::{
            AppMessage, Constraints, FrameId, RegionId, Style, CAP_INLINE_SURFACES,
            PROTOCOL_VERSION,
        };
        use ultty_vt::ansi::AnsiParser;
        use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

        use super::super::{visible_surfaces, SurfaceOrigin};
        use crate::protocol::pairing::AnchorPairing;
        use crate::protocol::NativeSession;
        use crate::terminal_manager::TabId;

        fn feed(term: &mut Terminal, bytes: &[u8]) {
            let mut parser = AnsiParser::new();
            let mut handler = TerminalHandler::new(term);
            parser.feed(bytes, |action| handler.handle(action));
        }

        /// A 20x4 terminal with session 3's two-row surface 9 placed on
        /// its first two lines.
        fn placed_card() -> (Terminal, NativeSession, UnixStream) {
            let (ours, peer) = UnixStream::pair().unwrap();
            let mut session = NativeSession::new(ours, 3, TabId::new(0)).unwrap();
            session.set_inline_surfaces(true);
            for msg in [
                AppMessage::ClientHello {
                    version: PROTOCOL_VERSION,
                    requested_capabilities: CAP_INLINE_SURFACES,
                },
                AppMessage::BeginFrame {
                    frame_id: FrameId(1),
                },
                AppMessage::CreateSurface {
                    id: RegionId::new(9),
                    rows: 2,
                    constraints: Constraints::default(),
                    style: Style::default(),
                },
                AppMessage::EndFrame {
                    frame_id: FrameId(1),
                },
            ] {
                session.handle_message_for_test(msg).unwrap();
            }
            let mut term = Terminal::new(Dimensions::new(20, 4));
            term.set_inline_anchors(true);
            feed(&mut term, b"\x1b_u3;9;2\x1b\\card 1\r\ncard 2\r\n");
            AnchorPairing::default().update(
                &mut term,
                &mut [&mut session],
                true,
                true,
                Instant::now(),
            );
            assert_eq!(session.placed_surfaces().count(), 1);
            (term, session, peer)
        }

        fn at(top: f32) -> Vec<(u64, SurfaceOrigin)> {
            vec![(
                3,
                SurfaceOrigin {
                    id: RegionId::new(9),
                    rows: 2,
                    top,
                },
            )]
        }

        /// A surface root is opaque even without a `bg`: it covers the
        /// fallback text with the terminal's background. Other regions
        /// without one stay transparent, and a `bg` wins.
        #[test]
        fn surface_roots_are_opaque_without_a_background() {
            use super::super::region_background;
            use crate::renderer::color::Rgba;
            let terminal = Rgba::new(0.1, 0.1, 0.1, 1.0);
            let own = Rgba::new(0.8, 0.2, 0.6, 1.0);
            assert_eq!(region_background(None, true, terminal), Some(terminal));
            assert_eq!(region_background(None, false, terminal), None);
            assert_eq!(region_background(Some(own), true, terminal), Some(own));
            assert_eq!(region_background(Some(own), false, terminal), Some(own));
        }

        /// The renderer rebuilds its cached native buffers when this list
        /// changes, so it must follow every move of the anchor rows.
        #[test]
        fn surfaces_move_with_output_and_scrollback_and_leave_the_screen() {
            let (mut term, session, _peer) = placed_card();
            let draw = |term: &Terminal| visible_surfaces(term, &[&session], 30.0, 10.0);
            assert_eq!(draw(&term), at(30.0));

            // Output scrolls the card half off the top, then off.
            feed(&mut term, b"1\r\n2\r\n");
            assert_eq!(draw(&term), at(20.0));
            feed(&mut term, b"3\r\n");
            assert_eq!(draw(&term), Vec::new(), "no row on the screen");

            // Scrolling back brings it to where its text is.
            term.buffer_mut().scroll_up(1);
            assert_eq!(draw(&term), at(20.0));
            term.buffer_mut().scroll_up(1);
            assert_eq!(draw(&term), at(30.0));

            // The alternate screen hides it.
            term.buffer_mut().scroll_to_bottom();
            feed(&mut term, b"\x1b[?1049h");
            assert_eq!(draw(&term), Vec::new());
        }
    }

    #[test]
    fn wrap_line_short_text_unchanged() {
        assert_eq!(wrap_line("hello", 10), vec!["hello"]);
    }

    #[test]
    fn wrap_line_exact_fit() {
        assert_eq!(wrap_line("hello", 5), vec!["hello"]);
    }

    #[test]
    fn wrap_line_breaks_at_space() {
        assert_eq!(wrap_line("hello world", 7), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_line_hard_break_no_space() {
        assert_eq!(wrap_line("abcdefghij", 5), vec!["abcde", "fghij"]);
    }

    #[test]
    fn wrap_line_multiple_wraps() {
        assert_eq!(
            wrap_line("the quick brown fox", 10),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn wrap_line_zero_cols_returns_original() {
        assert_eq!(wrap_line("hello", 0), vec!["hello"]);
    }

    #[test]
    fn wrap_line_empty_string() {
        assert_eq!(wrap_line("", 10), vec![""]);
    }

    #[test]
    fn wrap_line_single_long_word_then_short() {
        assert_eq!(
            wrap_line("abcdefghij end", 5),
            vec!["abcde", "fghij", "end"]
        );
    }

    #[test]
    fn wrap_line_cjk_uses_display_width() {
        assert_eq!(wrap_line("你好世界", 5), vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_line_mixed_ascii_cjk_exact_fit() {
        assert_eq!(wrap_line("hi你好", 6), vec!["hi你好"]);
    }

    #[test]
    fn wrap_line_emoji_uses_display_width() {
        assert_eq!(wrap_line("🙂🙂🙂", 4), vec!["🙂🙂", "🙂"]);
    }

    #[test]
    fn rich_text_chunk_uses_cjk_display_width() {
        let text = "你好世界";
        let end = next_display_width_chunk_end(text, 0, 5).unwrap();

        assert_eq!(&text[..end], "你好");
    }

    #[test]
    fn rich_text_chunk_uses_emoji_display_width() {
        let text = "🙂🙂🙂";
        let end = next_display_width_chunk_end(text, 0, 4).unwrap();

        assert_eq!(&text[..end], "🙂🙂");
    }

    #[test]
    fn rich_text_chunk_keeps_joined_emoji_together() {
        let family = "👨‍👩‍👧‍👦";
        let text = format!("{family}x");
        let end = next_display_width_chunk_end(&text, 0, 2).unwrap();

        assert_eq!(&text[..end], family);
    }

    #[test]
    fn rich_text_chunk_respects_remaining_columns_after_span_boundary() {
        let text = "a你b";
        let first = next_display_width_chunk_end(text, 0, 3).unwrap();

        assert_eq!(&text[..first], "a你");

        let second = next_display_width_chunk_end(text, first, 1).unwrap();
        assert_eq!(&text[first..second], "b");
    }
}
