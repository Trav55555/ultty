# Multi-window slice 1: second-window memory and startup (2026-11-23)

Measurements for [ADR 0027](../adr/0027-multiple-windows-per-process.md) slice 1
(in-process multiple windows with a shared `wgpu` instance, adapter, device and
queue).

All numbers below were measured on the development workstation under Xvfb
(`1280x800x24`) with Mesa's software Vulkan (`lavapipe`), using a `--release`
build, idle shells (`SHELL=/bin/sh`), and `behavior.cursor_blink = false`.
Memory is read from `/proc/<pid>/smaps_rollup`, `/proc/<pid>/smaps` and
`/proc/<pid>/status` after a 1.5 s settle window. Startup times are medians of
9 runs from `RUST_LOG=info,ultty::startup=debug`.

## 1. Memory

Comparing one Ultty process with 1, 2 and 3 in-process windows against opening
a second window as a separate process (`behavior.window_per_process = true`
behaviour):

| Configuration | RSS (MiB) | PSS (MiB) | Private dirty (MiB) | Threads | Marginal PSS vs 1 window |
|---|---:|---:|---:|---:|---:|
| 1 window (1 process) | 112.3 | 85.7 | 47.2 | 36 | — |
| 2 windows (1 process, slice 1) | 120.6 | 93.3 | 54.6 | 39 | **+7.6 MiB** (+3 threads) |
| 3 windows (1 process, slice 1) | 132.6 | 104.6 | 65.9 | 42 | **+18.9 MiB** (+9.5 MiB/win, +3 threads/win) |
| 2nd separate process (running alongside 1st) | 112.7 | 63.8 | 34.3 | 36 | **+63.8 MiB** (+36 threads) |

Opening a second window in-process uses **7.6 MiB PSS** and **3 threads**
versus **63.8 MiB PSS** and **36 threads** for a second process: an **8.4x
reduction in marginal memory (PSS)** and **12x fewer added threads**.

### Where the per-window memory goes (`/proc/<pid>/smaps`, RSS in MiB)

| Mapping | 1 window | 2 windows | 3 windows | Delta/window | What it is |
|---|---:|---:|---:|---:|---|
| `libLLVM.so` | 51.4 | 51.4 | 51.4 | 0.0 | lavapipe shader compiler (paid once) |
| `ultty` binary | 14.3 | 14.3 | 14.3 | 0.0 | text and data (paid once) |
| `libvulkan_lvp` / Mesa | 7.0 | 7.0 | 7.0 | 0.0 | Vulkan driver (paid once) |
| other `.so` | 8.6 | 8.6 | 8.6 | 0.0 | shared libraries (paid once) |
| `[anon]` | 14.1 | 14.2 | 14.4 | +0.15 | thread stacks and arenas |
| `memfd` (GPU memory) | 13.9 | 19.7 | 29.2 | +5.8 to +9.5 | swapchain images, per-window glyph atlas, vertex/uniform buffers |
| `[heap]` | 1.8 | 3.3 | 4.9 | +1.5 to +1.6 | per-window `FontSystem`, `SwashCache`, `TextRenderer` caches, grid |
| font files | 0.8 | 1.7 | 2.5 | +0.8 (RSS; shared in PSS) | mmapped font faces |

Slice 1 shares the GPU device, adapter, instance and driver libraries, so none
of the fixed driver or binary mappings repeat. Each window still constructs its
own `Fonts` (`FontSystem`) and `GlyphAtlas`; ADR 0027 slice 2 (shared
`FontSystem` and ref-counted `FontGrid`s) targets both the per-window glyph
atlas in `memfd` and the `[heap]` font caches.

## 2. Startup latency

Medians (and min–max) across 9 runs:

| Metric | Median | Min–Max |
|---|---:|---:|
| Process start (`Starting Ultty`) to `drawing the first frame window=0` (cold GPU) | 69.1 ms | 60.6–93.5 ms |
| In-process "New window" (`open_new_window` -> window shown + shared GPU ready) | **29 ms** | 24–41 ms |
| In-process "New window" (tab creation -> `drawing the first frame window=1`) | **44.0 ms** | 39.6–56.2 ms |

### Stage breakdown of opening window 2 (medians of 9 runs)

| Stage | Median | Share of `open_new_window` | Notes |
|---|---:|---:|---|
| Tab + PTY + X11 window creation | 4.0 ms | 14% | `create_window` + `event_loop.create_window` |
| `Fonts::load` (`cell metrics ready`) | **21.0 ms** | **72%** | Re-scans the system font database; slice 2 shares one `FontSystem` |
| Shared GPU surface + pipelines + `TextRenderer` | **2.7 ms** | **9%** | Reuses `SharedGpu` instance/adapter/device/queue (no adapter/device init) |
| AccessKit + show + tab bar + relayout | 1.3 ms | 4% | Window is ready and visible |
| Wait to `drawing the first frame window=1` | 15.2 ms | — | One 60 Hz frame pacing interval |

Reusing the shared GPU cuts window 2's GPU and renderer setup to **2.7 ms**.
The dominant remaining cost inside `open_new_window` is `Fonts::load`
(**21.0 ms**, 72%), which ADR 0027 slice 2 removes by sharing one `FontSystem`
and ref-counted `FontGrid`s across windows.
