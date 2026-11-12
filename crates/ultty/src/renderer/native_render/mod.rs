//! Native protocol session rendering.
//!
//! Renders native protocol scene trees (backgrounds, text, widgets, images)
//! for sessions using the ultty-protocol native UI framework.

use super::{DeferredImageRender, Renderer, Vertex};
use crate::scene::geometry;
use crate::scene::text_layout::{next_display_width_chunk_end, wrap_line};

mod borders;
mod cache;
mod images;
mod widgets;

/// Where an inline surface is drawn: its root, its height in rows, and
/// the pixel row its top is at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceOrigin {
    pub id: ultty_protocol::RegionId,
    pub rows: u16,
    pub top: f32,
}

/// A pane's rectangle in window pixels, `(min_x, min_y, max_x, max_y)`:
/// where a bound full scene is drawn and what it is clipped to (ADR 0025).
pub type PaneBounds = (f32, f32, f32, f32);

/// A scissor rectangle in whole surface pixels, `(x, y, width, height)`.
pub type Scissor = (u32, u32, u32, u32);

/// The scissor rectangle that keeps a pane's drawing inside `bounds`, on
/// a surface of `surface` pixels: `bounds` rounded outwards to whole
/// pixels and cut to the surface (a scissor outside it is a GPU error), or
/// `None` when nothing of it is on the surface.
pub fn scissor_for(bounds: PaneBounds, surface: (u32, u32)) -> Option<Scissor> {
    let (min_x, min_y, max_x, max_y) = bounds;
    let x0 = min_x.floor().max(0.0) as u32;
    let y0 = min_y.floor().max(0.0) as u32;
    let x1 = (max_x.ceil().max(0.0) as u32).min(surface.0);
    let y1 = (max_y.ceil().max(0.0) as u32).min(surface.1);
    (x0 < x1 && y0 < y1).then(|| (x0, y0, x1 - x0, y1 - y0))
}

/// What a session's scroll geometry was built for: where it is drawn, what
/// it is clipped to, the cell size, and the session's viewport. Any change
/// moves regions or clip rectangles.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct GeometryKey {
    origin: (f32, f32),
    bounds: Option<PaneBounds>,
    cell: (f32, f32),
    viewport: (f32, f32),
}

/// A full-scene session's scroll adjustments and clip rectangles, valid
/// for the placement they were built for.
#[derive(Default)]
pub struct SceneGeometry {
    placement: GeometryKey,
    scroll: geometry::ScrollAdjustments,
    clip: geometry::ClipRects,
}

/// Vertices of native regions, built before they are uploaded.
pub(super) struct NativeVertices {
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
            let origin = (0.0, self.tab_bar_height);
            let region_count: usize = sessions
                .iter()
                .map(|session| session.scene().iter_depth_first().count())
                .sum();
            let mut vertices = self.native_vertices_for(region_count);
            for session in sessions {
                self.build_native_session_vertices(
                    session,
                    origin,
                    None,
                    matches,
                    current,
                    &mut vertices,
                );
            }
            self.retain_native_geometry(sessions.iter().map(|s| s.session_id()));
            self.draw_native_vertices(encoder, view, vertices, None);
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
    pub(super) fn native_vertices_for(&self, region_count: usize) -> NativeVertices {
        let est = region_count.saturating_mul(4);
        NativeVertices {
            bg_vertices: Vec::with_capacity(Self::next_capacity(self.prev_bg_count.0, est, 4)),
            bg_indices: Vec::with_capacity(Self::next_capacity(self.prev_bg_count.1, est, 6)),
            text_vertices: Vec::with_capacity(Self::next_capacity(self.prev_text_count.0, est, 4)),
            text_indices: Vec::with_capacity(Self::next_capacity(self.prev_text_count.1, est, 6)),
            deferred_images: Vec::new(),
        }
    }

    /// Upload, draw and cache `vertices`, then draw their images, all
    /// within `scissor` when one is given: a pane's rectangle, which clips
    /// widget content too (glyphs and quads are also clipped as built).
    pub(super) fn draw_native_vertices(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertices: NativeVertices,
        scissor: Option<Scissor>,
    ) {
        self.upload_draw_and_cache_native_buffers(
            encoder,
            view,
            &vertices.bg_vertices,
            &vertices.bg_indices,
            &vertices.text_vertices,
            &vertices.text_indices,
            scissor,
        );
        self.render_deferred_native_images(encoder, view, &vertices.deferred_images, scissor);
    }

    /// Whether the cached native buffers still draw the right frame: no
    /// session changed, the glyph atlas is the one their UVs index (after
    /// it grows or empties they point at the wrong glyphs), and every scene
    /// is where it was: `surfaces` at their anchor rows, full scenes in
    /// `panes`' rectangles.
    pub(crate) fn native_cache_valid(
        &self,
        any_dirty: bool,
        surfaces: &[(u64, SurfaceOrigin)],
        panes: &[(u64, PaneBounds)],
    ) -> bool {
        !any_dirty
            && self.cached_native_atlas_epoch == self.text_renderer.atlas_epoch()
            && surfaces == self.cached_native_surfaces
            && panes == self.cached_native_panes
            && self.has_cached_native_buffers()
    }

    /// Forget the geometry of sessions other than `live`, which ended.
    pub(super) fn retain_native_geometry(&mut self, live: impl Iterator<Item = u64>) {
        let live: Vec<u64> = live.collect();
        self.native_geometry.retain(|id, _| live.contains(id));
    }

    /// Append the vertices of `session`'s full-scene tree (backgrounds,
    /// text, widgets, images), laid out from pixel `origin` and, for a pane
    /// of a split tab, clipped to `bounds`. The session's scroll geometry is
    /// reused from the last frame while it has not changed or moved; it is
    /// built again when another scene in the batch made the frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_native_session_vertices(
        &mut self,
        session: &crate::protocol::NativeSession,
        origin: (f32, f32),
        bounds: Option<PaneBounds>,
        search_matches: Option<&[crate::scene::SearchMatch]>,
        current_match_index: usize,
        vertices: &mut NativeVertices,
    ) {
        let scene = session.scene();
        let placement = GeometryKey {
            origin,
            bounds,
            cell: (self.cell_width, self.cell_height),
            viewport: session.viewport(),
        };
        let unchanged = !session.is_scroll_dirty() && !session.is_render_dirty();
        let geometry = match self.native_geometry.remove(&session.session_id()) {
            Some(geometry) if unchanged && geometry.placement == placement => geometry,
            stale => {
                let mut geometry = stale.unwrap_or_default();
                geometry.placement = placement;
                geometry::rebuild_scroll_geometry(
                    scene,
                    ultty_protocol::RegionId::ROOT,
                    |id| session.scroll_offset(id),
                    self.cell_width,
                    self.cell_height,
                    origin,
                    bounds,
                    &mut geometry.scroll,
                    &mut geometry.clip,
                );
                geometry
            }
        };

        self.build_tree_vertices(
            session,
            ultty_protocol::RegionId::ROOT,
            origin,
            &geometry.scroll,
            &geometry.clip,
            search_matches,
            current_match_index,
            vertices,
        );
        self.native_geometry.insert(session.session_id(), geometry);
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
        if let Some(scissor) = scissor_for(pane, self.surface_size()) {
            self.draw_native_vertices(encoder, view, vertices, Some(scissor));
        }
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
}

#[cfg(test)]
mod scissor_tests {
    use super::scissor_for;

    #[test]
    fn a_pane_rectangle_becomes_a_whole_pixel_scissor_rounded_outwards() {
        assert_eq!(
            scissor_for((500.5, 40.2, 999.5, 699.9), (1000, 700)),
            Some((500, 40, 500, 660))
        );
    }

    #[test]
    fn a_scissor_is_cut_to_the_surface() {
        // A pane laid out before the surface shrank must not reach past it.
        assert_eq!(
            scissor_for((500.0, 40.0, 1200.0, 900.0), (1000, 700)),
            Some((500, 40, 500, 660))
        );
        assert_eq!(
            scissor_for((-10.0, -5.0, 100.0, 50.0), (1000, 700)),
            Some((0, 0, 100, 50))
        );
    }

    #[test]
    fn nothing_on_the_surface_is_no_scissor() {
        assert_eq!(scissor_for((1000.0, 0.0, 1200.0, 700.0), (1000, 700)), None);
        assert_eq!(scissor_for((10.0, 10.0, 10.0, 50.0), (1000, 700)), None);
    }
}
