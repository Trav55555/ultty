//! Image pipeline rendering.
//!
//! Manages GPU textures for terminal images (Sixel) and native protocol
//! ImageWidget rendering, including pipeline creation, texture upload,
//! caching, and draw calls.

use super::{CachedImageTexture, ImagePipeline, Renderer, Vertex};
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
    /// Upload a native protocol ImageWidget to the GPU, caching by region ID.
    /// Returns true if the texture is ready for rendering.
    pub(super) fn upload_native_image(
        &mut self,
        region_id: ultty_protocol::RegionId,
        img: &ultty_protocol::ImageWidget,
    ) -> bool {
        // Check cache — re-upload if dimensions changed
        if let Some(cached) = self.native_image_cache.get(&region_id) {
            if cached.width == img.width && cached.height == img.height {
                return true;
            }
            // Dimensions changed, remove old entry
            self.native_image_cache.remove(&region_id);
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
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.data,
            wgpu::ImageDataLayout {
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
            region_id,
            CachedImageTexture {
                texture,
                bind_group,
                width: img.width,
                height: img.height,
            },
        );

        true
    }

    /// Upload an image to the GPU and cache it.
    fn upload_image(&mut self, image: &TerminalImage) -> bool {
        if self.image_cache.contains_key(&image.id) {
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

        // Evict oldest entries if cache exceeds limit (64 textures)
        const MAX_IMAGE_CACHE: usize = 64;
        if self.image_cache.len() >= MAX_IMAGE_CACHE {
            // Remove entries not currently referenced by the terminal
            let excess = self.image_cache.len() - MAX_IMAGE_CACHE + 1;
            let keys_to_remove: Vec<_> = self.image_cache.keys().take(excess).copied().collect();
            for key in keys_to_remove {
                self.image_cache.remove(&key);
            }
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
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.data,
            wgpu::ImageDataLayout {
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
            image.id,
            CachedImageTexture {
                texture,
                bind_group,
                width: image.width,
                height: image.height,
            },
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
            mipmap_filter: wgpu::FilterMode::Nearest,
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
                bind_group_layouts: &[&uniform_bind_group_layout, &bind_group_layout],
                push_constant_ranges: &[],
            });

        // Create render pipeline
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("image_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[Vertex::desc()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
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
                multiview: None,
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

    /// Render terminal images (Sixel, etc.).
    pub(super) fn render_images(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        terminal: &Terminal,
    ) -> Result<(), wgpu::SurfaceError> {
        if terminal.images().is_empty() {
            return Ok(());
        }

        // Ensure we have an image pipeline
        if self.image_pipeline.is_none() {
            self.create_image_pipeline();
        }

        // Upload any new image, then render each one with its own texture
        for image in terminal.images().images() {
            if !self.upload_image(image) {
                continue;
            }
            let Some(cached) = self.image_cache.get(&image.id) else {
                continue;
            };

            // Build vertices for this single image
            let x0 = image.start_col as f32 * self.cell_width;
            let y0 = image.start_row as f32 * self.cell_height + self.tab_bar_height;
            let x1 = x0 + image.cell_width as f32 * self.cell_width;
            let y1 = y0 + image.cell_height as f32 * self.cell_height;

            let color = [1.0f32, 1.0, 1.0, 1.0];
            let vertices = vec![
                Vertex {
                    position: [x0, y0],
                    tex_coord: [0.0, 0.0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [x1, y0],
                    tex_coord: [1.0, 0.0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [x1, y1],
                    tex_coord: [1.0, 1.0],
                    color,
                    is_color: 0,
                },
                Vertex {
                    position: [x0, y1],
                    tex_coord: [0.0, 1.0],
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
                    render_pass.set_pipeline(&img_pipeline.pipeline);
                    render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    render_pass.set_bind_group(1, &cached.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
