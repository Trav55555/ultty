//! Native session render buffer upload and cache drawing.

use wgpu::util::DeviceExt;

use super::{NativePrimitive, NativeVertices, Renderer, Scissor};
use crate::renderer::{NativeBufferDraw, Vertex};

impl Renderer {
    pub(crate) fn has_cached_native_buffers(&self) -> bool {
        !self.cached_native_buffers.is_empty()
    }

    pub(crate) fn clear_cached_native_buffers(&mut self) {
        self.cached_native_buffers.clear();
    }

    pub(super) fn upload_draw_and_cache_native_buffers(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertices: &NativeVertices,
        scissor: Option<Scissor>,
    ) {
        let NativeVertices {
            bg_vertices,
            bg_indices,
            text_vertices,
            text_indices,
            deferred_images,
            order,
        } = vertices;
        self.prev_bg_count = (bg_vertices.len(), bg_indices.len());
        self.prev_text_count = (text_vertices.len(), text_indices.len());
        self.cached_native_atlas_epoch = self.text_renderer.atlas_epoch();

        let backgrounds = self.upload_native_buffers(bg_vertices, bg_indices, "native_bg");
        let text = self.upload_native_buffers(text_vertices, text_indices, "native_text");
        let first = self.cached_native_buffers.order.len();
        for primitive in order {
            match primitive {
                NativePrimitive::Background(range) => {
                    if let Some((vertices, indices)) = &backgrounds {
                        self.cached_native_buffers.push_bg((
                            vertices.clone(),
                            indices.clone(),
                            range.clone(),
                            scissor,
                        ));
                    }
                }
                NativePrimitive::Text(range) => {
                    if let Some((vertices, indices)) = &text {
                        self.cached_native_buffers.push_text((
                            vertices.clone(),
                            indices.clone(),
                            range.clone(),
                            scissor,
                        ));
                    }
                }
                NativePrimitive::Image(index) => {
                    self.cached_native_buffers
                        .order
                        .push(NativeBufferDraw::Image(
                            deferred_images[*index].clone(),
                            scissor,
                        ))
                }
            }
        }
        self.draw_native_batches(encoder, view, &self.cached_native_buffers.order[first..]);
    }

    fn upload_native_buffers(
        &self,
        vertices: &[Vertex],
        indices: &[u32],
        label: &'static str,
    ) -> Option<(wgpu::Buffer, wgpu::Buffer)> {
        if vertices.is_empty() {
            return None;
        }

        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        Some((vertex_buffer, index_buffer))
    }

    /// Draw native session content from cached GPU buffers without rebuilding vertices.
    pub fn draw_cached_native_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        self.draw_native_batches(encoder, view, &self.cached_native_buffers.order);
    }

    fn draw_native_batches(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        order: &[NativeBufferDraw],
    ) {
        for draw in order {
            match draw {
                NativeBufferDraw::Background(index) => {
                    let (vertex_buffer, index_buffer, indices, scissor) =
                        &self.cached_native_buffers.bg[*index];
                    self.draw_native_bg_buffers(
                        encoder,
                        view,
                        vertex_buffer,
                        index_buffer,
                        indices.clone(),
                        *scissor,
                        "native_bg_pass_cached",
                    );
                }
                NativeBufferDraw::Text(index) => {
                    let (vertex_buffer, index_buffer, indices, scissor) =
                        &self.cached_native_buffers.text[*index];
                    self.draw_native_text_buffers(
                        encoder,
                        view,
                        vertex_buffer,
                        index_buffer,
                        indices.clone(),
                        *scissor,
                        "native_text_pass_cached",
                    );
                }
                NativeBufferDraw::Image(image, scissor) => self.render_deferred_native_images(
                    encoder,
                    view,
                    std::slice::from_ref(image),
                    *scissor,
                ),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_native_bg_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        indices: std::ops::Range<u32>,
        scissor: Option<Scissor>,
        label: &'static str,
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
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

        if let Some((x, y, width, height)) = scissor {
            render_pass.set_scissor_rect(x, y, width, height);
        }
        render_pass.set_pipeline(self.bg_pipeline.pipeline());
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(indices, 0, 0..1);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_native_text_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        indices: std::ops::Range<u32>,
        scissor: Option<Scissor>,
        label: &'static str,
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
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

        if let Some((x, y, width, height)) = scissor {
            render_pass.set_scissor_rect(x, y, width, height);
        }
        render_pass.set_pipeline(self.text_renderer.pipeline());
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(indices, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use ultty_protocol::{
        AppMessage, Color, Constraints, Content, Dimension, Display, FrameId, GridLine,
        GridPlacement, RegionId, Style, TextContent, TrackSizingFunction,
        CLIENT_DEFAULT_CAPABILITIES, PROTOCOL_VERSION,
    };

    use crate::grid::{Dimensions, Terminal};
    use crate::protocol::NativeSession;
    use crate::renderer::{OverlayState, PaneDraw, PaneInfo, Renderer};
    use crate::terminal_manager::TabId;

    #[test]
    fn test_opaque_native_overlay_covers_prior_text_and_replays_identically() {
        let Ok(mut renderer) = pollster::block_on(Renderer::new_headless(256, 96)) else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut session = NativeSession::new(socket, 91, TabId::new(0)).unwrap();
        session.set_viewport(10.0, 4.0);
        for message in [
            AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
            },
            AppMessage::BeginFrame {
                frame_id: FrameId(1),
            },
            AppMessage::CreateRegion {
                id: RegionId::new(1),
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints {
                    display: Display::Grid,
                    width: Dimension::Cells(10.0),
                    height: Dimension::Cells(4.0),
                    grid_template_columns: vec![TrackSizingFunction::Single(Dimension::Cells(
                        10.0,
                    ))],
                    grid_template_rows: vec![TrackSizingFunction::Single(Dimension::Cells(4.0))],
                    ..Default::default()
                },
                style: Style::default(),
            },
        ] {
            session.handle_message(message).unwrap();
        }
        for (id, width, bg, fg, label) in [
            (2, 10.0, None, Color::Rgb(255, 255, 255), "MMMMMMMMMM"),
            (
                3,
                4.0,
                Some(Color::Rgb(255, 0, 0)),
                Color::Rgb(0, 0, 255),
                "B",
            ),
        ] {
            session
                .handle_message(AppMessage::CreateRegion {
                    id: RegionId::new(id),
                    parent: RegionId::new(1),
                    insert_index: -1,
                    constraints: Constraints {
                        width: Dimension::Cells(width),
                        height: Dimension::Cells(2.0),
                        grid_column: GridPlacement {
                            start: GridLine::Line(1),
                            end: GridLine::Line(2),
                        },
                        grid_row: GridPlacement {
                            start: GridLine::Line(1),
                            end: GridLine::Line(2),
                        },
                        ..Default::default()
                    },
                    style: Style {
                        bg,
                        fg: Some(fg),
                        ..Default::default()
                    },
                })
                .unwrap();
            session
                .handle_message(AppMessage::SetContent {
                    id: RegionId::new(id),
                    content: Content::Text(TextContent::new(label)),
                })
                .unwrap();
        }
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(1),
            })
            .unwrap();
        let terminal = Terminal::new(Dimensions::new(10, 4));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overlay.png");
        fn capture(
            renderer: &mut Renderer,
            terminal: &Terminal,
            session: &NativeSession,
            path: &std::path::Path,
        ) -> Vec<u8> {
            renderer
                .capture_panes_screenshot(
                    &[PaneDraw {
                        selection: Vec::new(),
                        terminal,
                        info: PaneInfo::new(0.0, 0.0, 256.0, 96.0, true),
                        sessions: vec![session],
                    }],
                    &OverlayState::default(),
                    path,
                )
                .unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(std::fs::read(path).unwrap()))
                .read_info()
                .unwrap();
            let mut data = vec![0; reader.output_buffer_size()];
            reader.next_frame(&mut data).unwrap();
            data
        }
        let fresh = capture(&mut renderer, &terminal, &session, &path);
        assert_eq!(
            session.scene().get(RegionId::new(2)).unwrap().computed.y,
            session.scene().get(RegionId::new(3)).unwrap().computed.y,
            "fixture regions must actually overlap"
        );
        session.clear_render_dirty();
        session.clear_scroll_dirty();
        assert!(renderer.native_cache_valid(false, &[], &renderer.cached_native_panes));
        let replay = capture(&mut renderer, &terminal, &session, &path);
        assert_eq!(fresh, replay, "cached replay must preserve painter order");
        let (cell_width, cell_height) = renderer.cell_dimensions();
        let top = renderer.tab_bar_height() as usize;
        let mut covered_white = 0;
        let mut visible_white = 0;
        let mut modal_blue = 0;
        for y in top..top + cell_height as usize * 2 {
            for x in 0..(cell_width as usize * 10) {
                let offset = (y * 256 + x) * 4;
                let pixel = &fresh[offset..offset + 3];
                if pixel.iter().all(|channel| *channel > 150) {
                    if x < cell_width as usize * 4 {
                        covered_white += 1;
                    } else {
                        visible_white += 1;
                    }
                }
                if x < cell_width as usize * 4 && pixel[2] > 150 && pixel[0] < 100 {
                    modal_blue += 1;
                }
            }
        }
        assert!(
            visible_white > 0,
            "underlying text must actually be rendered outside modal"
        );
        assert!(modal_blue > 0, "modal's own text must remain visible");
        assert_eq!(
            covered_white, 0,
            "prior text must not paint over opaque modal"
        );

        // Negative control: reproduce the former background-first batching on
        // the same overlapping scene, proving the pixel oracle catches it.
        renderer
            .cached_native_buffers
            .order
            .sort_by_key(|draw| match draw {
                crate::renderer::NativeBufferDraw::Background(_) => 0,
                crate::renderer::NativeBufferDraw::Text(_) => 1,
                crate::renderer::NativeBufferDraw::Image(_, _) => 2,
            });
        let background_first = capture(&mut renderer, &terminal, &session, &path);
        assert_ne!(
            fresh, background_first,
            "background-first batching must fail the overlap oracle"
        );

        // Images must obey the same order and survive cached replay, rather
        // than being deferred until after the modal or omitted from replay.
        session
            .handle_message(AppMessage::BeginFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        session
            .handle_message(AppMessage::SetContent {
                id: RegionId::new(2),
                content: Content::Widget(ultty_protocol::WidgetContent::Image(
                    ultty_protocol::ImageWidget {
                        width: 1,
                        height: 1,
                        data: vec![0, 255, 0, 255],
                        scale: ultty_protocol::ImageScale::Stretch,
                    },
                )),
            })
            .unwrap();
        session
            .handle_message(AppMessage::EndFrame {
                frame_id: FrameId(2),
            })
            .unwrap();
        let image_frame = capture(&mut renderer, &terminal, &session, &path);
        session.clear_render_dirty();
        session.clear_scroll_dirty();
        assert!(renderer.native_cache_valid(false, &[], &renderer.cached_native_panes));
        assert_eq!(
            image_frame,
            capture(&mut renderer, &terminal, &session, &path),
            "images must replay in the same order too"
        );
        let mut visible_green = 0;
        // The terminal paints its two-pixel focus border after native content.
        // Check the modal interior, independently of that outer pane border.
        for y in top + 2..top + cell_height as usize * 2 - 2 {
            for x in 2..cell_width as usize * 10 - 2 {
                let offset = (y * 256 + x) * 4;
                let green = image_frame[offset] < 50
                    && image_frame[offset + 1] > 150
                    && image_frame[offset + 2] < 50;
                if x < cell_width as usize * 4 {
                    assert!(
                        !green,
                        "prior image must not paint over opaque modal at {x},{y}"
                    );
                } else if green {
                    visible_green += 1;
                }
            }
        }
        assert!(
            visible_green > 0,
            "underlying image must actually render outside modal"
        );
    }
}
