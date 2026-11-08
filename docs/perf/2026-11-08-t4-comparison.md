# T4 comparison, startup and idle wakeups (2026-11-08)

Plan items D1 (rerun the comparison with the idle, GL and startup fixes),
D2 (keypress-to-pixel latency) and D4 (where the T4's startup goes).

AWS g4dn.xlarge (Tesla T4, NVIDIA 580.178.04, 4 vCPU), Ubuntu 26.04.1,
kernel 7.0.0-1016-aws, headless Xorg on the GPU with openbox, and headless
sway 1.11 for Wayland. 120x40 cells, DejaVu Sans Mono 11. Ultty
`fb00cb41e14f` (the `agent-tui` branch before the B2 work), Ghostty 1.4.0,
Alacritty 0.16.1. `scripts/bench-aws.sh --role gpu --compare-args
"--terminals ultty,ghostty,alacritty"`, 3 rounds, terminal order shuffled
per round. Claim class `live-realgpu`. The instance was terminated after
the run.

## X11

| | Ultty | Ghostty | Alacritty |
|---|---:|---:|---:|
| keypress to pixel, idle (median / p99, n=600) | 3.56 / 3.70 ms | 22.4 / 30.9 ms | 3.53 / 3.67 ms |
| keypress to pixel, under 10 MiB/s output | 5.53 / 8.33 ms | 14.9 / 26.0 ms | 4.62 / 7.70 ms |
| keypress to pixel, under maximum output | 21.9 / 24.8 ms | 34.2 / 57.9 ms | 12.4 / 19.8 ms |
| throughput, ascii (MiB/s, n=9) | 82.2 | 124 | 47.3 |
| throughput, cursor-redraw | 91.9 | 86.4 | 59.6 |
| throughput, sgr-dense | 45.3 | 26.1 | 53.2 |
| throughput, unicode-mixed | 22.6 | 24.1 | 42.5 |
| exec to titled window (median, n=3) | 358 ms | 654 ms | 207 ms |
| RSS after the feed (n=3) | 171 MiB | 203 MiB | 115 MiB |
| idle CPU (cpu-s per minute) | 0.18 | 0.29 | 0 |
| idle context switches per second | 114 | 8.3 | 0 |
| memory with 32 tabs (Alacritty: windows) | 153 MiB | 760 MiB | 211 MiB |

Idle keypress latency is level with Alacritty (ratio 0.99) and six times
Ghostty's. Under output Alacritty stays ahead: at full flood Ultty's median
is 1.8 times Alacritty's, with a tight spread (IQR 1.1 ms against 3.4 ms).
Unicode-heavy output remains the weakest throughput case.

## Wayland (sway)

Throughput and startup ran; every keypress-to-pixel probe failed for all
three terminals ("no pixels changed after a key press"), so the Wayland
probe (`k2p-wayland.py`, wtype plus grim) does not work on this setup and
there are no Wayland latency numbers. The other rows are close to X11:
throughput ascii 79.3 / 122 / 46.5 MiB/s (Ultty / Ghostty / Alacritty),
unicode-mixed 22.0 / 23.6 / 42.4, startup 440 / 749 / 253 ms, idle
switches 114 / 12.3 / 0 per second.

## Where startup goes on the T4 (D4)

`RUST_LOG=ultty::startup=debug`, 15 launches on the Xorg display,
milliseconds from exec (median). New stage points were added inside
`Renderer::new` for this.

| stage | from exec | step |
|---|---:|---:|
| window created | 16 | 16 |
| GPU adapter ready (Vulkan instance, adapter) | 99 | 83 |
| GPU device ready (`request_device`) | 211 | 112 |
| surface configured (swapchain) | 257 | 46 |
| pipelines and text renderer ready | 265 | 8 |
| shell started | 268 | 3 |
| drawing the first frame | 284 | 16 |

About 240 ms of the 284 is the NVIDIA Vulkan driver bringing up an
instance, a device and a swapchain. Ultty's own work (fonts, pipelines,
shell) is under 30 ms. Alacritty uses OpenGL and is 150 ms faster to a
window.

**Fonts on a thread did not help.** Loading the font database on a thread
from the start of `App::run` hid it completely (the renderer waited 0 ms
in 15 of 15 launches) but the first frame came at 299 ms against 292 ms
without it, and window creation moved from 16.6 to 24.3 ms: the font scan
competes with the display connection on 4 vCPUs. The change was reverted.

**OpenGL is not an option here.** `WGPU_BACKEND=gl` (now honoured, for
diagnosis) finds no adapter: wgpu's GL backend cannot present to this X11
surface on NVIDIA ("gl not compatible with provided surface", after an X
`BadWindow`).

What could still cut it, not tried: a Vulkan pipeline cache does not help
(pipelines are 8 ms); creating the device on a thread while the window
opens would overlap at most the 16 ms before the window; the 112 ms of
`vkCreateDevice` and 46 ms of swapchain are the driver's.

## The 114 idle wakeups are the NVIDIA driver's

Per-thread voluntary context switches over 5 s of idle Ultty: the main
thread, the PTY thread, the wakeup thread and the clipboard thread are all
at 0 to 0.6 per second. One thread does 100 per second, and a second one
more. `gdb` backtraces put both inside `libnvidia-glcore.so` in a
`pthread_cond_timedwait` loop, started by the driver during device
creation, next to its named `[vkcf]`, `[vkrt]` and `[vkps]` threads.
Ultty's own idle is unchanged from the local measurement (no wakeups with
the cursor steady); the driver adds a timed poll whenever a Vulkan device
exists. Ghostty and Alacritty use OpenGL and do not get it. It costs 0.18
cpu-s per minute (0.3% of a core).

## Not measured here

- Keypress latency on the development workstation (D2 asks for both
  machines): it waits for the soak running there to finish.
- Wayland latency, until the probe works on headless sway.
