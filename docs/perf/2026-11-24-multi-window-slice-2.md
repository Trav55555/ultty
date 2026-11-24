# Multi-window slice 2: shared font system and font grids (2026-11-24)

Measurements for [ADR 0027](../adr/0027-multiple-windows-per-process.md) slice 2
(one process-wide `SharedFontSystem`, ref-counted per-device `FontGrid`s keyed
by font configuration and physical size, shared glyph atlas bind group layout
and sampler, and per-window zoom).

Memory and startup were measured on the same workstation and setup as
[slice 1](2026-11-23-multi-window-slice-1.md): Xvfb (`1280x800x24`), Mesa
`lavapipe`, `--release` build, idle `/bin/sh`, `behavior.cursor_blink = false`.
Startup times are medians of 9 runs from `RUST_LOG=info,ultty::startup=debug`.
The `render` benchmark ran on the workstation's integrated GPU (`radeonsi`,
OpenGL) comparing `feat/multiwindow-slice-2` against `main` (slice 1).

## 1. Memory

Idle process RSS/PSS across 1, 2 and 3 same-scale windows in one process:

| Configuration | RSS (MiB) | PSS (MiB) | Threads | Marginal RSS vs 1 window |
|---|---:|---:|---:|---:|
| 1 window (1 process) | 112.6 | 68.1 | 35 | — |
| 2 windows (1 process, slice 2) | 119.3 | 74.8 | 38 | **+6.7 MiB** (+3 threads) |
| 3 windows (1 process, slice 2) | 126.1 | 81.6 | 41 | **+13.5 MiB** (**+6.75 MiB/win**, +3 threads/win) |

Compared to slice 1 (10.2 MiB/window marginal RSS across 1 → 3 windows), slice
2 drops idle per-window RSS growth by **34%** and eliminates per-window font
and glyph-atlas duplication (`/proc/<pid>/smaps`, RSS in MiB):

| Mapping | Slice 1 delta/window | Slice 2 (1 → 2 → 3 windows) | Slice 2 delta/window |
|---|---:|---:|---:|
| `fonts` (mmapped font files) | +0.8 MiB | 0.8 → 0.8 → 0.8 | **+0.0 MiB** |
| `[heap]` (`FontSystem` + shaping caches) | +1.5 to +1.6 MiB | 1.8 → 2.9 → 4.1 | **+1.1 MiB** |
| `memfd` (GPU memory) | +5.8 to +9.5 MiB (up to +16 MiB at 2048²) | 13.9 → 19.4 → 24.9 | **+5.5 MiB** (swapchain + buffers only; 0 extra atlases) |
| `[anon]` (thread stacks/arenas) | +0.15 MiB | 14.0 → 14.2 → 14.3 | +0.14 MiB |

With two same-scale windows open, `cache_stats().atlas_count` stays **1** (one
shared `GlyphAtlas` texture, growing to 2 only while a window is independently
zoomed or on a different scale factor, and dropping back to 1 when the zoomed
window resets or closes).

## 2. Startup latency

Medians (and min–max) across 9 runs:

| Metric | Slice 1 | Slice 2 | Change |
|---|---:|---:|---:|
| `open_new_window` (click → window shown + shared GPU ready) | 29 ms (24–41 ms) | **9 ms** (8–12 ms) | **3.2× faster** |
| Tab creation → `drawing the first frame window=1` | 44.0 ms (39.6–56.2 ms) | **24.4 ms** (23.9–29.2 ms) | **1.8× faster** |

### Stage breakdown of opening window 2 (medians of 9 runs)

| Stage | Slice 1 | Slice 2 | Notes |
|---|---:|---:|---|
| Tab + PTY + X11 window creation | 4.0 ms | 4.9 ms | `create_window` + `event_loop.create_window` |
| `Fonts::load` (`cell metrics ready`) | 21.0 ms | **0.0 ms** | Skipped on `FontGridSet` cache hit |
| Shared GPU surface + pipelines + `TextRenderer` | 2.7 ms | 3.3 ms | Reuses shared `FontGrid` and `BindGroupLayout` |
| AccessKit + show + tab bar + relayout | 1.3 ms | 1.4 ms | Window is ready and visible |
| Wait to `drawing the first frame window=1` | 15.2 ms | 15.0 ms | One 60 Hz frame pacing interval |

## 3. Render benchmark parity

Criterion comparison (`cargo bench -p ultty --bench render`, `--measurement-time 2 --warm-up-time 1 --sample-size 15`, `radeonsi` GL) against `main` (slice 1):

| Benchmark | Slice 1 (`main`) | Slice 2 | Criterion verdict |
|---|---:|---:|---|
| `render_one_dirty_row@gpu-gl/300x75_code` | 1.98 ms | 1.92 ms | No change (p = 0.14) |
| `render_scrolling@gpu-gl/ascii/120x40` | 965 µs | 959 µs | No change (p = 0.48) |
| `render_scrolling@gpu-gl/unicode-mixed/120x40` | 864 µs | 1.01 ms | No change (p = 0.06) |
| `render_scrolling@gpu-gl/cjk/120x40` | 726 µs | 730 µs | No change (p = 0.52) |

Because `FontGrid` state is locked once per dirty frame (never per cell) and
atlas hits do not lock `SharedFontSystem`, steady-state text rendering stays
within noise of slice 1.
