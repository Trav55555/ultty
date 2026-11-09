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

## On the T4: Mailbox is chosen and changes nothing

Run 2026-11-09 on AWS g4dn.xlarge (Tesla T4, NVIDIA 580.178.04, 4 vCPUs),
Ubuntu 26.04.1, kernel 7.0.0-1016-aws, headless sway 1.11
(`WLR_RENDERER=gles2`, `--unsupported-gpu`), Ghostty 1.4.0; the instance ran
nothing else. Ultty release builds: **base** is `main` before PR #68
(`92185b0`), **fix** is this branch's head (`44f9705`) and, as a repeat,
`main` after the merge (`10a9185`, no other renderer change). Commands as in
"Confirming on the T4" above. The trace's log is
`logs/ub-ultty-k2p-idle-0.ultty.log` (rounds count from 0), not `-1`.

**Present mode.** Both fix builds log `Present mode: Mailbox` under sway:
the NVIDIA Vulkan surface offers Mailbox, so the change takes effect. Under
X11 they log `Present mode: AutoVsync`, as intended.

**Trace** (`wl-echo-trace.py`, 22 keys per build, idle, `cat`-style echo):

| build | commits per key | key to commit | key to frame callback done | callbacks outstanding at the key |
|---|---:|---:|---:|---|
| base (FIFO) | 1 | 0.9–1.7 ms | 17.8–20.2 ms (one at 28.3) | none on 20 keys, one on 2 |
| fix (Mailbox) | 1 | 1.0–2.1 ms | 15.9–21.1 ms (one at 1456) | none |

Every key gives one content commit with the echo about 1.5 ms after the key,
so the client neither draws an echo-less frame first nor holds the echo. The
frame callback for that commit then completes about 16.7 ms later with
either present mode. Per the reading guide above, that is sway holding the
commit for one output period: the frame on the T4 is spent in the
compositor, not in Ultty or the NVIDIA swapchain. (No
`wp_presentation_feedback` events were logged here, so the presented column
is empty. The 1456 ms callback on one fix key was not looked into; that
run's 20 probe samples had a p50 of 17.8 ms and no timeouts.)

**Latency**, keypress to pixel in ms, n = 600 per row at 60 Hz (3 launches)
and 200 at 240 Hz:

| session | case | Ultty base p50 / p99 | Ultty fix p50 / p99 | Ultty fix (main) p50 / p99 | Ghostty p50 / p99 |
|---|---|---:|---:|---:|---:|
| Wayland 60 Hz | idle | 17.79 / 17.95 | 17.74 / 17.90 | 17.81 / 18.00 | 36.02 / 37.57 |
| Wayland 60 Hz | flood 10 MiB/s | 29.10 / 37.26 | 29.09 / 35.93 | 29.15 / 37.29 | 27.30 / 38.18 |
| Wayland 60 Hz | flood max | 31.37 / 81.95 | 33.32 / 91.69 | 31.34 / 88.29 | 41.96 / 64.86 |
| Wayland 240 Hz | idle | 5.77 / 5.92 | 5.74 / 6.11 | 5.74 / 5.93 | 13.55 / 23.27 |
| Wayland 240 Hz | flood 10 MiB/s | 5.80 / 21.65 | 5.54 / 21.54 | 5.71 / 21.76 | 10.77 / 15.95 |
| Wayland 240 Hz | flood max | 22.05 / 46.52 | 21.48 / 69.69 | 21.73 / 69.96 | 24.75 / 44.69 |
| X11 | idle | 3.46 / 3.66 | 3.38 / 3.56 | 3.46 / 3.62 | 22.12 / 30.66 |
| X11 | flood 10 MiB/s | 4.86 / 20.29 | 4.77 / 8.16 | 5.32 / 8.02 | 13.30 / 24.17 |
| X11 | flood max | 21.54 / 24.37 | 21.57 / 24.22 | 21.70 / 24.20 | 32.65 / 59.83 |

Ghostty is from the campaign with this branch's build (it was not rerun
with base).
Live ascii throughput (4 MiB, 3 launches at 60 Hz) was 84.0 MiB/s for base
and 83.6 and 82.5 for the two fix builds, against Ghostty's 88–100.

**Result.** The idle Wayland p50 does not move: 17.8 ms at 60 Hz and 5.8 ms
at 240 Hz, and every 60 Hz idle sample on every build lies between 16.8 and
20.9 ms, one frame plus work. Flood rows, throughput and X11 are unchanged
within run-to-run noise. Two tails differ between campaigns, not between
builds: the Wayland max-flood p99 varies as much between the two fix
campaigns as against base, and X11's 10 MiB/s p99 was 20.3 ms for base
(FIFO on X11 in every build, so no code difference) against 8.0 and 8.2.
So Mailbox is harmless here but does not
remove the frame; FIFO was not what cost it on sway. The trace puts the
frame inside sway: it composites the echo commit one output period after
the commit, not at once. The suspects are now wlroots' headless output
scheduling (whether an idle headless output really composites a new commit
immediately, as the first-numbers note assumed) and how the probe's
screencopy request interacts with it. Ghostty, at two frames, may pay the
same period. The next step is a compositor-side trace (sway
with `-d`, or `WAYLAND_DEBUG=server`) of one key.

## Where the frame went: the probe

The frame was the probe's. `k2p-wayland.py` sent the key and then, in the
same instant, its first `zwlr_screencopy_frame_v1.copy_with_damage`
request. In wlroots 0.19 every screencopy copy request, with damage or
not, calls `wlr_output_update_needs_frame` ("we can't assume that the
current front buffer is still usable"), so on an idle output sway commits
an output frame at once, 0.1 ms after the key and before the terminal has
drawn anything. The headless backend arms its frame timer on every output
commit, and `output->frame_pending` stays set until it fires
(`backend/headless/output.c`, `types/output/output.c`). The echo commit
arrives 1 to 2 ms later, finds a frame pending, and waits for the timer:
`frame_delay = 1000000 / refresh_mHz` integer milliseconds, so 16 ms at
60 Hz and 4 ms at 240 Hz. Then sway draws it, screencopy reports `ready`,
and the terminal's frame callback completes in the same `handle_frame`.
Every number it produced for a terminal that echoes inside one frame was
therefore "forced frame + one frame timer + composite", whatever the
terminal did.

The probe now sends its first frame request before the key: right after
the baseline grab, so the forced frame and its period pass during the 50
to 300 ms gap the probe already waits (`arm_quietly`). A request that
completes during the gap (wlroots starts each client's damage tracking
full, or something else drew) is replaced, and the key waits until the
latest request is two frame periods old. A flood redraws every frame, so
its requests never get that old; after 0.25 s the latest one is used,
which forces nothing there because a busy output already has its next
frame scheduled. `--arm after-key` (compare.sh `--k2p-arm after-key`)
keeps the old order for comparison. `scripts/bench/test_k2p_wayland.py`
checks the order against a fake compositor (no copy request between the
key and the first frame; early completions re-armed; a busy output still
gives a sample), and fails on the old probe.

### Evidence

**Intervention, same build, only the probe order changed.** Ultty `main`
at 1915884 (release), probe order alternated run by run, idle, `cat`-style
echo, cursor blink off. The AMD rows are headless sway 1.11 (Ubuntu's
package, wlroots 0.19.2) on this workstation (RX 7900 XTX, Mesa,
`WLR_RENDERER=gles2`); the T4 rows are a fresh g4dn.xlarge from the bench
image (Tesla T4, NVIDIA 580.178.04, sway 1.11, wlroots 0.19.1, Ghostty
1.4.0) running the same binaries. p50 / p90 in ms:

| machine | output | terminal | after key (old) | before key (new) | n per arm |
|---|---|---|---:|---:|---:|
| AMD | 60 Hz | Ultty | 16.33 / 16.40 | 1.25 / 1.61 | 40 |
| AMD | 60 Hz | Alacritty 0.16 | 16.28 / 16.34 | 0.67 / 0.84 | 40 |
| AMD | 240 Hz | Ultty | 4.29 / 5.90 | 1.19 / 1.48 | 40 |
| AMD | 240 Hz | Alacritty 0.16 | 4.22 / 4.28 | 0.66 / 0.90 | 40 |
| T4 | 60 Hz | Ultty | 17.88 / 17.94 | 3.71 / 3.79 | 200 |
| T4 | 60 Hz | Ghostty | 36.25 / 36.38 | 25.79 / 26.27 | 200 |
| T4 | 240 Hz | Ultty | 5.80 / 5.88 | 3.67 / 3.80 | 200 |
| T4 | 240 Hz | Ghostty | 13.18 / 18.28 | 12.77 / 16.12 | 200 |

With the old order Ultty and Alacritty read the same 16.3 ms on the AMD
box: the probe could not tell apart terminals that echo within a frame.

**Protocol traces.** sway's own `WAYLAND_DEBUG=server` log (AMD, one key,
times after the virtual keyboard's `key` request):

```
old order                                   new order
+0.03  copy_with_damage (probe)             (copy_with_damage sent 163 ms before the key)
+1.09  wl_surface.commit (Ultty, the echo)  +1.00  wl_surface.commit (Ultty, the echo)
+17.58 screencopy ready, damage             +2.38  screencopy ready, damage
+17.59 wl_callback.done (Ultty)             +2.39  wl_callback.done (Ultty)
```

The output commit the copy request forces is not a protocol message, so it
does not appear; the wlroots source above and the intervention are the
evidence for that step. On the T4, Ultty's own `WAYLAND_DEBUG=client`
trace (`wl-echo-trace.py`, 32 keys per order, `Present mode: Mailbox`)
shows one content commit per key at 1.4 to 1.6 ms (p50) in both orders,
and the frame callback dispatched at 18.6 ms (p50) with the old order
against 4.4 ms with the new. That is the "callback about 16.7 ms after the
commit" reading from the T4 section above, and it moves with the probe,
not with anything Ultty does.

### The questions from the T4 section, answered

- **Is it sway's scheduling?** Not in the sense suspected. wlroots does
  composite an idle headless output at once (the first-numbers note was
  right about that); the probe was keeping it from being idle. On a real
  DRM output the same request would still force a frame, but that frame
  would be paced by vblank, so the bias there would be up to one frame
  rather than exactly one (reasoned from the code, not measured).
- **NVIDIA?** No: the extra frame reproduces on AMD/Mesa, with the same
  size. What NVIDIA adds is about 2.2 ms from Ultty's commit to the
  composited frame (3.7 ms key to frame, commit at 1.4 ms), against 0.3 to
  1.4 ms on the AMD box. Ultty's CPU side is about 0.8 ms a frame in the
  pipeline trace (acquire 0.07, draw 0.55, present 0.13 ms), so the rest is
  GPU time on both sides (Ultty's frame finishing before sway may use the
  buffer, then sway's GLES composite). Not split further here. Ultty's X11
  number on this instance type was 3.4 ms in the earlier runs (not rerun
  here).
- **`max_render_time`?** No effect on this setup, as expected from sway's
  code: it delays rendering relative to the predicted next refresh, which
  on an idle headless output is already in the past. `output *
  max_render_time 10` plus `for_window [title=".*"] max_render_time 5`:
  Ultty 16.31 against 16.33 ms (AMD, old order), 17.86 against 17.85 ms
  (T4, old order); 1.23 against 1.25 ms (AMD) and 3.66 against 3.64 ms
  (T4) with the new order. On a real display it can help a client whose
  commit misses sway's render point, but it is not a fix for anything
  measured here, so it is not recommended as user configuration.
- **Is screencopy itself biased (completing a frame late)?** No: it
  completes on the output commit that carries the damage, and its `ready`
  time matched the terminal's frame callback in the server trace. The bias
  was in when the probe asked.
- **Anything for Ultty to change?** No. Ultty commits once per key with the
  echo, 1 to 2 ms after the key, and the composite then follows at once.
  Tearing (`wp_tearing_control_v1` async presentation) would only shorten
  the vblank wait on a real display, which this setup does not have, and
  sway tears only for a fullscreen view on an output with `allow_tearing`
  (`output_can_tear`); not pursued.

### Corrected numbers

Idle Ultty on headless sway at 60 Hz is **3.7 ms on the T4** (about the
same as X11's 3.4 ms from the earlier T4 runs) and **1.2 ms on the AMD workstation**, not one
frame plus that. Ghostty's idle number drops from 36 to 26 ms at 60 Hz; it
still takes more than a frame, so its second frame is its own (not traced
here). Earlier Wayland k2p results from this probe,
[2026-11-09-wayland-latency.md](2026-11-09-wayland-latency.md) and the T4
table above, include the extra frame and should be read with it
subtracted for idle cases.

Flood rows, 10 MiB/s, T4, p50 / p90 in ms:

| terminal | after key (old) | before key (new) | n per arm |
|---|---:|---:|---:|
| Ultty | 29.37 / 34.97 | 29.81 / 35.00 | 200 |
| Ghostty | 26.86 / 35.25 | 27.87 / 35.38 | 200 |

Under a flood the order makes no difference, as expected: the output is
drawing every frame anyway, so a copy request forces nothing extra. Those
earlier flood numbers stand.

The T4 runs were two launches per terminal and case (`compare.sh --rounds
2`), 100 samples each; the 60 Hz rows are from one campaign that
alternated the two orders, and an earlier campaign gave the same idle
medians (Ultty 3.64 ms new; `max_render_time` rows above).

Raw data stayed on the machines that ran them (not committed); the T4
instance has been terminated.
