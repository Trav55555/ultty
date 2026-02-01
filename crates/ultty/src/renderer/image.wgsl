// Image rendering shader for Sixel and inline images

struct Uniforms {
    projection: mat4x4<f32>,
}

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(1) @binding(0)
var image_texture: texture_2d<f32>;

@group(1) @binding(1)
var image_sampler: sampler;

// Convert sRGB to linear color space.
// Colors from hex values and textures are in sRGB space; GPU expects linear for sRGB framebuffer.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = uniforms.projection * vec4<f32>(input.position, 0.0, 1.0);
    output.tex_coord = input.tex_coord;
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(image_texture, image_sampler, input.tex_coord);
    // Convert sRGB texture and vertex colors to linear for correct rendering
    let linear_tex = vec3<f32>(
        srgb_to_linear(tex_color.r),
        srgb_to_linear(tex_color.g),
        srgb_to_linear(tex_color.b)
    );
    let linear_vert = vec3<f32>(
        srgb_to_linear(input.color.r),
        srgb_to_linear(input.color.g),
        srgb_to_linear(input.color.b)
    );
    // Multiply for tinting/alpha support
    return vec4<f32>(linear_tex * linear_vert, tex_color.a * input.color.a);
}
