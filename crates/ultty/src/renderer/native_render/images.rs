//! Deferred native image draw submission.

use wgpu::util::DeviceExt;

use super::{Renderer, Scissor};
use crate::renderer::{DeferredImageRender, Vertex};

impl Renderer {
    /// Draw `deferred_images`, each within `scissor` when one is given.
    pub(super) fn render_deferred_native_images(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        deferred_images: &[DeferredImageRender],
        scissor: Option<Scissor>,
    ) {
        for image in deferred_images {
            let Some(cached) = self.native_image_cache.get(&image.resource) else {
                continue;
            };

            let (vertices, indices) = native_image_quad(image);
            let vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_image_vb"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native_image_ib"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native_image_pass"),
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

            if let Some(image_pipeline) = self.image_pipeline.as_ref() {
                if let Some((x, y, width, height)) = scissor {
                    render_pass.set_scissor_rect(x, y, width, height);
                }
                render_pass.set_pipeline(&image_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, &cached.bind_group, &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
            }
        }
    }
}

fn native_image_quad(image: &DeferredImageRender) -> ([Vertex; 4], [u32; 6]) {
    let color = [1.0f32, 1.0, 1.0, 1.0];
    let vertices = [
        Vertex {
            position: [image.x, image.y],
            tex_coord: [0.0, 0.0],
            color,
            is_color: 0,
        },
        Vertex {
            position: [image.x + image.w, image.y],
            tex_coord: [1.0, 0.0],
            color,
            is_color: 0,
        },
        Vertex {
            position: [image.x + image.w, image.y + image.h],
            tex_coord: [1.0, 1.0],
            color,
            is_color: 0,
        },
        Vertex {
            position: [image.x, image.y + image.h],
            tex_coord: [0.0, 1.0],
            color,
            is_color: 0,
        },
    ];
    let indices = [0, 1, 2, 0, 2, 3];

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use ultty_protocol::RegionId;

    use super::native_image_quad;
    use crate::renderer::DeferredImageRender;

    #[test]
    fn native_image_quad_preserves_geometry_and_indices() {
        let image = DeferredImageRender {
            resource: (1, RegionId::new(7)),
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };

        let (vertices, indices) = native_image_quad(&image);

        assert_eq!(vertices[0].position, [10.0, 20.0]);
        assert_eq!(vertices[1].position, [40.0, 20.0]);
        assert_eq!(vertices[2].position, [40.0, 60.0]);
        assert_eq!(vertices[3].position, [10.0, 60.0]);
        assert_eq!(indices, [0, 1, 2, 0, 2, 3]);
    }
}
