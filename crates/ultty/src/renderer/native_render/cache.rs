//! Native session render buffer upload and cache drawing.

use wgpu::util::DeviceExt;

use super::{Renderer, Scissor};
use crate::renderer::{NativeBufferSet, Vertex};

impl Renderer {
    pub(crate) fn has_cached_native_buffers(&self) -> bool {
        !self.cached_native_buffers.is_empty()
    }

    pub(crate) fn clear_cached_native_buffers(&mut self) {
        self.cached_native_buffers.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn upload_draw_and_cache_native_buffers(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        bg_vertices: &[Vertex],
        bg_indices: &[u32],
        text_vertices: &[Vertex],
        text_indices: &[u32],
        scissor: Option<Scissor>,
    ) {
        self.prev_bg_count = (bg_vertices.len(), bg_indices.len());
        self.prev_text_count = (text_vertices.len(), text_indices.len());
        self.cached_native_atlas_epoch = self.text_renderer.atlas_epoch();

        if let Some(buffer_set) =
            self.upload_and_draw_native_bg(encoder, view, bg_vertices, bg_indices, scissor)
        {
            self.cached_native_buffers.push_bg(buffer_set);
        }
        if let Some(buffer_set) =
            self.upload_and_draw_native_text(encoder, view, text_vertices, text_indices, scissor)
        {
            self.cached_native_buffers.push_text(buffer_set);
        }
    }

    fn upload_and_draw_native_bg(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertices: &[Vertex],
        indices: &[u32],
        scissor: Option<Scissor>,
    ) -> Option<NativeBufferSet> {
        if vertices.is_empty() {
            return None;
        }

        let index_count = indices.len() as u32;
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native_bg_vb"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native_bg_ib"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        self.draw_native_bg_buffers(
            encoder,
            view,
            &vertex_buffer,
            &index_buffer,
            index_count,
            scissor,
            "native_bg_pass",
        );

        Some((vertex_buffer, index_buffer, index_count, scissor))
    }

    fn upload_and_draw_native_text(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertices: &[Vertex],
        indices: &[u32],
        scissor: Option<Scissor>,
    ) -> Option<NativeBufferSet> {
        if vertices.is_empty() {
            return None;
        }

        let index_count = indices.len() as u32;
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native_text_vb"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native_text_ib"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        self.draw_native_text_buffers(
            encoder,
            view,
            &vertex_buffer,
            &index_buffer,
            index_count,
            scissor,
            "native_text_pass",
        );

        Some((vertex_buffer, index_buffer, index_count, scissor))
    }

    /// Draw native session content from cached GPU buffers without rebuilding vertices.
    pub fn draw_cached_native_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        for (vertex_buffer, index_buffer, index_count, scissor) in &self.cached_native_buffers.bg {
            self.draw_native_bg_buffers(
                encoder,
                view,
                vertex_buffer,
                index_buffer,
                *index_count,
                *scissor,
                "native_bg_pass_cached",
            );
        }

        for (vertex_buffer, index_buffer, index_count, scissor) in &self.cached_native_buffers.text
        {
            self.draw_native_text_buffers(
                encoder,
                view,
                vertex_buffer,
                index_buffer,
                *index_count,
                *scissor,
                "native_text_pass_cached",
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_native_bg_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        index_count: u32,
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
        render_pass.draw_indexed(0..index_count, 0, 0..1);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_native_text_buffers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        index_count: u32,
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
        render_pass.draw_indexed(0..index_count, 0, 0..1);
    }
}
