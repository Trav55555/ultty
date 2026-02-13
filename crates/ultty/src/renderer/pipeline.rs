//! wgpu render pipeline for terminal rendering.

use super::Vertex;

/// Shader source for background rendering (solid colors).
const BG_SHADER: &str = r#"
struct Uniforms {
    projection: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) is_color: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

// Convert sRGB to linear color space.
// Colors from hex values are in sRGB space; GPU expects linear for sRGB framebuffer.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.projection * vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Convert sRGB vertex colors to linear for correct rendering
    let linear_color = vec4<f32>(
        srgb_to_linear(in.color.r),
        srgb_to_linear(in.color.g),
        srgb_to_linear(in.color.b),
        in.color.a  // Alpha is not gamma-encoded
    );
    return linear_color;
}
"#;

/// Shader source for text rendering (textured quads with alpha).
const TEXT_SHADER: &str = r#"
struct Uniforms {
    projection: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(1) @binding(0)
var t_glyph: texture_2d<f32>;
@group(1) @binding(1)
var s_glyph: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) is_color: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) is_color: u32,
}

// Convert sRGB to linear color space.
// Colors from hex values are in sRGB space; GPU expects linear for sRGB framebuffer.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.projection * vec4<f32>(in.position, 0.0, 1.0);
    out.tex_coord = in.tex_coord;
    out.color = in.color;
    out.is_color = in.is_color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(t_glyph, s_glyph, in.tex_coord);
    if (in.is_color == 1u) {
        // Color emoji: texel has sRGB RGB + alpha from the RGBA atlas
        let linear_rgb = vec3<f32>(
            srgb_to_linear(texel.r),
            srgb_to_linear(texel.g),
            srgb_to_linear(texel.b)
        );
        return vec4<f32>(linear_rgb, texel.a);
    } else {
        // Monochrome glyph: alpha in texel.a (white mask), tint with vertex color
        let linear_color = vec3<f32>(
            srgb_to_linear(in.color.r),
            srgb_to_linear(in.color.g),
            srgb_to_linear(in.color.b)
        );
        return vec4<f32>(linear_color, in.color.a * texel.a);
    }
}
"#;

/// Render pipeline for background quads.
pub struct RenderPipeline {
    pipeline: wgpu::RenderPipeline,
}

impl RenderPipeline {
    /// Create the background render pipeline.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> anyhow::Result<Self> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bg_shader"),
            source: wgpu::ShaderSource::Wgsl(BG_SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg_pipeline_layout"),
            bind_group_layouts: &[uniform_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg_pipeline"),
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
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        Ok(Self { pipeline })
    }

    /// Get the underlying pipeline.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}

/// Render pipeline for text with glyph atlas.
pub struct TextPipeline {
    pipeline: wgpu::RenderPipeline,
}

impl TextPipeline {
    /// Create the text render pipeline.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        atlas_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> anyhow::Result<Self> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text_shader"),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text_pipeline_layout"),
            bind_group_layouts: &[uniform_bind_group_layout, atlas_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text_pipeline"),
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
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        Ok(Self { pipeline })
    }

    /// Get the underlying pipeline.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_sources_are_valid_wgsl() {
        // Basic syntax check - actual validation happens at runtime
        assert!(super::BG_SHADER.contains("@vertex"));
        assert!(super::BG_SHADER.contains("@fragment"));
        assert!(super::TEXT_SHADER.contains("@vertex"));
        assert!(super::TEXT_SHADER.contains("@fragment"));
        assert!(super::TEXT_SHADER.contains("textureSample"));
    }
}
