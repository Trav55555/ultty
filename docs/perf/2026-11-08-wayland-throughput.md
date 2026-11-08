# Wayland throughput collapse: presenting under the terminal lock (2026-11-08)

On the AWS box (Tesla T4, NVIDIA 580, headless sway 1.11 with
`--unsupported-gpu`, wlroots renderer on the GPU), `compare.sh --session
wayland` measured Ultty's live throughput at about 1 MiB/s, with a few
rounds at 44 to 66 MiB/s:

| corpus | Ultty, Wayland | Ultty, X11 (same box) | Ghostty, Wayland |
|---|---:|---:|---:|
| ascii | 1.2 | 58 | 119 |
| cursor-redraw | 1.4 | 86 | 78 |
| sgr-dense | 1.1 | 45 | |
| unicode-mixed | 0.8 | 14 | |

## Cause

`App::render` acquired the surface frame before locking the terminal, but
the renderer presented the frame from inside its draw call, while the app
still held the focused terminal's lock (or every pane's lock). Presenting
can block:

- An EGL swap on Wayland, and a FIFO Vulkan present on drivers that throttle
  in `vkQueuePresentKHR`, wait for the compositor's frame callback for the
  previous frame: up to a whole refresh.
- A compositor can withhold frame callbacks indefinitely: no output, an
  occluded or hidden window, a headless output that is not repainting.

The PTY I/O thread (`event_loop.rs`, `drain`) parses each 64 KiB chunk
under the same lock. While the main thread sat in present, the I/O thread
sat on the mutex; when present returned, the main thread released the lock,
went round the update loop and locked the terminal again within a frame.
Parsing got the gaps between presents, so throughput fell to roughly one
chunk per frame, and to zero when frame callbacks stopped.

The fix: the renderer draws into a view the caller owns and only submits;
`Renderer::present` is separate, and `App::render` calls it after the draw
functions have returned and dropped every terminal guard. Acquire was
already outside the lock. Parsing now proceeds while a frame is pending.

## Reproduction on this workstation

There is no sway on the workstation (AMD RX 7900, Mesa 26.0.8, RADV), so the
compositor was a private headless GNOME Shell 50.1: `dbus-run-session --
gnome-shell --headless --wayland --no-x11 --virtual-monitor 1920x1080@R`
with a private `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME` and friends, and
`GSETTINGS_BACKEND=memory`. Ultty ran `ubench-feed` (16 MiB CRLF corpora,
64 KiB writes, CPR fence) as its shell with the `compare.sh` Ultty config.

The Vulkan path (RADV) did not reproduce: on a 10 Hz output most frames
took about 4 ms and the feed ran at 167 to 231 MiB/s before the fix too, so
RADV's present does not wait for Mutter's frame callbacks with the lock
held. The GL backend does block in `eglSwapBuffers` until the frame
callback arrives, which is what we suspect the NVIDIA driver does under
sway (not verified on the T4). Forcing GL
(`VK_ICD_FILENAMES=/nonexistent.json`; Ultty falls back to GL when Vulkan
finds no adapter) reproduced the collapse:

| GL backend, MiB/s | before | after |
|---|---:|---:|
| 60 Hz output, ascii | 139, 96 | not measured |
| 10 Hz output, ascii | 4.28, 4.11, 3.51 | 170, 184, 88, 120, 87 |
| 10 Hz output, cursor-redraw | 4.85, 4.85, 3.55 | 186, 226, 70, 148, 139 |
| 10 Hz output, sgr-dense | 1.82 | 41, 79, 76 |
| 10 Hz output, unicode-mixed | 0.89 | 26, 28, 30 |
| no output, ascii | never finished (120 s fence timeout, twice) | 171, 185, 190, 112, 162 |
| no output, cursor-redraw | | 228, 209, 225, 194, 180 |

The "before" shape at 10 Hz (ascii 3.5, cursor-redraw 3.6, sgr-dense 1.8,
unicode-mixed 0.9) matches the AWS numbers. The lower "after" samples were
taken at load 14 to 17 (other builds on the machine); the first ones at
load 5 to 10.

With the pipeline trace on (`RUST_LOG=ultty::perf=info
ULTTY_TRACE_PIPELINE=1`), `pipeline_render_frame` now reports
`acquire_us`, `draw_us` and `present_us`. At 10 Hz under GL, after the fix:
median acquire 59 us, draw 1.5 ms, present 97 ms. Before the fix, one
launch's four 16 MiB passes drew 145 frames of about 100 ms each, all with
the lock held (13.9 s in all); after it, the same four passes drew 6 frames.
A stalled "before" process (no output, GL) had its main thread in `poll`
inside the swap and the `ultty-pty` thread in `futex_do_wait` on the
terminal mutex.

### Healthy case (60 Hz output, Vulkan/RADV)

Interleaved, 4 launches per arm, 3 passes per corpus per launch (n = 12),
load 14:

| MiB/s, median (min-max) | before | after |
|---|---:|---:|
| ascii | 125 (103-140) | 127 (117-156) |
| cursor-redraw | 187 (115-215) | 193 (129-226) |
| sgr-dense | 88 (69-111) | 93 (65-116) |
| unicode-mixed | 33.6 (27-37) | 33.3 (31-35) |

No change where present does not block.

## X11 latency and throughput (Xvfb, lavapipe; `live-swrast`)

`compare.sh --session xvfb --terminals ultty --corpora ascii,cursor-redraw
--rounds 2 --repeat 2 --k2p-samples 100`, 7 runs of each binary,
alternating (2 launches per run, 200 latency samples per run). The "after"
binary here was the first version of the fix, which presented from the same
place but let the renderer create the texture view; the final version was
checked with the traced latency runs below.

| | before | after | ratio, 95% CI |
|---|---:|---:|---|
| keypress-to-pixel, idle, median | 2.78 ms | 2.86 ms | 1.027 [1.016, 1.039] |
| keypress-to-pixel, min | 2.33 ms | 2.30 ms | |
| tp.live ascii/120x40 | 140 | 141 | 1.008 [0.916, 1.056] |
| tp.live cursor-redraw/120x40 | 151 | 158 | 1.044 [0.940, 1.199] |

`analyze.py` calls all of these "no detectable change" at a 5% threshold.
The latency p90 differed (3.6 ms before, 11 ms after) because two of the
"after" launches hit long stretches of 8 to 15 ms samples. Traced runs
showed those stretches in both binaries (one later "before" launch was the
worst of all, median 4.4 ms), and in every slow frame the time was inside
`queue.submit` on lavapipe (about 7.5 ms against 1.8 ms), with acquire and
present unchanged. Traced runs of the final version: 4 runs of 400
samples, medians 3.38, 3.06, 3.04, 2.99 ms; three traced "before" runs at
the same time: 4.36, 2.85, 3.39 ms. They are the software rasterizer under machine load,
not the change. The 3.6 ms idle figure from the T4 needs a fresh
`compare.sh --session x11` run there to confirm on real hardware.

## Not changed: the present mode

The surface still uses `PresentMode::AutoVsync` (FIFO) with
`desired_maximum_frame_latency: 2`. With the lock fix, a blocking present
only stalls the main thread, not parsing, so throughput no longer depends
on it. What a blocked main thread still delays while frame callbacks are
withheld: input handling, `ulttyctl` requests, native protocol sessions and
in-band pumping. Options, if that matters:

1. **Mailbox where the surface supports it, FIFO otherwise.** No tearing;
   present never waits for the compositor. Costs GPU work for frames the
   compositor drops, bounded by the 16 ms frame gate (input echo frames
   bypass it). Changes behaviour on every platform, so it needs its own
   latency and power measurements on the T4 and a laptop.
2. **Keep FIFO, latency 1.** Less queued work, same blocking.
3. **Skip rendering while occluded** (winit `WindowEvent::Occluded`), which
   addresses the indefinite case without changing the present mode.

Followed up in [frame pacing](2026-11-08-frame-pacing.md): frames are now
drawn from `RedrawRequested` with `pre_present_notify`, so on Wayland they
wait for the frame callback instead of blocking in present, and an occluded
window draws nothing.

## Rerunning on AWS

`scripts/bench/compare.sh --session wayland --terminals ultty,ghostty` on
the T4 is the oracle for the original report; this note's numbers come
from a different compositor and driver.
