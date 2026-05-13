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
mod cursor;
mod image;
mod native_render;
mod overlays;
mod panes;
mod pipeline;
mod screenshot;
mod scrollbar;
mod tabbar;
mod text;

pub use atlas::{AtlasStats, GlyphAtlas};
pub use color::{ColorScheme, Rgba};
pub use pipeline::RenderPipeline;
pub use scrollbar::ScrollbarState;
pub use text::TextRenderer;

use crate::config::{ColorConfig, FontConfig};
#[cfg(test)]
use crate::grid::Color;
use crate::grid::{CellFlags, CursorShape, ImageId, Terminal};
use crate::tabs::TabBarState;
use crate::url::DetectedUrl;
use crate::vim::CopyMode;
use block_chars::block_char_rects;
use std::sync::Arc;
use wgpu::util::DeviceExt;

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
        Self {
            x,
            y,
            width,
            height,
            is_focused,
        }
    }

    /// Create pane info for a single full-screen terminal.
    pub fn full_screen(width: f32, height: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width,
            height,
            is_focused: true,
        }
    }
}

/// Vertex for quad rendering.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 2],
    pub tex_coord: [f32; 2],
    pub color: [f32; 4],
    pub is_color: u32,
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x4,
        3 => Uint32,
    ];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Uniform buffer for projection matrix and window parameters.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub projection: [[f32; 4]; 4],
    /// Window size (width, height) and corner radius, padding.
    pub window_params: [f32; 4],
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
            window_params: [width, height, 10.0, 0.0], // width, height, corner_radius, padding
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
    /// Cached native session background GPU buffers (vertex, index, index_count)
    cached_native_bg: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    /// Cached native session text GPU buffers (vertex, index, index_count)
    cached_native_text: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    /// Previous frame vertex/index counts for smarter pre-allocation
    prev_bg_count: (usize, usize),
    prev_text_count: (usize, usize),
    /// Reused per-frame cache for native scroll adjustments.
    scroll_adjustments_cache: std::collections::HashMap<ultty_protocol::RegionId, (f32, f32)>,
    /// Reused per-frame cache for native clip rectangles.
    clip_rects_cache: std::collections::HashMap<ultty_protocol::RegionId, (f32, f32, f32, f32)>,
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
        let alpha_mode = if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        let bg_pipeline = RenderPipeline::new(&device, surface_format, &uniform_bind_group_layout)?;

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
            cached_native_bg: None,
            cached_native_text: None,
            prev_bg_count: (0, 0),
            prev_text_count: (0, 0),
            scroll_adjustments_cache: std::collections::HashMap::new(),
            clip_rects_cache: std::collections::HashMap::new(),
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
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to find suitable GPU adapter for headless rendering")
            })?;

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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        let bg_pipeline = RenderPipeline::new(&device, surface_format, &uniform_bind_group_layout)?;

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
            cached_native_bg: None,
            cached_native_text: None,
            prev_bg_count: (0, 0),
            prev_text_count: (0, 0),
            scroll_adjustments_cache: std::collections::HashMap::new(),
            clip_rects_cache: std::collections::HashMap::new(),
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
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench_bg_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
            let overlay_vertex_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("overlay_vertex_buffer"),
                        contents: bytemuck::cast_slice(&overlay_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });

            let overlay_index_buffer =
                self.device
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
    pub fn retain_native_images(
        &mut self,
        live_regions: &std::collections::HashSet<ultty_protocol::RegionId>,
    ) {
        self.native_image_cache
            .retain(|id, _| live_regions.contains(id));
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
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Pre-multiplied alpha for transparency support
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
            let tab_bar_vertex_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("tab_bar_vertex_buffer"),
                        contents: bytemuck::cast_slice(&tab_bar_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });

            let tab_bar_index_buffer =
                self.device
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
                    let title_vertex_buffer =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("tab_title_vertex_buffer"),
                                contents: bytemuck::cast_slice(&title_vertices),
                                usage: wgpu::BufferUsages::VERTEX,
                            });

                    let title_index_buffer =
                        self.device
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
                    render_pass
                        .set_index_buffer(title_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
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
            let overlay_vertex_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("overlay_vertex_buffer"),
                        contents: bytemuck::cast_slice(&overlay_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });

            let overlay_index_buffer =
                self.device
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
                render_pass
                    .set_index_buffer(overlay_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..overlay_indices.len() as u32, 0, 0..1);
            }
        }

        // Render vim mode indicator if in copy mode
        if let Some(copy_mode) = overlay.copy_mode {
            if copy_mode.is_active() {
                let (indicator_vertices, indicator_indices) =
                    self.build_vim_mode_indicator_vertices(copy_mode);
                if !indicator_vertices.is_empty() {
                    // Render background quads
                    let bg_vertices: Vec<_> = indicator_vertices
                        .iter()
                        .take(4) // First 4 vertices are the background quad
                        .cloned()
                        .collect();
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
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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

        let sessions: Vec<_> = sessions.collect();
        let has_native_sessions = !sessions.is_empty();

        if !has_native_sessions {
            self.render_images(&mut encoder, &view, terminal)?;
        }

        if !tab_bar_vertices.is_empty() {
            let tab_bar_vertex_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("tab_bar_vertex_buffer"),
                        contents: bytemuck::cast_slice(&tab_bar_vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
            let tab_bar_index_buffer =
                self.device
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

        if !has_native_sessions {
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
        }

        let any_dirty = sessions.iter().any(|s| s.is_render_dirty());

        let has_cached_native =
            self.cached_native_bg.is_some() || self.cached_native_text.is_some();

        if !any_dirty && has_cached_native {
            self.draw_cached_native_buffers(&mut encoder, &view);
        } else {
            for session in &sessions {
                self.render_native_session(
                    &mut encoder,
                    &view,
                    session,
                    native_search_matches,
                    native_search_index,
                );
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
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
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x + width, y],
            tex_coord: [1.0, 0.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x + width, y + height],
            tex_coord: [1.0, 1.0],
            color: color_arr,
            is_color: 0,
        });
        vertices.push(Vertex {
            position: [x, y + height],
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

    /// Build background vertex data from terminal state.
    fn build_background_vertices(
        &self,
        terminal: &Terminal,
        overlay: &OverlayState,
    ) -> (Vec<Vertex>, Vec<u32>) {
        let buffer = terminal.buffer();
        let dims = terminal.dimensions();
        let est_quads = dims.rows as usize * 4 + 10;
        let mut vertices = Vec::with_capacity(est_quads * 4);
        let mut indices = Vec::with_capacity(est_quads * 6);

        // Window border (1px, slightly lighter than bg)
        let bg = self.color_scheme.background;
        let w = self.config.width as f32;
        let h = self.config.height as f32;
        let border_color = Rgba::new(
            (bg.r + 0.12).min(1.0),
            (bg.g + 0.12).min(1.0),
            (bg.b + 0.12).min(1.0),
            1.0,
        );
        // Border: top and sides only (1px). Bottom uses separate rect to avoid corner gaps.
        self.emit_rect(&mut vertices, &mut indices, 0.0, 0.0, w, 1.0, border_color); // top
        self.emit_rect(&mut vertices, &mut indices, 0.0, 0.0, 1.0, h, border_color); // left
        self.emit_rect(
            &mut vertices,
            &mut indices,
            w - 1.0,
            0.0,
            1.0,
            h,
            border_color,
        ); // right
        self.emit_rect(
            &mut vertices,
            &mut indices,
            0.0,
            h - 1.0,
            w,
            1.0,
            border_color,
        );
        // Inner background
        self.emit_rect(&mut vertices, &mut indices, 1.0, 1.0, w - 2.0, h - 2.0, bg);

        for row_idx in 0..dims.rows {
            let row = match buffer.visible_row(row_idx) {
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
            let row = match buffer.visible_row(row_idx) {
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
                        Rgba::new(fg.r * 0.65, fg.g * 0.65, fg.b * 0.65, fg.a)
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

        // Draw cursor
        let cursor = terminal.buffer().grid().cursor();
        // Only draw if visible and (not blinking or blink phase is visible)
        let should_show_cursor =
            cursor.visible && (!cursor.blinking || overlay.cursor_blink_visible);

        if should_show_cursor {
            let cursor_color = self.color_scheme.cursor;
            let col = cursor.position.col;
            let row = cursor.position.row;

            match cursor.shape {
                CursorShape::Block => {
                    // Full cell block cursor
                    self.emit_quad(&mut vertices, &mut indices, col, col + 1, row, cursor_color);
                }
                CursorShape::Underline => {
                    // Thin underline cursor (bottom 2 pixels of cell)
                    self.emit_cursor_underline(&mut vertices, &mut indices, col, row, cursor_color);
                }
                CursorShape::Bar => {
                    // Thin bar cursor (left 2 pixels of cell)
                    self.emit_cursor_bar(&mut vertices, &mut indices, col, row, cursor_color);
                }
            }
        }

        (vertices, indices)
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
        assert!(
            linear_gruvbox.r < 0.02,
            "Expected r < 0.02, got {}",
            linear_gruvbox.r
        );
        assert!(
            linear_gruvbox.g < 0.02,
            "Expected g < 0.02, got {}",
            linear_gruvbox.g
        );
        assert!(
            linear_gruvbox.b < 0.02,
            "Expected b < 0.02, got {}",
            linear_gruvbox.b
        );

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
