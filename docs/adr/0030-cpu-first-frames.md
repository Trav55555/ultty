# ADR 0030: First frames on the CPU while the GPU starts

## Status

**Default on on Linux** 2026-11-22. Slice 1 (`startup-shell-before-gpu`,
PR #179), the CPU-grid prototype (PR #180), Slice 2's `[behavior]
cpu_first_frames` config setting and Slice 3's CPU tab bar and window chrome
rendering (PR #205) are all on `main`. `[behavior] cpu_first_frames` now
defaults to `true` on Linux (X11 and Wayland, both verified on NVIDIA T4) and
remains `false` on macOS/Windows until hardware handover is verified there,
with `ULTTY_CPU_FIRST_FRAMES=1`/`0` overrides still available on every platform.

## Context

### The problem

Ultty takes about twice as long as Alacritty to show a window on the
NVIDIA T4, and almost all of the difference is the GPU driver:

| exec to titled window, T4, X11 (median) | Ultty | Alacritty |
|---|---:|---:|
| 2026-11-08 (`docs/perf/2026-11-08-t4-comparison.md`) | 358 ms | 207 ms |
| 2026-11-11 (`docs/perf/2026-11-11-t4-three-way.md`) | 400 ms | 197 ms |
| 2026-11-19 (`docs/perf/2026-11-19-t4-rerun.md`) | 349 ms | 193 ms |

The 11-08 note broke startup into `ultty::startup` stages
(`RUST_LOG=ultty::startup=debug`, 15 launches, median, ms from exec):

| stage | from exec | step |
|---|---:|---:|
| window created | 16 | 16 |
| GPU adapter ready (Vulkan instance, adapter) | 99 | 83 |
| GPU device ready (`request_device`) | 211 | 112 |
| surface configured (swapchain) | 257 | 46 |
| pipelines and text renderer ready | 265 | 8 |
| shell started | 268 | 3 |
| drawing the first frame | 284 | 16 |

About 240 ms of 284 is the NVIDIA Vulkan driver bringing up an instance,
an adapter, a device and a swapchain. Ultty's own work is under 30 ms. A
Vulkan pipeline cache would not help (pipelines are 8 ms), and wgpu's GL
backend cannot present to that X11 surface on NVIDIA.

Plan row D4 (exec to titled window at or under Alacritty's on the T4) is
open for this reason.

### Others hit the same wall

On The Standup #080 (2026-11-20), Mitchell Hashimoto said acquiring the
first Metal context costs about 300 ms per app on macOS, and that
Ghostty's plan (with a paid contributor on it) is to render the first few
dozen frames on the CPU, which is faster, while the GPU context is
acquired on a background thread, then switch to the GPU. Casey Muratori
added that creating a D3D device on Windows is similarly slow, and that
mapping fresh memory is slow too, so a fast start keeps its footprint
small.

### What "first frame" means here

The current benchmark stops at the *titled* window: the shell's title
reaching the window manager. That was a fair proxy while the title could
only arrive after the GPU was up, but the `startup-shell-before-gpu`
commit starts the shell before the GPU, so the title now arrives early
with nothing drawn. The measure this ADR uses is **exec to the first
frame that shows the shell's prompt**: pixels a user can read, not a
blank or background-coloured window.

### What exists today

- `startup-shell-before-gpu` (5fb8ec3) moves the instance, adapter,
  surface, device and pipelines to a worker thread
  (`app::gpu::spawn_setup`), measures the cell on the CPU
  (`renderer::Fonts`), sizes the grid and starts the shells meanwhile, and
  hands the renderer back as `AppEvent::Renderer`. Until then
  `RendererState::Starting` answers layout questions and nothing is drawn.
- Nothing rasterises cells on the CPU. Screenshots (`renderer/screenshot.rs`,
  pane screenshots included) render to an offscreen wgpu texture and read
  it back. `tiny-skia` is in `Cargo.lock` only through winit's Wayland
  client-side decorations (`sctk-adwaita`); Ultty does not use it.
- Glyphs are rasterised by swash (through cosmic-text's `SwashCache`) on
  the CPU and uploaded to a GPU atlas (`renderer/atlas.rs`). The rasters
  themselves are CPU data, so a CPU path can use the same ones.

## Decision

Draw frames on the CPU from the moment the window exists until the GPU
renderer is ready, then hand over to the GPU and never draw on the CPU
again in that window. Ship it in slices behind a setting, default off
until the T4 numbers say it pays.

### Design

1. **Presenting CPU frames: softbuffer.** `softbuffer` (rust-windowing,
   MIT OR Apache-2.0, 0.4.8 released 2025-12-13, MSRV 1.71, commits in
   2026-07, 15M recent downloads) takes a `raw-window-handle` window, so it
   works with winit 0.30 directly. Backends: X11 (`XPutImage`, MIT-SHM
   when available; depth 24 and depth 32 visuals, so Ultty's transparent
   window works), Wayland (`wl_shm` buffers on the window's `wl_surface`),
   macOS (a `CALayer` sublayer with a `CGImage`), Windows (GDI `BitBlt`).
   Ultty builds it without the KMS backend and with the X11 and Wayland
   libraries loaded at run time, as winit does; it adds `softbuffer`,
   `tiny-xlib`, `ctor` and `dtor` to the lock file on Linux.
2. **What the CPU draws.** The focused pane's grid (cell backgrounds,
   glyphs and the cursor) and the tab bar / window chrome (`renderer::CpuFrame`,
   sharing `renderer::tabbar`'s geometry and colours). Glyphs come from the same
   `Fonts` the GPU renderer is built from: the font database is copied (its face
   list; no font file is read again), each character is shaped through
   cosmic-text (so fallback matches) and rasterised by swash. Pixels are opaque
   sRGB.
3. **The GPU on a thread.** As in `startup-shell-before-gpu`: instance,
   adapter, device, swapchain and pipelines on a worker, handed back
   through the event loop. On macOS the `CAMetalLayer` must be created on
   the main thread (`raw-window-metal`'s `Layer::from_ns_view` panics
   elsewhere), so there the surface is made on the main thread and only
   the instance, adapter and device on the worker.
4. **Handover.** When `AppEvent::Renderer` arrives, the softbuffer surface
   is dropped and the next frame is drawn by the GPU. The CPU's last frame
   stays on screen until the GPU's first present replaces it.

### Can one window go from softbuffer to a wgpu surface?

This is the crux; a window that cannot switch makes the idea moot on that
platform.

| platform | CPU present | GPU present | switch | evidence |
|---|---|---|---|---|
| X11 | `XPutImage`/`XShmPutImage` into the window | Vulkan WSI (DRI3/Present, or `XPutImage` for software drivers) | **works** | prototype on Xvfb+lavapipe (8/8 launches, 0 blank frames) and NVIDIA T4 Xorg `:0` (15/15 launches, 0 blank frames), below |
| Wayland | `wl_shm` buffer attached to the window's `wl_surface` | Vulkan WSI attaches its own buffers to the same `wl_surface` | **works** | prototype on NVIDIA T4 under headless `sway` (`WLR_RENDERER=gles2`, 10/10 launches cleanly hand over; `grim` confirms CPU prompt pixels before handover and GPU prompt + tab bar after). Continuous flicker polling still needed before defaulting on |
| macOS | a `CALayer` sublayer of the view's layer | a `CAMetalLayer` sublayer (`raw-window-metal`) | expected to work with care, not run | both are sublayers; the later one is on top. softbuffer 0.4.8 does not remove its sublayer when dropped, so its layer must be added first (Ultty creates it before the GPU setup starts) or removed by hand |
| Windows | GDI `BitBlt` to the window's DC | DXGI flip-model swapchain on the same `HWND` (D3D12 or Vulkan) | expected to work, not run | a flip-model swapchain replaces the window's GDI redirection surface; GDI drawing is not shown once it presents, which is what the handover wants. No two swapchains are involved |

X11 and Wayland have been run on real NVIDIA hardware (Tesla T4, driver
580.178.04). macOS and Windows are from reading softbuffer 0.4.8, wgpu-hal 30
and raw-window-metal 1.1 and the platform documentation, and need a run on
each platform before the setting can default on there.

### Handover risks

- **Flicker.** A window manager or compositor may show one frame of
  nothing between the CPU's last present and the GPU's first. On X11 the
  window keeps the CPU frame's pixels until the swapchain presents; on the
  prototype no blank frame was seen (see Measurements). Each platform needs
  the same check.
- **Size and DPI mismatch.** The CPU frame is drawn at the window's current
  physical size and the grid's cell size; the GPU renderer is built from
  the same `Fonts` and is resized to the window on arrival
  (`install_renderer`), so the grid is identical. What differs is
  everything the CPU does not draw yet: the tab bar's text and buttons,
  pane borders, selection, images. Those appear at the switch.
- **Colour.** The GPU renders into an sRGB surface and blends in linear
  space; the CPU blends glyph coverage in sRGB. Text weight can change
  slightly at the switch.
- **Damage and memory.** Each CPU frame redraws the whole window (no
  damage tracking); at 1280x800 that is a 4 MiB buffer and about 1 ms of
  fill on the development machine. softbuffer on X11 allocates an SHM
  segment of the window size; on Wayland, two `wl_shm` buffers. All of it
  is freed at the handover. Casey Muratori's point applies: a large
  window costs fresh pages, so CPU frames should be a few, not a mode.
- **A second font system.** The copy costs memory (the face list) and
  shaping caches, freed at the handover.
- **Failure.** If softbuffer cannot attach (an unusual visual, say), Ultty
  logs it and draws nothing until the GPU, as without the setting. A GPU
  that never comes up ends Ultty with the same error as before.

### Alternatives

1. **Only move the GPU setup off the main thread, and show an empty or
   background-coloured window meanwhile.** This is the
   `startup-shell-before-gpu` commit, plus at most a background fill.
   It makes the *titled window* early and lets the shell start sooner,
   but the prompt is still drawn only after the GPU is up, so the measure
   that matters moves only by the overlap of shell start with GPU setup.
   It is also the required first slice of this ADR.
2. **Present a pre-rendered first frame** (an image of an empty terminal
   saved from the last run). Shows something sooner, but not the prompt,
   is wrong whenever the size, theme or font changed, and needs the same
   CPU present path as this ADR to get onto the screen.
3. **Do nothing.** Accept ~350 ms on the T4 and keep D4 open. Ultty's own
   work is already under 30 ms; the rest is the driver.

## Slices and acceptance criteria

1. **GPU setup off the main thread** (`startup-shell-before-gpu`). Done
   when it merges; no regression in the steady-state benchmarks.
2. **CPU grid frames on X11 behind a setting** (this prototype, then a
   `[behavior]` setting instead of the environment variable). Accept when,
   on the T4 under X11: exec to first prompt pixels is under 250 ms
   (median of at least 15 launches, alternating with the setting off),
   exec to titled window is under 250 ms, the steady-state X11 rows of
   `scripts/bench/compare.sh` (keypress to pixel, throughput, RSS) do not
   regress beyond their run-to-run spread, and an `ulttyctl` capture taken
   across the handover shows no blank frame.
3. **Tab bar and chrome on the CPU,** so nothing visibly appears at the
   switch (implemented: `CpuFrame::draw` renders the CSD title band, logo,
   chrome buttons, divider, and multi-tab row matching `renderer::tabbar`).
4. **Wayland,** then **macOS** (surface on the main thread, layer order),
   then **Windows**: each needs a run that shows the switch without a
   blank frame before the setting can default on there.
5. **Default on** where slice 2's criteria hold.

## Measurements

### Local headless Xvfb + lavapipe (15 shuffled rounds)

Measured with `bash --norc --noprofile -i` polling the Xvfb framebuffer file
for bright prompt pixels in row 0 (medians, ms from `exec`):

| Metric | `main` (`0a5b77b`) | `off` (D4 only) | `cpu` (`ULTTY_CPU_FIRST_FRAMES=1`) |
|---|---:|---:|---:|
| Exec → prompt pixels | 252.5 (230.0–280.8) | 225.9 (198.4–252.5) | **82.3** (71.6–102.2) |
| Shell started | 115.1 | 49.7 | 50.2 |
| First CPU frame showing output | — | — | 79.4 |
| GPU renderer ready | 111.1 | 105.6 | 109.2 |
| Peak / settled RSS (`VmHWM` / `VmRSS`) | — | 119.2 MiB | 119.4 MiB |

A separate continuous framebuffer poll across 8 launches (`handover_check.py`,
2,068–4,137 samples per launch) observed **0 samples without the prompt**
between the CPU prompt appearing and the GPU tab bar taking over.

### NVIDIA T4 — X11 (Xorg `:0`, Tesla T4, driver 580.178.04, 15 shuffled rounds)

Measured with `ultty -e sh -c 'printf "\033]2;probe\007startup-probe-prompt$ "; sleep 8'`,
polling Xorg `:0` via `XGetImage` for row-0 prompt pixels and tab-bar pixels,
and `XQueryTree`/`WM_NAME` for title `"probe"` (medians, p25–p75 in ms from
`exec`):

| Metric | `main` (`df06cc0`) | `off` (D4 only) | `cpu` (`ULTTY_CPU_FIRST_FRAMES=1`) |
|---|---:|---:|---:|
| Exec → titled window (`probe`) | 320.7 (297.8–325.2) | 37.9 (37.4–39.4) | **39.0** (37.9–40.3) |
| **Exec → first prompt pixels** | **336.6** (313.1–342.3) | **331.3** (315.5–360.1) | **54.3** (53.7–56.3) |
| Shell started | 314.4 | 32.7 | 32.8 |
| First CPU frame showing output | — | — | 53.2 |
| GPU device ready | 244.4 | 247.1 | 264.9 |
| GPU renderer handed over | — | 298.5 | 321.9 |
| GPU tab-bar pixels on screen | 336.6 | 331.3 | 354.2 |
| Handover blank-frame samples (15 runs) | 0 | 0 | **0** |
| Peak / settled RSS (`VmHWM` / `VmRSS`) | 144.8 MiB | 154.3 MiB | 155.0 MiB |

CPU first frames cut T4 X11 exec-to-first-prompt-pixels by **6.2×** (336.6 ms
to **54.3 ms**, well below the 250 ms target and Alacritty's ~193 ms), with
**0 blank-frame samples** across all 15 handovers (~6,000 samples total) and
+0.7 MiB RSS over `off`.

### NVIDIA T4 — Wayland (headless `sway`, `WLR_RENDERER=gles2`, 10 shuffled rounds)

| Metric | `off` (D4 only) | `cpu` (`ULTTY_CPU_FIRST_FRAMES=1`) |
|---|---:|---:|
| Shell started | 18.4 | 18.5 |
| First CPU frame showing output | — | **38.8** (36.5–40.7) |
| **Exec → titled window (`swaymsg` tree)** | **331.4** (292.8–354.2) | **41.9** (39.5–44.8) |
| GPU renderer handed over | 304.1 | 328.1 |
| GPU drawing first frame | 324.8 | 346.4 |
| Alive after handover (no crash/error) | 10/10 | **10/10** |

On Wayland, `sway` does not map an `xdg_toplevel` until a buffer is committed.
Without CPU frames (`off`), the window stays unmapped until the GPU draws at
331.4 ms even though the shell started at 18.4 ms. Committing `wl_shm` CPU
frames maps the window with the prompt in **41.9 ms** (**7.9×** faster) and
hands over cleanly to NVIDIA Vulkan WSI in 10/10 runs. A `grim` pixel capture
confirmed prompt pixels before handover (`@120 ms`: 323 bright prompt pixels,
0 tab-bar pixels) and both prompt and GPU tab-bar pixels after handover
(`@870 ms`: 426 prompt pixels, 192 tab-bar pixels).

### Remaining before Slice 2 acceptance

- Run the steady-state `scripts/bench/compare.sh` suite on the T4 (keypress to
  pixel, throughput, RSS) to verify no regression.
- `[behavior] cpu_first_frames` config setting (done; `ULTTY_CPU_FIRST_FRAMES=1`/`0`
  overrides remain available for one-off launches).

## Consequences

- One more dependency (`softbuffer`) and a second, small renderer to keep
  in step with the GPU one for the cells it draws.
- Startup has a mode that lasts a fraction of a second, which makes bugs
  in it hard to see. Tests cover the rasteriser; the handover needs the
  per-platform runs above.
