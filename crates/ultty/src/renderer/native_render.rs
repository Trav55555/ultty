//! Native protocol session rendering.
//!
//! Renders native protocol scene trees (backgrounds, text, widgets, images)
//! for sessions using the ultty-protocol native UI framework.

use super::{DeferredImageRender, Renderer, Vertex};
use ultty_protocol::BorderType;
use wgpu::util::DeviceExt;

impl Renderer {
    /// Generate border quads for a region's border style.
    ///
    /// Appends to the provided bg_vertices/bg_indices buffers.
    /// Thickness: Thin/Rounded/Dashed/Ascii → 1.0px, Thick → 2.0px,
    /// Double → 1.0px outer + 1.0px inner with 1px gap.
    #[allow(clippy::too_many_arguments)]
    fn generate_border_quads(
        &self,
        border: &ultty_protocol::BorderStyle,
        border_color: &Option<ultty_protocol::Color>,
        fg_color: &Option<ultty_protocol::Color>,
        opacity: f32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        bg_vertices: &mut Vec<Vertex>,
        bg_indices: &mut Vec<u32>,
    ) {
        if border.style == BorderType::None {
            return;
        }

        // Resolve color: border_color -> fg_color -> foreground default
        let color = border_color
            .as_ref()
            .map(|c| self.color_scheme.resolve_protocol_color(c))
            .or_else(|| {
                fg_color
                    .as_ref()
                    .map(|c| self.color_scheme.resolve_protocol_color(c))
            })
            .unwrap_or(self.color_scheme.foreground);

        let color_arr = [color.r, color.g, color.b, color.a * opacity];

        let thickness = match border.style {
            BorderType::Thick => 2.0,
            _ => 1.0,
        };

        let edges = &border.edges;

        // Helper to emit a single quad
        let mut emit_quad = |qx: f32, qy: f32, qw: f32, qh: f32| {
            let base_idx = bg_vertices.len() as u32;
            bg_vertices.push(Vertex {
                position: [qx, qy],
                tex_coord: [0.0, 0.0],
                color: color_arr,
            });
            bg_vertices.push(Vertex {
                position: [qx + qw, qy],
                tex_coord: [1.0, 0.0],
                color: color_arr,
            });
            bg_vertices.push(Vertex {
                position: [qx + qw, qy + qh],
                tex_coord: [1.0, 1.0],
                color: color_arr,
            });
            bg_vertices.push(Vertex {
                position: [qx, qy + qh],
                tex_coord: [0.0, 1.0],
                color: color_arr,
            });
            bg_indices.extend_from_slice(&[
                base_idx,
                base_idx + 1,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 3,
            ]);
        };

        // Outer border edges
        if edges.top {
            emit_quad(x, y, w, thickness);
        }
        if edges.bottom {
            emit_quad(x, y + h - thickness, w, thickness);
        }
        if edges.left {
            emit_quad(x, y, thickness, h);
        }
        if edges.right {
            emit_quad(x + w - thickness, y, thickness, h);
        }

        // Double border: additional inner border with 1px gap
        if border.style == BorderType::Double {
            let offset = 2.0; // 1px outer + 1px gap
            if edges.top {
                emit_quad(x + offset, y + offset, w - 2.0 * offset, thickness);
            }
            if edges.bottom {
                emit_quad(
                    x + offset,
                    y + h - thickness - offset,
                    w - 2.0 * offset,
                    thickness,
                );
            }
            if edges.left {
                emit_quad(x + offset, y + offset, thickness, h - 2.0 * offset);
            }
            if edges.right {
                emit_quad(
                    x + w - thickness - offset,
                    y + offset,
                    thickness,
                    h - 2.0 * offset,
                );
            }
        }
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

        for region in scene.iter_depth_first() {
            if region.id == ultty_protocol::RegionId::ROOT {
                continue;
            }

            let layout = &region.computed;
            if layout.width <= 0.0 || layout.height <= 0.0 {
                continue;
            }

            let x = layout.x * self.cell_width;
            let y = layout.y * self.cell_height + self.tab_bar_height;
            let w = layout.width * self.cell_width;
            let h = layout.height * self.cell_height;

            let cx = layout.content_x * self.cell_width;
            let cy = layout.content_y * self.cell_height + self.tab_bar_height;
            let cw = layout.content_width;

            if let Some(bg_color) = &region.style.bg {
                let color = self.color_scheme.resolve_protocol_color(bg_color);
                let base_idx = bg_vertices.len() as u32;

                bg_vertices.push(Vertex {
                    position: [x, y],
                    tex_coord: [0.0, 0.0],
                    color: [color.r, color.g, color.b, color.a * region.style.opacity],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y],
                    tex_coord: [1.0, 0.0],
                    color: [color.r, color.g, color.b, color.a * region.style.opacity],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y + h],
                    tex_coord: [1.0, 1.0],
                    color: [color.r, color.g, color.b, color.a * region.style.opacity],
                });
                bg_vertices.push(Vertex {
                    position: [x, y + h],
                    tex_coord: [0.0, 1.0],
                    color: [color.r, color.g, color.b, color.a * region.style.opacity],
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

            // Render border quads
            self.generate_border_quads(
                &region.style.border,
                &region.style.border_color,
                &region.style.fg,
                region.style.opacity,
                x,
                y,
                w,
                h,
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

                    let chars: Vec<char> = text_content.text.chars().collect();
                    let fill_width = cw as usize;
                    let render_text = if chars.len() == 1 && fill_width > 1 {
                        chars[0].to_string().repeat(fill_width)
                    } else {
                        text_content.text.clone()
                    };

                    let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                        &render_text,
                        cx,
                        cy,
                        fg_color,
                        region.style.bold,
                        region.style.italic,
                    );

                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                }
                Content::RichText(rich_text) => {
                    let mut cursor_x = cx;
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

                        let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                            &span.text,
                            cursor_x,
                            cy,
                            fg_color,
                            span.style.bold || region.style.bold,
                            span.style.italic || region.style.italic,
                        );

                        let base_idx = text_vertices.len() as u32;
                        text_vertices.extend(verts);
                        text_indices.extend(idxs.iter().map(|i| i + base_idx));
                        cursor_x += advance;
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
                        });
                        bg_vertices.push(Vertex {
                            position: [highlight_x + highlight_w, y],
                            tex_coord: [1.0, 0.0],
                            color: highlight_color,
                        });
                        bg_vertices.push(Vertex {
                            position: [highlight_x + highlight_w, y + h],
                            tex_coord: [1.0, 1.0],
                            color: highlight_color,
                        });
                        bg_vertices.push(Vertex {
                            position: [highlight_x, y + h],
                            tex_coord: [0.0, 1.0],
                            color: highlight_color,
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
                },
                Vertex {
                    position: [img_render.x + img_render.w, img_render.y],
                    tex_coord: [1.0, 0.0],
                    color,
                },
                Vertex {
                    position: [img_render.x + img_render.w, img_render.y + img_render.h],
                    tex_coord: [1.0, 1.0],
                    color,
                },
                Vertex {
                    position: [img_render.x, img_render.y + img_render.h],
                    tex_coord: [0.0, 1.0],
                    color,
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

    #[allow(clippy::too_many_arguments)]
    fn render_widget_content(
        &mut self,
        widget: &ultty_protocol::WidgetContent,
        region_id: ultty_protocol::RegionId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        region_style: &ultty_protocol::Style,
        bg_vertices: &mut Vec<Vertex>,
        bg_indices: &mut Vec<u32>,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
        deferred_images: &mut Vec<DeferredImageRender>,
    ) {
        use ultty_protocol::WidgetContent;

        match widget {
            WidgetContent::Progress(progress) => {
                let bar_height = h.min(self.cell_height);
                let bar_y = y + (h - bar_height) / 2.0;

                // Compute the effective fill width
                let target_value = progress.value.clamp(0.0, 1.0);
                let display_value = if progress.indeterminate {
                    self.has_animations = true;
                    // Bouncing bar: time-based position
                    let t = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as f32)
                        / 1000.0;
                    // Ping-pong between 0 and 1
                    let phase = (t * 1.5) % 2.0;
                    if phase < 1.0 {
                        phase
                    } else {
                        2.0 - phase
                    }
                } else if progress.animated {
                    self.has_animations = true;
                    // Lerp stored value toward target
                    let current = self
                        .animated_progress
                        .get(&region_id)
                        .copied()
                        .unwrap_or(target_value);
                    let lerped = current + (target_value - current) * 0.1;
                    let lerped = if (lerped - target_value).abs() < 0.001 {
                        target_value
                    } else {
                        lerped
                    };
                    self.animated_progress.insert(region_id, lerped);
                    lerped
                } else {
                    target_value
                };

                let filled_width = if progress.indeterminate {
                    // For indeterminate, show a small bouncing highlight
                    let highlight_w = w * 0.25;
                    let _offset_x = display_value * (w - highlight_w);
                    // We'll handle this below
                    0.0 // Don't use the normal fill path
                } else {
                    w * display_value
                };

                let bg_color = self.color_scheme.named[8];
                let fg_color = region_style
                    .fg
                    .as_ref()
                    .map(|c| self.color_scheme.resolve_protocol_color(c))
                    .unwrap_or(self.color_scheme.named[2]);

                let base_idx = bg_vertices.len() as u32;
                bg_vertices.push(Vertex {
                    position: [x, bar_y],
                    tex_coord: [0.0, 0.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, bar_y],
                    tex_coord: [1.0, 0.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, bar_y + bar_height],
                    tex_coord: [1.0, 1.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
                });
                bg_vertices.push(Vertex {
                    position: [x, bar_y + bar_height],
                    tex_coord: [0.0, 1.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
                });
                bg_indices.extend_from_slice(&[
                    base_idx,
                    base_idx + 1,
                    base_idx + 2,
                    base_idx,
                    base_idx + 2,
                    base_idx + 3,
                ]);

                if progress.indeterminate {
                    // Bouncing highlight bar
                    let highlight_w = w * 0.25;
                    let offset_x = display_value * (w - highlight_w);
                    let hx = x + offset_x;
                    let base_idx = bg_vertices.len() as u32;
                    bg_vertices.push(Vertex {
                        position: [hx, bar_y],
                        tex_coord: [0.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_vertices.push(Vertex {
                        position: [hx + highlight_w, bar_y],
                        tex_coord: [1.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_vertices.push(Vertex {
                        position: [hx + highlight_w, bar_y + bar_height],
                        tex_coord: [1.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_vertices.push(Vertex {
                        position: [hx, bar_y + bar_height],
                        tex_coord: [0.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_indices.extend_from_slice(&[
                        base_idx,
                        base_idx + 1,
                        base_idx + 2,
                        base_idx,
                        base_idx + 2,
                        base_idx + 3,
                    ]);
                } else if filled_width > 0.0 {
                    let base_idx = bg_vertices.len() as u32;
                    bg_vertices.push(Vertex {
                        position: [x, bar_y],
                        tex_coord: [0.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_vertices.push(Vertex {
                        position: [x + filled_width, bar_y],
                        tex_coord: [1.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_vertices.push(Vertex {
                        position: [x + filled_width, bar_y + bar_height],
                        tex_coord: [1.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                    });
                    bg_vertices.push(Vertex {
                        position: [x, bar_y + bar_height],
                        tex_coord: [0.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
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

                if let Some(label) = &progress.label {
                    let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                        label,
                        x + w + self.cell_width,
                        y,
                        self.color_scheme.foreground,
                        false,
                        false,
                    );
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                }
            }
            WidgetContent::Spinner(spinner) => {
                static BRAILLE_FRAMES: &[char] =
                    &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
                static DOTS_FRAMES: &[char] = &['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];
                static LINE_FRAMES: &[char] = &['|', '/', '-', '\\'];
                static BOUNCE_FRAMES: &[char] = &['⠁', '⠂', '⠄', '⠂'];
                let frames: &[char] = match spinner.style {
                    ultty_protocol::SpinnerStyle::Braille => BRAILLE_FRAMES,
                    ultty_protocol::SpinnerStyle::Dots => DOTS_FRAMES,
                    ultty_protocol::SpinnerStyle::Line => LINE_FRAMES,
                    ultty_protocol::SpinnerStyle::Bounce => BOUNCE_FRAMES,
                };
                let frame_idx = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    / 80) as usize
                    % frames.len();
                let spinner_char = frames[frame_idx];
                self.has_animations = true;

                let fg_color = region_style
                    .fg
                    .as_ref()
                    .map(|c| self.color_scheme.resolve_protocol_color(c))
                    .unwrap_or(self.color_scheme.named[6]);

                let text = if let Some(label) = &spinner.label {
                    format!("{} {}", spinner_char, label)
                } else {
                    spinner_char.to_string()
                };

                let (verts, idxs, _) = self
                    .text_renderer
                    .build_styled_text_vertices(&text, x, y, fg_color, false, false);
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
            }
            WidgetContent::Input(input) => {
                let border_color = self.color_scheme.named[8];
                let base_idx = bg_vertices.len() as u32;
                bg_vertices.push(Vertex {
                    position: [x, y],
                    tex_coord: [0.0, 0.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y],
                    tex_coord: [1.0, 0.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y + h],
                    tex_coord: [1.0, 1.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
                });
                bg_vertices.push(Vertex {
                    position: [x, y + h],
                    tex_coord: [0.0, 1.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
                });
                bg_indices.extend_from_slice(&[
                    base_idx,
                    base_idx + 1,
                    base_idx + 2,
                    base_idx,
                    base_idx + 2,
                    base_idx + 3,
                ]);

                let password_mask;
                let display_text = if input.value.is_empty() {
                    &input.placeholder
                } else if input.password {
                    password_mask = "•".repeat(input.value.chars().count());
                    &password_mask
                } else {
                    &input.value
                };

                let text_color = if input.value.is_empty() {
                    self.color_scheme.named[8]
                } else {
                    self.color_scheme.foreground
                };

                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    display_text,
                    x + self.cell_width * 0.5,
                    y,
                    text_color,
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
            }
            WidgetContent::Markdown(md) => {
                self.render_markdown_content(&md.source, x, y, w, text_vertices, text_indices);
            }
            WidgetContent::Code(code) => {
                self.render_code_content(
                    &code.source,
                    &code.language,
                    code.line_numbers,
                    x,
                    y,
                    text_vertices,
                    text_indices,
                );
            }
            WidgetContent::Image(img) => {
                // Upload image to GPU and defer rendering to image pipeline pass
                if self.upload_native_image(region_id, img) {
                    // Compute quad dimensions based on ImageScale
                    let (img_x, img_y, img_w, img_h) = match img.scale {
                        ultty_protocol::ImageScale::Stretch => (x, y, w, h),
                        ultty_protocol::ImageScale::None => {
                            // Original size in pixels, convert to screen coords
                            let orig_w = img.width as f32;
                            let orig_h = img.height as f32;
                            (x, y, orig_w.min(w), orig_h.min(h))
                        }
                        ultty_protocol::ImageScale::Fit => {
                            let aspect = img.width as f32 / img.height.max(1) as f32;
                            let region_aspect = w / h.max(0.001);
                            let (fw, fh) = if aspect > region_aspect {
                                (w, w / aspect)
                            } else {
                                (h * aspect, h)
                            };
                            let fx = x + (w - fw) / 2.0;
                            let fy = y + (h - fh) / 2.0;
                            (fx, fy, fw, fh)
                        }
                        ultty_protocol::ImageScale::Fill => {
                            let aspect = img.width as f32 / img.height.max(1) as f32;
                            let region_aspect = w / h.max(0.001);
                            let (fw, fh) = if aspect < region_aspect {
                                (w, w / aspect)
                            } else {
                                (h * aspect, h)
                            };
                            let fx = x + (w - fw) / 2.0;
                            let fy = y + (h - fh) / 2.0;
                            (fx, fy, fw, fh)
                        }
                    };
                    deferred_images.push(DeferredImageRender {
                        region_id,
                        x: img_x,
                        y: img_y,
                        w: img_w,
                        h: img_h,
                    });
                } else {
                    // Fallback: show placeholder if upload failed
                    let placeholder = format!("[Image: {}x{}]", img.width, img.height);
                    let fg_color = region_style
                        .fg
                        .as_ref()
                        .map(|c| self.color_scheme.resolve_protocol_color(c))
                        .unwrap_or(self.color_scheme.named[8]);
                    let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                        &placeholder,
                        x,
                        y,
                        fg_color,
                        false,
                        true,
                    );
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                }
            }
            WidgetContent::Button(button) => {
                use ultty_protocol::ButtonStyle as Bs;

                let (bg_col, fg_col, border_col) = match button.style {
                    Bs::Primary => {
                        let bg = self.color_scheme.named[4];
                        let fg = self.color_scheme.background;
                        (bg, fg, bg)
                    }
                    Bs::Danger => {
                        let bg = self.color_scheme.named[1];
                        let fg = self.color_scheme.background;
                        (bg, fg, bg)
                    }
                    Bs::Success => {
                        let bg = self.color_scheme.named[2];
                        let fg = self.color_scheme.background;
                        (bg, fg, bg)
                    }
                    Bs::Default => {
                        let bg = self.color_scheme.named[8];
                        let fg = self.color_scheme.foreground;
                        let border = self.color_scheme.named[7];
                        (bg, fg, border)
                    }
                };

                let padding = self.cell_width * 0.5;
                let inner_x = x + padding;
                let inner_w = w - padding * 2.0;

                let base_idx = bg_vertices.len() as u32;
                bg_vertices.push(Vertex {
                    position: [x, y],
                    tex_coord: [0.0, 0.0],
                    color: [bg_col.r, bg_col.g, bg_col.b, 0.8 * region_style.opacity],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y],
                    tex_coord: [1.0, 0.0],
                    color: [bg_col.r, bg_col.g, bg_col.b, 0.8 * region_style.opacity],
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y + h],
                    tex_coord: [1.0, 1.0],
                    color: [bg_col.r, bg_col.g, bg_col.b, 0.8 * region_style.opacity],
                });
                bg_vertices.push(Vertex {
                    position: [x, y + h],
                    tex_coord: [0.0, 1.0],
                    color: [bg_col.r, bg_col.g, bg_col.b, 0.8 * region_style.opacity],
                });
                bg_indices.extend_from_slice(&[
                    base_idx,
                    base_idx + 1,
                    base_idx + 2,
                    base_idx,
                    base_idx + 2,
                    base_idx + 3,
                ]);

                let thickness = 1.0f32;
                let bc = [
                    border_col.r,
                    border_col.g,
                    border_col.b,
                    region_style.opacity,
                ];
                let edges = [
                    (
                        [x, y],
                        [x + w, y],
                        [x + w, y + thickness],
                        [x, y + thickness],
                    ),
                    (
                        [x + w - thickness, y],
                        [x + w, y],
                        [x + w, y + h],
                        [x + w - thickness, y + h],
                    ),
                    (
                        [x, y + h - thickness],
                        [x + w, y + h - thickness],
                        [x + w, y + h],
                        [x, y + h],
                    ),
                    (
                        [x, y],
                        [x + thickness, y],
                        [x + thickness, y + h],
                        [x, y + h],
                    ),
                ];
                for (tl, tr, br, bl) in edges {
                    let base = bg_vertices.len() as u32;
                    bg_vertices.push(Vertex {
                        position: tl,
                        tex_coord: [0.0, 0.0],
                        color: bc,
                    });
                    bg_vertices.push(Vertex {
                        position: tr,
                        tex_coord: [1.0, 0.0],
                        color: bc,
                    });
                    bg_vertices.push(Vertex {
                        position: br,
                        tex_coord: [1.0, 1.0],
                        color: bc,
                    });
                    bg_vertices.push(Vertex {
                        position: bl,
                        tex_coord: [0.0, 1.0],
                        color: bc,
                    });
                    bg_indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                }

                let label_width = button.label.chars().count() as f32 * self.cell_width;
                let text_x = inner_x + (inner_w - label_width).max(0.0) / 2.0;
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &button.label,
                    text_x,
                    y,
                    fg_col,
                    true,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
            }
        }
    }

    fn render_markdown_content(
        &mut self,
        source: &str,
        x: f32,
        y: f32,
        _width: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_y = y;
        let line_height = self.cell_height;
        let mut in_code_block = false;
        let mut fence_language = "";
        let mut fence_start_idx = 0usize;
        let lines: Vec<&str> = source.lines().collect();
        let mut line_idx = 0;

        while line_idx < lines.len() {
            let line = lines[line_idx];

            // Fenced code blocks
            if line.starts_with("```") {
                if !in_code_block {
                    // Opening fence: extract language
                    fence_language = line.trim_start_matches('`').trim();
                    fence_start_idx = line_idx;
                    in_code_block = true;
                    line_idx += 1;
                    continue;
                } else {
                    // Closing fence: render accumulated code lines
                    let code_lines = &lines[fence_start_idx + 1..line_idx];
                    if !code_lines.is_empty() {
                        let code_source = code_lines.join("\n");
                        self.render_code_content(
                            &code_source,
                            fence_language,
                            false,
                            x + self.cell_width,
                            cursor_y,
                            text_vertices,
                            text_indices,
                        );
                        cursor_y += code_lines.len() as f32 * line_height;
                    }
                    in_code_block = false;
                    line_idx += 1;
                    continue;
                }
            }
            if in_code_block {
                // Lines inside a fenced code block are handled on close
                line_idx += 1;
                continue;
            }

            // Horizontal rules
            if line == "---" || line == "***" || line == "___" {
                let rule = "─".repeat(40);
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &rule,
                    x,
                    cursor_y,
                    self.color_scheme.named[8],
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }

            // Tables: detect lines starting with | and containing at least one more |
            if line.starts_with('|') && line.matches('|').count() >= 2 {
                // Collect all consecutive table lines
                let _table_start = line_idx;
                let mut table_lines = Vec::new();
                while line_idx < lines.len() && lines[line_idx].starts_with('|') {
                    table_lines.push(lines[line_idx]);
                    line_idx += 1;
                }
                self.render_markdown_table(&table_lines, x, cursor_y, text_vertices, text_indices);
                cursor_y += line_height * table_lines.len() as f32;
                continue;
            }

            // Headers
            if let Some(stripped) = line.strip_prefix("### ") {
                self.render_markdown_inline(
                    stripped,
                    x,
                    cursor_y,
                    true,
                    self.color_scheme.named[6],
                    text_vertices,
                    text_indices,
                );
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }
            if let Some(stripped) = line.strip_prefix("## ") {
                self.render_markdown_inline(
                    stripped,
                    x,
                    cursor_y,
                    true,
                    self.color_scheme.named[5],
                    text_vertices,
                    text_indices,
                );
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }
            if let Some(stripped) = line.strip_prefix("# ") {
                self.render_markdown_inline(
                    stripped,
                    x,
                    cursor_y,
                    true,
                    self.color_scheme.named[4],
                    text_vertices,
                    text_indices,
                );
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }

            // Task lists: - [ ] or - [x]
            if let Some(rest) = line.strip_prefix("- [ ] ") {
                let checkbox = "☐ ";
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    checkbox,
                    x,
                    cursor_y,
                    self.color_scheme.named[8],
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                self.render_markdown_inline(
                    rest,
                    x + advance,
                    cursor_y,
                    false,
                    self.color_scheme.foreground,
                    text_vertices,
                    text_indices,
                );
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }
            if let Some(rest) = line
                .strip_prefix("- [x] ")
                .or_else(|| line.strip_prefix("- [X] "))
            {
                let checkbox = "☑ ";
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    checkbox,
                    x,
                    cursor_y,
                    self.color_scheme.named[2],
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                self.render_markdown_inline(
                    rest,
                    x + advance,
                    cursor_y,
                    false,
                    self.color_scheme.foreground,
                    text_vertices,
                    text_indices,
                );
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }

            // Nested blockquotes: count leading > characters
            let trimmed = line;
            let mut quote_depth = 0u32;
            let mut rest = trimmed;
            while let Some(inner) = rest.strip_prefix("> ") {
                quote_depth += 1;
                rest = inner;
            }
            // Also handle > without trailing space at line end
            if rest == ">" {
                quote_depth += 1;
                rest = "";
            }

            if quote_depth > 0 {
                let indent = self.cell_width * 2.0 * quote_depth as f32;
                // Render blockquote bars
                for depth in 0..quote_depth {
                    let bar_x = x + self.cell_width * 2.0 * depth as f32;
                    let bar = "│";
                    let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                        bar,
                        bar_x,
                        cursor_y,
                        self.color_scheme.named[8],
                        false,
                        false,
                    );
                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                }
                if !rest.is_empty() {
                    self.render_markdown_inline(
                        rest,
                        x + indent,
                        cursor_y,
                        false,
                        self.color_scheme.foreground,
                        text_vertices,
                        text_indices,
                    );
                }
                cursor_y += line_height;
                line_idx += 1;
                continue;
            }

            // Unordered list items
            let (prefix, content) =
                if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                    ("• ", rest)
                } else {
                    ("", line)
                };

            // Numbered list items (1. 2. etc.)
            let (prefix, content) = if prefix.is_empty() {
                if let Some(dot_pos) = content.find(". ") {
                    if dot_pos <= 3 && content[..dot_pos].chars().all(|c| c.is_ascii_digit()) {
                        let num_prefix = format!("{}. ", &content[..dot_pos]);
                        // Render number prefix then inline content
                        let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                            &num_prefix,
                            x,
                            cursor_y,
                            self.color_scheme.named[8],
                            false,
                            false,
                        );
                        let base_idx = text_vertices.len() as u32;
                        text_vertices.extend(verts);
                        text_indices.extend(idxs.iter().map(|i| i + base_idx));
                        self.render_markdown_inline(
                            &content[dot_pos + 2..],
                            x + advance,
                            cursor_y,
                            false,
                            self.color_scheme.foreground,
                            text_vertices,
                            text_indices,
                        );
                        cursor_y += line_height;
                        line_idx += 1;
                        continue;
                    }
                }
                ("", content)
            } else {
                (prefix, content)
            };

            let mut cursor_x = x;
            if !prefix.is_empty() {
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    prefix,
                    cursor_x,
                    cursor_y,
                    self.color_scheme.named[8],
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_x += advance;
            }

            self.render_markdown_inline(
                content,
                cursor_x,
                cursor_y,
                false,
                self.color_scheme.foreground,
                text_vertices,
                text_indices,
            );
            cursor_y += line_height;
            line_idx += 1;
        }

        // Handle unclosed fenced code block: render remaining lines as code
        if in_code_block {
            let code_lines = &lines[fence_start_idx + 1..];
            if !code_lines.is_empty() {
                let code_source = code_lines.join("\n");
                self.render_code_content(
                    &code_source,
                    fence_language,
                    false,
                    x + self.cell_width,
                    cursor_y,
                    text_vertices,
                    text_indices,
                );
            }
        }
    }

    /// Render a markdown table (rows of `|`-delimited cells).
    fn render_markdown_table(
        &mut self,
        lines: &[&str],
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let line_height = self.cell_height;
        let mut cursor_y = y;

        // Parse cells from each row
        let parse_row = |line: &str| -> Vec<String> {
            line.split('|')
                .skip(1) // skip leading empty
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() || line.ends_with('|'))
                .collect::<Vec<_>>()
        };

        // Compute column widths from all non-separator rows
        let mut col_widths: Vec<usize> = Vec::new();
        for &line in lines {
            let trimmed = line.trim();
            // Skip separator rows like |---|---|
            if trimmed.contains("---") && !trimmed.chars().any(|c| c.is_alphanumeric()) {
                continue;
            }
            let cells = parse_row(line);
            for (ci, cell) in cells.iter().enumerate() {
                let w = cell.len();
                if ci >= col_widths.len() {
                    col_widths.push(w);
                } else if w > col_widths[ci] {
                    col_widths[ci] = w;
                }
            }
        }

        for (row_idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            // Separator row: render a horizontal line
            if trimmed.contains("---") && !trimmed.chars().any(|c| c.is_alphanumeric()) {
                let total_chars: usize = col_widths.iter().map(|w| w + 3).sum::<usize>() + 1;
                let rule = "─".repeat(total_chars);
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &rule,
                    x,
                    cursor_y,
                    self.color_scheme.named[8],
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_y += line_height;
                continue;
            }

            let cells = parse_row(line);
            let is_header = row_idx == 0;
            let mut cursor_x = x;

            for (ci, cell) in cells.iter().enumerate() {
                let col_w = col_widths.get(ci).copied().unwrap_or(cell.len());
                let padded = format!(" {:width$} │", cell, width = col_w);

                let color = if is_header {
                    self.color_scheme.named[6] // cyan for header
                } else {
                    self.color_scheme.foreground
                };

                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    &padded, cursor_x, cursor_y, color, is_header, false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_x += advance;
            }
            cursor_y += line_height;
        }
    }

    /// Render a line of markdown with inline formatting (**bold**, *italic*, `code`, [links](url)).
    #[allow(clippy::too_many_arguments)]
    fn render_markdown_inline(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        line_bold: bool,
        line_color: super::color::Rgba,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_x = x;
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut i = 0;
        let mut segment = String::new();

        while i < len {
            // **bold**
            if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 2;
                let mut bold_text = String::new();
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                    bold_text.push(chars[i]);
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                } // skip closing **
                cursor_x += self.emit_md_segment(
                    &bold_text,
                    cursor_x,
                    y,
                    true,
                    false,
                    line_color,
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // *italic*
            if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 1;
                let mut italic_text = String::new();
                while i < len && chars[i] != '*' {
                    italic_text.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    i += 1;
                } // skip closing *
                cursor_x += self.emit_md_segment(
                    &italic_text,
                    cursor_x,
                    y,
                    line_bold,
                    true,
                    line_color,
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // ~~strikethrough~~
            if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 2;
                let mut strike_text = String::new();
                while i + 1 < len && !(chars[i] == '~' && chars[i + 1] == '~') {
                    strike_text.push(chars[i]);
                    i += 1;
                }
                if i + 1 < len {
                    i += 2;
                } // skip closing ~~
                  // Render strikethrough text with dim color and strikethrough decoration
                  // Since we don't have actual text decoration support, render with dim + overstrike
                let strike_color = self.color_scheme.named[8]; // dim
                cursor_x += self.emit_md_segment(
                    &strike_text,
                    cursor_x,
                    y,
                    false,
                    false,
                    strike_color,
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // `inline code`
            if chars[i] == '`' {
                if !segment.is_empty() {
                    cursor_x += self.emit_md_segment(
                        &segment,
                        cursor_x,
                        y,
                        line_bold,
                        false,
                        line_color,
                        text_vertices,
                        text_indices,
                    );
                    segment.clear();
                }
                i += 1;
                let mut code_text = String::new();
                while i < len && chars[i] != '`' {
                    code_text.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    i += 1;
                } // skip closing `
                cursor_x += self.emit_md_segment(
                    &code_text,
                    cursor_x,
                    y,
                    false,
                    false,
                    self.color_scheme.named[7],
                    text_vertices,
                    text_indices,
                );
                continue;
            }

            // [link text](url) — render text as underlined
            if chars[i] == '[' {
                let mut link_text = String::new();
                let mut j = i + 1;
                while j < len && chars[j] != ']' {
                    link_text.push(chars[j]);
                    j += 1;
                }
                if j + 1 < len && chars[j] == ']' && chars[j + 1] == '(' {
                    // Skip past (url)
                    j += 2;
                    while j < len && chars[j] != ')' {
                        j += 1;
                    }
                    if j < len {
                        j += 1;
                    }

                    if !segment.is_empty() {
                        cursor_x += self.emit_md_segment(
                            &segment,
                            cursor_x,
                            y,
                            line_bold,
                            false,
                            line_color,
                            text_vertices,
                            text_indices,
                        );
                        segment.clear();
                    }
                    // Render link text in blue/cyan
                    cursor_x += self.emit_md_segment(
                        &link_text,
                        cursor_x,
                        y,
                        false,
                        false,
                        self.color_scheme.named[6],
                        text_vertices,
                        text_indices,
                    );
                    i = j;
                    continue;
                }
            }

            segment.push(chars[i]);
            i += 1;
        }

        if !segment.is_empty() {
            self.emit_md_segment(
                &segment,
                cursor_x,
                y,
                line_bold,
                false,
                line_color,
                text_vertices,
                text_indices,
            );
        }
    }

    /// Emit a styled text segment and return the x advance.
    #[allow(clippy::too_many_arguments)]
    fn emit_md_segment(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        bold: bool,
        italic: bool,
        color: super::color::Rgba,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) -> f32 {
        let (verts, idxs, advance) = self
            .text_renderer
            .build_styled_text_vertices(text, x, y, color, bold, italic);
        let base_idx = text_vertices.len() as u32;
        text_vertices.extend(verts);
        text_indices.extend(idxs.iter().map(|i| i + base_idx));
        advance
    }

    #[allow(clippy::too_many_arguments)]
    fn render_code_content(
        &mut self,
        source: &str,
        language: &str,
        line_numbers: bool,
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_y = y;
        let line_height = self.cell_height;
        let line_num_color = self.color_scheme.named[8];
        let keywords = code_keywords(language);
        let mut in_block_comment = false;

        for (idx, line) in source.lines().enumerate() {
            let mut cursor_x = x;

            if line_numbers {
                let line_num = format!("{:4} ", idx + 1);
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    &line_num,
                    cursor_x,
                    cursor_y,
                    line_num_color,
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_x += advance;
            }

            self.render_code_line(
                line,
                language,
                &keywords,
                &mut in_block_comment,
                cursor_x,
                cursor_y,
                text_vertices,
                text_indices,
            );
            cursor_y += line_height;
        }
    }

    /// Render a single line of code with basic syntax highlighting.
    #[allow(clippy::too_many_arguments)]
    fn render_code_line(
        &mut self,
        line: &str,
        language: &str,
        keywords: &[&str],
        in_block_comment: &mut bool,
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let keyword_color = self.color_scheme.named[5]; // magenta
        let string_color = self.color_scheme.named[2]; // green
        let comment_color = self.color_scheme.named[8]; // bright black (dim)
        let number_color = self.color_scheme.named[3]; // yellow
        let type_color = self.color_scheme.named[6]; // cyan
        let operator_color = self.color_scheme.named[1]; // red
        let code_color = self.color_scheme.named[7]; // white/default code

        // If we're inside a block comment, render entire line as comment
        // and check for closing */
        if *in_block_comment {
            if let Some(end_pos) = line.find("*/") {
                // Comment ends on this line
                let comment_part = &line[..end_pos + 2];
                let rest = &line[end_pos + 2..];
                let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                    comment_part,
                    x,
                    y,
                    comment_color,
                    false,
                    true,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                *in_block_comment = false;
                if !rest.is_empty() {
                    self.render_code_line(
                        rest,
                        language,
                        keywords,
                        in_block_comment,
                        x + advance,
                        y,
                        text_vertices,
                        text_indices,
                    );
                }
            } else {
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    line,
                    x,
                    y,
                    comment_color,
                    false,
                    true,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
            }
            return;
        }

        let trimmed = line.trim_start();

        // Full-line comment detection (language-aware)
        if is_line_comment(trimmed, language) {
            let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                line,
                x,
                y,
                comment_color,
                false,
                true,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            return;
        }

        // Tokenize the line into segments
        let tokens = tokenize_code_line(line, keywords, in_block_comment);
        let mut cursor_x = x;

        for token in &tokens {
            let (color, bold, italic) = match token.kind {
                TokenKind::Keyword => (keyword_color, true, false),
                TokenKind::String => (string_color, false, false),
                TokenKind::Comment => (comment_color, false, true),
                TokenKind::Number => (number_color, false, false),
                TokenKind::Type => (type_color, false, false),
                TokenKind::Operator => (operator_color, false, false),
                TokenKind::Plain => (code_color, false, false),
            };
            let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                &token.text,
                cursor_x,
                y,
                color,
                bold,
                italic,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            cursor_x += advance;
        }
    }
}

// ── Code syntax highlighting helpers ────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Operator,
    Plain,
}

struct CodeToken {
    text: String,
    kind: TokenKind,
}

/// Returns keyword list for a given language identifier.
fn code_keywords(language: &str) -> Vec<&'static str> {
    match language.to_lowercase().as_str() {
        "rust" | "rs" => vec![
            "fn", "let", "mut", "const", "static", "struct", "enum", "impl", "trait", "pub", "use",
            "mod", "crate", "self", "super", "return", "if", "else", "match", "for", "while",
            "loop", "break", "continue", "where", "type", "async", "await", "move", "ref", "as",
            "in", "true", "false",
        ],
        "python" | "py" => vec![
            "def", "class", "import", "from", "return", "if", "elif", "else", "for", "while",
            "break", "continue", "pass", "with", "as", "try", "except", "finally", "raise",
            "yield", "lambda", "and", "or", "not", "in", "is", "True", "False", "None", "self",
            "async", "await",
        ],
        "javascript" | "js" | "typescript" | "ts" => vec![
            "function",
            "const",
            "let",
            "var",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "class",
            "extends",
            "import",
            "export",
            "from",
            "default",
            "new",
            "this",
            "typeof",
            "instanceof",
            "async",
            "await",
            "try",
            "catch",
            "finally",
            "throw",
            "true",
            "false",
            "null",
            "undefined",
        ],
        "go" => vec![
            "func",
            "package",
            "import",
            "return",
            "if",
            "else",
            "for",
            "range",
            "switch",
            "case",
            "break",
            "continue",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "go",
            "defer",
            "select",
            "var",
            "const",
            "true",
            "false",
            "nil",
        ],
        "c" | "cpp" | "c++" | "h" => vec![
            "int", "char", "float", "double", "void", "return", "if", "else", "for", "while", "do",
            "switch", "case", "break", "continue", "struct", "enum", "typedef", "const", "static",
            "extern", "include", "define", "true", "false", "NULL", "nullptr",
        ],
        "ruby" | "rb" => vec![
            "def",
            "end",
            "class",
            "module",
            "require",
            "include",
            "extend",
            "return",
            "if",
            "elsif",
            "else",
            "unless",
            "case",
            "when",
            "for",
            "while",
            "until",
            "do",
            "begin",
            "rescue",
            "ensure",
            "raise",
            "yield",
            "block_given?",
            "self",
            "super",
            "nil",
            "true",
            "false",
            "and",
            "or",
            "not",
            "attr_reader",
            "attr_writer",
            "attr_accessor",
            "puts",
            "print",
            "lambda",
            "proc",
        ],
        "java" => vec![
            "public",
            "private",
            "protected",
            "static",
            "final",
            "abstract",
            "class",
            "interface",
            "extends",
            "implements",
            "import",
            "package",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "new",
            "this",
            "super",
            "try",
            "catch",
            "finally",
            "throw",
            "throws",
            "void",
            "int",
            "long",
            "double",
            "float",
            "boolean",
            "char",
            "byte",
            "short",
            "String",
            "true",
            "false",
            "null",
            "instanceof",
            "synchronized",
            "volatile",
        ],
        _ => vec![
            "fn", "function", "def", "class", "return", "if", "else", "for", "while", "import",
            "from", "let", "const", "var", "true", "false",
        ],
    }
}

/// Multi-character operators to highlight.
const OPERATORS: &[&str] = &[
    "=>", "->", "::", "+=", "-=", "*=", "/=", "%=", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>",
    "..", "?.",
];

/// Check if a trimmed line starts with a line-comment prefix for the given language.
fn is_line_comment(trimmed: &str, language: &str) -> bool {
    // // is a comment in all languages
    if trimmed.starts_with("//") {
        return true;
    }
    // # is a comment in scripting languages; also for unknown/empty (backward compat)
    if trimmed.starts_with('#') {
        let lang = language.to_lowercase();
        let known_no_hash = matches!(
            lang.as_str(),
            "rust"
                | "rs"
                | "c"
                | "cpp"
                | "c++"
                | "h"
                | "javascript"
                | "js"
                | "typescript"
                | "ts"
                | "java"
                | "go"
                | "sql"
                | "lua"
                | "haskell"
                | "hs"
        );
        return !known_no_hash;
    }
    // -- is a comment only in SQL, Lua, Haskell
    if trimmed.starts_with("--") {
        let lang = language.to_lowercase();
        return matches!(lang.as_str(), "sql" | "lua" | "haskell" | "hs");
    }
    false
}

/// Check if a word looks like a type name (starts with uppercase letter).
fn is_type_like(word: &str) -> bool {
    let first = match word.chars().next() {
        Some(c) => c,
        None => return false,
    };
    first.is_ascii_uppercase()
        && word.len() > 1
        && word.chars().skip(1).any(|c| c.is_ascii_lowercase())
}

/// Tokenize a line of code into styled segments.
#[allow(clippy::too_many_lines)]
fn tokenize_code_line(
    line: &str,
    keywords: &[&str],
    in_block_comment: &mut bool,
) -> Vec<CodeToken> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Block comment start: /*
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            let mut s = String::from("/*");
            i += 2;
            while i < len {
                if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                    s.push_str("*/");
                    i += 2;
                    // Block comment closed on same line
                    tokens.push(CodeToken {
                        text: s,
                        kind: TokenKind::Comment,
                    });
                    s = String::new();
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            if !s.is_empty() {
                // Block comment continues to next line
                *in_block_comment = true;
                tokens.push(CodeToken {
                    text: s,
                    kind: TokenKind::Comment,
                });
            }
            continue;
        }

        // Inline comment: //
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            tokens.push(CodeToken {
                text: chars[i..].iter().collect(),
                kind: TokenKind::Comment,
            });
            break;
        }

        // Python triple-quoted strings: """...""" or '''...'''
        if i + 2 < len
            && (chars[i] == '"' || chars[i] == '\'')
            && chars[i + 1] == chars[i]
            && chars[i + 2] == chars[i]
        {
            let quote = chars[i];
            let mut s: String = chars[i..i + 3].iter().collect();
            i += 3;
            loop {
                if i + 2 < len
                    && chars[i] == quote
                    && chars[i + 1] == quote
                    && chars[i + 2] == quote
                {
                    s.push(quote);
                    s.push(quote);
                    s.push(quote);
                    i += 3;
                    break;
                }
                if i >= len {
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::String,
            });
            continue;
        }

        // Rust raw strings: r"..." or r#"..."#
        if chars[i] == 'r' && i + 1 < len && (chars[i + 1] == '"' || chars[i + 1] == '#') {
            let mut s = String::from('r');
            let mut j = i + 1;
            let mut hash_count = 0usize;
            while j < len && chars[j] == '#' {
                s.push('#');
                hash_count += 1;
                j += 1;
            }
            if j < len && chars[j] == '"' {
                s.push('"');
                j += 1;
                // Consume until closing " followed by hash_count #'s
                'raw_outer: loop {
                    if j >= len {
                        break;
                    }
                    if chars[j] == '"' {
                        // Check for matching closing hashes
                        let mut ok = true;
                        for k in 1..=hash_count {
                            if j + k >= len || chars[j + k] != '#' {
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            s.push('"');
                            j += 1;
                            for _ in 0..hash_count {
                                s.push('#');
                                j += 1;
                            }
                            break 'raw_outer;
                        }
                    }
                    s.push(chars[j]);
                    j += 1;
                }
                i = j;
                tokens.push(CodeToken {
                    text: s,
                    kind: TokenKind::String,
                });
                continue;
            }
            // Not a raw string (e.g. just identifier starting with 'r'), fall through
        }

        // Backtick template literals (JS/TS): `...`
        if chars[i] == '`' {
            let mut s = String::from('`');
            i += 1;
            while i < len && chars[i] != '`' {
                if chars[i] == '\\' && i + 1 < len {
                    s.push(chars[i]);
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < len {
                s.push('`');
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::String,
            });
            continue;
        }

        // String literals: "..." or '...'
        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            let mut s = String::new();
            s.push(chars[i]);
            i += 1;
            while i < len && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < len {
                    s.push(chars[i]);
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < len {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::String,
            });
            continue;
        }

        // Numbers
        if chars[i].is_ascii_digit()
            || (chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit())
        {
            let mut s = String::new();
            while i < len
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::Number,
            });
            continue;
        }

        // Identifiers / keywords / types
        if chars[i].is_alphanumeric() || chars[i] == '_' {
            let mut word = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                word.push(chars[i]);
                i += 1;
            }
            let kind = if keywords.contains(&word.as_str()) {
                TokenKind::Keyword
            } else if is_type_like(&word) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            tokens.push(CodeToken { text: word, kind });
            continue;
        }

        // Multi-character operators
        let remaining: String = chars[i..].iter().collect();
        let mut matched_op = false;
        for op in OPERATORS {
            if remaining.starts_with(op) {
                tokens.push(CodeToken {
                    text: op.to_string(),
                    kind: TokenKind::Operator,
                });
                i += op.len();
                matched_op = true;
                break;
            }
        }
        if matched_op {
            continue;
        }

        // Whitespace and single-char operators — accumulate as plain
        let mut plain = String::new();
        while i < len
            && !chars[i].is_alphanumeric()
            && chars[i] != '_'
            && chars[i] != '"'
            && chars[i] != '\''
            && chars[i] != '`'
            && !(i + 1 < len && chars[i] == '/' && chars[i + 1] == '/')
            && !(i + 1 < len && chars[i] == '/' && chars[i + 1] == '*')
        {
            // Check for multi-char operators before consuming
            let rest: String = chars[i..].iter().collect();
            if OPERATORS.iter().any(|op| rest.starts_with(op)) {
                break;
            }
            plain.push(chars[i]);
            i += 1;
        }
        if !plain.is_empty() {
            tokens.push(CodeToken {
                text: plain,
                kind: TokenKind::Plain,
            });
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── code_keywords tests ─────────────────────────────────────────

    #[test]
    fn code_keywords_rust_and_rs_are_equivalent() {
        let rust = code_keywords("rust");
        let rs = code_keywords("rs");
        assert_eq!(rust, rs);
        assert!(rust.contains(&"fn"));
        assert!(rust.contains(&"let"));
        assert!(rust.contains(&"struct"));
    }

    #[test]
    fn code_keywords_python() {
        let py = code_keywords("python");
        assert_eq!(py, code_keywords("py"));
        assert!(py.contains(&"def"));
        assert!(py.contains(&"class"));
        assert!(py.contains(&"lambda"));
    }

    #[test]
    fn code_keywords_javascript_typescript() {
        let js = code_keywords("javascript");
        assert_eq!(js, code_keywords("js"));
        assert_eq!(js, code_keywords("typescript"));
        assert_eq!(js, code_keywords("ts"));
        assert!(js.contains(&"function"));
        assert!(js.contains(&"const"));
        assert!(js.contains(&"async"));
    }

    #[test]
    fn code_keywords_unknown_returns_default_and_known_non_empty() {
        let default = code_keywords("brainfuck");
        assert!(default.contains(&"fn"));
        assert!(default.contains(&"function"));
        assert!(default.contains(&"def"));

        // Known languages return non-empty lists
        for lang in &["go", "c", "ruby", "java"] {
            assert!(
                !code_keywords(lang).is_empty(),
                "{lang} should have keywords"
            );
        }
    }

    // ── is_type_like tests ──────────────────────────────────────────

    #[test]
    fn is_type_like_uppercase_start_with_lowercase() {
        assert!(is_type_like("String"));
        assert!(is_type_like("Vec"));
        assert!(is_type_like("MyStruct"));
        assert!(is_type_like("Ab"));
    }

    #[test]
    fn is_type_like_rejects_lowercase_start() {
        assert!(!is_type_like("string"));
        assert!(!is_type_like("myStruct"));
    }

    #[test]
    fn is_type_like_rejects_all_uppercase() {
        assert!(!is_type_like("HTTP"));
        assert!(!is_type_like("ABC"));
    }

    #[test]
    fn is_type_like_rejects_empty_and_single_char() {
        assert!(!is_type_like(""));
        assert!(!is_type_like("A"));
    }

    #[test]
    fn is_type_like_edge_cases() {
        assert!(!is_type_like("IO")); // all uppercase, no lowercase after first
        assert!(is_type_like("Io")); // upper + lower
        assert!(!is_type_like("123")); // digits
    }

    // ── tokenize_code_line tests ────────────────────────────────────

    fn rust_keywords() -> Vec<&'static str> {
        code_keywords("rust")
    }

    #[test]
    fn tokenize_keyword_in_rust() {
        let mut bc = false;
        let tokens = tokenize_code_line("let x = 5", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[0].text, "let");
        assert!(!bc);
    }

    #[test]
    fn tokenize_string_double_quotes() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""hello world""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, "\"hello world\"");
    }

    #[test]
    fn tokenize_string_with_escaped_quote() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""hello \"world\"""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.contains("\\\""));
    }

    #[test]
    fn tokenize_single_quoted_string() {
        let mut bc = false;
        let tokens = tokenize_code_line("'c'", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, "'c'");
    }

    #[test]
    fn tokenize_inline_comment() {
        let mut bc = false;
        let tokens = tokenize_code_line("x // comment", &rust_keywords(), &mut bc);
        let comment_token = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .unwrap();
        assert_eq!(comment_token.text, "// comment");
        assert!(!bc);
    }

    #[test]
    fn tokenize_block_comment_single_line() {
        let mut bc = false;
        let tokens = tokenize_code_line("a /* b */ c", &rust_keywords(), &mut bc);
        let comment_token = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .unwrap();
        assert!(comment_token.text.contains("/* b */"));
        assert!(!bc, "block comment should close on same line");
    }

    #[test]
    fn tokenize_block_comment_spanning_lines() {
        let mut bc = false;
        let tokens = tokenize_code_line("a /* b", &rust_keywords(), &mut bc);
        assert!(bc, "block comment should remain open");
        let comment_token = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .unwrap();
        assert!(comment_token.text.starts_with("/*"));
    }

    #[test]
    fn tokenize_integer_number() {
        let mut bc = false;
        let tokens = tokenize_code_line("42", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].text, "42");
    }

    #[test]
    fn tokenize_float_number() {
        let mut bc = false;
        let tokens = tokenize_code_line("3.14", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].text, "3.14");
    }

    #[test]
    fn tokenize_hex_number() {
        let mut bc = false;
        let tokens = tokenize_code_line("0xFF", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].text, "0xFF");
    }

    #[test]
    fn tokenize_type_like_word() {
        let mut bc = false;
        let tokens = tokenize_code_line("Vec", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Type);
        assert_eq!(tokens[0].text, "Vec");
    }

    #[test]
    fn tokenize_multi_char_operator() {
        let mut bc = false;
        let tokens = tokenize_code_line("=>", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Operator);
        assert_eq!(tokens[0].text, "=>");
    }

    #[test]
    fn tokenize_fn_main_line() {
        let mut bc = false;
        let tokens = tokenize_code_line("fn main() {", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[0].text, "fn");
        // "main" should be Plain (not a keyword)
        let main_tok = tokens.iter().find(|t| t.text == "main").unwrap();
        assert_eq!(main_tok.kind, TokenKind::Plain);
    }

    #[test]
    fn tokenize_empty_line() {
        let mut bc = false;
        let tokens = tokenize_code_line("", &rust_keywords(), &mut bc);
        assert!(tokens.is_empty());
    }

    #[test]
    fn tokenize_whitespace_only() {
        let mut bc = false;
        let tokens = tokenize_code_line("   ", &rust_keywords(), &mut bc);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Plain);
        assert_eq!(tokens[0].text, "   ");
    }

    #[test]
    fn tokenize_operator_and_and() {
        let mut bc = false;
        let tokens = tokenize_code_line("a && b", &rust_keywords(), &mut bc);
        let op = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Operator)
            .unwrap();
        assert_eq!(op.text, "&&");
    }

    // ── is_line_comment tests ───────────────────────────────────────

    #[test]
    fn rust_attribute_not_comment() {
        assert!(!is_line_comment("#[derive(Debug)]", "rust"));
        assert!(!is_line_comment("#[cfg(test)]", "rs"));
    }

    #[test]
    fn c_include_not_comment() {
        assert!(!is_line_comment("#include <stdio.h>", "c"));
        assert!(!is_line_comment("#define FOO", "cpp"));
    }

    #[test]
    fn python_hash_is_comment() {
        assert!(is_line_comment("# this is a comment", "python"));
        assert!(is_line_comment("# comment", "py"));
    }

    #[test]
    fn sql_double_dash_is_comment() {
        assert!(is_line_comment("-- select all", "sql"));
        assert!(is_line_comment("-- comment", "lua"));
    }

    #[test]
    fn unknown_language_hash_is_comment() {
        assert!(is_line_comment("# comment", ""));
        assert!(is_line_comment("# comment", "unknown_lang"));
        // But -- is not a comment for unknown languages
        assert!(!is_line_comment("-- something", ""));
    }

    #[test]
    fn double_slash_comment_all_languages() {
        assert!(is_line_comment("// comment", "rust"));
        assert!(is_line_comment("// comment", "python"));
        assert!(is_line_comment("// comment", "sql"));
        assert!(is_line_comment("// comment", ""));
    }

    // ── tokenizer string literal improvement tests ──────────────────

    #[test]
    fn tokenize_backtick_simple() {
        let mut bc = false;
        let tokens = tokenize_code_line("`hello world`", &code_keywords("js"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, "`hello world`");
    }

    #[test]
    fn tokenize_backtick_with_escape() {
        let mut bc = false;
        let tokens = tokenize_code_line(r"`hello \` world`", &code_keywords("js"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with('`'));
        assert!(tokens[0].text.ends_with('`'));
    }

    #[test]
    fn tokenize_raw_string_no_hash() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#"r"hello""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, r#"r"hello""#);
    }

    #[test]
    fn tokenize_raw_string_with_hashes() {
        let mut bc = false;
        let input = r###"r#"hello "world""#"###;
        let tokens = tokenize_code_line(input, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with("r#\""));
        assert!(tokens[0].text.ends_with("\"#"));
    }

    #[test]
    fn tokenize_triple_quote_double() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""""hello world""""#, &code_keywords("python"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with("\"\"\""));
        assert!(tokens[0].text.ends_with("\"\"\""));
    }

    #[test]
    fn tokenize_triple_quote_single() {
        let mut bc = false;
        let tokens = tokenize_code_line("'''hello world'''", &code_keywords("python"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with("'''"));
        assert!(tokens[0].text.ends_with("'''"));
    }

    // ── tokenizer edge case tests ───────────────────────────────────

    #[test]
    fn tokenize_unclosed_string_at_eol() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#"let s = "unterminated"#, &rust_keywords(), &mut bc);
        // Should still produce a String token (without closing quote)
        let string_tok = tokens
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .expect("should have a string token");
        assert!(string_tok.text.starts_with('"'));
        assert!(!string_tok.text.ends_with('"'));
    }

    #[test]
    fn tokenize_escaped_backslash_at_end() {
        let mut bc = false;
        // "test\\" — the \\ is an escaped backslash, string should close properly
        let tokens = tokenize_code_line(r#""test\\""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, r#""test\\""#);
    }

    #[test]
    fn tokenize_nested_block_comment() {
        let mut bc = false;
        // /* /* */ */ — first */ closes the comment, second */ is plain text
        let tokens = tokenize_code_line("/* /* */ */", &rust_keywords(), &mut bc);
        let comment = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .expect("should have comment token");
        assert_eq!(comment.text, "/* /* */");
        assert!(!bc, "block comment should be closed");
        // Remaining " */" should be plain tokens
        let rest: String = tokens.iter().skip(1).map(|t| t.text.as_str()).collect();
        assert!(rest.contains("*/"));
    }

    #[test]
    fn tokenize_unicode_identifier() {
        let mut bc = false;
        let tokens = tokenize_code_line("let café = 42", &rust_keywords(), &mut bc);
        let kw = &tokens[0];
        assert_eq!(kw.kind, TokenKind::Keyword);
        assert_eq!(kw.text, "let");
        let ident = tokens
            .iter()
            .find(|t| t.text.contains("caf"))
            .expect("should find café identifier");
        assert_eq!(ident.text, "café");
        assert_eq!(ident.kind, TokenKind::Plain);
    }

    #[test]
    fn tokenize_mixed_quotes() {
        let mut bc = false;
        // 'hello"world' — single-quoted string containing a double quote
        let tokens = tokenize_code_line(r#"'hello"world'"#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, r#"'hello"world'"#);
    }

    #[test]
    fn tokenize_unclosed_backtick_at_eol() {
        let mut bc = false;
        let tokens = tokenize_code_line("`unterminated", &code_keywords("js"), &mut bc);
        let string_tok = tokens
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .expect("should have a string token");
        assert!(string_tok.text.starts_with('`'));
        assert!(!string_tok.text.ends_with('`'));
    }

    #[test]
    fn tokenize_unclosed_triple_quote_at_eol() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""""unterminated"#, &code_keywords("python"), &mut bc);
        let string_tok = tokens
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .expect("should have a string token");
        assert!(string_tok.text.starts_with("\"\"\""));
        // Not closed, so should contain the rest of the line
        assert!(string_tok.text.contains("unterminated"));
    }
}
