//! Image pipeline rendering.
//!
//! Manages GPU textures for terminal images (Sixel) and native protocol
//! ImageWidget rendering, including pipeline creation, texture upload,
//! caching, and draw calls.

use super::image_cache::ImageKey;
use super::{CachedImageTexture, ImagePipeline, NativeResourceKey, Renderer, Vertex};
use crate::grid::{Terminal, TerminalImage};
use wgpu::util::DeviceExt;

/// Byte length of a tightly packed RGBA8 image, or `None` on overflow.
fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

/// Whether an RGBA image of the given size can be uploaded as one 2D texture.
///
/// Rejects zero or oversize dimensions (beyond the device limit) and pixel
/// buffers that are shorter than the dimensions claim.
fn image_upload_size_ok(
    width: u32,
    height: u32,
    data_len: usize,
    max_texture_dimension: u32,
    label: &str,
) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    if width > max_texture_dimension || height > max_texture_dimension {
        tracing::warn!(
            "{label}: image {width}x{height} exceeds device texture limit {max_texture_dimension}; not rendering"
        );
        return false;
    }
    match rgba_byte_len(width, height) {
        Some(expected) if data_len >= expected => true,
        Some(expected) => {
            tracing::warn!(
                "{label}: image {width}x{height} needs {expected} bytes but only {data_len} were supplied; not rendering"
            );
            false
        }
        None => {
            tracing::warn!("{label}: image {width}x{height} byte size overflows; not rendering");
            false
        }
    }
}

impl Renderer {
    /// Upload a native protocol ImageWidget, caching by session, region and revision.
    /// Returns true if the texture is ready for rendering.
    pub(super) fn upload_native_image(
        &mut self,
        resource: NativeResourceKey,
        revision: u64,
        img: &ultty_protocol::ImageWidget,
    ) -> bool {
        if let Some(cached) = self.native_image_cache.get(&resource) {
            if cached.width == img.width
                && cached.height == img.height
                && cached.revision == Some(revision)
            {
                return true;
            }
            self.native_image_cache.remove(&resource);
        }

        if !image_upload_size_ok(
            img.width,
            img.height,
            img.data.len(),
            self.device.limits().max_texture_dimension_2d,
            "native image",
        ) {
            return false;
        }

        if self.image_pipeline.is_none() {
            self.create_image_pipeline();
        }

        let Some(pipeline) = &self.image_pipeline else {
            return false;
        };

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native_image_texture"),
            size: wgpu::Extent3d {
                width: img.width,
                height: img.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(img.width * 4),
                rows_per_image: Some(img.height),
            },
            wgpu::Extent3d {
                width: img.width,
                height: img.height,
                depth_or_array_layers: 1,
            },
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native_image_bind_group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
            ],
        });

        self.native_image_cache.insert(
            resource,
            CachedImageTexture {
                texture,
                bind_group,
                width: img.width,
                height: img.height,
                revision: Some(revision),
            },
        );

        true
    }

    /// Upload an image to the GPU and cache it.
    fn upload_image(&mut self, owner: u64, image: &TerminalImage) -> bool {
        let key = ImageKey::new(owner, image.id);
        if self.image_cache.contains(&key) {
            return true;
        }

        if !image_upload_size_ok(
            image.width,
            image.height,
            image.data.len(),
            self.device.limits().max_texture_dimension_2d,
            "terminal image",
        ) {
            return false;
        }

        // Ensure image pipeline exists
        if self.image_pipeline.is_none() {
            self.create_image_pipeline();
        }

        let Some(pipeline) = &self.image_pipeline else {
            return false;
        };

        // Create texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image_texture"),
            size: wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload pixel data
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );

        // Create texture view
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image_bind_group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
            ],
        });

        self.image_cache.insert(
            key,
            CachedImageTexture {
                texture,
                bind_group,
                width: image.width,
                height: image.height,
                revision: None,
            },
            image.data.len(),
        );

        true
    }

    /// Create the image rendering pipeline.
    pub(super) fn create_image_pipeline(&mut self) {
        // Image shader (same as text shader but for images)
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("image_shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
            });

        // Create bind group layout for image texture
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("image_bind_group_layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });

        // Create sampler
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Get uniform bind group layout from existing pipeline
        let uniform_bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("image_uniform_bind_group_layout"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });

        // Create pipeline layout
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("image_pipeline_layout"),
                bind_group_layouts: &[Some(&uniform_bind_group_layout), Some(&bind_group_layout)],
                immediate_size: 0,
            });

        // Create render pipeline
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("image_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(Vertex::desc())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        self.image_pipeline = Some(ImagePipeline {
            pipeline,
            bind_group_layout,
            sampler,
        });
    }

    /// Check if there are images to render.
    pub fn has_images(&self, terminal: &Terminal) -> bool {
        !terminal.images().is_empty()
    }

    /// Render terminal images at a pane origin with optional outer clipping.
    pub(super) fn render_images(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        terminal: &Terminal,
        origin: (f32, f32),
        scissor: Option<super::native_render::Scissor>,
    ) {
        if terminal.images().is_empty() {
            return;
        }

        // Ensure we have an image pipeline
        if self.image_pipeline.is_none() {
            self.create_image_pipeline();
        }

        let buffer = terminal.buffer();
        let dims = terminal.dimensions();
        // The line shown on the top row, as the view is scrolled back.
        let top_line = buffer
            .screen_top_line()
            .saturating_sub(buffer.scroll_offset());
        let owner = terminal.uid();

        // Upload any new image, then render each one with its own texture
        for image in terminal.images().images() {
            let Some(quad) = clip_image(
                image.line as isize - top_line as isize,
                image.start_col,
                image.cell_width,
                image.cell_height,
                dims.cols,
                dims.rows,
            ) else {
                continue;
            };
            if !self.upload_image(owner, image) {
                continue;
            }
            let Some(cached) = self.image_cache.get(&ImageKey::new(owner, image.id)) else {
                continue;
            };

            let x0 = origin.0 + quad.col0 * self.cell_width;
            let y0 = origin.1 + quad.row0 * self.cell_height;
            let x1 = origin.0 + quad.col1 * self.cell_width;
            let y1 = origin.1 + quad.row1 * self.cell_height;
            let (u0, v0, u1, v1) = (quad.u0, quad.v0, quad.u1, quad.v1);

            let color = [1.0f32, 1.0, 1.0, 1.0];
            let vertices = vec![
                Vertex {
                    position: [x0, y0],
                    tex_coord: [u0, v0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [x1, y0],
                    tex_coord: [u1, v0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [x1, y1],
                    tex_coord: [u1, v1],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [x0, y1],
                    tex_coord: [u0, v1],
                    color,
                    is_color: 0,
                },
            ];
            let indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

            let vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("image_vertex_buffer"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

            let index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("image_index_buffer"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            // Render pass for this image
            {
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("image_render_pass"),
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

                if let Some(img_pipeline) = self.image_pipeline.as_ref() {
                    if let Some((x, y, width, height)) = scissor {
                        render_pass.set_scissor_rect(x, y, width, height);
                    }
                    render_pass.set_pipeline(&img_pipeline.pipeline);
                    render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    render_pass.set_bind_group(1, &cached.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
                }
            }
        }
    }
}

/// Where an image is drawn, in cells from the terminal's top-left corner,
/// and which part of its texture shows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClippedImage {
    col0: f32,
    row0: f32,
    col1: f32,
    row1: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

/// Clip an image of `cell_width` x `cell_height` cells, whose top-left cell
/// is at `visual_row` (negative above the view) and `start_col`, to a view
/// of `cols` x `rows` cells. `None` when none of it shows.
fn clip_image(
    visual_row: isize,
    start_col: u16,
    cell_width: u16,
    cell_height: u16,
    cols: u16,
    rows: u16,
) -> Option<ClippedImage> {
    let (width, height) = (f32::from(cell_width), f32::from(cell_height));
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let col0 = f32::from(start_col);
    let row0 = visual_row as f32;
    let col1 = (col0 + width).min(f32::from(cols));
    let row1 = (row0 + height).min(f32::from(rows));
    let (shown_col0, shown_row0) = (col0.max(0.0), row0.max(0.0));
    if col1 <= shown_col0 || row1 <= shown_row0 {
        return None;
    }
    Some(ClippedImage {
        col0: shown_col0,
        row0: shown_row0,
        col1,
        row1,
        u0: (shown_col0 - col0) / width,
        v0: (shown_row0 - row0) / height,
        u1: (col1 - col0) / width,
        v1: (row1 - row0) / height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_in_view_is_drawn_whole() {
        let quad = clip_image(2, 3, 4, 2, 80, 24).unwrap();
        assert_eq!(
            (quad.col0, quad.row0, quad.col1, quad.row1),
            (3.0, 2.0, 7.0, 4.0)
        );
        assert_eq!((quad.u0, quad.v0, quad.u1, quad.v1), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn an_image_scrolled_partly_off_the_top_shows_its_lower_part() {
        // Four rows tall, one row above the view: the top quarter is cut.
        let quad = clip_image(-1, 0, 2, 4, 80, 24).unwrap();
        assert_eq!((quad.row0, quad.row1), (0.0, 3.0));
        assert_eq!((quad.v0, quad.v1), (0.25, 1.0));
    }

    #[test]
    fn an_image_past_the_bottom_and_right_edges_is_clipped() {
        let quad = clip_image(22, 78, 4, 4, 80, 24).unwrap();
        assert_eq!((quad.col1, quad.row1), (80.0, 24.0));
        assert_eq!((quad.u1, quad.v1), (0.5, 0.5));
    }

    #[test]
    fn an_image_wholly_out_of_view_is_not_drawn() {
        assert_eq!(clip_image(-4, 0, 2, 4, 80, 24), None);
        assert_eq!(clip_image(24, 0, 2, 4, 80, 24), None);
        assert_eq!(clip_image(0, 80, 2, 4, 80, 24), None);
    }

    #[test]
    fn rgba_byte_len_rejects_overflow() {
        assert_eq!(rgba_byte_len(2, 3), Some(24));
        assert_eq!(rgba_byte_len(u32::MAX, u32::MAX), None);
        assert_eq!(rgba_byte_len(0, 10), Some(0));
    }

    #[test]
    fn upload_size_check_rejects_oversize_and_short_buffers() {
        assert!(image_upload_size_ok(4, 4, 64, 8192, "t"));
        assert!(!image_upload_size_ok(0, 4, 0, 8192, "t"));
        assert!(!image_upload_size_ok(8193, 1, 8193 * 4, 8192, "t"));
        assert!(!image_upload_size_ok(1, 8193, 8193 * 4, 8192, "t"));
        assert!(!image_upload_size_ok(4, 4, 63, 8192, "t"));
        // width * height * 4 would overflow u32 but not the u64 check;
        // the buffer is far too short so it is still rejected.
        assert!(!image_upload_size_ok(65536, 65536, 16, u32::MAX, "t"));
    }

    /// Exercises the actual pane renderer and GPU readback. Run explicitly on
    /// a machine with a software or hardware Vulkan adapter.
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_pane_images_are_scoped_and_native_updates_replace_pixels() {
        use crate::attach::native::NativeSnapshot;
        use crate::grid::{Dimensions, ImageId};
        use crate::protocol::NativeSession;
        use crate::renderer::{OverlayState, PaneDraw, PaneInfo};
        use crate::scene::view::NativeRenderSource;
        use crate::terminal_manager::TabId;
        use ultty_protocol::{
            AppMessage, Constraints, Content, Dimension, FrameId, ImageScale, ImageWidget,
            RegionId, Style, WidgetContent, CLIENT_DEFAULT_CAPABILITIES, PROTOCOL_VERSION,
        };
        use ultty_vt::grid::snapshot::TerminalSnapshot;

        fn native(id: u64, rgba: [u8; 4]) -> (NativeSession, std::os::unix::net::UnixStream) {
            let (socket, peer) = std::os::unix::net::UnixStream::pair().unwrap();
            let mut session = NativeSession::new(socket, id, TabId::new(0)).unwrap();
            session.set_viewport(16.0, 8.0);
            session
                .handle_message(AppMessage::ClientHello {
                    version: PROTOCOL_VERSION,
                    requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
                })
                .unwrap();
            session
                .handle_message(AppMessage::BeginFrame {
                    frame_id: FrameId(1),
                })
                .unwrap();
            session
                .handle_message(AppMessage::CreateRegion {
                    id: RegionId::new(7),
                    parent: RegionId::ROOT,
                    insert_index: -1,
                    constraints: Constraints {
                        width: Dimension::Cells(8.0),
                        height: Dimension::Cells(3.0),
                        ..Default::default()
                    },
                    style: Style::default(),
                })
                .unwrap();
            content(&mut session, rgba);
            session
                .handle_message(AppMessage::EndFrame {
                    frame_id: FrameId(1),
                })
                .unwrap();
            (session, peer)
        }
        fn content(session: &mut NativeSession, rgba: [u8; 4]) {
            session
                .handle_message(AppMessage::SetContent {
                    id: RegionId::new(7),
                    content: Content::Widget(WidgetContent::Image(ImageWidget {
                        width: 1,
                        height: 1,
                        data: rgba.to_vec(),
                        scale: ImageScale::Stretch,
                    })),
                })
                .unwrap();
        }
        fn pixels(
            renderer: &mut Renderer,
            panes: &[PaneDraw<'_>],
            path: &std::path::Path,
        ) -> Vec<u8> {
            renderer
                .capture_panes_screenshot(panes, &OverlayState::default(), path)
                .unwrap();
            let bytes = std::fs::read(path).unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
                .read_info()
                .unwrap();
            let mut data = vec![0; reader.output_buffer_size()];
            reader.next_frame(&mut data).unwrap();
            data
        }
        fn rgb(data: &[u8], x: usize, y: usize) -> [u8; 3] {
            let offset = (y * 256 + x) * 4;
            [data[offset], data[offset + 1], data[offset + 2]]
        }
        fn draw<'a>(
            a: &'a Terminal,
            b: &'a Terminal,
            left: &'a dyn NativeRenderSource,
            right: &'a dyn NativeRenderSource,
        ) -> Vec<PaneDraw<'a>> {
            vec![
                PaneDraw {
                    selection: Vec::new(),
                    terminal: a,
                    info: PaneInfo::new(0.0, 0.0, 128.0, 96.0, true),
                    sessions: vec![left],
                },
                PaneDraw {
                    selection: Vec::new(),
                    terminal: b,
                    info: PaneInfo::new(128.0, 0.0, 128.0, 96.0, true),
                    sessions: vec![right],
                },
            ]
        }
        let mut renderer = pollster::block_on(Renderer::new_headless(256, 100)).unwrap();
        let x = (renderer.cell_width * 3.0) as usize;
        let y = (4.0 + renderer.cell_height) as usize;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panes.png");
        let (mut left, _left_peer) = native(1, [255, 0, 0, 255]);
        let (right, _right_peer) = native(2, [0, 0, 255, 255]);
        let a = Terminal::new(Dimensions::new(16, 8));
        let b = Terminal::new(Dimensions::new(16, 8));
        let left_view = NativeSnapshot::capture(&left).unwrap().to_view().unwrap();
        let right_view = NativeSnapshot::capture(&right).unwrap().to_view().unwrap();
        let image = pixels(&mut renderer, &draw(&a, &b, &left_view, &right_view), &path);
        assert_eq!(rgb(&image, x, y), [255, 0, 0]);
        assert_eq!(rgb(&image, 128 + x, y), [0, 0, 255]);
        left.handle_message(AppMessage::BeginFrame {
            frame_id: FrameId(2),
        })
        .unwrap();
        content(&mut left, [0, 255, 0, 255]);
        left.handle_message(AppMessage::EndFrame {
            frame_id: FrameId(2),
        })
        .unwrap();
        let left_view = NativeSnapshot::capture(&left).unwrap().to_view().unwrap();
        let image = pixels(&mut renderer, &draw(&a, &b, &left_view, &right_view), &path);
        assert_eq!(rgb(&image, x, y), [0, 255, 0]);
        assert_eq!(rgb(&image, 128 + x, y), [0, 0, 255]);

        let mut a = Terminal::new(Dimensions::new(16, 8));
        let mut b = Terminal::new(Dimensions::new(16, 8));
        for (terminal, color) in [(&mut a, [255, 0, 0, 255]), (&mut b, [0, 0, 255, 255])] {
            terminal.images_mut().add(TerminalImage {
                id: ImageId(0),
                width: 1,
                height: 1,
                data: color.to_vec(),
                start_col: 0,
                line: 0,
                cell_width: 8,
                cell_height: 3,
            });
        }
        let a = TerminalSnapshot::capture(&a).unwrap().to_view().unwrap();
        let b = TerminalSnapshot::capture(&b).unwrap().to_view().unwrap();
        let panes = [
            PaneDraw {
                selection: Vec::new(),
                terminal: &a,
                info: PaneInfo::new(0.0, 0.0, 128.0, 96.0, true),
                sessions: vec![],
            },
            PaneDraw {
                selection: Vec::new(),
                terminal: &b,
                info: PaneInfo::new(128.0, 0.0, 128.0, 96.0, true),
                sessions: vec![],
            },
        ];
        let image = pixels(&mut renderer, &panes, &path);
        assert_eq!(rgb(&image, x, y), [255, 0, 0]);
        assert_eq!(rgb(&image, 128 + x, y), [0, 0, 255]);
        let mut screens = Terminal::new(Dimensions::new(16, 8));
        let mut red = a.images().images().next().unwrap().clone();
        red.id = screens.allocate_image_id();
        screens.images_mut().add(red);
        screens.enter_alternate_screen_clear();
        let mut blue = b.images().images().next().unwrap().clone();
        blue.id = screens.allocate_image_id();
        screens.images_mut().add(blue);
        let mut screens = TerminalSnapshot::capture(&screens)
            .unwrap()
            .to_view()
            .unwrap();
        fn single(terminal: &Terminal) -> [PaneDraw<'_>; 1] {
            [PaneDraw {
                selection: Vec::new(),
                terminal,
                info: PaneInfo::new(0.0, 0.0, 128.0, 96.0, true),
                sessions: vec![],
            }]
        }
        assert_eq!(
            rgb(&pixels(&mut renderer, &single(&screens), &path), x, y),
            [0, 0, 255]
        );
        screens.leave_alternate_screen();
        assert_eq!(
            rgb(&pixels(&mut renderer, &single(&screens), &path), x, y),
            [255, 0, 0]
        );
        let mut partial = TerminalSnapshot::capture(&screens).unwrap();
        partial.main.evicted_lines = 1;
        let partial = partial.to_view().unwrap();
        let image = pixels(&mut renderer, &single(&partial), &path);
        assert_eq!(rgb(&image, x, y), [255, 0, 0]);
        assert_ne!(
            rgb(&image, x, (4.0 + renderer.cell_height * 2.5) as usize),
            [255, 0, 0]
        );
        assert!(renderer
            .image_cache
            .contains(&ImageKey::new(a.uid(), ImageId(0))));
        assert!(renderer
            .image_cache
            .contains(&ImageKey::new(b.uid(), ImageId(0))));

        // A complete attachment replacement keeps its pane/image identities,
        // but owns a fresh Terminal UID even when image dimensions stay equal.
        let mut replacement = TerminalSnapshot::capture(&a).unwrap();
        replacement.main.images[0].data = vec![0, 255, 0, 255];
        let updated = replacement.to_view().unwrap();
        assert_ne!(updated.uid(), a.uid());
        let updated_panes = [
            PaneDraw {
                selection: Vec::new(),
                terminal: &updated,
                info: PaneInfo::new(0.0, 0.0, 128.0, 96.0, true),
                sessions: vec![],
            },
            PaneDraw {
                selection: Vec::new(),
                terminal: &b,
                info: PaneInfo::new(128.0, 0.0, 128.0, 96.0, true),
                sessions: vec![],
            },
        ];
        let image = pixels(&mut renderer, &updated_panes, &path);
        assert_eq!(rgb(&image, x, y), [0, 255, 0]);
        assert_eq!(rgb(&image, 128 + x, y), [0, 0, 255]);
        assert!(renderer
            .image_cache
            .contains(&ImageKey::new(a.uid(), ImageId(0))));
        assert!(renderer
            .image_cache
            .contains(&ImageKey::new(updated.uid(), ImageId(0))));
        let image = pixels(&mut renderer, &panes, &path);
        assert_eq!(rgb(&image, x, y), [255, 0, 0]);
        assert_eq!(rgb(&image, 128 + x, y), [0, 0, 255]);
    }
}
