# Idle wakeups, memory and startup (2026-11-07)

The comparison run on the AWS g4dn.xlarge (Tesla T4, X11, openbox, 120x40,
`scripts/bench/compare.sh`) put Ultty well behind Alacritty and GNOME
Terminal on three things that have nothing to do with throughput:

| | Ultty | Alacritty | GNOME Terminal |
|---|---:|---:|---:|
| idle, focused, no blink: context switches/s | 174 | 0 | 0 |
| idle CPU, CPU-s per minute | 0.61 | 0 | 0 |
| RSS after the feed workload | 216 MB | 115 MB | 55 MB |
| exec to titled window | 476 ms | 197 ms | |

This note records where each went and what changed. All numbers below
were measured on the development workstation, not the T4: Xvfb with
lavapipe (Mesa's software Vulkan), a release build, the window running
`sleep 600` (or `cat` of a 16 MiB CRLF text file, then `sleep`). Idle
wakeups and RSS barely move with machine load; startup times do, so they
are medians of 9 runs. The T4 numbers need a fresh `compare.sh` run.

## Method

- **Wakeups.** Sum `voluntary_ctxt_switches` and
  `nonvoluntary_ctxt_switches` from `/proc/<pid>/task/*/status` before and
  after a 30 or 60 s idle window, per thread, with each thread's name and
  `wchan`, so the waking thread is named. CPU is `utime + stime` from
  `/proc/<pid>/task/*/stat`.
- **Memory.** `/proc/<pid>/smaps_rollup` for RSS and PSS, `/proc/<pid>/smaps`
  summed per mapping for the breakdown, and valgrind massif for the heap.
- **Startup.** Wall time from exec until `xdotool` finds the window with
  the title the shell sets (what `compare.sh` measures), and the new
  `RUST_LOG=ultty::startup=debug` stage log for the split.

## 1. Idle: every periodic wakeup

| source | period | fate |
|---|---|---|
| cursor blink | 530 ms, plus a full frame | runs only while the cursor blinks |
| maintenance tick (config watcher poll, tab titles, keepalives, wait-for timeouts) | 250 ms | only with a native session or a pending wait-for |
| tab title refresh (foreground program, cwd, git branch) | 500 ms | after PTY output only |
| socket wakeup thread poll timeout | 100 ms | blocks with no timeout |
| socket wakeup re-registration | 1 s | on a changed socket identity only |
| socket wakeup coalescing | 1 ms sleep, repeated while a socket stayed ready | waits for the UI to rearm |
| PTY I/O threads | none | already blocked in `epoll_wait` |
| config watcher (notify, inotify) | none | already blocked |
| frame pacing, PTY catch-up poll (1 ms) | only while a frame is pending | unchanged |
| title typing animation | first ~second after launch | unchanged |

Before any change the process made 392-449 switches/s and used 2.5-2.8
CPU-s/min. The cursor always blinked (Ultty had no setting to stop it,
so the bench's "no blink" case blinked), and each blink drew a frame,
which in turn woke twelve llvmpipe workers and two swapchain threads.

| step | switches/s | CPU-s/min | UI thread switches/s |
|---|---:|---:|---:|
| before | 392-449 | 2.5-2.8 | 34-54 |
| blink only while blinking, `cursor_blink = false` | 19.1 | 1.46 | 8.9 |
| no maintenance tick, event-driven config reload and titles | 10.3 | 0.00 | 0 |
| socket wakeup thread blocks | **0.00** | **0.00** | 0 |

With the cursor blinking in a focused window the only wakeups are the
blink: the UI thread wakes about twice a second (17 switches/s including
the frame's present), and the rest are the frame itself in the driver
(lavapipe: ~340 switches/s, 0.86 CPU-s/min; a hardware driver does far
less per frame).

The pieces that keep correctness without polling:

- **Child exit behind a held PTY.** The exit used to be noticed by the
  250 ms tick when a background process kept the PTY open. The I/O thread
  now watches a pidfd for the child and pokes the UI when it exits
  (Linux). Without a pidfd it pokes every 250 ms, the old behaviour.
- **Config reload** is woken by the watcher thread and waits until the file
  has been quiet for 100 ms, so a multi-event save is read once, complete.
- **Tab titles** refresh after output, at most every 500 ms, and once more
  a full interval after output stops so a program started by the command
  line just echoed has taken the foreground.
- **Socket readiness** wakes the UI once; the thread re-arms the sockets
  when the UI calls `rearm` after an update, so a socket the UI has not yet
  drained cannot spin the thread.

What still ticks, and why: with a native app connected (or an `ulttyctl
wait-for` pending) the 250 ms maintenance tick runs, because session
keepalives (ping after 30 s idle, close after 120 s) and wait-for timeouts
are checked there. Exact per-session deadlines would remove it; it is
left because it only runs while a client is attached.

## 2. Memory

RSS of the idle window, then after `cat` of the 16 MiB file (10,000 lines
of scrollback at about 120 columns):

| | idle | after feed | threads | heap |
|---|---:|---:|---:|---:|
| before | 140 MB | 152 MB | 58 | 22.4-24.1 MiB |
| Vulkan only, GL as fallback | 120 MB | 130 MB | 33 | 13.0-14.5 MiB |

Where the 130 MB goes after the change (lavapipe):

| mapping | MiB | what it is |
|---|---:|---|
| `libLLVM.so` | 52 | lavapipe's shader compiler; a hardware driver maps its own libraries instead |
| `memfd:allocation` | 22-24 | GPU memory (lavapipe keeps it in host memory): the 2048x2048 RGBA8 glyph atlas alone is 16 MiB, plus swapchain images and vertex buffers |
| `[heap]` | 13-15 | massif: almost all of it is the Vulkan driver; Ultty's own allocations are under 7 MiB in total, scattered over ~2,800 sites, none above 0.36 MiB |
| `ultty` | 10-11 | the binary's text and data |
| `libvulkan_lvp.so`, `libgallium` | 7-9 | driver |
| `[anon]` | 4-10 | thread stacks and allocator arenas; grows with the feed |
| font files | 0.4 | only the faces in use are paged in |

The biggest avoidable cost was the GL backend. `wgpu::Backends::all()`
initialized EGL next to Vulkan: a second Mesa context, its own twelve
llvmpipe threads and their heap, although Vulkan was then chosen. On the
T4 the same probe loads NVIDIA's EGL and GL libraries. Asking for the
primary backend first and GL only when that finds no adapter cut 20 MB
and 25 threads here; starting with no Vulkan driver still falls back to
GL and draws.

Checked and left:

- **Font database.** `FontSystem::new` scans all 703 system faces in
  12-13 ms. The database is metadata; font files are memory-mapped and only
  the pages of faces in use are resident (0.4 MiB). Loading lazily would
  save little memory.
- **Scrollback.** Feeding 16 MiB grew the heap by about 1.5 MiB; the
  terminal model is gated by `cargo bench -p ultty --bench memory`, which
  this work did not touch (no re-baseline).
- **tokio** is a dependency of `ultty` but only the render bench uses it;
  no runtime threads are started.
- **Glyph atlas.** A 2048x2048 RGBA8 texture is 16 MiB of GPU memory, most
  of it empty for a terminal using one face. An R8 atlas for monochrome
  glyphs with a separate colour atlas, or a smaller atlas that grows, would
  cut most of that. Not done here: `renderer/atlas.rs` and `text.rs` are
  being reworked for text throughput at the same time.

## 3. Startup

Stage log of one launch (lavapipe), before and after:

| stage | before | after |
|---|---:|---:|
| `main` to configuration loaded (winit event loop, X connection) | 38-42 ms | 38-40 ms |
| window to GPU adapter ready | 36-45 ms | 14 ms |
| adapter to renderer ready (device, `FontSystem::new` 12-13 ms, pipelines) | 33-35 ms | 32-33 ms |
| renderer to shell started | 2 ms | 2 ms |
| shell started to first frame | 16-17 ms | 16 ms |
| **exec to titled window, median of 9** | **140 ms** | **123 ms** |

- Probing GL was the one avoidable cost found: 22-31 ms here, likely more
  with NVIDIA's EGL.
- The ~40 ms before `App::new` runs is winit building the event loop and
  connecting to X; not Ultty code.
- **Not done:** building the font database on a thread while the window
  and GPU come up would hide its 12-13 ms (more on a machine with more
  fonts or a cold cache). It needs `TextRenderer` to take a prebuilt
  `FontSystem`, in `renderer/text.rs`, which another change is reworking;
  it is the next step.
- The 476 ms on the T4 is more than three times this; the stage log there
  will say whether device creation or the window manager dominates.

## Reproducing

- Startup stages: `RUST_LOG=ultty::startup=debug target/release/ultty`.
- Process totals: `scripts/bench/compare.sh` records `idle.cpu` and
  `idle.wakeups`; its Ultty config now sets `cursor_blink = false`, like
  the other terminals' no-blink settings.
- Per thread: read each `/proc/<pid>/task/*` entry's `comm`, `wchan`, the
  two `*_ctxt_switches` lines of `status`, and fields 14-15 of `stat`, once
  before and once after the idle window, and subtract.
