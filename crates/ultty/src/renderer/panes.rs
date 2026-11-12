//! Multi-pane rendering.
//!
//! Handles rendering multiple terminal panes in split-view layout,
//! including per-pane backgrounds, text, borders, and the shared
//! tab bar / vim indicator / shortcuts overlay.

use super::block_chars::block_char_rects;
use super::native_render::{scissor_for, PaneBounds};
use super::powerline::powerline_triangle;
use super::{OverlayState, PaneDraw, PaneInfo, Renderer, Rgba, Vertex};
use crate::grid::{CellFlags, CursorShape, Terminal};
use wgpu::util::DeviceExt;

impl Renderer {
    /// Draw multiple terminal panes into `view`, without presenting the
    /// frame.
    pub fn render_panes(
        &mut self,
        view: &wgpu::TextureView,
        panes: &[PaneDraw<'_>],
        overlay: &OverlayState,
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("panes_render_encoder"),
            });

        self.encode_panes(&mut encoder, view, panes, overlay);

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Record the whole multi-pane frame (clear, chrome, panes, borders,
    /// overlays) into `encoder`, drawing onto `view`. The window and the
    /// screenshot path both draw through this, so they stay pixel-identical.
    ///
    /// A pane with a bound full-scene session shows the scene in place of
    /// its grid text, in its own rectangle and clipped to it by a scissor
    /// rectangle (so widget content stays inside too), under its border
    /// (ADR 0025 §3). Each such pane is one vertex batch; the batches are
    /// cached like the one-pane frame's until a session changes or a pane
    /// moves.
    pub(super) fn encode_panes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        panes: &[PaneDraw<'_>],
        overlay: &OverlayState,
    ) {
        // Update tab bar height from overlay state
        self.tab_bar_height = overlay
            .tab_bar
            .map_or(crate::tabs::TERMINAL_TOP_PADDING, |tb| tb.content_top());
        self.text_renderer.begin_frame();

        // Clear screen with background color
        // Convert sRGB colors to linear space for correct rendering in sRGB framebuffer
        let linear_bg = self.color_scheme.background.to_linear();
        {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Pre-multiplied alpha for transparency support
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: linear_bg.r as f64 * self.opacity as f64,
                            g: linear_bg.g as f64 * self.opacity as f64,
                            b: linear_bg.b as f64 * self.opacity as f64,
                            a: self.opacity as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }

        // Build tab bar vertices if visible
        if let Some(tab_bar) = overlay.tab_bar {
            let (tab_bar_vertices, tab_bar_indices) = self.build_tab_bar_vertices(tab_bar);
            if !tab_bar_vertices.is_empty() {
                let vb = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("tab_bar_vb"),
                        contents: bytemuck::cast_slice(&tab_bar_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                let ib = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("tab_bar_ib"),
                        contents: bytemuck::cast_slice(&tab_bar_indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("tab_bar_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                rp.set_pipeline(self.bg_pipeline.pipeline());
                rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                rp.set_vertex_buffer(0, vb.slice(..));
                rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..tab_bar_indices.len() as u32, 0, 0..1);
            }

            // Render tab titles
            if tab_bar.visible {
                let (tv, ti) = self.build_tab_title_vertices(tab_bar);
                if !tv.is_empty() {
                    let vb = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tab_title_vb"),
                            contents: bytemuck::cast_slice(&tv),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    let ib = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tab_title_ib"),
                            contents: bytemuck::cast_slice(&ti),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("tab_title_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });

                    rp.set_pipeline(self.text_renderer.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..ti.len() as u32, 0, 0..1);
                }
            }
        }

        // Full scenes bound to panes, each with the rectangle it is drawn
        // in; the cache is reused while none changed and none moved.
        let tab_bar_height = self.tab_bar_height;
        let pane_bounds = move |pane: &PaneInfo| -> PaneBounds {
            let y = pane.y + tab_bar_height;
            (pane.x, y, pane.x + pane.width, y + pane.height)
        };
        let native_panes: Vec<(u64, PaneBounds)> = panes
            .iter()
            .flat_map(|pane| {
                let bounds = pane_bounds(&pane.info);
                pane.sessions
                    .iter()
                    .filter(|session| session.is_full_scene())
                    .map(move |session| (session.session_id(), bounds))
            })
            .collect();
        // Inline surfaces are drawn on top of the grid for their owning pane.
        // Rebuild while any exist until their pane geometry joins the cache key.
        let has_inline = panes.iter().any(|pane| {
            !pane.sessions.iter().any(|s| s.is_full_scene())
                && !self
                    .visible_surfaces(pane.terminal, &pane.sessions)
                    .is_empty()
        });
        let any_dirty = has_inline
            || panes
                .iter()
                .flat_map(|pane| pane.sessions.iter())
                .any(|session| session.is_render_dirty());
        let native_cached =
            !native_panes.is_empty() && self.native_cache_valid(any_dirty, &[], &native_panes);
        if (!native_panes.is_empty() && !native_cached) || has_inline {
            self.clear_cached_native_buffers();
        }
        let surface = self.surface_size();

        // Render each pane
        for pane in panes {
            let terminal = pane.terminal;
            let info = &pane.info;
            self.pane_offset_x = info.x;
            self.pane_offset_y = info.y + self.tab_bar_height;
            let cursor_blink_visible =
                pane_cursor_blink_visible(overlay.cursor_blink_visible, info.is_focused);

            // Render pane backgrounds
            let (bg_v, bg_i) =
                self.build_background_vertices_with_offset(terminal, cursor_blink_visible);
            if !bg_v.is_empty() {
                let vb = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("pane_bg_vb"),
                        contents: bytemuck::cast_slice(&bg_v),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                let ib = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("pane_bg_ib"),
                        contents: bytemuck::cast_slice(&bg_i),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("pane_bg_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                rp.set_pipeline(self.bg_pipeline.pipeline());
                rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                rp.set_vertex_buffer(0, vb.slice(..));
                rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..bg_i.len() as u32, 0, 0..1);
            }

            let scenes: Vec<_> = pane
                .sessions
                .iter()
                .copied()
                .filter(|session| session.is_full_scene())
                .collect();
            if scenes.is_empty() {
                // Render pane text
                self.text_renderer.render_with_offset(
                    encoder,
                    view,
                    terminal,
                    &self
                        .color_scheme
                        .for_terminal(terminal.palette(), terminal.modes().reverse_video),
                    self.cell_width,
                    self.cell_height,
                    self.pane_offset_x,
                    self.pane_offset_y,
                    cursor_blink_visible,
                    &self.uniform_bind_group,
                );
                let bounds = pane_bounds(info);
                let mut surfaces = self.visible_surfaces(terminal, &pane.sessions);
                for (_, origin) in &mut surfaces {
                    origin.top += info.y;
                }
                for session in &pane.sessions {
                    let origins: Vec<_> = surfaces
                        .iter()
                        .filter(|(id, _)| *id == session.session_id())
                        .map(|(_, origin)| *origin)
                        .collect();
                    if !origins.is_empty() {
                        self.render_native_surfaces(encoder, view, session, &origins, bounds);
                    }
                }
            } else if !native_cached {
                // The scenes hide the grid: one batch for this pane, drawn
                // within its rectangle.
                let bounds = pane_bounds(info);
                let origin = (bounds.0, bounds.1);
                let regions: usize = scenes
                    .iter()
                    .map(|session| session.scene().iter_depth_first().count())
                    .sum();
                let mut native = self.native_vertices_for(regions);
                for session in scenes {
                    self.build_native_session_vertices(
                        session,
                        origin,
                        Some(bounds),
                        None,
                        0,
                        &mut native,
                    );
                }
                if let Some(scissor) = scissor_for(bounds, surface) {
                    self.draw_native_vertices(encoder, view, native, Some(scissor));
                }
            }
        }

        if native_cached {
            self.draw_cached_native_buffers(encoder, view);
        } else if !native_panes.is_empty() {
            self.retain_native_geometry(native_panes.iter().map(|(id, _)| *id));
            self.cached_native_surfaces.clear();
            self.cached_native_panes = native_panes;
        }

        // Pane borders go over the panes' content, scenes included.
        for pane in panes {
            let info = &pane.info;
            // Draw pane border if not focused
            if !info.is_focused {
                let border_color = Rgba::new(0.3, 0.3, 0.3, 1.0);
                let (bv, bi) = self.build_pane_border_vertices(info, border_color);
                if !bv.is_empty() {
                    let vb = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("pane_border_vb"),
                            contents: bytemuck::cast_slice(&bv),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    let ib = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("pane_border_ib"),
                            contents: bytemuck::cast_slice(&bi),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("pane_border_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });

                    rp.set_pipeline(self.bg_pipeline.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..bi.len() as u32, 0, 0..1);
                }
            } else {
                // Draw focused pane border in accent color
                let accent_color = Rgba::new(0.3, 0.5, 0.8, 1.0);
                let (bv, bi) = self.build_pane_border_vertices(info, accent_color);
                if !bv.is_empty() {
                    let vb = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("focused_border_vb"),
                            contents: bytemuck::cast_slice(&bv),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    let ib = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("focused_border_ib"),
                            contents: bytemuck::cast_slice(&bi),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("focused_border_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });

                    rp.set_pipeline(self.bg_pipeline.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..bi.len() as u32, 0, 0..1);
                }
            }
        }

        // Reset pane offset
        self.pane_offset_x = 0.0;
        self.pane_offset_y = 0.0;

        // Render vim mode indicator if in copy mode
        if let Some(copy_mode) = overlay.copy_mode {
            if copy_mode.is_active() {
                let (indicator_vertices, indicator_indices) =
                    self.build_vim_mode_indicator_vertices(copy_mode);
                if !indicator_vertices.is_empty() {
                    // Render background quads
                    let bg_vertices: Vec<_> = indicator_vertices.iter().take(4).cloned().collect();
                    let bg_indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

                    let indicator_bg_vb =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("indicator_bg_vb"),
                                contents: bytemuck::cast_slice(&bg_vertices),
                                usage: wgpu::BufferUsages::VERTEX,
                            });
                    let indicator_bg_ib =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("indicator_bg_ib"),
                                contents: bytemuck::cast_slice(&bg_indices),
                                usage: wgpu::BufferUsages::INDEX,
                            });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("indicator_bg_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });

                    rp.set_pipeline(self.bg_pipeline.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_vertex_buffer(0, indicator_bg_vb.slice(..));
                    rp.set_index_buffer(indicator_bg_ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
                }

                // Render text vertices
                if indicator_vertices.len() > 4 {
                    let text_vertices: Vec<_> = indicator_vertices.into_iter().skip(4).collect();
                    let text_indices: Vec<u32> = indicator_indices
                        .into_iter()
                        .filter(|i| *i >= 4)
                        .map(|i| i - 4)
                        .collect();

                    if !text_vertices.is_empty() {
                        let indicator_text_vb =
                            self.device
                                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("indicator_text_vb"),
                                    contents: bytemuck::cast_slice(&text_vertices),
                                    usage: wgpu::BufferUsages::VERTEX,
                                });
                        let indicator_text_ib =
                            self.device
                                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("indicator_text_ib"),
                                    contents: bytemuck::cast_slice(&text_indices),
                                    usage: wgpu::BufferUsages::INDEX,
                                });

                        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("indicator_text_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                resolve_target: None,
                                depth_slice: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });

                        rp.set_pipeline(self.text_renderer.pipeline());
                        rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                        rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
                        rp.set_vertex_buffer(0, indicator_text_vb.slice(..));
                        rp.set_index_buffer(indicator_text_ib.slice(..), wgpu::IndexFormat::Uint32);
                        rp.draw_indexed(0..text_indices.len() as u32, 0, 0..1);
                    }
                }
            }
        }

        if let Some(preedit) = overlay.ime_preedit {
            self.render_ime_preedit(encoder, view, preedit);
        }

        // Render keyboard shortcuts overlay if visible
        if overlay.shortcuts_visible {
            if let Some(keybindings) = overlay.keybindings {
                self.render_shortcuts_overlay(encoder, view, keybindings);
            }
        }
        if let Some(rows) = overlay.protocol_status {
            self.render_protocol_status_overlay(encoder, view, rows);
        }
        if let Some(tab_bar) = overlay.tab_bar {
            self.render_main_menu(encoder, view, tab_bar);
        }
    }

    /// Build background vertices with pane offset.
    pub(super) fn build_background_vertices_with_offset(
        &self,
        terminal: &Terminal,
        cursor_blink_visible: bool,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();
        let scheme = self
            .color_scheme
            .for_terminal(terminal.palette(), terminal.modes().reverse_video);
        // Cells in this color need no quad: the frame was cleared to it.
        // Under reverse video the screen's default background differs from
        // it, so those cells are drawn too.
        let cleared = self.color_scheme.background;
        let selection_bg = scheme.selection_background();
        let selection = terminal.selection();
        // Pre-allocate based on terminal size: ~1 bg run per row on average = rows quads
        let est_quads = dims.rows as usize * 4;
        let mut vertices = Vec::with_capacity(est_quads * 4);
        let mut indices = Vec::with_capacity(est_quads * 6);

        for row_idx in 0..dims.rows {
            let row = match buffer.visible_row(row_idx) {
                Some(r) => r,
                None => continue,
            };

            let mut run_start = 0u16;
            let mut run_color: Option<Rgba> = None;

            for col_idx in 0..dims.cols {
                let cell = match row.cell(col_idx) {
                    Some(c) => c,
                    None => continue,
                };

                let is_selected = selection.is_some_and(|sel| sel.contains(col_idx, row_idx));
                let bg_color = if is_selected {
                    selection_bg
                } else if cell.has_flag(CellFlags::REVERSE) {
                    scheme.resolve(cell.fg(), true)
                } else {
                    scheme.resolve(cell.bg(), false)
                };

                let should_flush = match run_color {
                    None => false,
                    Some(rc) => rc != bg_color,
                };

                if should_flush {
                    if let Some(color) = run_color {
                        if color != cleared {
                            self.emit_quad_with_offset(
                                &mut vertices,
                                &mut indices,
                                run_start,
                                col_idx,
                                row_idx,
                                color,
                            );
                        }
                    }
                    run_start = col_idx;
                }

                run_color = Some(bg_color);
            }

            if let Some(color) = run_color {
                if color != cleared {
                    self.emit_quad_with_offset(
                        &mut vertices,
                        &mut indices,
                        run_start,
                        dims.cols,
                        row_idx,
                        color,
                    );
                }
            }
        }

        // Block characters (U+2580..U+259F) rendered as solid quads for pixel-perfect cell fill
        for row_idx in 0..dims.rows {
            let row = match buffer.visible_row(row_idx) {
                Some(r) => r,
                None => continue,
            };
            for col_idx in 0..dims.cols {
                let cell = match row.cell(col_idx) {
                    Some(c) => c,
                    None => continue,
                };
                let ch = cell.char();
                if let Some((rects, count)) = block_char_rects(ch) {
                    let fg = if cell.has_flag(CellFlags::REVERSE) {
                        scheme.resolve(cell.bg(), false)
                    } else {
                        scheme.resolve(cell.fg(), true)
                    };
                    let fg = if cell.has_flag(CellFlags::DIM) {
                        Rgba::new(fg.r * 0.65, fg.g * 0.65, fg.b * 0.65, fg.a)
                    } else {
                        fg
                    };
                    let c = fg.to_array();
                    let cx = (col_idx as f32 * self.cell_width + self.pane_offset_x).round();
                    let cy = (row_idx as f32 * self.cell_height + self.pane_offset_y).round();
                    for &(rx, ry, rw, rh) in &rects[..count] {
                        let x0 = (cx + rx * self.cell_width).round();
                        let y0 = (cy + ry * self.cell_height).round();
                        let w = (rw * self.cell_width).round();
                        let h = (rh * self.cell_height).round();
                        let base = vertices.len() as u32;
                        vertices.push(Vertex {
                            position: [x0, y0],
                            tex_coord: [0.0, 0.0],
                            color: c,
                            is_color: 0,
                        });
                        vertices.push(Vertex {
                            position: [x0 + w, y0],
                            tex_coord: [0.0, 0.0],
                            color: c,
                            is_color: 0,
                        });
                        vertices.push(Vertex {
                            position: [x0 + w, y0 + h],
                            tex_coord: [0.0, 0.0],
                            color: c,
                            is_color: 0,
                        });
                        vertices.push(Vertex {
                            position: [x0, y0 + h],
                            tex_coord: [0.0, 0.0],
                            color: c,
                            is_color: 0,
                        });
                        indices.push(base);
                        indices.push(base + 1);
                        indices.push(base + 2);
                        indices.push(base);
                        indices.push(base + 2);
                        indices.push(base + 3);
                    }
                }
            }
        }

        // Full Powerline separators rendered as solid triangles for edge-to-edge
        // statusline transitions.
        for row_idx in 0..dims.rows {
            let row = match buffer.visible_row(row_idx) {
                Some(r) => r,
                None => continue,
            };
            for col_idx in 0..dims.cols {
                let cell = match row.cell(col_idx) {
                    Some(c) => c,
                    None => continue,
                };
                let Some(direction) = powerline_triangle(cell.char()) else {
                    continue;
                };

                let fg = if cell.has_flag(CellFlags::REVERSE) {
                    scheme.resolve(cell.bg(), false)
                } else {
                    scheme.resolve(cell.fg(), true)
                };
                let fg = if cell.has_flag(CellFlags::DIM) {
                    Rgba::new(fg.r * 0.65, fg.g * 0.65, fg.b * 0.65, fg.a)
                } else {
                    fg
                };
                self.emit_powerline_triangle(
                    &mut vertices,
                    &mut indices,
                    (col_idx as f32 * self.cell_width + self.pane_offset_x).round(),
                    (row_idx as f32 * self.cell_height + self.pane_offset_y).round(),
                    self.cell_width.round(),
                    self.cell_height.round(),
                    direction,
                    fg,
                );
            }
        }

        // Draw cursor
        let cursor = terminal.buffer().grid().cursor();
        let should_show_cursor = cursor.visible && (!cursor.blinking || cursor_blink_visible);

        if should_show_cursor {
            let cursor_color = scheme.cursor;
            let col = cursor.position.col;
            let row = cursor.position.row;

            match cursor.shape {
                CursorShape::Block => {
                    self.emit_cursor_block_with_offset(
                        &mut vertices,
                        &mut indices,
                        col,
                        row,
                        cursor_color,
                    );
                }
                CursorShape::Underline => {
                    self.emit_cursor_underline_with_offset(
                        &mut vertices,
                        &mut indices,
                        col,
                        row,
                        cursor_color,
                    );
                }
                CursorShape::Bar => {
                    self.emit_cursor_bar_with_offset(
                        &mut vertices,
                        &mut indices,
                        col,
                        row,
                        cursor_color,
                    );
                }
            }
        }

        (vertices, indices)
    }

    /// Emit a quad with pane offset.
    pub(super) fn emit_quad_with_offset(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        start_col: u16,
        end_col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = (start_col as f32 * self.cell_width + self.pane_offset_x).round();
        let x1 = (end_col as f32 * self.cell_width + self.pane_offset_x).round();
        let y0 = (row as f32 * self.cell_height + self.pane_offset_y).round();
        let y1 = (y0 + self.cell_height).round();

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [0.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [1.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [1.0, 1.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [0.0, 1.0],
            color: color_arr,
            is_color: 0,
        });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Build pane border vertices.
    pub(super) fn build_pane_border_vertices(
        &self,
        pane: &PaneInfo,
        color: Rgba,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let border_width = 2.0;
        let x = pane.x.round();
        let y = (pane.y + self.tab_bar_height).round();
        let w = pane.width.round();
        let h = pane.height.round();

        // Top border
        self.emit_rect(&mut vertices, &mut indices, x, y, w, border_width, color);
        // Bottom border
        self.emit_rect(
            &mut vertices,
            &mut indices,
            x,
            y + h - border_width,
            w,
            border_width,
            color,
        );
        // Left border
        self.emit_rect(&mut vertices, &mut indices, x, y, border_width, h, color);
        // Right border
        self.emit_rect(
            &mut vertices,
            &mut indices,
            x + w - border_width,
            y,
            border_width,
            h,
            color,
        );

        (vertices, indices)
    }
}

/// Whether a pane's blinking cursor is lit this frame: only the focused pane
/// blinks; the others hold their cursor steady.
fn pane_cursor_blink_visible(blink_phase_visible: bool, pane_focused: bool) -> bool {
    blink_phase_visible || !pane_focused
}

#[cfg(test)]
mod tests {
    use super::pane_cursor_blink_visible;

    #[test]
    fn only_the_focused_pane_blinks() {
        assert!(pane_cursor_blink_visible(true, true));
        assert!(!pane_cursor_blink_visible(false, true));
        assert!(pane_cursor_blink_visible(true, false));
        assert!(pane_cursor_blink_visible(false, false));
    }
}
