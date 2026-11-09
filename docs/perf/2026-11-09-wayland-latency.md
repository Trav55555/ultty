# Wayland keypress-to-pixel latency on headless sway (2026-11-09)

The Wayland latency probe (`scripts/bench/k2p-wayland.py`) had never
produced a sample: every run on the T4 failed for every terminal with "no
pixels changed after a key press" (see
[2026-11-08-t4-comparison.md](2026-11-08-t4-comparison.md)). This note
records why, the rewritten probe, its resolution, and the first Wayland
numbers for Ultty and Ghostty, with X11 from the same run for reference.

## Environment

AWS g4dn.xlarge (Tesla T4, NVIDIA 580.178.04, 4 vCPU), Ubuntu 26.04.1,
kernel 7.0.0-1016-aws. Wayland: private headless sway 1.11 (wlroots 0.19.1,
`WLR_RENDERER=gles2`, `--unsupported-gpu`), one 2560x1440 output at 60 Hz,
scale 1. X11: headless Xorg on the GPU with openbox. Ultty `866fa0f2da94`
(`main` at `d47629f` plus the probe change; no Ultty source differs from
`main`), release build. Ghostty 1.4.0. 120x40 cells, DejaVu Sans Mono 11,
cursor blink off. Claim class `live-realgpu`.

## Commands

```bash
scripts/bench-aws.sh --host <ip> --user ubuntu --key <key.pem> --known-hosts <file> \
  --role gpu --ref wayland-k2p-probe --skip-setup \
  --compare-args "--terminals ultty,ghostty --idle-secs 0 --tabs 1 --size-mib 4 --repeat 1 --corpora ascii"
```

Default `--rounds 3 --k2p-samples 200`, so 600 samples per terminal and case,
from three launches per terminal with the terminal order shuffled per round.
Throughput, idle and tab measurements were cut down to keep the run short and
are not reported here. Both sessions ran with no failures.

## What was wrong

A headless sway has no input devices (`WLR_LIBINPUT_NO_DEVICES=1`), so
`swaymsg -t get_seats` reports `"capabilities": 0`: the seat has no keyboard.
The old probe ran one `wtype` process per key. `wtype` creates a virtual
keyboard, which gives the seat a keyboard capability, sends the key at once,
and destroys the keyboard on exit. The terminal only creates its
`wl_keyboard` after it sees the capability appear, by which time the key has
been delivered to nobody. Checked directly on the instance with Ultty running
`ubench-echo`: `wtype x` left the echo cell unchanged, `wtype -s 300 o` (sleep
300 ms after creating the keyboard) changed it, and `get_seats` showed
capabilities 0 before and 2 (keyboard) while a `wtype -s 1500` was alive.
Focus, output name, geometry and scale were all correct; the probe never got
far enough for `grim`'s cost to matter.

## The probe now

`k2p-wayland.py` is a small Wayland client written against the wire protocol
(Python standard library only, no new packages on the instance):

- **Input:** one `zwp_virtual_keyboard_v1` for the whole run, with a keymap
  holding only the keys it sends. After creating it the probe focuses the
  window and waits one second so the terminal binds `wl_keyboard`. Each
  sample sends press and release in one write.
- **Output:** `zwlr_screencopy_manager_v1.capture_output_region` on the echo
  cell, into `wl_shm` buffers, on the same connection. After the key the probe
  asks for `copy_with_damage`, which completes on the next output frame with
  damage, so waiting does not force extra frames. The next frame request is
  armed before the current one is inspected.
- **Clock:** a sample ends at the screencopy `ready` timestamp, the time the
  compositor committed the output frame that first shows the change. It is
  CLOCK_MONOTONIC, the clock the start is read from (the probe checks that
  start <= ready <= receipt and stops if not). The probe's read-back cost is
  therefore outside the number. The time the probe received the frame is
  recorded as `received_ms`; it was 0.3 ms later at the median (worst case
  5.2 ms) across all 3,600 Wayland samples. Nothing is subtracted from
  either.
- **Calibration** is as before: diff the bottom quarter of the window around
  a key, then check a second key changes the same box. It now tries both
  keys, and every sample sends the key the cell is not already showing.
- **Pacing:** the baseline grab before each sample forces an output frame,
  so the key is sent 50 to 300 ms (random) after it, never inside that
  frame's period.

### Resolution and what the number includes

The compositor only makes pixels when it composites an output frame, so a
change can only be seen at a frame boundary: the resolution is one output
frame, recorded with each sample as `frame_ms` (16.67 ms at 60 Hz) and as
`resolution_ms`. The timestamp itself is exact to well under a millisecond.

The headless output has no vblank. wlroots composites on demand: when the
output has been idle the next commit is composited at once, and after a
composite the next one waits a full frame period. So on headless sway,
latency comes in whole frame periods plus the terminal's own work, with
almost no phase noise; on a real 60 Hz display each frame wait would instead
be 0 to 16.7 ms depending on where the key lands relative to vblank, plus
scanout. Treat these numbers as "how many compositor frames does the echo
take, plus work", not as photon latency.

## Summary (60 Hz, n=600 per cell)

| case | Ultty Wayland p50 / p90 / p99 | Ghostty Wayland p50 / p90 / p99 | Ultty X11 p50 / p99 | Ghostty X11 p50 / p99 |
|---|---:|---:|---:|---:|
| idle | **17.9** / 17.9 / 18.0 ms | 36.1 / 36.2 / 38.7 ms | **3.41** / 3.62 ms | 22.5 / 30.5 ms |
| 10 MiB/s flood | 29.1 / 34.8 / 36.1 ms | **27.4** / 35.0 / 38.0 ms | **5.28** / 7.31 ms | 14.9 / 25.8 ms |
| max flood | **31.1** / 52.7 / 84.0 ms | 41.1 / 51.4 / 65.9 ms | **21.7** / 23.7 ms | 33.2 / 53.2 ms |

Ratios from `analyze.py` (Ghostty / Ultty, bootstrap 95% CI, Mann-Whitney p):

| session | case | ratio | 95% CI | p | verdict |
|---|---|---:|---|---:|---|
| Wayland | idle | 2.02 | [2.020, 2.021] | 3e-182 | Ultty faster |
| Wayland | flood-10 | 0.94 | [0.910, 0.982] | 0.026 | no detectable change (inside the 5% threshold) |
| Wayland | flood-max | 1.32 | [1.286, 1.361] | 4e-27 | Ultty faster |
| X11 | idle | 6.62 | [6.422, 6.749] | 1e-197 | Ultty faster |
| X11 | flood-10 | 2.82 | [2.693, 2.935] | 4e-185 | Ultty faster |
| X11 | flood-max | 1.53 | [1.484, 1.588] | 5e-118 | Ultty faster |

X11 poll resolution (`resolution_us`) was 34 to 41 us. The X11 numbers match
the 2026-11-08 T4 run (Ultty idle 3.56 ms, Ghostty 22.4 ms).

## Detail

Wayland p10 / p50 / p90 in ms:

| case | Ultty | Ghostty |
|---|---|---|
| idle | 17.78 / 17.86 / 17.94 | 35.96 / 36.09 / 36.22 |
| flood-10 | 21.06 / 29.11 / 34.78 | 21.33 / 27.41 / 35.00 |
| flood-max | 22.49 / 31.14 / 52.74 | 29.99 / 41.12 / 51.43 |

Spot check at 240 Hz (`compare.sh --session wayland --wayland-refresh 240
--rounds 1 --k2p-samples 200`, same build, frame 4.17 ms), medians:

| case | Ultty | Ghostty |
|---|---:|---:|
| idle | 5.76 ms | 13.68 ms |
| flood-10 | 5.65 ms | 10.52 ms |
| flood-max | 21.44 ms | 22.59 ms |

Idle latency scales with the frame period: Ultty is one frame plus about
1.6 ms at 240 Hz (one frame plus 1.2 ms at 60 Hz), and Ghostty three frames
plus 1.2 ms at 240 Hz (two frames plus 2.8 ms at 60 Hz). That confirms the
Wayland numbers are counted in compositor frames.

Raw data: `results.jsonl`, `report.md` and probe logs per session under
`target/bench-results/aws-k2p-20261109/20261109-163629-866fa0f2da94/` on the
machine that ran the campaign (not committed).

## Interpretation

- The Wayland probe works for both terminals and all three cases, with no
  timeouts in 3,600 samples.
- At idle on Wayland, Ultty's echo appears one compositor frame after the
  key (16.7 ms + 1.2 ms) and Ghostty's two frames after (33.3 ms + 2.8 ms).
  On X11, with no compositor in the path, Ultty shows the echo in 3.4 ms.
  Ultty therefore spends a whole compositor frame before the echo is on
  screen. A likely cause, not yet confirmed with a protocol trace: the first
  frame Ultty presents after the key does not yet contain the echo (it is
  drawn before the PTY output arrives), and the frame that does has to wait
  for the compositor's frame callback. A `WAYLAND_DEBUG=client` trace of one
  keypress would settle it, and if it holds, Ultty's idle Wayland latency
  could drop to about 1 ms plus the composite on this setup.
- Under a 10 MiB/s flood the two are level on Wayland; at maximum flood Ultty
  is ahead at the median but has the longer tail (p99 84 ms against 66 ms).
- None of this is a release claim: it is a headless compositor with no
  vblank, one run on one instance.

## Caveats

- Headless sway composites on demand, not on a vblank grid, so the idle
  numbers have almost no spread and real displays add up to one frame of
  phase noise per frame waited, plus scanout.
- `copy_with_damage` completes on any damaged frame; the probe compares the
  cell's pixels each time, so a frame damaged elsewhere only costs another
  round, but if the probe ever fell more than a frame behind it would report
  a later frame than the first one showing the change. The receipt lag
  (0.3 ms median, 5.2 ms worst) shows it stayed well inside a frame.
- The virtual keyboard's keymap replaces the terminal's keymap for the run;
  both terminals receive the same keymap.
- Ghostty's new-tab shortcut in `compare.sh` (tab scaling) still uses a
  one-shot `wtype` with modifiers. It was not changed here.
