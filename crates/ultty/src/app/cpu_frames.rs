//! Frames drawn on the CPU until the GPU renderer arrives (ADR 0030).
//!
//! Controlled by `[behavior] cpu_first_frames` (default on on Linux, off on
//! other platforms until verified there), with
//! `ULTTY_CPU_FIRST_FRAMES=1` forcing it on and `ULTTY_CPU_FIRST_FRAMES=0`
//! forcing it off. While the GPU starts on its worker thread (see `gpu`),
//! frames are drawn by [`ultty::renderer::CpuFrame`] and presented with
//! softbuffer. When the renderer is handed over, the softbuffer surface is
//! dropped before the GPU's first present, and the GPU draws every frame from
//! then on.
//!
//! The softbuffer surface is made before the GPU setup starts. On macOS
//! both attach a sublayer to the view, and the later one is drawn on top;
//! softbuffer does not remove its layer when dropped, so the GPU's must be
//! the later.

use std::num::NonZeroU32;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ultty::config::ColorConfig;
use ultty::renderer::{CpuFrame, Fonts};
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use super::{WindowCtx, WindowState};

/// Whether to draw the first frames on the CPU: `ULTTY_CPU_FIRST_FRAMES=1`
/// forces on, `ULTTY_CPU_FIRST_FRAMES=0` forces off, otherwise uses
/// `config_default`.
pub(super) fn enabled(config_default: bool) -> bool {
    ultty::config::resolve_cpu_first_frames(
        config_default,
        std::env::var_os("ULTTY_CPU_FIRST_FRAMES").as_deref(),
    )
}

/// The window's softbuffer surface, and what draws into it once the fonts
/// are measured.
pub(super) struct CpuFrames {
    surface: softbuffer::Surface<OwnedDisplayHandle, Arc<Window>>,
    frame: Option<CpuFrame>,
    /// Frames presented so far (saturating).
    frames: u32,
}

impl CpuFrames {
    /// A softbuffer surface for `window`. Make it before the GPU's surface;
    /// see the module docs.
    pub(super) fn new(display: OwnedDisplayHandle, window: Arc<Window>) -> Result<Self> {
        let context = softbuffer::Context::new(display)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("softbuffer context")?;
        let surface = softbuffer::Surface::new(&context, window)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("softbuffer surface")?;
        Ok(Self {
            surface,
            frame: None,
            frames: 0,
        })
    }

    /// Draw with `fonts`, measured on the main thread, and `colors`.
    pub(super) fn set_fonts(&mut self, fonts: &Fonts, colors: &ColorConfig) {
        self.frame = Some(CpuFrame::new(fonts, colors));
    }
}

impl WindowState {
    /// Whether a frame can be drawn: by the GPU renderer, or on the CPU
    /// until it arrives.
    pub(super) fn can_draw(&self) -> bool {
        self.renderer.is_ready() || self.cpu_frames.is_some()
    }

    /// Log the first frame, drawn by `by`, that shows the shell's output:
    /// startup's end as a user sees it.
    pub(super) fn note_output_shown(&mut self, shows_output: bool, by: &str) {
        if shows_output && !self.output_shown {
            self.output_shown = true;
            tracing::debug!(
                target: "ultty::startup",
                window = self.key.raw(),
                "first frame showing shell output ({by})"
            );
        }
    }
}

impl WindowCtx<'_> {
    /// Draw and present a frame on the CPU, if the CPU frames are on and the
    /// GPU renderer has not arrived. Returns whether they are on.
    pub(super) fn render_cpu_frame(&mut self) -> bool {
        let Some(cpu) = self.win.cpu_frames.as_mut() else {
            return false;
        };
        let Some(frame) = cpu.frame.as_mut() else {
            return true;
        };
        let size = self.win.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return true;
        };
        if cpu.frames == 0 {
            tracing::debug!(target: "ultty::startup", "drawing the first CPU frame");
        }
        let top = self.win.renderer.tab_bar_height();
        let tab_bar = self.win.tab_bar.visible.then_some(&self.win.tab_bar);
        if let Err(error) = cpu.surface.resize(width, height) {
            tracing::warn!("CPU frame: resize failed: {error}");
            return true;
        }
        let mut buffer = match cpu.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                tracing::warn!("CPU frame: no buffer: {error}");
                return true;
            }
        };
        let shows_output = match self
            .app
            .runtime
            .manager
            .window_focused_terminal(self.win.key)
        {
            Some(terminal) => frame.draw(
                &mut buffer,
                width.get(),
                top,
                &terminal.event_loop.terminal(),
                tab_bar,
            ),
            None => false,
        };
        self.win.window.pre_present_notify();
        if let Err(error) = buffer.present() {
            tracing::warn!("CPU frame: present failed: {error}");
            return true;
        }
        if cpu.frames == 0 {
            tracing::debug!(target: "ultty::startup", "first CPU frame presented");
        }
        cpu.frames = cpu.frames.saturating_add(1);
        self.win.note_output_shown(shows_output, "CPU");
        self.app
            .runtime
            .manager
            .clear_window_active_dirty(self.win.key);
        true
    }
}
