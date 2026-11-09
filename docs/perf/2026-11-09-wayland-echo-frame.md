# Wayland echo frame: what Ultty presents after a key (2026-11-09)

[The first Wayland latency numbers](2026-11-09-wayland-latency.md) put
Ultty's idle keypress-to-pixel latency on headless sway at one compositor
frame plus about 1.2 ms (17.9 ms at 60 Hz, 5.8 ms at 240 Hz), against 3.4 ms
on X11. The suggested cause was that Ultty presents a frame without the echo
first (for the key itself, a cursor blink reset, or the echo fast path firing
before the PTY output), which uses up the compositor's frame callback, so the
frame with the echo waits for the next one. This note tests that, records
what was found instead, and the change made.

## Setup

Workstation: AMD RX 7900 XTX, Mesa RADV (Vulkan), private headless GNOME
Shell 50.1 (`gnome-shell --headless --wayland --no-x11 --virtual-monitor
1280x800@60`, own `dbus-run-session`, `XDG_RUNTIME_DIR` and XDG dirs). Ultty
debug builds (`opt-level = 1`), `main` at 49d1479 ("base") and this branch
("fix"), copied out of the target dir. Cursor blink off unless stated. The
machine was shared with other builds: load average 25 to 45 for the first
runs, 8 to 20 for the last ones. These are protocol traces, not latency
numbers.

Real key events: Mutter's RemoteDesktop API
([`scripts/bench/gnome-keys.py`](../../scripts/bench/gnome-keys.py)) presses
keys through the compositor's input path, so they reach Ultty as
`wl_keyboard.key` and go through winit like a typed key. One process owns the
session (Mutter ends it when the creating bus connection closes); GNOME
starts in the overview, and an Escape first focuses the window.
[`scripts/bench/gnome-echo-trace.sh`](../../scripts/bench/gnome-echo-trace.sh)
runs the whole thing and writes Ultty's `WAYLAND_DEBUG=client` log and its
pipeline trace (`RUST_LOG=ultty::perf=info ULTTY_TRACE_PIPELINE=1`).
[`scripts/bench/wl-echo-trace.py`](../../scripts/bench/wl-echo-trace.py)
turns the log into one block per key: frame callbacks outstanding when the
key arrived, every content commit (`attach` then `commit`) on the window's
surface, frame callbacks completing, and `wp_presentation_feedback` times,
in ms after the key on the compositor's clock.

The shell was `cat` (the tty echoes at once) or a raw-mode echo that answers
each byte after 30 ms, which separates a frame caused by the key from the
frame caused by its echo.

## The hypothesis does not hold here

Across 556 traced key presses (366 on base, 190 on the fix):

- Every key produced exactly one content commit, and no frame callback was
  outstanding when any key arrived.
- That commit carries the echo: in the pipeline trace each frame is
  scheduled 0.1 to 0.3 ms after the PTY drain that read the echo, and never
  before it.
- With the 30 ms echo the single commit lands 33 to 39 ms after the key
  (and up to 97 ms when the loaded machine delayed the echo program), so the
  key by itself draws nothing. The same holds with cursor blink on: the key
  restarts the blink phase without drawing, and the echo frame shows the lit
  cursor.

This matches the code: `handle_key` sets `needs_redraw` only for key
bindings and overlays; a plain key just writes to the PTY, and the frame
comes from `TerminalEvent::Redraw` through the echo fast path. One base
trace, `cat` shell:

```
key at 483525297 ms (compositor clock); callbacks outstanding: none
  +11.7  wl_surface.frame (winit, from pre_present_notify)
  +11.8  attach, wp_commit_timer_v1.set_timestamp(past), wp_fifo_v1.set_barrier,
         wp_fifo_v1.wait_barrier, commit; wp_fifo_v1.wait_barrier, commit   [Mesa WSI]
  +30.4  wl_callback.done
  +33.9  wp_presentation_feedback.presented (msc 3: one output frame per key)
```

## What did hold an echo back: FIFO

That trace shows the echo commit sitting in Mutter for 22 ms, more than a
frame, with the output otherwise idle. With FIFO, Mesa's Wayland WSI fences
each present with `wp_fifo_v1` (the commit waits for the previous frame's
barrier, and a second commit waits for this one's) and a
`wp_commit_timing_v1` target time. Ultty already paces frames with winit's
frame callbacks (PR #43, `app/pacing.rs`), so this second throttle buys
nothing.

Time from the echo commit to its frame callback (the compositor has drawn
it), with Mesa forced to Mailbox (`MESA_VK_WSI_PRESENT_MODE=mailbox`) or with
the fix, against FIFO (base, default):

| load | present mode | keys | p50 | p90 | max | held > 12 ms |
|---|---|---:|---:|---:|---:|---:|
| 25 to 45 | FIFO (base) | 160 | 0.9 ms | 17.6 ms | 22.5 ms | 34 |
| 25 to 45 | Mailbox (Mesa override, then the fix) | 140 | 0.8 ms | 2.7 ms | 7.5 ms | 0 |
| 8 to 20 | FIFO (base) | 120 | 0.6 ms | 0.9 ms | 19.1 ms | 3 |
| 8 to 20 | Mailbox (fix) | 120 | 0.6 ms | 0.8 ms | 17.6 ms | 1 |

The loaded rows alternated FIFO and Mailbox launch by launch. Under load,
FIFO held one echo in five for a whole extra frame; Mailbox held none. On a
quieter machine Mutter showed the echo within a millisecond either way, and
the odd late frame happened with both. Key-to-commit (Ultty's own work) was
the same for both builds (median 33 ms with the 30 ms echo).

So on Mutter the extra frame comes from the swapchain's FIFO throttling when
the compositor is slow to clear its barrier, not from an extra Ultty frame,
and only some of the time.

## sway is different, and was not available here

The T4 result is a full frame on every sample, which Mutter does not
reproduce. Two things differ on the T4 and could not be tried locally:

- **Compositor.** sway 1.11 is built on wlroots 0.19, which has no
  `wp_fifo_v1` or `wp_commit_timing_v1` (the local `libwlroots-0.19.so`
  0.19.2 has neither interface), so a FIFO swapchain there throttles some
  other way, typically by waiting on a frame callback inside the driver.
  sway is not installed on this machine.
- **Driver.** The T4 runs NVIDIA's Vulkan WSI, not Mesa's.

Whether FIFO is what costs the frame on sway is therefore not established.
It is the one client-side throttle on the echo path left after the traces
above, and the change below removes it, but the T4 run is the confirmation.

## The change

`renderer::present_mode` picks the swapchain's present mode: **Mailbox when
the window system paces frames with callbacks (Wayland) and the surface
supports it**, `AutoVsync` (FIFO) otherwise. X11, which has no frame
callbacks, keeps FIFO and so keeps its pacing exactly as before. Immediate
is not used: it may tear (Mesa can ask the compositor for asynchronous page
flips with it). The chosen
mode is logged at startup (`Present mode: Mailbox`).

Unit tests in `renderer/mod.rs` cover the three cases; the Mailbox test
fails when the function always returns `AutoVsync` (checked by mutation).

Checks on the live GNOME setup, debug builds, base then fix:

- **Frames:** one content commit per key on both; idle draws nothing between
  keys (one output frame per key in the presentation counter).
- **Flood** (`while :; do cat 4 MiB; done`, 60 Hz virtual monitor, two
  alternating launches each): base 11.8 and 12.4 MiB/s at 59.8 and 59.3
  frames/s; fix 12.4 and 14.8 MiB/s at 59.4 and 59.5 frames/s. The frame rate
  stays the compositor's; Mailbox draws no extra frames.
- **No output at all** (headless GNOME without a virtual monitor, so no frame
  callback ever comes; PR #43's responsiveness case), streaming output: ten
  `ulttyctl ping`s answered in 8 to 28 ms (base) and 12 to 25 ms (fix), one
  frame drawn in total by each.

## Confirming on the T4

On the instance, with this branch and `main` built (`--ultty-dir` pointing at
each build in turn), cursor blink off as `compare.sh` configures it:

1. **Trace one idle run per build** (the probe's virtual keyboard sends real
   key events with CLOCK_MONOTONIC timestamps, so the trace aligns with the
   probe's clock):

   ```bash
   WAYLAND_DEBUG=client RUST_LOG=info,ultty::perf=info ULTTY_TRACE_PIPELINE=1 \
     scripts/bench/compare.sh --session wayland --terminals ultty --rounds 1 \
     --k2p-samples 20 --flood '' --idle-secs 0 --tabs '' --size-mib 1 --repeat 1 \
     --corpora ascii --ultty-dir <build> --out <dir>
   python3 -I scripts/bench/wl-echo-trace.py <dir>/logs/ub-ultty-k2p-idle-1.ultty.log
   grep -m1 'Present mode' <dir>/logs/ub-ultty-k2p-idle-1.ultty.log
   ```

   Reading the table: one content commit per key and a frame callback about
   1 ms after the commit means sway drew the echo at once, and the 16.7 ms is
   outside Ultty (look at the compositor or the probe next). A callback about
   16.7 ms after the commit means sway held the commit; a first commit about
   16.7 ms after the key means the client held it (the pipeline trace's
   `present_us` and `acquire_us` show whether in the driver). If the log says
   `Present mode: AutoVsync`, the NVIDIA surface offers no Mailbox and this
   change does nothing there.

2. **Measure** (the same campaign as the first numbers, Wayland k2p suites,
   then a 240 Hz spot check and X11 to confirm it is unchanged), once per
   build:

   ```bash
   scripts/bench/compare.sh --session wayland --terminals ultty,ghostty \
     --idle-secs 0 --tabs '' --size-mib 4 --repeat 1 --corpora ascii \
     --ultty-dir <build>
   scripts/bench/compare.sh --session wayland --wayland-refresh 240 --rounds 1 \
     --terminals ultty,ghostty --idle-secs 0 --tabs '' --size-mib 4 --repeat 1 \
     --corpora ascii --ultty-dir <build>
   scripts/bench/compare.sh --session x11 --terminals ultty,ghostty \
     --idle-secs 0 --tabs '' --size-mib 4 --repeat 1 --corpora ascii --ultty-dir <build>
   ```

   (the X11 run in the Xorg session `scripts/bench/aws-remote.sh` starts; the
   `scripts/bench-aws.sh` command in the first-numbers note, with
   `--ref wayland-echo-frame`, drives both sessions from the workstation.)
   Success is
   Ultty's Wayland idle p50 dropping from 17.9 ms towards 1 to 2 ms at 60 Hz
   (5.8 ms towards 1.6 ms at 240 Hz), flood rows and throughput no worse, and
   X11 idle still about 3.4 ms. If the idle number does not move, step 1 says
   where the frame goes.
