//! Native protocol session rendering.
//!
//! Renders native protocol scene trees (backgrounds, text, widgets, images)
//! for sessions using the ultty-protocol native UI framework.

use super::{DeferredImageRender, Renderer, Vertex};
use wgpu::util::DeviceExt;

mod borders;
mod code;
mod markdown;
pub(super) mod syntax;
mod widgets;

/// Word-wrap a line to fit within `max_cols` columns.
/// Breaks at spaces when possible, falls back to hard character breaks.
fn wrap_line(line: &str, max_cols: usize) -> Vec<String> {
    if max_cols == 0 {
        return vec![line.to_string()];
    }

    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= max_cols {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut row_start = 0;

    while row_start < chars.len() {
        let remaining = chars.len() - row_start;
        if remaining <= max_cols {
            result.push(chars[row_start..].iter().collect());
            break;
        }

        let end = row_start + max_cols;
        let break_at = (row_start..end).rev().find(|&i| chars[i] == ' ');

        match break_at {
            Some(space_idx) => {
                result.push(chars[row_start..space_idx].iter().collect());
                row_start = space_idx + 1;
            }
            None => {
                result.push(chars[row_start..end].iter().collect());
                row_start = end;
                while row_start < chars.len() && chars[row_start] == ' ' {
                    row_start += 1;
                }
            }
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
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
        let est = region_count * 4;
        let mut bg_vertices = Vec::with_capacity(est * 4);
        let mut bg_indices = Vec::with_capacity(est * 6);
        let mut text_vertices = Vec::with_capacity(est * 4);
        let mut text_indices = Vec::with_capacity(est * 6);
        let mut deferred_images: Vec<DeferredImageRender> = Vec::new();

        // Build a set of matched region IDs for quick lookup
        let matched_regions: std::collections::HashSet<ultty_protocol::RegionId> = search_matches
            .map(|matches| matches.iter().map(|m| m.region_id).collect())
            .unwrap_or_default();

        // Pre-compute accumulated scroll adjustments (in cells) for each region.
        // For each region, the adjustment is the sum of scroll offsets from all
        // overflow:Scroll ancestors. The scroll container itself is NOT self-adjusted.
        let mut scroll_adjustments: std::collections::HashMap<
            ultty_protocol::RegionId,
            (f32, f32),
        > = std::collections::HashMap::new();
        for region in scene.iter_depth_first() {
            if region.id == ultty_protocol::RegionId::ROOT {
                continue;
            }
            let mut adj_x = 0.0f32;
            let mut adj_y = 0.0f32;
            let mut ancestor_id = region.parent;
            while let Some(aid) = ancestor_id {
                if let Some(ancestor) = scene.get(aid) {
                    let ov = &ancestor.constraints.overflow;
                    let (sx, sy) = session.scroll_offset(aid);
                    if matches!(ov.x, ultty_protocol::OverflowAxis::Scroll) {
                        adj_x += sx;
                    }
                    if matches!(ov.y, ultty_protocol::OverflowAxis::Scroll) {
                        adj_y += sy;
                    }
                    ancestor_id = ancestor.parent;
                } else {
                    break;
                }
            }
            if adj_x != 0.0 || adj_y != 0.0 {
                scroll_adjustments.insert(region.id, (adj_x, adj_y));
            }
        }

        // Pre-compute inherited clip rects from overflow:hidden/scroll ancestors.
        // Each entry is (min_x, min_y, max_x, max_y) in pixel coords.
        // Positions are scroll-adjusted so clipping works correctly.
        let mut clip_rects: std::collections::HashMap<
            ultty_protocol::RegionId,
            (f32, f32, f32, f32),
        > = std::collections::HashMap::new();
        for region in scene.iter_depth_first() {
            if region.id == ultty_protocol::RegionId::ROOT {
                continue;
            }
            let layout = &region.computed;
            let (sadj_x, sadj_y) = scroll_adjustments
                .get(&region.id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let rx = ((layout.x - sadj_x) * self.cell_width).round();
            let ry = ((layout.y - sadj_y) * self.cell_height + self.tab_bar_height).round();
            let rw = (layout.width * self.cell_width).round();
            let rh = (layout.height * self.cell_height).round();

            // Start with own bounds (scroll-adjusted)
            let mut clip = (rx, ry, rx + rw, ry + rh);

            // Walk up ancestors, intersecting with any that clip
            let mut ancestor_id = region.parent;
            while let Some(aid) = ancestor_id {
                if let Some(ancestor) = scene.get(aid) {
                    let ov = &ancestor.constraints.overflow;
                    let clips_x = matches!(
                        ov.x,
                        ultty_protocol::OverflowAxis::Hidden | ultty_protocol::OverflowAxis::Scroll
                    );
                    let clips_y = matches!(
                        ov.y,
                        ultty_protocol::OverflowAxis::Hidden | ultty_protocol::OverflowAxis::Scroll
                    );
                    if clips_x || clips_y {
                        let al = &ancestor.computed;
                        // Ancestor uses its own scroll adjustment (from its own ancestors)
                        let (a_sadj_x, a_sadj_y) =
                            scroll_adjustments.get(&aid).copied().unwrap_or((0.0, 0.0));
                        let ax = ((al.x - a_sadj_x) * self.cell_width).round();
                        let ay =
                            ((al.y - a_sadj_y) * self.cell_height + self.tab_bar_height).round();
                        let aw = (al.width * self.cell_width).round();
                        let ah = (al.height * self.cell_height).round();
                        if clips_x {
                            clip.0 = clip.0.max(ax);
                            clip.2 = clip.2.min(ax + aw);
                        }
                        if clips_y {
                            clip.1 = clip.1.max(ay);
                            clip.3 = clip.3.min(ay + ah);
                        }
                    }
                    ancestor_id = ancestor.parent;
                } else {
                    break;
                }
            }
            clip_rects.insert(region.id, clip);
        }

        for region in scene.iter_depth_first() {
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
            let x = ((layout.x - sadj_x) * self.cell_width).round();
            let y = ((layout.y - sadj_y) * self.cell_height + self.tab_bar_height).round();
            let w = (layout.width * self.cell_width).round();
            let h = (layout.height * self.cell_height).round();

            let cx = ((layout.content_x - sadj_x) * self.cell_width).round();
            let cy = ((layout.content_y - sadj_y) * self.cell_height + self.tab_bar_height).round();
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

                    let (_, cell_h) = self.text_renderer.cell_dimensions();
                    let max_cols = cw as usize;
                    let mut visual_line = 0usize;

                    for line in text_content.text.split('\n') {
                        let chars: Vec<char> = line.chars().collect();
                        let fill_width = max_cols;
                        let render_text = if chars.len() == 1 && fill_width > 1 {
                            chars[0].to_string().repeat(fill_width)
                        } else {
                            line.to_string()
                        };

                        if render_text.is_empty() {
                            visual_line += 1;
                            continue;
                        }

                        let wrapped = wrap_line(&render_text, max_cols);
                        for sub_line in &wrapped {
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

                            let span_chars: Vec<char> = segment.chars().collect();
                            let mut char_pos = 0usize;

                            while char_pos < span_chars.len() {
                                let cols_used = ((cursor_x - cx) / cell_w).round() as usize;
                                let cols_left = max_cols.saturating_sub(cols_used);

                                if cols_left == 0 {
                                    cursor_x = cx;
                                    line_idx += 1;
                                    continue;
                                }

                                let chars_remaining = span_chars.len() - char_pos;
                                let chunk_len = chars_remaining.min(cols_left);
                                let chunk: String =
                                    span_chars[char_pos..char_pos + chunk_len].iter().collect();

                                let span_y = cy + line_idx as f32 * cell_h;
                                let (verts, idxs, advance) =
                                    self.text_renderer.build_styled_text_vertices(
                                        &chunk, cursor_x, span_y, fg_color, bold, italic,
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
                                char_pos += chunk_len;

                                if char_pos < span_chars.len() {
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

        if !bg_vertices.is_empty() {
            let bg_count = bg_indices.len() as u32;
            let bg_vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_bg_vb"),
                    contents: bytemuck::cast_slice(&bg_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let bg_ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_bg_ib"),
                    contents: bytemuck::cast_slice(&bg_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native_bg_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                rp.set_pipeline(self.bg_pipeline.pipeline());
                rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                rp.set_vertex_buffer(0, bg_vb.slice(..));
                rp.set_index_buffer(bg_ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..bg_count, 0, 0..1);
            }

            // Cache bg buffers for reuse when sessions are clean
            self.cached_native_bg = Some((bg_vb, bg_ib, bg_count));
        } else {
            self.cached_native_bg = None;
        }

        if !text_vertices.is_empty() {
            let text_count = text_indices.len() as u32;
            let text_vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_text_vb"),
                    contents: bytemuck::cast_slice(&text_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let text_ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_text_ib"),
                    contents: bytemuck::cast_slice(&text_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native_text_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                rp.set_pipeline(self.text_renderer.pipeline());
                rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
                rp.set_vertex_buffer(0, text_vb.slice(..));
                rp.set_index_buffer(text_ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..text_count, 0, 0..1);
            }

            // Cache text buffers for reuse when sessions are clean
            self.cached_native_text = Some((text_vb, text_ib, text_count));
        } else {
            self.cached_native_text = None;
        }

        // Render deferred native images using the image pipeline
        for img_render in &deferred_images {
            let Some(cached) = self.native_image_cache.get(&img_render.region_id) else {
                continue;
            };

            let color = [1.0f32, 1.0, 1.0, 1.0];
            let vertices = vec![
                Vertex {
                    position: [img_render.x, img_render.y],
                    tex_coord: [0.0, 0.0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [img_render.x + img_render.w, img_render.y],
                    tex_coord: [1.0, 0.0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [img_render.x + img_render.w, img_render.y + img_render.h],
                    tex_coord: [1.0, 1.0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [img_render.x, img_render.y + img_render.h],
                    tex_coord: [0.0, 1.0],
                    color,
                    is_color: 0,
                },
            ];
            let indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

            let vb = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_image_vb"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let ib = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_image_ib"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native_image_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                if let Some(img_pipeline) = self.image_pipeline.as_ref() {
                    rp.set_pipeline(&img_pipeline.pipeline);
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_bind_group(1, &cached.bind_group, &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..indices.len() as u32, 0, 0..1);
                }
            }
        }
    }

    /// Draw native session content from cached GPU buffers (skip vertex rebuild).
    pub fn draw_cached_native_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        if let Some((ref bg_vb, ref bg_ib, bg_count)) = self.cached_native_bg {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native_bg_pass_cached"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(self.bg_pipeline.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_vertex_buffer(0, bg_vb.slice(..));
            rp.set_index_buffer(bg_ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..bg_count, 0, 0..1);
        }

        if let Some((ref text_vb, ref text_ib, text_count)) = self.cached_native_text {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native_text_pass_cached"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(self.text_renderer.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
            rp.set_vertex_buffer(0, text_vb.slice(..));
            rp.set_index_buffer(text_ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..text_count, 0, 0..1);
        }
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
}
