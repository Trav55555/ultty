# GPU soak: memory and the heap trim (2026-11-13)

Plan item D5. Live soaks with `scripts/bench/soak.sh` on a private headless
sway (`SOAK_SESSION=sway`), Ultty on the Ryzen 5 7600's integrated GPU
(RADV RAPHAEL_MENDOCINO, render node renderD129), built with
`--features heap-stats` so the live Rust heap is counted next to the
process's anonymous memory. Two tabs of `agent-stream.py`, a resize every
5 minutes. Host load 5 to 46 (median 21) from unrelated jobs.

## Why the earlier soak failed

The 150-minute run on 2026-11-12 failed the memory check (+2.46 MiB/hour):
anonymous memory rose for about an hour, held flat, then stepped up 1.8
MiB once. The process map showed the main heap at 23.5 MiB with about
12 MiB of Rust heap live. glibc returns memory to the OS only from the
top of an arena; scrollback freed by `/clear` or a reflow sat below live
blocks and stayed resident, so RSS followed the highest scrollback
reached, in steps. Not a leak in Ultty's data.

## The fix

`ultty::heap_trim` (#137): `/clear` (scrollback shrinking by 1,000+ lines
in one read), every resize and every closed terminal request a
`malloc_trim(0)`, which the main loop runs after a frame, at most every
2 s.

## Results (180 minutes each, run side by side)

| | without trim | with trim |
|---|---|---|
| anonymous RSS + swap after a 60-minute warm-up | 41.7 MiB | 26.8 MiB |
| at the end | 41.7 MiB | 19.9 MiB |
| peak | 44.0 MiB | 30.2 MiB |
| trend after 60-minute warm-up | -0.02 MiB/h | -0.04 MiB/h |
| trend after 30-minute warm-up (the report's default) | +0.91 MiB/h | +0.03 MiB/h |
| live Rust heap trend | -0.11 MiB/h | -0.13 MiB/h |
| CPU over the run | 871 s | 875 s |

Live Rust heap is flat in both (10 to 14 MiB, moving with scrollback and
`/clear`), matching the `agent_session_10h` memory-bench case (10.6 to
11.1 MB ceiling over ten simulated hours). With the trim, resident memory
follows the live heap and passes the default check with no extra CPU.

Frame and draw times from these runs are not usable: two soaks ran at
once on a loaded host, and draw p99 per window size swung by up to 3x
between hours in both runs. D5's frame-time criterion still needs a
GPU run on an idle machine.

## Commands

```bash
cargo build --release -p ultty --bins --features heap-stats
env -u WAYLAND_DISPLAY SOAK_NO_BUILD=1 SOAK_SESSION=sway \
  WLR_RENDER_DRM_DEVICE=/dev/dri/renderD129 \
  MESA_VK_DEVICE_SELECT=1002:164e MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE=1 \
  scripts/bench/soak.sh 180 <out>
python3 scripts/bench/soak-report.py <out>/samples.csv --warmup 60
```
