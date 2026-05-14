//! Screenshot capture functionality.
//!
//! Renders the terminal to an offscreen texture and saves as PNG.
//!
//! The GPU-dependent capture methods live on [`Renderer`]. The pure helper
//! functions ([`padded_bytes_per_row`], [`unpad_and_convert_pixels`],
//! [`encode_rgba_png`]) are free functions so they can be unit-tested
//! without a GPU context.

use super::{OverlayState, Renderer};
use crate::grid::Terminal;
use crate::renderer::Vertex;
use wgpu::util::DeviceExt;

struct ScreenshotGeometry<'a> {
    vertices: &'a [Vertex],
    indices: &'a [u32],
}

enum ScreenshotPipeline<'a> {
    Color,
    Text(&'a wgpu::BindGroup),
}

/// Compute the row stride (in bytes) after aligning to `alignment`.
///
/// wgpu requires that `bytes_per_row` is a multiple of
/// `COPY_BYTES_PER_ROW_ALIGNMENT` (256).  This function performs the
/// ceiling-division rounding.
pub(crate) fn padded_bytes_per_row(width: u32, bytes_per_pixel: u32, alignment: u32) -> u32 {
    let unpadded = width * bytes_per_pixel;
    unpadded.div_ceil(alignment) * alignment
}

/// Strip row padding and optionally swizzle BGRA -> RGBA.
///
/// `data` is the raw GPU read-back buffer (with padding).  The function
/// returns a tightly-packed RGBA pixel buffer suitable for PNG encoding.
pub(crate) fn unpad_and_convert_pixels(
    data: &[u8],
    width: u32,
    height: u32,
    padded_row_bytes: u32,
    bytes_per_pixel: u32,
    is_bgra: bool,
) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row_start = (y * padded_row_bytes) as usize;
        let row_end = row_start + (width * bytes_per_pixel) as usize;
        let row = &data[row_start..row_end];

        for chunk in row.chunks(4) {
            if is_bgra {
                pixels.push(chunk[2]); // R
                pixels.push(chunk[1]); // G
                pixels.push(chunk[0]); // B
                pixels.push(chunk[3]); // A
            } else {
                pixels.extend_from_slice(chunk);
            }
        }
    }
    pixels
}

/// Encode a tightly-packed RGBA pixel buffer as a PNG into an in-memory `Vec<u8>`.
pub(crate) fn encode_rgba_png(pixels: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(pixels)?;
    }
    Ok(png_data)
}

impl Renderer {
    fn render_screenshot_vertices(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        label: &'static str,
        geometry: ScreenshotGeometry<'_>,
        pipeline: ScreenshotPipeline<'_>,
    ) {
        if geometry.vertices.is_empty() || geometry.indices.is_empty() {
            return;
        }

        let vertex_label = format!("{label}_vb");
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&vertex_label),
                contents: bytemuck::cast_slice(geometry.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let index_label = format!("{label}_ib");
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&index_label),
                contents: bytemuck::cast_slice(geometry.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
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

        match pipeline {
            ScreenshotPipeline::Color => {
                render_pass.set_pipeline(self.bg_pipeline.pipeline());
            }
            ScreenshotPipeline::Text(atlas_bind_group) => {
                render_pass.set_pipeline(self.text_renderer.pipeline());
                render_pass.set_bind_group(1, atlas_bind_group, &[]);
            }
        }
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..geometry.indices.len() as u32, 0, 0..1);
    }

    fn render_screenshot_chrome(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        overlay: &OverlayState,
    ) {
        if let Some(tab_bar) = overlay.tab_bar {
            let (tab_vertices, tab_indices) = self.build_tab_bar_vertices(tab_bar);
            self.render_screenshot_vertices(
                encoder,
                view,
                "screenshot_tab_bar_pass",
                ScreenshotGeometry {
                    vertices: &tab_vertices,
                    indices: &tab_indices,
                },
                ScreenshotPipeline::Color,
            );

            let (title_vertices, title_indices) = self.build_tab_title_vertices(tab_bar);
            let atlas = self.text_renderer.atlas_bind_group();
            self.render_screenshot_vertices(
                encoder,
                view,
                "screenshot_tab_title_pass",
                ScreenshotGeometry {
                    vertices: &title_vertices,
                    indices: &title_indices,
                },
                ScreenshotPipeline::Text(atlas),
            );
        }
    }

    fn render_screenshot_overlays(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) {
        let (overlay_vertices, overlay_indices) = self.build_overlay_vertices(terminal, overlay);
        self.render_screenshot_vertices(
            encoder,
            view,
            "screenshot_overlay_pass",
            ScreenshotGeometry {
                vertices: &overlay_vertices,
                indices: &overlay_indices,
            },
            ScreenshotPipeline::Color,
        );
    }

    /// Capture a screenshot of the terminal to a PNG file.
    ///
    /// This renders the terminal to an offscreen texture and saves it as PNG.
    /// Includes native protocol session rendering when sessions are provided.
    pub fn capture_screenshot(
        &mut self,
        terminal: &Terminal,
        overlay: &OverlayState,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        self.capture_screenshot_with_native(terminal, overlay, path, &[])
    }

    /// Capture a screenshot including native protocol sessions.
    pub fn capture_screenshot_with_native(
        &mut self,
        terminal: &Terminal,
        overlay: &OverlayState,
        path: &std::path::Path,
        native_sessions: &[&crate::protocol::NativeSession],
    ) -> anyhow::Result<()> {
        let width = self.config.width;
        let height = self.config.height;

        // Create a texture we can copy from
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Update tab bar height from overlay state
        self.tab_bar_height = overlay.tab_bar.map(|tb| tb.height()).unwrap_or(0.0);

        // Create encoder
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot_encoder"),
            });

        // Build vertex data for backgrounds
        let (bg_vertices, bg_indices) = self.build_background_vertices(terminal, overlay);

        // Create background vertex/index buffers
        let bg_vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("screenshot_bg_vertex_buffer"),
                contents: bytemuck::cast_slice(&bg_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let bg_index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("screenshot_bg_index_buffer"),
                contents: bytemuck::cast_slice(&bg_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        // Render pass for backgrounds
        {
            let linear_bg = self.color_scheme.background.to_linear();
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("screenshot_bg_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
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
            });

            render_pass.set_pipeline(self.bg_pipeline.pipeline());
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, bg_vertex_buffer.slice(..));
            render_pass.set_index_buffer(bg_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
        }

        self.render_screenshot_chrome(&mut encoder, &view, overlay);

        // Render text
        self.render_screenshot_chrome(&mut encoder, &view, overlay);

        let _ = self.text_renderer.render(
            &mut encoder,
            &view,
            terminal,
            &self.color_scheme,
            self.cell_width,
            self.cell_height,
            self.tab_bar_height,
            &self.uniform_bind_group,
        );

        for session in native_sessions {
            self.render_native_session(&mut encoder, &view, session, None, 0);
        }

        self.render_screenshot_overlays(&mut encoder, &view, terminal, overlay);

        // Calculate buffer size with proper alignment
        let bytes_per_pixel = 4u32; // RGBA or BGRA
        let padded_row =
            padded_bytes_per_row(width, bytes_per_pixel, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer_size = (padded_row * height) as u64;

        // Create output buffer
        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy texture to buffer
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        // Submit and wait
        self.queue.submit(std::iter::once(encoder.finish()));

        // Map buffer and read data
        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;

        let data = buffer_slice.get_mapped_range();
        let is_bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm
        );
        let pixels =
            unpad_and_convert_pixels(&data, width, height, padded_row, bytes_per_pixel, is_bgra);
        drop(data);
        output_buffer.unmap();

        let png_data = encode_rgba_png(&pixels, width, height)?;
        std::fs::write(path, &png_data)?;

        tracing::info!("Screenshot saved to {}", path.display());
        Ok(())
    }

    /// Capture a screenshot as PNG bytes.
    pub fn capture_screenshot_bytes(
        &mut self,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) -> anyhow::Result<Vec<u8>> {
        self.capture_screenshot_bytes_with_native(terminal, overlay, &[])
    }

    /// Capture a screenshot as PNG bytes, including native protocol sessions.
    pub fn capture_screenshot_bytes_with_native(
        &mut self,
        terminal: &Terminal,
        overlay: &OverlayState,
        native_sessions: &[&crate::protocol::NativeSession],
    ) -> anyhow::Result<Vec<u8>> {
        let width = self.config.width;
        let height = self.config.height;

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.tab_bar_height = overlay.tab_bar.map(|tb| tb.height()).unwrap_or(0.0);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot_encoder"),
            });

        let (bg_vertices, bg_indices) = self.build_background_vertices(terminal, overlay);
        let bg_vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("screenshot_bg_vb"),
                contents: bytemuck::cast_slice(&bg_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let bg_index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("screenshot_bg_ib"),
                contents: bytemuck::cast_slice(&bg_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        {
            let linear_bg = self.color_scheme.background.to_linear();
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("screenshot_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: linear_bg.r as f64,
                            g: linear_bg.g as f64,
                            b: linear_bg.b as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(self.bg_pipeline.pipeline());
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, bg_vertex_buffer.slice(..));
            render_pass.set_index_buffer(bg_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
        }

        let _ = self.text_renderer.render(
            &mut encoder,
            &view,
            terminal,
            &self.color_scheme,
            self.cell_width,
            self.cell_height,
            self.tab_bar_height,
            &self.uniform_bind_group,
        );

        for session in native_sessions {
            self.render_native_session(&mut encoder, &view, session, None, 0);
        }

        self.render_screenshot_overlays(&mut encoder, &view, terminal, overlay);

        let bytes_per_pixel = 4u32;
        let padded_row =
            padded_bytes_per_row(width, bytes_per_pixel, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer_size = (padded_row * height) as u64;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;

        let data = buffer_slice.get_mapped_range();
        let is_bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm
        );
        let pixels =
            unpad_and_convert_pixels(&data, width, height, padded_row, bytes_per_pixel, is_bgra);
        drop(data);
        output_buffer.unmap();

        encode_rgba_png(&pixels, width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The wgpu alignment constant is 256 bytes.
    const WGPU_ALIGNMENT: u32 = 256;

    // ---------------------------------------------------------------
    // padded_bytes_per_row
    // ---------------------------------------------------------------

    #[test]
    fn padded_row_exact_multiple_of_alignment() {
        // 64 pixels * 4 bpp = 256 bytes, already aligned.
        assert_eq!(padded_bytes_per_row(64, 4, WGPU_ALIGNMENT), 256);
    }

    #[test]
    fn padded_row_rounds_up_to_alignment() {
        // 80 pixels * 4 bpp = 320 bytes -> next multiple of 256 = 512.
        assert_eq!(padded_bytes_per_row(80, 4, WGPU_ALIGNMENT), 512);
    }

    #[test]
    fn padded_row_small_width() {
        // 1 pixel * 4 bpp = 4 bytes -> rounds up to 256.
        assert_eq!(padded_bytes_per_row(1, 4, WGPU_ALIGNMENT), 256);
    }

    #[test]
    fn padded_row_large_width() {
        // 1920 pixels * 4 = 7680 bytes.  7680 / 256 = 30, exact.
        assert_eq!(padded_bytes_per_row(1920, 4, WGPU_ALIGNMENT), 7680);
    }

    #[test]
    fn padded_row_not_exact_large() {
        // 1921 pixels * 4 = 7684 bytes.  ceil(7684/256)*256 = 7936.
        assert_eq!(padded_bytes_per_row(1921, 4, WGPU_ALIGNMENT), 7936);
    }

    #[test]
    fn padded_row_alignment_of_1() {
        // With alignment=1, no padding needed.
        assert_eq!(padded_bytes_per_row(123, 4, 1), 492);
    }

    // ---------------------------------------------------------------
    // unpad_and_convert_pixels: RGBA pass-through (no swizzle)
    // ---------------------------------------------------------------

    #[test]
    fn unpad_rgba_no_padding_needed() {
        // 1x1 pixel, padded row = 256 bytes (but only first 4 bytes used).
        let padded = padded_bytes_per_row(1, 4, WGPU_ALIGNMENT);
        let mut data = vec![0u8; padded as usize];
        data[0] = 10; // R
        data[1] = 20; // G
        data[2] = 30; // B
        data[3] = 255; // A

        let pixels = unpad_and_convert_pixels(&data, 1, 1, padded, 4, false);
        assert_eq!(pixels, vec![10, 20, 30, 255]);
    }

    #[test]
    fn unpad_rgba_multiple_rows() {
        // 2x2 image, each row padded to 256 bytes.
        let padded = padded_bytes_per_row(2, 4, WGPU_ALIGNMENT);
        assert_eq!(padded, 256); // 2*4=8 < 256
        let mut data = vec![0u8; (padded * 2) as usize];
        // Row 0: pixels (R0,G0,B0,A0) and (R1,G1,B1,A1)
        data[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        // Row 1: pixels at offset 256
        data[256..264].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);

        let pixels = unpad_and_convert_pixels(&data, 2, 2, padded, 4, false);
        assert_eq!(
            pixels,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    // ---------------------------------------------------------------
    // unpad_and_convert_pixels: BGRA -> RGBA swizzle
    // ---------------------------------------------------------------

    #[test]
    fn bgra_to_rgba_single_pixel() {
        let padded = padded_bytes_per_row(1, 4, WGPU_ALIGNMENT);
        let mut data = vec![0u8; padded as usize];
        // BGRA: B=30, G=20, R=10, A=255
        data[0] = 30; // B
        data[1] = 20; // G
        data[2] = 10; // R
        data[3] = 255; // A

        let pixels = unpad_and_convert_pixels(&data, 1, 1, padded, 4, true);
        // Should be converted to RGBA: R=10, G=20, B=30, A=255
        assert_eq!(pixels, vec![10, 20, 30, 255]);
    }

    #[test]
    fn bgra_to_rgba_preserves_alpha() {
        let padded = padded_bytes_per_row(1, 4, WGPU_ALIGNMENT);
        let mut data = vec![0u8; padded as usize];
        data[0] = 100; // B
        data[1] = 150; // G
        data[2] = 200; // R
        data[3] = 128; // A (semi-transparent)

        let pixels = unpad_and_convert_pixels(&data, 1, 1, padded, 4, true);
        assert_eq!(pixels, vec![200, 150, 100, 128]);
    }

    #[test]
    fn bgra_to_rgba_multiple_pixels_per_row() {
        // 3 pixels wide, 1 row
        let padded = padded_bytes_per_row(3, 4, WGPU_ALIGNMENT);
        let mut data = vec![0u8; padded as usize];
        // Pixel 0: BGRA(0, 0, 255, 255) -> RGBA(255, 0, 0, 255) (pure red)
        data[0..4].copy_from_slice(&[0, 0, 255, 255]);
        // Pixel 1: BGRA(0, 255, 0, 255) -> RGBA(0, 255, 0, 255) (pure green)
        data[4..8].copy_from_slice(&[0, 255, 0, 255]);
        // Pixel 2: BGRA(255, 0, 0, 255) -> RGBA(0, 0, 255, 255) (pure blue)
        data[8..12].copy_from_slice(&[255, 0, 0, 255]);

        let pixels = unpad_and_convert_pixels(&data, 3, 1, padded, 4, true);
        assert_eq!(
            pixels,
            vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
            ]
        );
    }

    // ---------------------------------------------------------------
    // unpad_and_convert_pixels: output size
    // ---------------------------------------------------------------

    #[test]
    fn output_pixel_count_matches_dimensions() {
        let width = 10u32;
        let height = 5u32;
        let padded = padded_bytes_per_row(width, 4, WGPU_ALIGNMENT);
        let data = vec![0u8; (padded * height) as usize];

        let pixels = unpad_and_convert_pixels(&data, width, height, padded, 4, false);
        assert_eq!(pixels.len(), (width * height * 4) as usize);
    }

    // ---------------------------------------------------------------
    // encode_rgba_png
    // ---------------------------------------------------------------

    #[test]
    fn encode_png_produces_valid_png_header() {
        // A 1x1 red pixel
        let pixels = vec![255u8, 0, 0, 255];
        let png_data = encode_rgba_png(&pixels, 1, 1).unwrap();

        // PNG magic bytes: 137 80 78 71 13 10 26 10
        assert!(png_data.len() > 8);
        assert_eq!(&png_data[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[test]
    fn encode_png_roundtrip_1x1() {
        let original_pixels = vec![42u8, 100, 200, 255];
        let png_data = encode_rgba_png(&original_pixels, 1, 1).unwrap();

        // Decode the PNG and verify pixel values
        let decoder = png::Decoder::new(std::io::Cursor::new(&png_data));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        let decoded = &buf[..info.buffer_size()];

        assert_eq!(decoded, &original_pixels[..]);
    }

    #[test]
    fn encode_png_roundtrip_2x2() {
        // 2x2 image: red, green, blue, white
        let original_pixels = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 255, 255, // white
        ];
        let png_data = encode_rgba_png(&original_pixels, 2, 2).unwrap();

        let decoder = png::Decoder::new(std::io::Cursor::new(&png_data));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        let decoded = &buf[..info.buffer_size()];

        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        assert_eq!(decoded, &original_pixels[..]);
    }

    #[test]
    fn encode_png_reports_rgba_color_type() {
        let pixels = vec![0u8; 4]; // 1x1 transparent black
        let png_data = encode_rgba_png(&pixels, 1, 1).unwrap();

        let decoder = png::Decoder::new(std::io::Cursor::new(&png_data));
        let reader = decoder.read_info().unwrap();
        assert_eq!(reader.info().color_type, png::ColorType::Rgba);
        assert_eq!(reader.info().bit_depth, png::BitDepth::Eight);
    }

    // ---------------------------------------------------------------
    // Integration: unpad + encode pipeline
    // ---------------------------------------------------------------

    #[test]
    fn full_pipeline_bgra_padded_to_png() {
        // Simulate a 2x1 BGRA buffer with wgpu alignment.
        let width = 2u32;
        let height = 1u32;
        let padded = padded_bytes_per_row(width, 4, WGPU_ALIGNMENT);
        let mut data = vec![0u8; padded as usize];
        // Pixel 0: BGRA(255, 0, 0, 255) = pure blue in BGRA
        data[0..4].copy_from_slice(&[255, 0, 0, 255]);
        // Pixel 1: BGRA(0, 255, 0, 128) = pure green in BGRA (semi-transparent)
        data[4..8].copy_from_slice(&[0, 255, 0, 128]);

        let pixels = unpad_and_convert_pixels(&data, width, height, padded, 4, true);
        let png_data = encode_rgba_png(&pixels, width, height).unwrap();

        // Decode and verify
        let decoder = png::Decoder::new(std::io::Cursor::new(&png_data));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        let decoded = &buf[..info.buffer_size()];

        // BGRA(255,0,0,255) -> RGBA(0,0,255,255)
        // BGRA(0,255,0,128) -> RGBA(0,255,0,128)
        assert_eq!(decoded, &[0, 0, 255, 255, 0, 255, 0, 128]);
    }
}
