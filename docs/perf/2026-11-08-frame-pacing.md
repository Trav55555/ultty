# Frame pacing: draw when the compositor asks (2026-11-08)

[The Wayland throughput note](2026-11-08-wayland-throughput.md) moved
`present` out from under the terminal locks, which let parsing go on while
a frame was waiting. The main thread could still block in `present` (or in
acquiring the next surface texture) for a refresh, or for as long as the
compositor withheld frame callbacks: a hidden or minimized window, a window
on another workspace, an output that is not repainting. While it was
blocked, Ultty handled no input, no `ulttyctl` requests, no native protocol
sessions and no in-band pumping.

## Prior art

**Alacritty 0.16.1** (read from the crate source):

- It turns vsync off: `Display::new` in `display/mod.rs` calls
  `surface.set_swap_interval(&context, SwapInterval::DontWait)`, so
  `swap_buffers` never waits for the compositor.
- Every frame is drawn from `WindowEvent::RedrawRequested`
  (`Processor::window_event` in `event.rs` calls `WindowContext::draw`), and
  `Display::draw` calls `window.pre_present_notify()` just before
  `swap_buffers`. On Wayland that makes winit request a frame callback with
  the commit and hold the next `RedrawRequested` until it arrives, so the
  compositor paces redraws.
- Terminal output only marks the window dirty and calls `request_redraw`
  (the `TerminalEvent::Wakeup` arm in `event.rs`, the end of
  `WindowContext::handle_event` in `window_context.rs`), at most once per
  frame (`Window::request_redraw` keeps a `requested_redraw` flag).
- On X11 and other platforms without frame callbacks, `Display::draw` calls
  `request_frame` after the swap: it clears `window.has_frame` and schedules
  an `EventType::Frame` timer from `FrameTimer::compute_timeout`, which
  aligns the next frame to the monitor's refresh interval
  (`refresh_rate_millihertz`, 60 Hz if unknown). The `Frame` event sets
  `has_frame` and requests a redraw if the window is dirty. Redraw requests
  wait while `has_frame` is false. On Wayland `has_frame` stays true.
- `WindowEvent::Occluded` sets `occluded`; `WindowContext::draw` returns
  without drawing while it is set, and no redraw is requested.
- There is no input fast path: a keystroke's echo waits for the next frame
  like any other output.

**Ghostty**: no source was available offline, so it was not checked.

**winit 0.30.13** (the version in `Cargo.lock`):

- Wayland `Window::pre_present_notify` calls
  `WindowState::request_frame_callback`, which asks for a `wl_surface.frame`
  callback unless one is already in flight.
- `request_redraw` sets an atomic flag and wakes the loop. Each loop
  iteration (`wayland/event_loop/mod.rs`, after user events and before
  `AboutToWait`) skips `RedrawRequested` for a window whose frame callback
  state is `Requested`; the flag stays set until the callback arrives.
- On X11, `pre_present_notify` does nothing and `request_redraw` is
  delivered on the same loop iteration, after user events.
- Only X11 sends `WindowEvent::Occluded` (from `VisibilityNotify`); the
  Wayland backend never does, so on Wayland withheld frame callbacks are the
  only signal.

## Design

Ultty keeps its own decision of when a frame is due (`update::should_render`:
the 16 ms frame interval, the input echo drawn at once, synchronized
updates), but no longer draws from the update loop. Instead
(`app/pacing.rs`, `FramePacing`):

1. When a frame is due, the update loop calls `Window::request_redraw`, once:
   a request already in flight covers later output.
2. `WindowEvent::RedrawRequested` draws the frame if one is due then. A
   redraw the window system sends on its own before the frame interval is up
   draws nothing; the loop asks again when it is.
3. `App::render` calls `Window::pre_present_notify` right before `present`,
   and only when a present follows: a callback requested without a commit
   would never arrive and would stop all drawing.
4. While a request is in flight, the loop sets no frame timer and no 1 ms
   catch-up poll. A withheld frame would otherwise keep the loop spinning.
   The loop still wakes for PTY output, sockets and its other timers.
5. `WindowEvent::Occluded(true)` stops requests and drawing; `Occluded(false)`
   draws once. PTY parsing and everything else carry on.

On Wayland this gives at most one frame per frame callback and no frames
while the compositor withholds callbacks. Presenting no longer waits either,
because the previous frame's callback has already arrived when the next
`RedrawRequested` comes. On X11, where winit delivers the request on the
same iteration, pacing stays as before: at most one frame per 16 ms, with the
echo drawn at once. The echo of a keystroke arrives as a user event, and
winit delivers redraws after user events in the same iteration on both
backends, so the request adds no loop iteration to the echo path.

Unlike Alacritty, Ultty keeps FIFO (`PresentMode::AutoVsync`). On X11 a
present can therefore still block for a refresh. The brief was to keep X11
as it is, and Alacritty's X11 frame timer has no input fast path, which
Ultty's echo latency depends on.

### Not changed: frame latency

`desired_maximum_frame_latency` stays at 2. With callbacks pacing frames on
Wayland, at most one frame is queued, so the second slot is unused, and
present took 60 to 240 us in the traced runs below. Nothing measured here
suggests latency 1 would help, so it was left alone.

## Measurements

Workstation: AMD RX 7900, Mesa (RADV and GL), private headless GNOME Shell
50.1 compositors (`gnome-shell --headless --wayland --no-x11`, private
`XDG_RUNTIME_DIR`, D-Bus and XDG dirs). "Base" is `origin/main` at d4c3f26
(PR #42 merged); "fix" is this branch. Both were release builds, copied out of
the shared target dir before running.

### Main thread responsiveness on Wayland

Ultty ran a shell that either streamed a 16 MiB ASCII corpus in a loop
(`while :; do cat ascii.txt; done`) or slept (idle). After 4 s, ten
`ulttyctl ping` calls 0.5 s apart and one `ulttyctl text` were timed, and the
main thread's CPU time and context switches (`/proc/<pid>/task/<pid>`) were
sampled over 10 s. A ping that took longer than its 5 s read timeout counts
as failed (F).

| compositor output | backend | shell | base: ping ms | fix: ping ms | fix: main thread CPU, ctx switches/s |
|---|---|---|---|---|---|
| none | GL | streaming | all 10 F, twice (`text` F) | 6 to 19 | 5%, 1000 |
| none | GL | idle | all 10 F | 5 to 7 | 0%, 1 |
| none | Vulkan | streaming | 1262 to 1543 | 6 to 13 | 5%, 990 |
| none | Vulkan | idle | 1047 first, then 6 to 10 | 6 to 7 | 0%, 1 |
| 1920x1080 at 60 Hz | Vulkan | streaming | 814 to 2050, twice | 6 to 16 | 6%, 1000 to 1900 |
| 1920x1080 at 10 Hz | GL | streaming | 186 to 195, twice | 5 to 18 | 7%, 1380 |

With no output at all, the base build's main thread never came back from
its first GL swap, so it answered nothing, idle or not. Under Vulkan it
blocked for about 1.5 s at a time in acquire. The fix answered every request
in a few milliseconds, and idle it woke about once a second, as before.
While streaming, its main thread woke about 250 times a second for output
(the I/O thread's 4 ms wake throttle) and used about 5% of a core; it did
not spin.

The 60 Hz case shows that a visible-but-not-repainting output is common
here. Traced runs show both builds getting frames at 60 Hz for the first
0.2 s of the stream, then none: the headless Mutter stopped repainting.
The base build then spent its frames in a blocking acquire (one frame took
302 ms; most pings took 1.5 s). The fix simply stopped drawing until the
next callback.

At 10 Hz under GL, the traced feed shows where the time went. Base: median
present 48 ms, 258 ms of render time for 5 frames. Fix: median present
234 us, 11 ms for 4 frames.

### Throughput on Wayland

`ubench-feed` (16 MiB corpora, 3 passes per launch), launches alternating
between builds, MiB/s median (min to max):

| case | base | fix |
|---|---:|---:|
| 60 Hz Vulkan, ascii, n=9 | 173 (155-184) | 153 (136-177) |
| 60 Hz Vulkan, cursor-redraw, n=9 | 224 (182-258) | 223 (182-247) |
| 60 Hz Vulkan, sgr-dense, n=9 | 122 (100-127) | 113 (102-126) |
| 60 Hz Vulkan, unicode-mixed, n=9 | 41.6 (37-43) | 37.1 (36-41) |
| 60 Hz Vulkan, ascii, n=15, base first | 155 (125-172) | 140 (129-169) |
| 60 Hz Vulkan, ascii, n=15, fix first | 153 (129-170) | 150 (125-167) |
| 60 Hz Vulkan, unicode-mixed, n=15, base first | 38.3 (35-41) | 36.5 (33-38) |
| 60 Hz Vulkan, unicode-mixed, n=15, fix first | 37.4 (32-42) | 35.8 (33-40) |
| 60 Hz Vulkan, ascii, ABBA | 160 (143-185) | 156 (141-169) |
| 60 Hz Vulkan, sgr-dense, ABBA | 114 (92-126) | 109 (86-121) |
| 10 Hz GL, ascii, ABBA | 152 (132-169) | 142 (122-170) |
| 10 Hz GL, cursor-redraw, ABBA | 233 (193-239) | 214 (201-233) |
| 10 Hz GL, sgr-dense, ABBA | 113 (95-120) | 104 (90-116) |
| 10 Hz GL, unicode-mixed, ABBA | 38.3 (36-39) | 35.8 (35-37) |

(The ABBA runs reused labels within a round, so each arm kept 2 of its 4
launches per round, n=6 to 12.)

The ranges overlap throughout, but the fix's median is lower in every row:
2 to 4% when the fix ran first or in ABBA order, up to 12% in the first runs.
Frames do not explain it: traced 60 Hz feeds drew 10 to 12 frames per launch
in both builds, with 12 to 16 ms of drawing in all. The likely cause, which
these measurements do not prove, is the main thread itself. On this
compositor the base build's main thread spends most of a stream asleep in a
blocked present or acquire (the ping column above), so the I/O thread has
the terminal lock to itself. The fix's main thread stays awake and handles
output wakes, each of which takes the lock briefly, as the base build does
whenever its present does not block. If that is the cause, the difference is
the price of a responsive main thread, not of the pacing. The Xvfb runs
below, where present does not stall, are the check on that.

### X11 (Xvfb, lavapipe; `live-swrast`)

`compare.sh --session xvfb --terminals ultty --corpora ascii,cursor-redraw
--rounds 2 --repeat 2 --k2p-samples 100`, alternating builds:

16 runs, ABBA order (8 base, 8 fix), 200 latency samples and 4 throughput
passes per corpus per run. Two fix runs (fix-8 and fix-15) collapsed:
keypress-to-pixel medians of 20 and 30 ms and 5 to 17 MiB/s. Both ran while
the load average rose to 19 to 26 from other builds on the machine, and in
both the window took 260 to 830 ms to appear instead of about 95 ms. To check
whether that was the load or the change, four short runs (ascii only)
alternated the builds while 16 busy loops competed for the 12 cores. Both
builds collapsed the same way:

| under 16 busy loops | k2p median | throughput, MiB/s | startup |
|---|---:|---:|---:|
| base | 66 ms | 6.3, 6.9 | 520 ms |
| fix | 71 ms | 6.4, 13.7 | 444 ms |
| fix | 64 ms | 6.1, 8.4 | 257 ms |
| base | 57 ms | 11.5, 7.5 | 914 ms |

So those two runs were load. Without them (8 base runs, 6 fix runs; ratio
CIs from resampling whole runs):

| | base | fix | fix / base, 95% CI |
|---|---:|---:|---|
| keypress-to-pixel, idle, median | 3.25 ms | 3.26 ms | 1.00 [0.91, 1.12] |
| keypress-to-pixel, p90 | 10.1 ms | 10.4 ms | |
| keypress-to-pixel, min | 2.47 ms | 2.31 ms | |
| per-run medians | 3.08 to 3.61 ms | 2.88 to 5.63 ms | |
| tp.live ascii/120x40, MiB/s | 129 | 129 | 1.00 [0.90, 1.11] |
| tp.live cursor-redraw/120x40, MiB/s | 145 | 141 | 0.98 [0.88, 1.15] |

No detectable change on X11. That fits the explanation of the Wayland
throughput difference: where present does not stall the base build's main
thread, the two builds parse at the same speed.

## Not verified

- A real desktop: minimizing, switching workspaces, and occlusion on X11
  under a window manager. The headless compositor stood in for a hidden
  window by not repainting, and no X11 window manager ran.
- The AWS T4 (NVIDIA, sway), where the original report came from:
  `compare.sh --session wayland` there is still the oracle.
