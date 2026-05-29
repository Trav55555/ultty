//! Native protocol session rendering.
//!
//! Renders native protocol scene trees (backgrounds, text, widgets, images)
//! for sessions using the ultty-protocol native UI framework.

use super::{DeferredImageRender, Renderer, Vertex};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

mod borders;
mod cache;
mod code;
mod code_spans;
mod geometry;
mod images;
mod markdown;
mod markdown_blocks;
mod markdown_inline;
pub(super) mod syntax;
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
    for quad in idxs.chunks_exact(6) {
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

    /// Render a native protocol session's scene tree (backgrounds, text, widgets, images).
    pub fn render_native_session(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        session: &crate::protocol::NativeSession,
        search_matches: Option<&[crate::scene::SearchMatch]>,
        current_match_index: usize,
    ) {
        use ultty_protocol::Content;

        let scene = session.scene();
        let region_count = scene.iter_depth_first().count();
        let est = region_count.saturating_mul(4);
        let bg_vertex_cap = Self::next_capacity(self.prev_bg_count.0, est, 4);
        let bg_index_cap = Self::next_capacity(self.prev_bg_count.1, est, 6);
        let text_vertex_cap = Self::next_capacity(self.prev_text_count.0, est, 4);
        let text_index_cap = Self::next_capacity(self.prev_text_count.1, est, 6);
        let mut bg_vertices = Vec::with_capacity(bg_vertex_cap);
        let mut bg_indices = Vec::with_capacity(bg_index_cap);
        let mut text_vertices = Vec::with_capacity(text_vertex_cap);
        let mut text_indices = Vec::with_capacity(text_index_cap);
        let mut deferred_images: Vec<DeferredImageRender> = Vec::new();

        // Build a set of matched region IDs for quick lookup
        let matched_regions: std::collections::HashSet<ultty_protocol::RegionId> = search_matches
            .map(|matches| matches.iter().map(|m| m.region_id).collect())
            .unwrap_or_default();

        let needs_scroll_rebuild =
            session.is_scroll_dirty() || self.scroll_adjustments_cache.is_empty();

        if needs_scroll_rebuild {
            geometry::rebuild_scroll_geometry(
                scene,
                |id| session.scroll_offset(id),
                self.cell_width,
                self.cell_height,
                self.tab_bar_height,
                &mut self.scroll_adjustments_cache,
                &mut self.clip_rects_cache,
            );
        }

        for region in scene.iter_depth_first() {
            if region.id == ultty_protocol::RegionId::ROOT {
                continue;
            }

            let layout = &region.computed;
            if layout.width <= 0.0 || layout.height <= 0.0 {
                continue;
            }

            let (sadj_x, sadj_y) = self
                .scroll_adjustments_cache
                .get(&region.id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let x = ((layout.x - sadj_x) * self.cell_width).round();
            let y = ((layout.y - sadj_y) * self.cell_height + self.tab_bar_height).round();
            let w = (layout.width * self.cell_width).round();
            let h = (layout.height * self.cell_height).round();

            let cx = ((layout.content_x - sadj_x) * self.cell_width).round();
            let cy = ((layout.content_y - sadj_y) * self.cell_height + self.tab_bar_height).round();
            let cw = layout.content_width;

            // Use inherited clip rect (intersected with overflow ancestors)
            let (clip_min_x, clip_min_y, clip_max_x, clip_max_y) = self
                .clip_rects_cache
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

            if let Some(bg_color) = &region.style.bg {
                if bg_x0 < bg_x1 && bg_y0 < bg_y1 {
                    let color = self.color_scheme.resolve_protocol_color(bg_color);
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
                &mut bg_vertices,
                &mut bg_indices,
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
                                &mut text_vertices,
                                &mut text_indices,
                            );
                            visual_line += 1;
                        }
                    }

                    if session.is_streaming(region.id) {
                        let blink_on = ((std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                            / 500)
                            % 2)
                            == 0;

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
                                &mut text_vertices,
                                &mut text_indices,
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
                                &mut text_vertices,
                                &mut text_indices,
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
                                    &mut text_vertices,
                                    &mut text_indices,
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
                        &mut bg_vertices,
                        &mut bg_indices,
                        &mut text_vertices,
                        &mut text_indices,
                        &mut deferred_images,
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

        self.upload_draw_and_cache_native_buffers(
            encoder,
            view,
            &bg_vertices,
            &bg_indices,
            &text_vertices,
            &text_indices,
        );

        self.render_deferred_native_images(encoder, view, &deferred_images);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
