//! The renderer, and the two-phase startup that brings it up.
//!
//! Bringing up the GPU (instance, adapter, device, surface, pipelines) takes
//! most of startup: about 240 ms on an NVIDIA T4. None of it is needed to
//! size the grid, which only takes the cell size, and that is measured on
//! the CPU ([`Fonts`]). So the window opens, the GPU setup starts on a
//! worker thread ([`spawn_setup`]), and meanwhile the main thread measures
//! the cell, spawns the shells and runs the event loop: the shell starts,
//! and the titles it sets reach the window, without waiting for the GPU.
//! The worker hands the finished renderer back through the event loop
//! ([`super::AppEvent::Renderer`]), and the first frame follows.
//!
//! Until then [`RendererState::Starting`] answers the layout questions the
//! app asks of the renderer from the CPU metrics, remembers what would have
//! changed the renderer (window size, tab bar height, opacity, colors,
//! fonts), and has nothing to draw with. [`App::install_renderer`] applies
//! all of it to the renderer when it arrives.

use std::sync::mpsc;
use std::sync::Arc;

use anyhow::Result;
use ultty::config::{clamp_font_size, ColorConfig, FontConfig};
use ultty::renderer::{grid_dimensions_for, stepped_font_size, ColorScheme, Fonts, Renderer};
use winit::dpi::PhysicalSize;
use winit::event_loop::{EventLoopProxy, OwnedDisplayHandle};
use winit::window::Window;

use super::{App, AppEvent};

/// A renderer and the window surface it draws to.
pub(super) struct Gpu {
    pub(super) surface: wgpu::Surface<'static>,
    pub(super) renderer: Renderer,
}

/// The outcome of the GPU setup, sent from its worker thread.
pub(super) struct Handoff(pub(super) Result<Box<Gpu>>);

impl std::fmt::Debug for Handoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Ok(_) => f.write_str("Handoff(Ok(..))"),
            Err(error) => write!(f, "Handoff(Err({error:#}))"),
        }
    }
}

/// What the app draws with: nothing yet while the GPU starts, then the
/// renderer.
pub(super) enum RendererState {
    Starting(Starting),
    /// Boxed: the renderer is large, and lives as long as the app.
    Ready(Box<Gpu>),
}

/// The renderer-to-be while the GPU starts: the layout the app needs, from
/// the CPU cell metrics, and the changes to apply once it arrives.
#[derive(Clone)]
pub(super) struct Starting {
    /// Cell size the renderer is being built with.
    cell: (f32, f32),
    /// The font size the renderer should have, in physical pixels.
    font_size: f32,
    /// Fonts applied (a config reload) while starting.
    font_config: Option<Box<FontConfig>>,
    /// The latest window size.
    size: (u32, u32),
    tab_bar_height: f32,
    opacity: f32,
    /// Colors changed while starting.
    color_scheme: Option<Box<ColorScheme>>,
}

impl Starting {
    /// Before the GPU is up: drawing with `fonts` in a window of `size`.
    pub(super) fn new(fonts: &Fonts, size: PhysicalSize<u32>, opacity: f32) -> Self {
        Self {
            cell: fonts.cell_dimensions(),
            font_size: fonts.font_size(),
            font_config: None,
            size: (size.width, size.height),
            tab_bar_height: 0.0,
            opacity: opacity.clamp(0.0, 1.0),
            color_scheme: None,
        }
    }

    /// Bring `renderer`, built from the startup fonts and colors, up to the
    /// changes made while it was being built. The window size is not among
    /// them: the caller resizes the surface.
    pub(super) fn apply_to(self, renderer: &mut Renderer) {
        renderer.set_tab_bar_height(self.tab_bar_height);
        renderer.set_opacity(self.opacity);
        if let Some(scheme) = self.color_scheme {
            renderer.set_color_scheme(*scheme);
        }
        if let Some(font) = &self.font_config {
            renderer.apply_font_config(font);
        }
        if renderer.font_size() != self.font_size {
            renderer.set_font_size(self.font_size);
        }
    }
}

impl RendererState {
    /// The renderer, once the GPU is up.
    pub(super) fn ready_mut(&mut self) -> Option<&mut Renderer> {
        match self {
            Self::Starting(_) => None,
            Self::Ready(gpu) => Some(&mut gpu.renderer),
        }
    }

    /// The renderer, once the GPU is up.
    pub(super) fn ready(&self) -> Option<&Renderer> {
        match self {
            Self::Starting(_) => None,
            Self::Ready(gpu) => Some(&gpu.renderer),
        }
    }

    /// The renderer to capture a screenshot with: there is nothing to
    /// capture before the GPU is up.
    pub(super) fn for_screenshot(&mut self) -> Result<&mut Renderer> {
        self.ready_mut()
            .ok_or_else(|| anyhow::anyhow!("the GPU is still starting; nothing is drawn yet"))
    }

    pub(super) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// See [`Renderer::cell_dimensions`].
    pub(super) fn cell_dimensions(&self) -> (f32, f32) {
        match self {
            Self::Starting(starting) => starting.cell,
            Self::Ready(gpu) => gpu.renderer.cell_dimensions(),
        }
    }

    /// See [`Renderer::terminal_dimensions`].
    pub(super) fn terminal_dimensions(&self) -> (u16, u16) {
        match self {
            Self::Starting(starting) => {
                let (width, height) = self.terminal_area();
                grid_dimensions_for(width, height, starting.cell.0, starting.cell.1)
            }
            Self::Ready(gpu) => gpu.renderer.terminal_dimensions(),
        }
    }

    /// See [`Renderer::surface_size`]. While starting, the window size: the
    /// GPU's texture limit is not known yet.
    pub(super) fn surface_size(&self) -> (u32, u32) {
        match self {
            Self::Starting(starting) => starting.size,
            Self::Ready(gpu) => gpu.renderer.surface_size(),
        }
    }

    /// See [`Renderer::terminal_area`].
    pub(super) fn terminal_area(&self) -> (f32, f32) {
        match self {
            Self::Starting(starting) => (
                starting.size.0 as f32,
                (starting.size.1 as f32 - starting.tab_bar_height).max(0.0),
            ),
            Self::Ready(gpu) => gpu.renderer.terminal_area(),
        }
    }

    pub(super) fn tab_bar_height(&self) -> f32 {
        match self {
            Self::Starting(starting) => starting.tab_bar_height,
            Self::Ready(gpu) => gpu.renderer.tab_bar_height(),
        }
    }

    pub(super) fn set_tab_bar_height(&mut self, height: f32) {
        match self {
            Self::Starting(starting) => starting.tab_bar_height = height,
            Self::Ready(gpu) => gpu.renderer.set_tab_bar_height(height),
        }
    }

    /// Resize the surface; while starting, remember the size for the
    /// renderer.
    pub(super) fn resize(&mut self, width: u32, height: u32) {
        match self {
            Self::Starting(starting) => {
                if width != 0 && height != 0 {
                    starting.size = (width, height);
                }
            }
            Self::Ready(gpu) => gpu.renderer.resize(&gpu.surface, width, height),
        }
    }

    pub(super) fn set_opacity(&mut self, opacity: f32) {
        match self {
            Self::Starting(starting) => starting.opacity = opacity.clamp(0.0, 1.0),
            Self::Ready(gpu) => gpu.renderer.set_opacity(opacity),
        }
    }

    pub(super) fn set_color_scheme(&mut self, scheme: ColorScheme) {
        match self {
            Self::Starting(starting) => starting.color_scheme = Some(Box::new(scheme)),
            Self::Ready(gpu) => gpu.renderer.set_color_scheme(scheme),
        }
    }

    /// See [`Renderer::font_size`]; while starting, the size the renderer
    /// will have.
    pub(super) fn font_size(&self) -> f32 {
        match self {
            Self::Starting(starting) => starting.font_size,
            Self::Ready(gpu) => gpu.renderer.font_size(),
        }
    }

    /// See [`Renderer::apply_font_config`]. While starting, the fonts are
    /// applied when the renderer arrives, and the cell size is only known
    /// then: this reports it unchanged, and the handoff lays the panes out
    /// again.
    pub(super) fn apply_font_config(&mut self, config: &FontConfig) -> bool {
        match self {
            Self::Starting(starting) => {
                starting.font_size = clamp_font_size(config.size);
                starting.font_config = Some(Box::new(config.clone()));
                false
            }
            Self::Ready(gpu) => gpu.renderer.apply_font_config(config),
        }
    }

    /// See [`Renderer::set_font_size`]; while starting, as for
    /// [`Self::apply_font_config`].
    pub(super) fn set_font_size(&mut self, size: f32) -> f32 {
        match self {
            Self::Starting(starting) => {
                starting.font_size = clamp_font_size(size);
                starting.font_size
            }
            Self::Ready(gpu) => gpu.renderer.set_font_size(size),
        }
    }

    /// See [`Renderer::step_font_size`].
    pub(super) fn step_font_size(&mut self, increase: bool) -> f32 {
        self.set_font_size(stepped_font_size(self.font_size(), increase))
    }
}

impl App {
    /// Take over the renderer from the GPU setup: bring it up to what
    /// changed meanwhile, size it to the window and draw the first frame.
    pub(super) fn install_renderer(&mut self, mut gpu: Box<Gpu>) {
        let RendererState::Starting(starting) = &self.renderer else {
            tracing::warn!("A second renderer arrived; keeping the first");
            return;
        };
        starting.clone().apply_to(&mut gpu.renderer);
        self.renderer = RendererState::Ready(gpu);
        tracing::debug!(target: "ultty::startup", "renderer handed over");
        // The window may have been resized, and the fonts changed, while
        // the GPU started: size the surface and every grid again.
        self.handle_resize(self.window.inner_size());
        self.needs_redraw = true;
    }
}

/// Start the GPU setup on a worker thread: a surface for `window` of
/// `size`, an adapter, the device and the renderer, drawing with the fonts
/// that arrive on `fonts` (measured meanwhile on the main thread, see
/// [`Fonts`]) and `colors`. The result comes back through `proxy` as
/// [`AppEvent::Renderer`]. `paced_by_frame_callbacks` is as for
/// [`Renderer::with_config`].
pub(super) fn spawn_setup(
    display: OwnedDisplayHandle,
    window: Arc<Window>,
    size: PhysicalSize<u32>,
    colors: ColorConfig,
    fonts: mpsc::Receiver<Fonts>,
    paced_by_frame_callbacks: bool,
    proxy: EventLoopProxy<AppEvent>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ultty-gpu-setup".into())
        .spawn(move || {
            let gpu = setup(
                display,
                window,
                size,
                &colors,
                fonts,
                paced_by_frame_callbacks,
            );
            // With the loop gone, Ultty is exiting and nobody needs it.
            let _ = proxy.send_event(AppEvent::Renderer(Handoff(gpu)));
        })
        .map(drop)
}

fn setup(
    display: OwnedDisplayHandle,
    window: Arc<Window>,
    size: PhysicalSize<u32>,
    colors: &ColorConfig,
    fonts: mpsc::Receiver<Fonts>,
    paced_by_frame_callbacks: bool,
) -> Result<Box<Gpu>> {
    // Vulkan (or the platform's primary API) first, GL only if that
    // finds no adapter. An instance with every backend brings up GL as
    // well, which loads and initializes the EGL driver even when Vulkan
    // is then chosen: startup time and resident memory for nothing.
    // `WGPU_BACKEND` (vulkan, gl, ...) picks one for diagnosis.
    let primary = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
    let (surface, adapter) = match request_gpu(&display, &window, primary) {
        Ok(gpu) => gpu,
        Err(primary) => {
            tracing::info!("No primary GPU backend ({primary:#}); trying GL");
            request_gpu(&display, &window, wgpu::Backends::SECONDARY).map_err(|gl| {
                if ultty::platform::vulkan_icd::narrowed_env() {
                    anyhow::anyhow!(
                        "{primary:#}; with GL: {gl:#} \
                         (Vulkan ICDs were narrowed by behavior.narrow_vulkan_icd; \
                         set narrow_vulkan_icd = false or VK_DRIVER_FILES to override)"
                    )
                } else {
                    anyhow::anyhow!("{primary:#}; with GL: {gl:#}")
                }
            })?
        }
    };
    tracing::debug!(target: "ultty::startup", "GPU adapter ready");
    let info = adapter.get_info();
    tracing::info!(
        "GPU: {} ({:?}, {:?}, {})",
        info.name,
        info.backend,
        info.device_type,
        info.driver
    );

    let fonts = fonts
        .recv()
        .map_err(|_| anyhow::anyhow!("Startup ended before the fonts were measured"))?;
    let renderer = pollster::block_on(Renderer::with_fonts(
        &surface,
        &adapter,
        size.width,
        size.height,
        colors,
        fonts,
        paced_by_frame_callbacks,
    ))?;
    tracing::debug!(target: "ultty::startup", "renderer ready");
    Ok(Box::new(Gpu { surface, renderer }))
}

/// A surface for `window` and an adapter that can draw to it, from an
/// instance limited to `backends`.
fn request_gpu(
    display: &OwnedDisplayHandle,
    window: &Arc<Window>,
    backends: wgpu::Backends,
) -> Result<(wgpu::Surface<'static>, wgpu::Adapter)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        // GL presents through the display connection (Wayland needs it).
        display: Some(Box::new(display.clone())),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance.create_surface(Arc::clone(window))?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .map_err(|e| anyhow::anyhow!("Failed to find suitable GPU adapter: {e}"))?;
    Ok((surface, adapter))
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalSize;

    use super::{RendererState, Starting};
    use ultty::config::FontConfig;
    use ultty::renderer::{Fonts, Renderer};

    fn starting(fonts: &Fonts, width: u32, height: u32) -> RendererState {
        RendererState::Starting(Starting::new(fonts, PhysicalSize::new(width, height), 1.0))
    }

    #[test]
    fn before_the_gpu_is_up_layout_comes_from_the_cpu_metrics() {
        let fonts = Fonts::load(&FontConfig::default());
        let (cell_width, cell_height) = fonts.cell_dimensions();
        let mut state = starting(&fonts, 800, 600);
        state.set_tab_bar_height(30.0);

        assert!(state.ready().is_none() && state.ready_mut().is_none());
        assert!(state.for_screenshot().is_err());
        assert_eq!(state.cell_dimensions(), (cell_width, cell_height));
        assert_eq!(state.terminal_area(), (800.0, 570.0));
        assert_eq!(
            state.terminal_dimensions(),
            ((800.0 / cell_width) as u16, (570.0 / cell_height) as u16)
        );

        // A resize is remembered for the renderer; a minimized window's
        // zero size is not a size.
        state.resize(1000, 700);
        state.resize(0, 0);
        assert_eq!(state.surface_size(), (1000, 700));
        assert_eq!(state.terminal_area(), (1000.0, 670.0));
    }

    #[test]
    fn the_grid_before_the_gpu_is_the_grid_after() {
        let Ok(mut renderer) = pollster::block_on(Renderer::new_headless(812, 613)) else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        renderer.set_tab_bar_height(37.0);
        let mut state = starting(&Fonts::load(&FontConfig::default()), 812, 613);
        state.set_tab_bar_height(37.0);
        assert_eq!(state.terminal_dimensions(), renderer.terminal_dimensions());
        assert_eq!(state.terminal_area(), renderer.terminal_area());
    }

    #[test]
    fn the_renderer_takes_over_what_changed_while_the_gpu_started() {
        let Ok(mut renderer) = pollster::block_on(Renderer::new_headless(400, 200)) else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let config = FontConfig::default();
        let mut state = starting(&Fonts::load(&config), 400, 200);
        state.set_tab_bar_height(24.0);
        let bigger = FontConfig {
            size: config.size * 2.0,
            ..config
        };
        assert!(
            !state.apply_font_config(&bigger),
            "the cell size is only known once the renderer arrives"
        );
        let zoomed = state.step_font_size(true);
        assert_eq!(state.font_size(), zoomed);

        let RendererState::Starting(starting) = state else {
            unreachable!("no renderer was installed");
        };
        starting.apply_to(&mut renderer);

        let mut live = pollster::block_on(Renderer::new_headless(400, 200)).unwrap();
        live.set_tab_bar_height(24.0);
        live.apply_font_config(&bigger);
        live.step_font_size(true);

        assert_eq!(renderer.tab_bar_height(), 24.0);
        assert_eq!(renderer.font_size(), zoomed);
        assert_eq!(renderer.font_size(), live.font_size());
        assert_eq!(renderer.cell_dimensions(), live.cell_dimensions());
        assert_eq!(renderer.terminal_dimensions(), live.terminal_dimensions());
    }
}
