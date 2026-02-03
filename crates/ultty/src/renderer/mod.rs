//! GPU rendering subsystem for terminal emulation.
//!
//! This module provides hardware-accelerated rendering using wgpu:
//! - [`Renderer`] - Main renderer with device/queue management
//! - [`GlyphAtlas`] - Texture atlas for glyph caching
//! - [`TextRenderer`] - Text shaping and rendering with cosmic-text
//! - [`ColorScheme`] - Terminal color palette

mod atlas;
pub mod block_chars;
pub mod color;
mod image;
mod native_render;
mod pipeline;
mod screenshot;
mod text;

pub use atlas::{AtlasStats, GlyphAtlas};
pub use color::{ColorScheme, Rgba};
pub use pipeline::RenderPipeline;
pub use text::TextRenderer;

use block_chars::block_char_rects;
use crate::config::{ColorConfig, FontConfig};
use crate::grid::{CellFlags, CursorShape, ImageId, Terminal};
#[cfg(test)]
use crate::grid::Color;
use crate::tabs::TabBarState;
use crate::url::DetectedUrl;
use crate::vim::{CopyMode, Selection as VimSelection};
use ultty_protocol::vim::VimMode;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Scrollbar state for rendering.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollbarState {
    /// Total number of lines (history + viewport)
    pub total_lines: usize,
    /// Number of visible lines (viewport rows)
    pub viewport_lines: usize,
    /// Current scroll offset (0 = bottom, max = top of history)
    pub scroll_offset: usize,
}

impl ScrollbarState {
    /// Create a new scrollbar state.
    pub fn new(total_lines: usize, viewport_lines: usize, scroll_offset: usize) -> Self {
        Self {
            total_lines,
            viewport_lines,
            scroll_offset,
        }
    }

    /// Check if the scrollbar should be visible.
    pub fn is_visible(&self) -> bool {
        self.total_lines > self.viewport_lines
    }

    /// Calculate the thumb position and size as fractions (0.0-1.0).
    /// Returns (position, size) where position is from top.
    pub fn thumb_geometry(&self) -> (f32, f32) {
        if self.total_lines <= self.viewport_lines {
            return (0.0, 1.0);
        }

        let total = self.total_lines as f32;
        let viewport = self.viewport_lines as f32;
        let max_offset = (self.total_lines - self.viewport_lines) as f32;
        let offset = self.scroll_offset as f32;

        // Thumb size proportional to viewport / total
        let thumb_size = (viewport / total).clamp(0.05, 1.0);

        // Position: 0 when at bottom (offset=0), 1-size when at top (offset=max)
        let thumb_pos = if max_offset > 0.0 {
            (offset / max_offset) * (1.0 - thumb_size)
        } else {
            0.0
        };

        // Invert because 0 offset = at bottom = thumb at bottom
        let thumb_pos = (1.0 - thumb_size) - thumb_pos;

        (thumb_pos, thumb_size)
    }
}

/// Overlay state for rendering vim mode and URL highlights.
#[derive(Debug)]
pub struct OverlayState<'a> {
    /// Vim copy mode state (if active)
    pub copy_mode: Option<&'a CopyMode>,
    /// Detected URLs to highlight
    pub urls: &'a [DetectedUrl],
    /// Whether Ctrl is held (for URL highlighting)
    pub ctrl_held: bool,
    /// Tab bar state (if tabs are visible)
    pub tab_bar: Option<&'a TabBarState>,
    /// Scrollbar state (if scrollback exists)
    pub scrollbar: Option<ScrollbarState>,
    /// Visual bell active (border flash)
    pub bell_active: bool,
    /// Keyboard shortcuts overlay visible
    pub shortcuts_visible: bool,
    /// Keybindings to display in shortcuts overlay
    pub keybindings: Option<&'a [(String, String)]>,
    /// Whether cursor should be visible in current blink phase
    pub cursor_blink_visible: bool,
}

impl<'a> Default for OverlayState<'a> {
    fn default() -> Self {
        Self {
            copy_mode: None,
            urls: &[],
            ctrl_held: false,
            tab_bar: None,
            scrollbar: None,
            bell_active: false,
            shortcuts_visible: false,
            keybindings: None,
            cursor_blink_visible: true, // Default visible
        }
    }
}

/// Information about a pane for rendering.
#[derive(Debug, Clone, Copy)]
pub struct PaneInfo {
    /// X offset in pixels
    pub x: f32,
    /// Y offset in pixels
    pub y: f32,
    /// Width in pixels
    pub width: f32,
    /// Height in pixels
    pub height: f32,
    /// Whether this pane is focused
    pub is_focused: bool,
}

impl PaneInfo {
    /// Create a new pane info.
    pub fn new(x: f32, y: f32, width: f32, height: f32, is_focused: bool) -> Self {
        Self { x, y, width, height, is_focused }
    }

    /// Create pane info for a single full-screen terminal.
    pub fn full_screen(width: f32, height: f32) -> Self {
        Self { x: 0.0, y: 0.0, width, height, is_focused: true }
    }
}

/// Vertex for quad rendering.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 2],
    pub tex_coord: [f32; 2],
    pub color: [f32; 4],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x4,
    ];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Uniform buffer for projection matrix.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub projection: [[f32; 4]; 4],
}

impl Uniforms {
    pub fn orthographic(width: f32, height: f32) -> Self {
        // Orthographic projection: (0,0) top-left, (width, height) bottom-right
        Self {
            projection: [
                [2.0 / width, 0.0, 0.0, 0.0],
                [0.0, -2.0 / height, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [-1.0, 1.0, 0.0, 1.0],
            ],
        }
    }
}

/// Cached GPU texture for a terminal image.
#[allow(dead_code)] // Infrastructure for Sixel/image rendering pipeline
pub(super) struct CachedImageTexture {
    pub(super) texture: wgpu::Texture,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// GPU renderer for the terminal.
pub struct Renderer {
    /// wgpu device
    device: Arc<wgpu::Device>,
    /// wgpu queue
    queue: Arc<wgpu::Queue>,
    /// Surface configuration
    config: wgpu::SurfaceConfiguration,
    /// Render pipeline for backgrounds
    bg_pipeline: RenderPipeline,
    /// Render pipeline for images
    image_pipeline: Option<ImagePipeline>,
    /// Text renderer
    text_renderer: TextRenderer,
    /// Color scheme
    color_scheme: ColorScheme,
    /// Cell dimensions in pixels
    cell_width: f32,
    cell_height: f32,
    /// Uniform buffer
    uniform_buffer: wgpu::Buffer,
    /// Uniform bind group
    uniform_bind_group: wgpu::BindGroup,
    /// Current tab bar height (0 if not visible)
    tab_bar_height: f32,
    /// Current pane X offset for rendering
    pane_offset_x: f32,
    /// Current pane Y offset for rendering
    pane_offset_y: f32,
    /// Window opacity (0.0-1.0)
    opacity: f32,
    /// Cached image textures (terminal sixel images)
    image_cache: std::collections::HashMap<ImageId, CachedImageTexture>,
    /// Cached image textures for native protocol ImageWidget, keyed by RegionId
    native_image_cache: std::collections::HashMap<ultty_protocol::RegionId, CachedImageTexture>,
    /// Animated progress: region_id -> current displayed value (lerped toward target)
    animated_progress: std::collections::HashMap<ultty_protocol::RegionId, f32>,
    /// Whether active animations exist (spinners, animated progress)
    has_animations: bool,
}

/// Deferred native image render request (collected during widget content rendering).
pub(super) struct DeferredImageRender {
    region_id: ultty_protocol::RegionId,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Pipeline for rendering images.
pub(super) struct ImagePipeline {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) bind_group_layout: wgpu::BindGroupLayout,
    pub(super) sampler: wgpu::Sampler,
}

impl Renderer {
    /// Create a new renderer with configuration.
    pub async fn with_config(
        surface: &wgpu::Surface<'_>,
        adapter: &wgpu::Adapter,
        width: u32,
        height: u32,
        color_config: &ColorConfig,
        font_config: &FontConfig,
    ) -> anyhow::Result<Self> {
        let mut renderer = Self::new(surface, adapter, width, height, font_config).await?;
        renderer.color_scheme = ColorScheme::from_config(color_config);
        Ok(renderer)
    }

    /// Create a new renderer.
    pub async fn new(
        surface: &wgpu::Surface<'_>,
        adapter: &wgpu::Adapter,
        width: u32,
        height: u32,
        font_config: &FontConfig,
    ) -> anyhow::Result<Self> {
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("ultty_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let surface_caps = surface.get_capabilities(adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        // Prefer pre-multiplied alpha for transparency support
        let alpha_mode = if surface_caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            surface_caps.alpha_modes[0]
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Create uniform buffer
        let uniforms = Uniforms::orthographic(width as f32, height as f32);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniform_buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create uniform bind group layout
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("uniform_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform_bind_group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Create render pipeline
        let bg_pipeline =
            RenderPipeline::new(&device, surface_format, &uniform_bind_group_layout)?;

        // Create text renderer
        let text_renderer = TextRenderer::with_config(
            device.clone(),
            queue.clone(),
            surface_format,
            &uniform_bind_group_layout,
            font_config,
        )?;

        // Get cell dimensions from text renderer
        let (cell_width, cell_height) = text_renderer.cell_dimensions();

        Ok(Self {
            device,
            queue,
            config,
            bg_pipeline,
            image_pipeline: None, // Created lazily when needed
            text_renderer,
            color_scheme: ColorScheme::default(),
            cell_width,
            cell_height,
            uniform_buffer,
            uniform_bind_group,
            tab_bar_height: 0.0,
            pane_offset_x: 0.0,
            pane_offset_y: 0.0,
            opacity: 1.0,
            image_cache: std::collections::HashMap::new(),
            native_image_cache: std::collections::HashMap::new(),
            animated_progress: std::collections::HashMap::new(),
            has_animations: false,
        })
    }

    /// Create a new headless renderer for benchmarking (no window/surface required).
    ///
    /// This creates a renderer that can render to textures without a window.
    /// Useful for benchmarking and testing.
    pub async fn new_headless(width: u32, height: u32) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Request adapter without a surface (headless)
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to find suitable GPU adapter for headless rendering"))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("ultty_headless_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        // Use a common sRGB format for headless rendering
        let surface_format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Create a dummy surface configuration (not actually used for surface rendering)
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        // Create uniform buffer
        let uniforms = Uniforms::orthographic(width as f32, height as f32);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniform_buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create uniform bind group layout
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("uniform_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform_bind_group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Create render pipeline
        let bg_pipeline =
            RenderPipeline::new(&device, surface_format, &uniform_bind_group_layout)?;

        // Create text renderer with default font config
        let font_config = crate::config::FontConfig::default();
        let text_renderer = TextRenderer::with_config(
            device.clone(),
            queue.clone(),
            surface_format,
            &uniform_bind_group_layout,
            &font_config,
        )?;

        // Get cell dimensions from text renderer
        let (cell_width, cell_height) = text_renderer.cell_dimensions();

        Ok(Self {
            device,
            queue,
            config,
            bg_pipeline,
            image_pipeline: None,
            text_renderer,
            color_scheme: ColorScheme::default(),
            cell_width,
            cell_height,
            uniform_buffer,
            uniform_bind_group,
            tab_bar_height: 0.0,
            pane_offset_x: 0.0,
            pane_offset_y: 0.0,
            opacity: 1.0,
            image_cache: std::collections::HashMap::new(),
            native_image_cache: std::collections::HashMap::new(),
            animated_progress: std::collections::HashMap::new(),
            has_animations: false,
        })
    }

    /// Render to a texture (headless) and return timing information.
    ///
    /// This is useful for benchmarking the render pipeline without a window.
    /// Returns the time taken to render in microseconds.
    pub fn render_to_texture(
        &mut self,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) -> anyhow::Result<()> {
        let width = self.config.width;
        let height = self.config.height;

        // Create render target texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_render_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.tab_bar_height = overlay.tab_bar.map(|tb| tb.height()).unwrap_or(0.0);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bench_render_encoder"),
            });

        // Build vertex data for backgrounds
        let (bg_vertices, bg_indices) = self.build_background_vertices(terminal, overlay);

        // Create background vertex/index buffers
        let bg_vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_vertex_buffer"),
                contents: bytemuck::cast_slice(&bg_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let bg_index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_index_buffer"),
                contents: bytemuck::cast_slice(&bg_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        // Render pass for backgrounds
        {
            let linear_bg = self.color_scheme.background.to_linear();
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench_bg_render_pass"),
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

        // Render text
        self.text_renderer.render(
            &mut encoder,
            &view,
            terminal,
            &self.color_scheme,
            self.cell_width,
            self.cell_height,
            self.tab_bar_height,
            &self.uniform_bind_group,
        )?;

        // Build and render overlays
        let (overlay_vertices, overlay_indices) = self.build_overlay_vertices(terminal, overlay);
        if !overlay_vertices.is_empty() {
            let overlay_vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("overlay_vertex_buffer"),
                    contents: bytemuck::cast_slice(&overlay_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

            let overlay_index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("overlay_index_buffer"),
                    contents: bytemuck::cast_slice(&overlay_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench_overlay_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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

            render_pass.set_pipeline(self.bg_pipeline.pipeline());
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, overlay_vertex_buffer.slice(..));
            render_pass.set_index_buffer(overlay_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..overlay_indices.len() as u32, 0, 0..1);
        }

        // Submit and wait for completion
        self.queue.submit(std::iter::once(encoder.finish()));
        self.device.poll(wgpu::Maintain::Wait);

        Ok(())
    }

    /// Get cell dimensions in pixels.
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Get terminal dimensions in cells for the current surface size.
    pub fn terminal_dimensions(&self) -> (u16, u16) {
        let cols = (self.config.width as f32 / self.cell_width).floor() as u16;
        let available_height = self.config.height as f32 - self.tab_bar_height;
        let rows = (available_height / self.cell_height).floor() as u16;
        (cols.max(1), rows.max(1))
    }

    /// Set the tab bar height for dimension calculations.
    pub fn set_tab_bar_height(&mut self, height: f32) {
        self.tab_bar_height = height;
    }

    /// Get the current tab bar height.
    pub fn tab_bar_height(&self) -> f32 {
        self.tab_bar_height
    }

    /// Resize the renderer.
    pub fn resize(&mut self, surface: &wgpu::Surface, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }

        self.config.width = width;
        self.config.height = height;
        surface.configure(&self.device, &self.config);

        // Update projection matrix
        let uniforms = Uniforms::orthographic(width as f32, height as f32);
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
    }

    /// Clear cached image textures for terminal images (e.g., on tab close).
    pub fn clear_image_cache(&mut self) {
        self.image_cache.clear();
    }

    /// Clear cached native image textures, retaining only the given region IDs.
    pub fn retain_native_images(&mut self, live_regions: &std::collections::HashSet<ultty_protocol::RegionId>) {
        self.native_image_cache.retain(|id, _| live_regions.contains(id));
    }

    /// Set the color scheme.
    pub fn set_color_scheme(&mut self, scheme: ColorScheme) {
        self.color_scheme = scheme;
    }

    /// Set window opacity (0.0-1.0).
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// Enable or disable ligatures.
    pub fn set_ligatures(&mut self, enabled: bool) {
        self.text_renderer.set_ligatures(enabled);
    }

    /// Get the current font size.
    pub fn font_size(&self) -> f32 {
        self.text_renderer.font_size()
    }

    /// Set the font size and update cell dimensions.
    /// Returns the new font size (may be clamped).
    pub fn set_font_size(&mut self, size: f32) -> f32 {
        self.text_renderer.set_font_size(size);
        let (cell_width, cell_height) = self.text_renderer.cell_dimensions();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.text_renderer.font_size()
    }

    /// Increase font size by a step.
    pub fn increase_font_size(&mut self) -> f32 {
        let current = self.font_size();
        // Use larger steps for larger sizes
        let step = if current >= 20.0 { 2.0 } else { 1.0 };
        self.set_font_size(current + step)
    }

    /// Decrease font size by a step.
    pub fn decrease_font_size(&mut self) -> f32 {
        let current = self.font_size();
        // Use larger steps for larger sizes
        let step = if current > 20.0 { 2.0 } else { 1.0 };
        self.set_font_size(current - step)
    }

    /// Render the terminal to a surface.
    pub fn render(
        &mut self,
        surface: &wgpu::Surface,
        terminal: &Terminal,
    ) -> Result<(), wgpu::SurfaceError> {
        self.render_with_overlay(surface, terminal, &OverlayState::default())
    }

    /// Render the terminal with overlay state (vim mode, URL highlights).
    pub fn render_with_overlay(
        &mut self,
        surface: &wgpu::Surface,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) -> Result<(), wgpu::SurfaceError> {
        // Update tab bar height from overlay state
        self.tab_bar_height = overlay.tab_bar.map(|tb| tb.height()).unwrap_or(0.0);

        let output = surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });

        // Build tab bar vertices if visible
        let (tab_bar_vertices, tab_bar_indices) = if let Some(tab_bar) = overlay.tab_bar {
            self.build_tab_bar_vertices(tab_bar)
        } else {
            (Vec::new(), Vec::new())
        };

        // Build vertex data for backgrounds and text
        let (bg_vertices, bg_indices) = self.build_background_vertices(terminal, overlay);

        // Create background vertex/index buffers
        let bg_vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_vertex_buffer"),
                contents: bytemuck::cast_slice(&bg_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let bg_index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_index_buffer"),
                contents: bytemuck::cast_slice(&bg_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        // Render pass
        {
            // Convert sRGB colors to linear space for correct rendering in sRGB framebuffer
            let linear_bg = self.color_scheme.background.to_linear();
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
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
            });

            // Draw backgrounds
            render_pass.set_pipeline(self.bg_pipeline.pipeline());
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, bg_vertex_buffer.slice(..));
            render_pass.set_index_buffer(bg_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
        }

        // Render images (Sixel, etc.) after backgrounds, before text
        self.render_images(&mut encoder, &view, terminal)?;

        // Render tab bar if visible
        if !tab_bar_vertices.is_empty() {
            let tab_bar_vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tab_bar_vertex_buffer"),
                    contents: bytemuck::cast_slice(&tab_bar_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

            let tab_bar_index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tab_bar_index_buffer"),
                    contents: bytemuck::cast_slice(&tab_bar_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tab_bar_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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

            render_pass.set_pipeline(self.bg_pipeline.pipeline());
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, tab_bar_vertex_buffer.slice(..));
            render_pass.set_index_buffer(tab_bar_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..tab_bar_indices.len() as u32, 0, 0..1);
        }

        // Render tab titles if tab bar is visible
        if let Some(tab_bar) = overlay.tab_bar {
            if tab_bar.visible {
                let (title_vertices, title_indices) = self.build_tab_title_vertices(tab_bar);

                if !title_vertices.is_empty() {
                    let title_vertex_buffer = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tab_title_vertex_buffer"),
                            contents: bytemuck::cast_slice(&title_vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });

                    let title_index_buffer = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tab_title_index_buffer"),
                            contents: bytemuck::cast_slice(&title_indices),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("tab_title_render_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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

                    render_pass.set_pipeline(self.text_renderer.pipeline());
                    render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    render_pass.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
                    render_pass.set_vertex_buffer(0, title_vertex_buffer.slice(..));
                    render_pass.set_index_buffer(title_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..title_indices.len() as u32, 0, 0..1);
                }
            }
        }

        // Render text (offset by tab bar height)
        self.text_renderer.render(
            &mut encoder,
            &view,
            terminal,
            &self.color_scheme,
            self.cell_width,
            self.cell_height,
            self.tab_bar_height,
            &self.uniform_bind_group,
        )?;

        // Render overlays (vim cursor, URL underlines)
        let (overlay_vertices, overlay_indices) = self.build_overlay_vertices(terminal, overlay);
        if !overlay_vertices.is_empty() {
            let overlay_vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("overlay_vertex_buffer"),
                    contents: bytemuck::cast_slice(&overlay_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

            let overlay_index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("overlay_index_buffer"),
                    contents: bytemuck::cast_slice(&overlay_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            // Overlay render pass
            {
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("overlay_render_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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

                render_pass.set_pipeline(self.bg_pipeline.pipeline());
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, overlay_vertex_buffer.slice(..));
                render_pass.set_index_buffer(overlay_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..overlay_indices.len() as u32, 0, 0..1);
            }
        }

        // Render vim mode indicator if in copy mode
        if let Some(copy_mode) = overlay.copy_mode {
            if copy_mode.is_active() {
                let (indicator_vertices, indicator_indices) = self.build_vim_mode_indicator_vertices(copy_mode);
                if !indicator_vertices.is_empty() {
                    // Render background quads
                    let bg_vertices: Vec<_> = indicator_vertices.iter()
                        .take(4) // First 4 vertices are the background quad
                        .cloned()
                        .collect();
                    let bg_indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

                    let indicator_bg_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("indicator_bg_vb"),
                        contents: bytemuck::cast_slice(&bg_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let indicator_bg_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("indicator_bg_ib"),
                        contents: bytemuck::cast_slice(&bg_indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("indicator_bg_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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

                    rp.set_pipeline(self.bg_pipeline.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_vertex_buffer(0, indicator_bg_vb.slice(..));
                    rp.set_index_buffer(indicator_bg_ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
                }

                // Render text vertices (skip the first 4 background vertices)
                if indicator_vertices.len() > 4 {
                    let text_vertices: Vec<_> = indicator_vertices.into_iter().skip(4).collect();
                    let text_indices: Vec<u32> = indicator_indices.into_iter()
                        .filter(|i| *i >= 4)
                        .map(|i| i - 4)
                        .collect();

                    if !text_vertices.is_empty() {
                        let indicator_text_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("indicator_text_vb"),
                            contents: bytemuck::cast_slice(&text_vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                        let indicator_text_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("indicator_text_ib"),
                            contents: bytemuck::cast_slice(&text_indices),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("indicator_text_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
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

        // Render keyboard shortcuts overlay if visible
        if overlay.shortcuts_visible {
            if let Some(keybindings) = overlay.keybindings {
                self.render_shortcuts_overlay(&mut encoder, &view, keybindings);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    pub fn render_with_native_sessions<'a>(
        &mut self,
        surface: &wgpu::Surface,
        terminal: &Terminal,
        overlay: &OverlayState,
        sessions: impl Iterator<Item = &'a crate::protocol::NativeSession>,
        native_search_matches: Option<&[crate::scene::SearchMatch]>,
        native_search_index: usize,
    ) -> Result<(), wgpu::SurfaceError> {
        self.tab_bar_height = overlay.tab_bar.map(|tb| tb.height()).unwrap_or(0.0);

        let output = surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });

        let (tab_bar_vertices, tab_bar_indices) = if let Some(tab_bar) = overlay.tab_bar {
            self.build_tab_bar_vertices(tab_bar)
        } else {
            (Vec::new(), Vec::new())
        };

        let (bg_vertices, bg_indices) = self.build_background_vertices(terminal, overlay);

        let bg_vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_vertex_buffer"),
                contents: bytemuck::cast_slice(&bg_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let bg_index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_index_buffer"),
                contents: bytemuck::cast_slice(&bg_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        {
            let linear_bg = self.color_scheme.background.to_linear();
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_render_pass"),
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

        self.render_images(&mut encoder, &view, terminal)?;

        if !tab_bar_vertices.is_empty() {
            let tab_bar_vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tab_bar_vertex_buffer"),
                    contents: bytemuck::cast_slice(&tab_bar_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let tab_bar_index_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tab_bar_index_buffer"),
                    contents: bytemuck::cast_slice(&tab_bar_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tab_bar_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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

            render_pass.set_pipeline(self.bg_pipeline.pipeline());
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_vertex_buffer(0, tab_bar_vertex_buffer.slice(..));
            render_pass.set_index_buffer(tab_bar_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..tab_bar_indices.len() as u32, 0, 0..1);
        }

        self.text_renderer.render(
            &mut encoder,
            &view,
            terminal,
            &self.color_scheme,
            self.cell_width,
            self.cell_height,
            self.tab_bar_height,
            &self.uniform_bind_group,
        )?;

        for session in sessions {
            self.render_native_session(&mut encoder, &view, session, native_search_matches, native_search_index);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Render the keyboard shortcuts overlay.
    fn render_shortcuts_overlay(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        keybindings: &[(String, String)],
    ) {
        let (vertices, indices) = self.build_shortcuts_overlay_vertices(keybindings);
        if vertices.is_empty() {
            return;
        }

        // Separate background vertices (quads) from text vertices
        // The first several vertices are background quads (backdrop, modal, borders)
        // Text vertices come after
        // We need to count how many background quads we have
        // Each quad has 4 vertices and 6 indices
        // Let's render all vertices with bg_pipeline first (for backgrounds) then text_pipeline

        // For simplicity, render everything with the background pipeline
        // The text vertices will use the text renderer which has its own atlas

        // Count background quads: backdrop (1) + modal (1) + 4 borders + separator (1) = 7 quads = 28 vertices
        // Actually let's just render everything together since emit_rect uses Vertex which works with bg_pipeline

        let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shortcuts_vb"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shortcuts_ib"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Render with background pipeline (handles colored quads)
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shortcuts_bg_pass"),
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

            rp.set_pipeline(self.bg_pipeline.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }

        // Render text with text pipeline
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shortcuts_text_pass"),
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

            rp.set_pipeline(self.text_renderer.pipeline());
            rp.set_bind_group(0, &self.uniform_bind_group, &[]);
            rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }
    }

    /// Render multiple terminal panes.
    pub fn render_panes(
        &mut self,
        surface: &wgpu::Surface,
        panes: &[(&Terminal, PaneInfo)],
        overlay: &OverlayState,
    ) -> Result<(), wgpu::SurfaceError> {
        // Update tab bar height from overlay state
        self.tab_bar_height = overlay.tab_bar.map(|tb| tb.height()).unwrap_or(0.0);

        let output = surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("panes_render_encoder"),
            });

        // Clear screen with background color
        // Convert sRGB colors to linear space for correct rendering in sRGB framebuffer
        let linear_bg = self.color_scheme.background.to_linear();
        {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
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
            });
        }

        // Build tab bar vertices if visible
        if let Some(tab_bar) = overlay.tab_bar {
            let (tab_bar_vertices, tab_bar_indices) = self.build_tab_bar_vertices(tab_bar);
            if !tab_bar_vertices.is_empty() {
                let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tab_bar_vb"),
                    contents: bytemuck::cast_slice(&tab_bar_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("tab_bar_ib"),
                    contents: bytemuck::cast_slice(&tab_bar_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("tab_bar_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
                    let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("tab_title_vb"),
                        contents: bytemuck::cast_slice(&tv),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("tab_title_ib"),
                        contents: bytemuck::cast_slice(&ti),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("tab_title_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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

                    rp.set_pipeline(self.text_renderer.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_bind_group(1, self.text_renderer.atlas_bind_group(), &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..ti.len() as u32, 0, 0..1);
                }
            }
        }

        // Render each pane
        for (terminal, pane) in panes {
            self.pane_offset_x = pane.x;
            self.pane_offset_y = pane.y + self.tab_bar_height;

            // Render pane backgrounds
            let (bg_v, bg_i) = self.build_background_vertices_with_offset(terminal, overlay);
            if !bg_v.is_empty() {
                let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pane_bg_vb"),
                    contents: bytemuck::cast_slice(&bg_v),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("pane_bg_ib"),
                    contents: bytemuck::cast_slice(&bg_i),
                    usage: wgpu::BufferUsages::INDEX,
                });

                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("pane_bg_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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

                rp.set_pipeline(self.bg_pipeline.pipeline());
                rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                rp.set_vertex_buffer(0, vb.slice(..));
                rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..bg_i.len() as u32, 0, 0..1);
            }

            // Render pane text
            self.text_renderer.render_with_offset(
                &mut encoder,
                &view,
                terminal,
                &self.color_scheme,
                self.cell_width,
                self.cell_height,
                self.pane_offset_x,
                self.pane_offset_y,
                &self.uniform_bind_group,
            )?;

            // Draw pane border if not focused
            if !pane.is_focused {
                let border_color = Rgba::new(0.3, 0.3, 0.3, 1.0);
                let (bv, bi) = self.build_pane_border_vertices(pane, border_color);
                if !bv.is_empty() {
                    let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("pane_border_vb"),
                        contents: bytemuck::cast_slice(&bv),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("pane_border_ib"),
                        contents: bytemuck::cast_slice(&bi),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("pane_border_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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

                    rp.set_pipeline(self.bg_pipeline.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..bi.len() as u32, 0, 0..1);
                }
            } else {
                // Draw focused pane border in accent color
                let accent_color = Rgba::new(0.3, 0.5, 0.8, 1.0);
                let (bv, bi) = self.build_pane_border_vertices(pane, accent_color);
                if !bv.is_empty() {
                    let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("focused_border_vb"),
                        contents: bytemuck::cast_slice(&bv),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("focused_border_ib"),
                        contents: bytemuck::cast_slice(&bi),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("focused_border_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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
                let (indicator_vertices, indicator_indices) = self.build_vim_mode_indicator_vertices(copy_mode);
                if !indicator_vertices.is_empty() {
                    // Render background quads
                    let bg_vertices: Vec<_> = indicator_vertices.iter()
                        .take(4)
                        .cloned()
                        .collect();
                    let bg_indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];

                    let indicator_bg_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("indicator_bg_vb"),
                        contents: bytemuck::cast_slice(&bg_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    let indicator_bg_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("indicator_bg_ib"),
                        contents: bytemuck::cast_slice(&bg_indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });

                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("indicator_bg_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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

                    rp.set_pipeline(self.bg_pipeline.pipeline());
                    rp.set_bind_group(0, &self.uniform_bind_group, &[]);
                    rp.set_vertex_buffer(0, indicator_bg_vb.slice(..));
                    rp.set_index_buffer(indicator_bg_ib.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..bg_indices.len() as u32, 0, 0..1);
                }

                // Render text vertices
                if indicator_vertices.len() > 4 {
                    let text_vertices: Vec<_> = indicator_vertices.into_iter().skip(4).collect();
                    let text_indices: Vec<u32> = indicator_indices.into_iter()
                        .filter(|i| *i >= 4)
                        .map(|i| i - 4)
                        .collect();

                    if !text_vertices.is_empty() {
                        let indicator_text_vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("indicator_text_vb"),
                            contents: bytemuck::cast_slice(&text_vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                        let indicator_text_ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("indicator_text_ib"),
                            contents: bytemuck::cast_slice(&text_indices),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("indicator_text_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
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

        // Render keyboard shortcuts overlay if visible
        if overlay.shortcuts_visible {
            if let Some(keybindings) = overlay.keybindings {
                self.render_shortcuts_overlay(&mut encoder, &view, keybindings);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Build background vertices with pane offset.
    fn build_background_vertices_with_offset(&self, terminal: &Terminal, overlay: &OverlayState) -> (Vec<Vertex>, Vec<u32>) {
        let grid = terminal.grid();
        let dims = grid.dimensions();
        // Pre-allocate based on terminal size: ~1 bg run per row on average = rows quads
        let est_quads = dims.rows as usize * 4;
        let mut vertices = Vec::with_capacity(est_quads * 4);
        let mut indices = Vec::with_capacity(est_quads * 6);

        for row_idx in 0..dims.rows {
            let row = match grid.row(row_idx) {
                Some(r) => r,
                None => continue,
            };

            let mut run_start = 0u16;
            let mut run_color: Option<Rgba> = None;

            for col_idx in 0..dims.cols {
                let cell = match row.get(col_idx) {
                    Some(c) => c,
                    None => continue,
                };

                let is_selected = terminal.is_selected(col_idx, row_idx);
                let bg_color = if is_selected {
                    self.color_scheme.selection
                } else if cell.has_flag(CellFlags::REVERSE) {
                    self.color_scheme.resolve(cell.fg(), true)
                } else {
                    self.color_scheme.resolve(cell.bg(), false)
                };

                let should_flush = match run_color {
                    None => false,
                    Some(rc) => rc != bg_color,
                };

                if should_flush {
                    if let Some(color) = run_color {
                        if color != self.color_scheme.background {
                            self.emit_quad_with_offset(&mut vertices, &mut indices, run_start, col_idx, row_idx, color);
                        }
                    }
                    run_start = col_idx;
                }

                run_color = Some(bg_color);
            }

            if let Some(color) = run_color {
                if color != self.color_scheme.background {
                    self.emit_quad_with_offset(&mut vertices, &mut indices, run_start, dims.cols, row_idx, color);
                }
            }
        }

        // Block characters (U+2580..U+259F) rendered as solid quads for pixel-perfect cell fill
        for row_idx in 0..dims.rows {
            let row = match grid.row(row_idx) {
                Some(r) => r,
                None => continue,
            };
            for col_idx in 0..dims.cols {
                let cell = match row.get(col_idx) {
                    Some(c) => c,
                    None => continue,
                };
                let ch = cell.char();
                if let Some((rects, count)) = block_char_rects(ch) {
                    let fg = if cell.has_flag(CellFlags::REVERSE) {
                        self.color_scheme.resolve(cell.bg(), false)
                    } else {
                        self.color_scheme.resolve(cell.fg(), true)
                    };
                    let fg = if cell.has_flag(CellFlags::DIM) {
                        Rgba::new(fg.r * 0.5, fg.g * 0.5, fg.b * 0.5, fg.a)
                    } else {
                        fg
                    };
                    let c = fg.to_array();
                    let cx = col_idx as f32 * self.cell_width + self.pane_offset_x;
                    let cy = row_idx as f32 * self.cell_height + self.pane_offset_y;
                    for &(rx, ry, rw, rh) in &rects[..count] {
                        let x0 = cx + rx * self.cell_width;
                        let y0 = cy + ry * self.cell_height;
                        let w = rw * self.cell_width;
                        let h = rh * self.cell_height;
                        let base = vertices.len() as u32;
                        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: c });
                        vertices.push(Vertex { position: [x0 + w, y0], tex_coord: [0.0, 0.0], color: c });
                        vertices.push(Vertex { position: [x0 + w, y0 + h], tex_coord: [0.0, 0.0], color: c });
                        vertices.push(Vertex { position: [x0, y0 + h], tex_coord: [0.0, 0.0], color: c });
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

        // Draw cursor
        let cursor = terminal.buffer().grid().cursor();
        let should_show_cursor = cursor.visible
            && (!cursor.blinking || overlay.cursor_blink_visible);

        if should_show_cursor {
            let cursor_color = self.color_scheme.cursor;
            let col = cursor.position.col;
            let row = cursor.position.row;

            match cursor.shape {
                CursorShape::Block => {
                    self.emit_quad_with_offset(&mut vertices, &mut indices, col, col + 1, row, cursor_color);
                }
                CursorShape::Underline => {
                    self.emit_cursor_underline_with_offset(&mut vertices, &mut indices, col, row, cursor_color);
                }
                CursorShape::Bar => {
                    self.emit_cursor_bar_with_offset(&mut vertices, &mut indices, col, row, cursor_color);
                }
            }
        }

        (vertices, indices)
    }

    /// Emit a thin underline cursor with pane offset.
    fn emit_cursor_underline_with_offset(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = col as f32 * self.cell_width + self.pane_offset_x;
        let x1 = (col + 1) as f32 * self.cell_width + self.pane_offset_x;
        let underline_height = 2.0f32.min(self.cell_height * 0.15);
        let y0 = (row + 1) as f32 * self.cell_height + self.pane_offset_y - underline_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.pane_offset_y;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a thin bar cursor with pane offset.
    fn emit_cursor_bar_with_offset(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let bar_width = 2.0f32.min(self.cell_width * 0.15);
        let x0 = col as f32 * self.cell_width + self.pane_offset_x;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * self.cell_height + self.pane_offset_y;
        let y1 = (row + 1) as f32 * self.cell_height + self.pane_offset_y;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a quad with pane offset.
    fn emit_quad_with_offset(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        start_col: u16,
        end_col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = start_col as f32 * self.cell_width + self.pane_offset_x;
        let x1 = end_col as f32 * self.cell_width + self.pane_offset_x;
        let y0 = row as f32 * self.cell_height + self.pane_offset_y;
        let y1 = y0 + self.cell_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Build pane border vertices.
    fn build_pane_border_vertices(&self, pane: &PaneInfo, color: Rgba) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let border_width = 2.0;
        let x = pane.x;
        let y = pane.y + self.tab_bar_height;
        let w = pane.width;
        let h = pane.height;

        // Top border
        self.emit_rect(&mut vertices, &mut indices, x, y, w, border_width, color);
        // Bottom border
        self.emit_rect(&mut vertices, &mut indices, x, y + h - border_width, w, border_width, color);
        // Left border
        self.emit_rect(&mut vertices, &mut indices, x, y, border_width, h, color);
        // Right border
        self.emit_rect(&mut vertices, &mut indices, x + w - border_width, y, border_width, h, color);

        (vertices, indices)
    }

    /// Build overlay vertex data (vim cursor, selection, URL underlines).
    fn build_overlay_vertices(
        &self,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let dims = terminal.grid().dimensions();

        // Render vim copy mode overlays
        if let Some(copy_mode) = overlay.copy_mode {
            if copy_mode.is_active() {
                let (cursor_row, cursor_col) = copy_mode.cursor_position();
                let scroll_offset = copy_mode.scroll_offset();

                // Draw vim cursor (different color from normal cursor)
                let vim_cursor_color = Rgba::new(0.8, 0.4, 0.0, 0.8); // Orange
                let visible_row = cursor_row.saturating_sub(scroll_offset);
                if visible_row < dims.rows as usize {
                    self.emit_quad(
                        &mut vertices,
                        &mut indices,
                        cursor_col as u16,
                        cursor_col as u16 + 1,
                        visible_row as u16,
                        vim_cursor_color,
                    );
                }

                // Draw vim selection
                if let Some(selection) = copy_mode.selection() {
                    let selection_color = Rgba::new(0.4, 0.6, 0.8, 0.4); // Blue highlight
                    self.emit_vim_selection(
                        &mut vertices,
                        &mut indices,
                        &selection,
                        scroll_offset,
                        dims.rows as usize,
                        dims.cols as usize,
                        selection_color,
                    );
                }

                // Draw search match highlights
                let (matches, current_match) = copy_mode.search_matches();
                if !matches.is_empty() {
                    let match_color = Rgba::new(0.8, 0.6, 0.2, 0.4); // Yellow highlight
                    let current_match_color = Rgba::new(1.0, 0.5, 0.0, 0.6); // Bright orange for current

                    for (i, &(match_row, match_col)) in matches.iter().enumerate() {
                        let visible_row = match_row.saturating_sub(scroll_offset);
                        if visible_row < dims.rows as usize {
                            let color = if i == current_match {
                                current_match_color
                            } else {
                                match_color
                            };
                            // Highlight single character for now (could extend to match length)
                            self.emit_quad(
                                &mut vertices,
                                &mut indices,
                                match_col as u16,
                                match_col as u16 + 1,
                                visible_row as u16,
                                color,
                            );
                        }
                    }
                }
            }
        }

        // Render URL underlines when Ctrl is held
        if overlay.ctrl_held && !overlay.urls.is_empty() {
            let underline_color = Rgba::new(0.4, 0.6, 1.0, 0.8); // Light blue
            let underline_height = 2.0;

            for url in overlay.urls {
                if url.row < dims.rows {
                    let x0 = url.start_col as f32 * self.cell_width;
                    let x1 = url.end_col as f32 * self.cell_width;
                    let y = (url.row as f32 + 1.0) * self.cell_height - underline_height + self.tab_bar_height;

                    let base_idx = vertices.len() as u32;
                    let color_arr = underline_color.to_array();

                    vertices.push(Vertex {
                        position: [x0, y],
                        tex_coord: [0.0, 0.0],
                        color: color_arr,
                    });
                    vertices.push(Vertex {
                        position: [x1, y],
                        tex_coord: [1.0, 0.0],
                        color: color_arr,
                    });
                    vertices.push(Vertex {
                        position: [x1, y + underline_height],
                        tex_coord: [1.0, 1.0],
                        color: color_arr,
                    });
                    vertices.push(Vertex {
                        position: [x0, y + underline_height],
                        tex_coord: [0.0, 1.0],
                        color: color_arr,
                    });

                    indices.push(base_idx);
                    indices.push(base_idx + 1);
                    indices.push(base_idx + 2);
                    indices.push(base_idx);
                    indices.push(base_idx + 2);
                    indices.push(base_idx + 3);
                }
            }
        }

        // Render scrollbar if there's scrollback
        if let Some(scrollbar) = &overlay.scrollbar {
            if scrollbar.is_visible() {
                self.emit_scrollbar(&mut vertices, &mut indices, scrollbar);
            }
        }

        // Render visual bell (border flash)
        if overlay.bell_active {
            self.emit_bell_border(&mut vertices, &mut indices);
        }

        (vertices, indices)
    }

    /// Emit visual bell border vertices (flashing border).
    fn emit_bell_border(&self, vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
        let border_width = 4.0;
        let bell_color = Rgba::new(1.0, 0.8, 0.2, 0.8); // Bright yellow/orange

        let width = self.config.width as f32;
        let height = self.config.height as f32;

        // Top border
        self.emit_rect(vertices, indices, 0.0, 0.0, width, border_width, bell_color);
        // Bottom border
        self.emit_rect(vertices, indices, 0.0, height - border_width, width, border_width, bell_color);
        // Left border
        self.emit_rect(vertices, indices, 0.0, 0.0, border_width, height, bell_color);
        // Right border
        self.emit_rect(vertices, indices, width - border_width, 0.0, border_width, height, bell_color);
    }

    /// Emit scrollbar vertices.
    fn emit_scrollbar(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        scrollbar: &ScrollbarState,
    ) {
        let scrollbar_width = 8.0;
        let scrollbar_margin = 2.0;
        let terminal_height = self.config.height as f32 - self.tab_bar_height;

        // Scrollbar track position (right side of terminal)
        let track_x = self.config.width as f32 - scrollbar_width - scrollbar_margin;
        let track_y = self.tab_bar_height + scrollbar_margin;
        let track_height = terminal_height - scrollbar_margin * 2.0;

        // Draw track (darker background)
        let track_color = Rgba::new(0.15, 0.15, 0.15, 0.6);
        self.emit_rect(
            vertices,
            indices,
            track_x,
            track_y,
            scrollbar_width,
            track_height,
            track_color,
        );

        // Calculate thumb position and size
        let (thumb_pos, thumb_size) = scrollbar.thumb_geometry();
        let thumb_y = track_y + thumb_pos * track_height;
        let thumb_height = (thumb_size * track_height).max(20.0); // Minimum thumb size

        // Draw thumb (brighter)
        let thumb_color = if scrollbar.scroll_offset > 0 {
            Rgba::new(0.5, 0.5, 0.5, 0.8) // Active scroll position
        } else {
            Rgba::new(0.35, 0.35, 0.35, 0.6) // At bottom
        };

        self.emit_rect(
            vertices,
            indices,
            track_x + 1.0,
            thumb_y,
            scrollbar_width - 2.0,
            thumb_height,
            thumb_color,
        );
    }

    /// Emit quads for vim selection.
    #[allow(clippy::too_many_arguments)]
    fn emit_vim_selection(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        selection: &VimSelection,
        scroll_offset: usize,
        viewport_rows: usize,
        viewport_cols: usize,
        color: Rgba,
    ) {
        match selection {
            VimSelection::Char { start_row, start_col, end_row, end_col } => {
                for row in *start_row..=*end_row {
                    let visible_row = row.saturating_sub(scroll_offset);
                    if visible_row >= viewport_rows {
                        continue;
                    }

                    let sc = if row == *start_row { *start_col } else { 0 };
                    let ec = if row == *end_row { *end_col } else { viewport_cols - 1 };

                    self.emit_quad(
                        vertices,
                        indices,
                        sc as u16,
                        (ec + 1) as u16,
                        visible_row as u16,
                        color,
                    );
                }
            }
            VimSelection::Line { start_row, end_row } => {
                for row in *start_row..=*end_row {
                    let visible_row = row.saturating_sub(scroll_offset);
                    if visible_row >= viewport_rows {
                        continue;
                    }

                    self.emit_quad(
                        vertices,
                        indices,
                        0,
                        viewport_cols as u16,
                        visible_row as u16,
                        color,
                    );
                }
            }
            VimSelection::Block { start_row, start_col, end_row, end_col } => {
                for row in *start_row..=*end_row {
                    let visible_row = row.saturating_sub(scroll_offset);
                    if visible_row >= viewport_rows {
                        continue;
                    }

                    self.emit_quad(
                        vertices,
                        indices,
                        *start_col as u16,
                        (*end_col + 1) as u16,
                        visible_row as u16,
                        color,
                    );
                }
            }
        }
    }

    /// Build tab bar vertex data.
    fn build_tab_bar_vertices(&self, tab_bar: &TabBarState) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        if !tab_bar.visible {
            return (vertices, indices);
        }

        let height = tab_bar.height();

        // Tab bar background - slightly lighter than terminal background
        let bar_bg = Rgba::new(
            (self.color_scheme.background.r + 0.05).min(1.0),
            (self.color_scheme.background.g + 0.05).min(1.0),
            (self.color_scheme.background.b + 0.05).min(1.0),
            1.0,
        );

        // Draw full tab bar background
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            0.0,
            self.config.width as f32,
            height,
            bar_bg,
        );

        // Draw individual tabs
        for tab in &tab_bar.tabs {
            let tab_bg = if tab.is_active {
                // Active tab - same as terminal background (appears "connected")
                self.color_scheme.background
            } else if tab.is_hovered {
                // Hovered tab - slightly brighter
                Rgba::new(
                    (self.color_scheme.background.r + 0.1).min(1.0),
                    (self.color_scheme.background.g + 0.1).min(1.0),
                    (self.color_scheme.background.b + 0.1).min(1.0),
                    1.0,
                )
            } else {
                // Inactive tab - slightly darker
                Rgba::new(
                    (self.color_scheme.background.r - 0.02).max(0.0),
                    (self.color_scheme.background.g - 0.02).max(0.0),
                    (self.color_scheme.background.b - 0.02).max(0.0),
                    1.0,
                )
            };

            // Tab background with small margin
            let margin = 2.0;
            let tab_height = height - margin * 2.0;
            self.emit_rect(
                &mut vertices,
                &mut indices,
                tab.x + margin,
                margin,
                tab.width - margin * 2.0,
                tab_height,
                tab_bg,
            );

            // Active tab indicator (bottom border)
            if tab.is_active {
                let accent_color = Rgba::new(0.3, 0.5, 0.8, 1.0); // Blue accent
                self.emit_rect(
                    &mut vertices,
                    &mut indices,
                    tab.x + margin,
                    height - 3.0,
                    tab.width - margin * 2.0,
                    3.0,
                    accent_color,
                );
            }
        }

        // Separator line at bottom of tab bar
        let separator_color = Rgba::new(0.2, 0.2, 0.2, 1.0);
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            height - 1.0,
            self.config.width as f32,
            1.0,
            separator_color,
        );

        (vertices, indices)
    }

    /// Build tab title text vertices.
    fn build_tab_title_vertices(&mut self, tab_bar: &TabBarState) -> (Vec<Vertex>, Vec<u32>) {
        let mut all_vertices = Vec::new();
        let mut all_indices = Vec::new();

        if !tab_bar.visible {
            return (all_vertices, all_indices);
        }

        let tab_padding = 12.0; // Match TAB_PADDING from tabs.rs
        let text_y = tab_bar.height() / 2.0 - self.cell_height / 2.0 + 2.0;

        for tab in &tab_bar.tabs {
            // Text color: brighter for active tab
            let text_color = if tab.is_active {
                self.color_scheme.foreground
            } else {
                Rgba::new(
                    self.color_scheme.foreground.r * 0.7,
                    self.color_scheme.foreground.g * 0.7,
                    self.color_scheme.foreground.b * 0.7,
                    1.0,
                )
            };

            // Calculate text position and max width
            let text_x = tab.x + tab_padding;
            let max_text_width = tab.width - tab_padding * 2.0 - 16.0; // Leave room for close button

            let (vertices, indices) = self.text_renderer.build_label_vertices(
                &tab.title,
                text_x,
                text_y,
                max_text_width,
                text_color,
            );

            // Merge vertices, adjusting indices
            let base_idx = all_vertices.len() as u32;
            all_vertices.extend(vertices);
            all_indices.extend(indices.iter().map(|i| i + base_idx));
        }

        (all_vertices, all_indices)
    }

    /// Build vim mode indicator vertices.
    /// Displays the current mode at the bottom-left of the terminal area.
    fn build_vim_mode_indicator_vertices(&mut self, copy_mode: &CopyMode) -> (Vec<Vertex>, Vec<u32>) {
        if !copy_mode.is_active() {
            return (Vec::new(), Vec::new());
        }

        let mode = copy_mode.vim_mode();
        let text = match mode {
            VimMode::Normal => "-- NORMAL --".to_string(),
            VimMode::Visual => "-- VISUAL --".to_string(),
            VimMode::VisualLine => "-- VISUAL LINE --".to_string(),
            VimMode::VisualBlock => "-- VISUAL BLOCK --".to_string(),
            VimMode::Search => {
                let query = copy_mode.search_query();
                format!("/ {}", query)
            }
            VimMode::Command => ": ".to_string(),
            VimMode::Insert => "-- INSERT --".to_string(),
        };

        // Position at bottom-left with some padding
        let padding = 8.0;
        let x = padding;
        let y = self.config.height as f32 - self.cell_height - padding;

        // Mode indicator background
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        // Draw a semi-transparent background for the indicator
        let bg_color = Rgba::new(0.1, 0.1, 0.1, 0.9);
        let text_width = text.len() as f32 * self.cell_width * 0.6; // Approximate width
        let bg_width = text_width + padding * 2.0;
        let bg_height = self.cell_height + padding;

        self.emit_rect(
            &mut vertices,
            &mut indices,
            x - padding / 2.0,
            y - padding / 4.0,
            bg_width,
            bg_height,
            bg_color,
        );

        // Build text vertices using the text renderer
        let text_color = match mode {
            VimMode::Normal => Rgba::new(0.5, 0.8, 0.5, 1.0),  // Green
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => {
                Rgba::new(0.8, 0.6, 0.2, 1.0)  // Orange
            }
            VimMode::Search => Rgba::new(0.5, 0.7, 1.0, 1.0),  // Blue
            _ => self.color_scheme.foreground,
        };

        let (text_vertices, text_indices) = self.text_renderer.build_label_vertices(
            &text,
            x,
            y,
            self.config.width as f32 - padding * 2.0,
            text_color,
        );

        // Merge text vertices
        let base_idx = vertices.len() as u32;
        vertices.extend(text_vertices);
        indices.extend(text_indices.iter().map(|i| i + base_idx));

        (vertices, indices)
    }

    /// Build keyboard shortcuts overlay vertices.
    /// Displays a centered modal with all available keybindings.
    fn build_shortcuts_overlay_vertices(
        &mut self,
        keybindings: &[(String, String)],
    ) -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let padding = 20.0;
        let line_height = self.cell_height * 1.5;
        let title = "Keyboard Shortcuts";

        // Calculate dimensions
        let max_key_width = keybindings
            .iter()
            .map(|(k, _)| k.len())
            .max()
            .unwrap_or(10) as f32
            * self.cell_width
            * 0.6;
        let max_desc_width = keybindings
            .iter()
            .map(|(_, d)| d.len())
            .max()
            .unwrap_or(20) as f32
            * self.cell_width
            * 0.6;
        let content_width = max_key_width + max_desc_width + padding * 3.0;
        let content_height = (keybindings.len() as f32 + 2.0) * line_height + padding * 2.0;

        // Center the modal
        let modal_width = content_width.max(300.0);
        let modal_height = content_height.min(self.config.height as f32 - 100.0);
        let modal_x = (self.config.width as f32 - modal_width) / 2.0;
        let modal_y = (self.config.height as f32 - modal_height) / 2.0;

        // Draw semi-transparent backdrop
        let backdrop_color = Rgba::new(0.0, 0.0, 0.0, 0.7);
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            0.0,
            self.config.width as f32,
            self.config.height as f32,
            backdrop_color,
        );

        // Draw modal background
        let modal_bg = Rgba::new(0.15, 0.15, 0.15, 0.95);
        self.emit_rect(
            &mut vertices,
            &mut indices,
            modal_x,
            modal_y,
            modal_width,
            modal_height,
            modal_bg,
        );

        // Draw modal border
        let border_color = Rgba::new(0.4, 0.4, 0.4, 1.0);
        let border_width = 2.0;
        // Top border
        self.emit_rect(
            &mut vertices,
            &mut indices,
            modal_x,
            modal_y,
            modal_width,
            border_width,
            border_color,
        );
        // Bottom border
        self.emit_rect(
            &mut vertices,
            &mut indices,
            modal_x,
            modal_y + modal_height - border_width,
            modal_width,
            border_width,
            border_color,
        );
        // Left border
        self.emit_rect(
            &mut vertices,
            &mut indices,
            modal_x,
            modal_y,
            border_width,
            modal_height,
            border_color,
        );
        // Right border
        self.emit_rect(
            &mut vertices,
            &mut indices,
            modal_x + modal_width - border_width,
            modal_y,
            border_width,
            modal_height,
            border_color,
        );

        // Draw title
        let title_color = Rgba::new(0.9, 0.9, 0.9, 1.0);
        let (title_vertices, title_indices) = self.text_renderer.build_label_vertices(
            title,
            modal_x + padding,
            modal_y + padding,
            modal_width - padding * 2.0,
            title_color,
        );
        let base_idx = vertices.len() as u32;
        vertices.extend(title_vertices);
        indices.extend(title_indices.iter().map(|i| i + base_idx));

        // Draw separator line under title
        let sep_y = modal_y + padding + line_height;
        self.emit_rect(
            &mut vertices,
            &mut indices,
            modal_x + padding,
            sep_y,
            modal_width - padding * 2.0,
            1.0,
            Rgba::new(0.3, 0.3, 0.3, 1.0),
        );

        // Draw keybindings
        let key_color = Rgba::new(0.6, 0.8, 1.0, 1.0); // Light blue for keys
        let desc_color = Rgba::new(0.8, 0.8, 0.8, 1.0); // Light gray for descriptions

        let start_y = sep_y + line_height * 0.5;
        for (i, (key, desc)) in keybindings.iter().enumerate() {
            let y = start_y + (i as f32 + 1.0) * line_height;
            if y + line_height > modal_y + modal_height - padding {
                break; // Don't overflow the modal
            }

            // Draw key
            let (key_vertices, key_indices) = self.text_renderer.build_label_vertices(
                key,
                modal_x + padding,
                y,
                max_key_width + padding,
                key_color,
            );
            let base_idx = vertices.len() as u32;
            vertices.extend(key_vertices);
            indices.extend(key_indices.iter().map(|i| i + base_idx));

            // Draw description
            let (desc_vertices, desc_indices) = self.text_renderer.build_label_vertices(
                desc,
                modal_x + padding + max_key_width + padding,
                y,
                max_desc_width + padding,
                desc_color,
            );
            let base_idx = vertices.len() as u32;
            vertices.extend(desc_vertices);
            indices.extend(desc_indices.iter().map(|i| i + base_idx));
        }

        // Draw footer hint
        let footer = "Press Esc or F1 to close";
        let footer_color = Rgba::new(0.5, 0.5, 0.5, 1.0);
        let (footer_vertices, footer_indices) = self.text_renderer.build_label_vertices(
            footer,
            modal_x + padding,
            modal_y + modal_height - padding - line_height * 0.5,
            modal_width - padding * 2.0,
            footer_color,
        );
        let base_idx = vertices.len() as u32;
        vertices.extend(footer_vertices);
        indices.extend(footer_indices.iter().map(|i| i + base_idx));

        (vertices, indices)
    }

    /// Emit a rectangle (helper for tab bar).
    #[allow(clippy::too_many_arguments)]
    fn emit_rect(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Rgba,
    ) {
        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x, y],
            tex_coord: [0.0, 0.0],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x + width, y],
            tex_coord: [1.0, 0.0],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x + width, y + height],
            tex_coord: [1.0, 1.0],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x, y + height],
            tex_coord: [0.0, 1.0],
            color: color_arr,
        });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Build background vertex data from terminal state.
    fn build_background_vertices(&self, terminal: &Terminal, overlay: &OverlayState) -> (Vec<Vertex>, Vec<u32>) {
        let grid = terminal.grid();
        let dims = grid.dimensions();
        let est_quads = dims.rows as usize * 4;
        let mut vertices = Vec::with_capacity(est_quads * 4);
        let mut indices = Vec::with_capacity(est_quads * 6);

        for row_idx in 0..dims.rows {
            let row = match grid.row(row_idx) {
                Some(r) => r,
                None => continue,
            };

            // Track runs of same background color
            let mut run_start = 0u16;
            let mut run_color: Option<Rgba> = None;

            for col_idx in 0..dims.cols {
                let cell = match row.get(col_idx) {
                    Some(c) => c,
                    None => continue,
                };

                let is_selected = terminal.is_selected(col_idx, row_idx);
                let bg_color = if is_selected {
                    self.color_scheme.selection
                } else if cell.has_flag(CellFlags::REVERSE) {
                    self.color_scheme.resolve(cell.fg(), true)
                } else {
                    self.color_scheme.resolve(cell.bg(), false)
                };

                // Check if we should extend the run or start a new one
                let should_flush = match run_color {
                    None => false,
                    Some(rc) => {
                        // Different color or at end
                        rc != bg_color
                    }
                };

                if should_flush {
                    if let Some(color) = run_color {
                        // Only emit non-default backgrounds
                        if color != self.color_scheme.background {
                            self.emit_quad(
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

            // Flush final run
            if let Some(color) = run_color {
                if color != self.color_scheme.background {
                    self.emit_quad(
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
            let row = match grid.row(row_idx) {
                Some(r) => r,
                None => continue,
            };
            for col_idx in 0..dims.cols {
                let cell = match row.get(col_idx) {
                    Some(c) => c,
                    None => continue,
                };
                let ch = cell.char();
                if let Some((rects, count)) = block_char_rects(ch) {
                    let fg = if cell.has_flag(CellFlags::REVERSE) {
                        self.color_scheme.resolve(cell.bg(), false)
                    } else {
                        self.color_scheme.resolve(cell.fg(), true)
                    };
                    let fg = if cell.has_flag(CellFlags::DIM) {
                        Rgba::new(fg.r * 0.5, fg.g * 0.5, fg.b * 0.5, fg.a)
                    } else {
                        fg
                    };
                    let c = fg.to_array();
                    let cx = col_idx as f32 * self.cell_width;
                    let cy = row_idx as f32 * self.cell_height + self.tab_bar_height;
                    for &(rx, ry, rw, rh) in &rects[..count] {
                        let x0 = cx + rx * self.cell_width;
                        let y0 = cy + ry * self.cell_height;
                        let w = rw * self.cell_width;
                        let h = rh * self.cell_height;
                        let base = vertices.len() as u32;
                        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: c });
                        vertices.push(Vertex { position: [x0 + w, y0], tex_coord: [0.0, 0.0], color: c });
                        vertices.push(Vertex { position: [x0 + w, y0 + h], tex_coord: [0.0, 0.0], color: c });
                        vertices.push(Vertex { position: [x0, y0 + h], tex_coord: [0.0, 0.0], color: c });
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

        // Draw cursor
        let cursor = terminal.buffer().grid().cursor();
        // Only draw if visible and (not blinking or blink phase is visible)
        let should_show_cursor = cursor.visible
            && (!cursor.blinking || overlay.cursor_blink_visible);

        if should_show_cursor {
            let cursor_color = self.color_scheme.cursor;
            let col = cursor.position.col;
            let row = cursor.position.row;

            match cursor.shape {
                CursorShape::Block => {
                    // Full cell block cursor
                    self.emit_quad(
                        &mut vertices,
                        &mut indices,
                        col,
                        col + 1,
                        row,
                        cursor_color,
                    );
                }
                CursorShape::Underline => {
                    // Thin underline cursor (bottom 2 pixels of cell)
                    self.emit_cursor_underline(
                        &mut vertices,
                        &mut indices,
                        col,
                        row,
                        cursor_color,
                    );
                }
                CursorShape::Bar => {
                    // Thin bar cursor (left 2 pixels of cell)
                    self.emit_cursor_bar(
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

    /// Emit a thin underline cursor at the bottom of a cell.
    fn emit_cursor_underline(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = col as f32 * self.cell_width;
        let x1 = (col + 1) as f32 * self.cell_width;
        let underline_height = 2.0f32.min(self.cell_height * 0.15);
        let y0 = (row + 1) as f32 * self.cell_height + self.tab_bar_height - underline_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.tab_bar_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a thin bar cursor at the left of a cell.
    fn emit_cursor_bar(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        col: u16,
        row: u16,
        color: Rgba,
    ) {
        let bar_width = 2.0f32.min(self.cell_width * 0.15);
        let x0 = col as f32 * self.cell_width;
        let x1 = x0 + bar_width;
        let y0 = row as f32 * self.cell_height + self.tab_bar_height;
        let y1 = (row + 1) as f32 * self.cell_height + self.tab_bar_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex { position: [x0, y0], tex_coord: [0.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y0], tex_coord: [1.0, 0.0], color: color_arr });
        vertices.push(Vertex { position: [x1, y1], tex_coord: [1.0, 1.0], color: color_arr });
        vertices.push(Vertex { position: [x0, y1], tex_coord: [0.0, 1.0], color: color_arr });

        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }

    /// Emit a quad for a cell range (offset by tab bar height).
    fn emit_quad(
        &self,
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        start_col: u16,
        end_col: u16,
        row: u16,
        color: Rgba,
    ) {
        let x0 = start_col as f32 * self.cell_width;
        let x1 = end_col as f32 * self.cell_width;
        let y0 = row as f32 * self.cell_height + self.tab_bar_height;
        let y1 = y0 + self.cell_height;

        let base_idx = vertices.len() as u32;
        let color_arr = color.to_array();

        vertices.push(Vertex {
            position: [x0, y0],
            tex_coord: [0.0, 0.0],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x1, y0],
            tex_coord: [1.0, 0.0],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x1, y1],
            tex_coord: [1.0, 1.0],
            color: color_arr,
        });
        vertices.push(Vertex {
            position: [x0, y1],
            tex_coord: [0.0, 1.0],
            color: color_arr,
        });

        // Two triangles per quad
        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
    }


    /// Check if there are active animations that require continuous redraw.
    pub fn has_active_animations(&self) -> bool {
        self.has_animations
    }

    /// Reset animation flags for this frame. Call at the start of each render.
    pub fn reset_animation_flags(&mut self) {
        self.has_animations = false;
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_scheme_resolves_colors() {
        let scheme = ColorScheme::dark();

        // Default colors
        assert_eq!(scheme.resolve(Color::Default, true), scheme.foreground);
        assert_eq!(scheme.resolve(Color::Default, false), scheme.background);

        // Named colors
        let red = scheme.resolve(Color::RED, true);
        assert!(red.r > 0.5);
        assert!(red.g < 0.1);
        assert!(red.b < 0.1);

        // RGB
        let custom = scheme.resolve(Color::Rgb(100, 150, 200), true);
        assert!((custom.r - 100.0 / 255.0).abs() < 0.01);
        assert!((custom.g - 150.0 / 255.0).abs() < 0.01);
        assert!((custom.b - 200.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn indexed_colors() {
        let scheme = ColorScheme::dark();

        // Standard colors
        assert_eq!(scheme.indexed_color(0), scheme.named[0]);
        assert_eq!(scheme.indexed_color(15), scheme.named[15]);

        // 216-color cube
        let color = scheme.indexed_color(196); // Should be bright red
        assert!(color.r > 0.8);

        // Grayscale
        let gray = scheme.indexed_color(244);
        assert!((gray.r - gray.g).abs() < 0.01);
        assert!((gray.g - gray.b).abs() < 0.01);
    }

    #[test]
    fn rgba_conversions() {
        let color = Rgba::from_u8(128, 64, 255);
        assert!((color.r - 0.502).abs() < 0.01);
        assert!((color.g - 0.251).abs() < 0.01);
        assert!((color.b - 1.0).abs() < 0.01);

        let arr = color.to_array();
        assert_eq!(arr.len(), 4);
    }

    #[test]
    fn srgb_to_linear_conversion() {
        // Test the sRGB to linear conversion
        // Black stays black
        let black = Rgba::new(0.0, 0.0, 0.0, 1.0);
        let linear_black = black.to_linear();
        assert!((linear_black.r).abs() < 0.0001);
        assert!((linear_black.g).abs() < 0.0001);
        assert!((linear_black.b).abs() < 0.0001);

        // White stays white
        let white = Rgba::new(1.0, 1.0, 1.0, 1.0);
        let linear_white = white.to_linear();
        assert!((linear_white.r - 1.0).abs() < 0.0001);
        assert!((linear_white.g - 1.0).abs() < 0.0001);
        assert!((linear_white.b - 1.0).abs() < 0.0001);

        // Mid gray in sRGB (~0.5) should be darker in linear (~0.214)
        let mid_gray = Rgba::new(0.5, 0.5, 0.5, 1.0);
        let linear_mid = mid_gray.to_linear();
        assert!((linear_mid.r - 0.214).abs() < 0.01);

        // Gruvbox dark background #1D2021 (29, 32, 33) -> ~(0.114, 0.125, 0.129) sRGB
        // Should convert to much darker linear values
        let gruvbox_bg = Rgba::from_u8(0x1D, 0x20, 0x21);
        let linear_gruvbox = gruvbox_bg.to_linear();
        // Linear values should be much smaller (< 0.02)
        assert!(linear_gruvbox.r < 0.02, "Expected r < 0.02, got {}", linear_gruvbox.r);
        assert!(linear_gruvbox.g < 0.02, "Expected g < 0.02, got {}", linear_gruvbox.g);
        assert!(linear_gruvbox.b < 0.02, "Expected b < 0.02, got {}", linear_gruvbox.b);

        // Alpha should not be affected
        let with_alpha = Rgba::new(0.5, 0.5, 0.5, 0.75);
        let linear_alpha = with_alpha.to_linear();
        assert!((linear_alpha.a - 0.75).abs() < 0.0001);
    }

    #[test]
    fn orthographic_projection() {
        let uniforms = Uniforms::orthographic(800.0, 600.0);
        // Top-left should map to (-1, 1)
        // Bottom-right should map to (1, -1)
        assert!((uniforms.projection[0][0] - 2.0 / 800.0).abs() < 0.0001);
        assert!((uniforms.projection[1][1] - (-2.0 / 600.0)).abs() < 0.0001);
    }

    #[test]
    fn scrollbar_visibility() {
        // No scrollback - not visible
        let scrollbar = ScrollbarState::new(24, 24, 0);
        assert!(!scrollbar.is_visible());

        // Some scrollback - visible
        let scrollbar = ScrollbarState::new(100, 24, 0);
        assert!(scrollbar.is_visible());
    }

    #[test]
    fn scrollbar_thumb_at_bottom() {
        // At bottom (offset 0), thumb should be at bottom
        let scrollbar = ScrollbarState::new(100, 24, 0);
        let (pos, size) = scrollbar.thumb_geometry();

        // Size should be viewport/total
        assert!((size - 24.0 / 100.0).abs() < 0.01);

        // Position should be near bottom (1.0 - size)
        assert!((pos - (1.0 - size)).abs() < 0.01);
    }

    #[test]
    fn scrollbar_thumb_at_top() {
        // At top (max offset), thumb should be at top
        let scrollbar = ScrollbarState::new(100, 24, 76); // 100 - 24 = 76 max offset
        let (pos, _size) = scrollbar.thumb_geometry();

        // Position should be near top (0)
        assert!(pos < 0.01);
    }

    #[test]
    fn scrollbar_thumb_middle() {
        // At middle offset, thumb should be in middle
        let scrollbar = ScrollbarState::new(100, 24, 38); // Half of max offset
        let (pos, size) = scrollbar.thumb_geometry();

        // Position should be roughly in middle
        let expected_middle = (1.0 - size) / 2.0;
        assert!((pos - expected_middle).abs() < 0.1);
    }
}

