//! Native protocol session rendering.
//!
//! Renders native protocol scene trees (backgrounds, text, widgets, images)
//! for sessions using the ultty-protocol native UI framework.

use super::{DeferredImageRender, Renderer, Vertex};
use wgpu::util::DeviceExt;

impl Renderer {
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
        let mut bg_vertices = Vec::new();
        let mut bg_indices = Vec::new();
        let mut text_vertices = Vec::new();
        let mut text_indices = Vec::new();
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

            match &region.content {
                Content::Text(text_content) => {
                    let fg_color = region
                        .style
                        .fg
                        .as_ref()
                        .map(|c| self.color_scheme.resolve_protocol_color(c))
                        .unwrap_or(self.color_scheme.foreground);

                    let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                        &text_content.text,
                        x,
                        y,
                        fg_color,
                        region.style.bold,
                        region.style.italic,
                    );

                    let base_idx = text_vertices.len() as u32;
                    text_vertices.extend(verts);
                    text_indices.extend(idxs.iter().map(|i| i + base_idx));
                }
                Content::RichText(rich_text) => {
                    let mut cursor_x = x;
                    for span in &rich_text.spans {
                        let fg_color = span
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

                        let (verts, idxs, advance) = self.text_renderer.build_styled_text_vertices(
                            &span.text,
                            cursor_x,
                            y,
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
                        x,
                        y,
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
                    for (match_idx, m) in matches.iter().enumerate().filter(|(_, m)| m.region_id == region.id) {
                        // Approximate character position: byte_offset * char_width
                        let char_w = self.cell_width;
                        let highlight_x = x + m.byte_offset as f32 * char_w;
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
            rp.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
        }

        if !text_vertices.is_empty() {
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
            rp.draw_indexed(0..text_indices.len() as u32, 0, 0..1);
        }

        // Render deferred native images using the image pipeline
        for img_render in &deferred_images {
            let Some(cached) = self.native_image_cache.get(&img_render.region_id) else {
                continue;
            };

            let color = [1.0f32, 1.0, 1.0, 1.0];
            let vertices = vec![
                Vertex { position: [img_render.x, img_render.y], tex_coord: [0.0, 0.0], color },
                Vertex { position: [img_render.x + img_render.w, img_render.y], tex_coord: [1.0, 0.0], color },
                Vertex { position: [img_render.x + img_render.w, img_render.y + img_render.h], tex_coord: [1.0, 1.0], color },
                Vertex { position: [img_render.x, img_render.y + img_render.h], tex_coord: [0.0, 1.0], color },
            ];
            let indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native_image_vb"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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
                    if phase < 1.0 { phase } else { 2.0 - phase }
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
                static BRAILLE_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
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

                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &text,
                    x,
                    y,
                    fg_color,
                    false,
                    false,
                );
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

                let display_text = if input.value.is_empty() {
                    &input.placeholder
                } else if input.password {
                    &"•".repeat(input.value.len())
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
                self.render_markdown_content(
                    &md.source,
                    x,
                    y,
                    w,
                    text_vertices,
                    text_indices,
                );
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

        for line in source.lines() {
            let (text, bold, color) = if let Some(stripped) = line.strip_prefix("### ") {
                (stripped, true, self.color_scheme.named[6])
            } else if let Some(stripped) = line.strip_prefix("## ") {
                (stripped, true, self.color_scheme.named[5])
            } else if let Some(stripped) = line.strip_prefix("# ") {
                (stripped, true, self.color_scheme.named[4])
            } else if line.starts_with("```") {
                (line, false, self.color_scheme.named[8])
            } else if line.starts_with("- ") || line.starts_with("* ") {
                let bullet_text = format!("• {}", &line[2..]);
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &bullet_text,
                    x,
                    cursor_y,
                    self.color_scheme.foreground,
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
                cursor_y += line_height;
                continue;
            } else {
                (line, false, self.color_scheme.foreground)
            };

            let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                text,
                x,
                cursor_y,
                color,
                bold,
                false,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            cursor_y += line_height;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_code_content(
        &mut self,
        source: &str,
        _language: &str,
        line_numbers: bool,
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let mut cursor_y = y;
        let line_height = self.cell_height;
        let code_color = self.color_scheme.named[7];
        let line_num_color = self.color_scheme.named[8];

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

            let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                line,
                cursor_x,
                cursor_y,
                code_color,
                false,
                false,
            );
            let base_idx = text_vertices.len() as u32;
            text_vertices.extend(verts);
            text_indices.extend(idxs.iter().map(|i| i + base_idx));
            cursor_y += line_height;
        }
    }
}
