use super::super::{DeferredImageRender, Renderer, Vertex};

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_widget_content(
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
                let bar_height = h.min(self.cell_height).round();
                let bar_y = (y + (h - bar_height) / 2.0).round();

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
                    is_color: 0,
                });
                bg_vertices.push(Vertex {
                    position: [x + w, bar_y],
                    tex_coord: [1.0, 0.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
                    is_color: 0,
                });
                bg_vertices.push(Vertex {
                    position: [x + w, bar_y + bar_height],
                    tex_coord: [1.0, 1.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
                    is_color: 0,
                });
                bg_vertices.push(Vertex {
                    position: [x, bar_y + bar_height],
                    tex_coord: [0.0, 1.0],
                    color: [bg_color.r, bg_color.g, bg_color.b, 0.5],
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
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [hx + highlight_w, bar_y],
                        tex_coord: [1.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [hx + highlight_w, bar_y + bar_height],
                        tex_coord: [1.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [hx, bar_y + bar_height],
                        tex_coord: [0.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
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
                } else if filled_width > 0.0 {
                    let base_idx = bg_vertices.len() as u32;
                    bg_vertices.push(Vertex {
                        position: [x, bar_y],
                        tex_coord: [0.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [x + filled_width, bar_y],
                        tex_coord: [1.0, 0.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [x + filled_width, bar_y + bar_height],
                        tex_coord: [1.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
                        is_color: 0,
                    });
                    bg_vertices.push(Vertex {
                        position: [x, bar_y + bar_height],
                        tex_coord: [0.0, 1.0],
                        color: [fg_color.r, fg_color.g, fg_color.b, 1.0],
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
                    is_color: 0,
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y],
                    tex_coord: [1.0, 0.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
                    is_color: 0,
                });
                bg_vertices.push(Vertex {
                    position: [x + w, y + h],
                    tex_coord: [1.0, 1.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
                    is_color: 0,
                });
                bg_vertices.push(Vertex {
                    position: [x, y + h],
                    tex_coord: [0.0, 1.0],
                    color: [border_color.r, border_color.g, border_color.b, 0.3],
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
                    x,
                    y,
                    text_color,
                    false,
                    false,
                );
                let base_idx = text_vertices.len() as u32;
                text_vertices.extend(verts);
                text_indices.extend(idxs.iter().map(|i| i + base_idx));
            }
            // Deprecated by ADR 0018: the terminal no longer interprets
            // Markdown or highlights Code. It shows the source verbatim so
            // older apps stay legible during the overlap release.
            WidgetContent::Markdown(md) => {
                self.render_plain_source(&md.source, x, y, text_vertices, text_indices);
            }
            WidgetContent::Code(code) => {
                self.render_plain_source(&code.source, x, y, text_vertices, text_indices);
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

                let fg_col = match button.style {
                    Bs::Primary => self.color_scheme.named[4],
                    Bs::Danger => self.color_scheme.named[1],
                    Bs::Success => self.color_scheme.named[2],
                    Bs::Default => self.color_scheme.foreground,
                };

                let label = button.label.clone();
                let label_px = label.chars().count() as f32 * self.cell_width;
                let offset_x = ((w - label_px) / 2.0).max(0.0);
                let offset_y = ((h - self.cell_height) / 2.0).max(0.0);
                let (verts, idxs, _) = self.text_renderer.build_styled_text_vertices(
                    &label,
                    x + offset_x,
                    y + offset_y,
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

    /// Draw `source` one line per cell row in the foreground color.
    fn render_plain_source(
        &mut self,
        source: &str,
        x: f32,
        y: f32,
        text_vertices: &mut Vec<Vertex>,
        text_indices: &mut Vec<u32>,
    ) {
        let color = self.color_scheme.foreground;
        for (row, line) in source.lines().enumerate() {
            self.text_renderer.append_styled_text_vertices(
                line,
                x,
                y + row as f32 * self.cell_height,
                color,
                false,
                false,
                text_vertices,
                text_indices,
            );
        }
    }
}
