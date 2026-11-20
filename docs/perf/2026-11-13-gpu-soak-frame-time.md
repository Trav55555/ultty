# GPU soak: frame time (2026-11-13)

Plan item D5, frame-time half. The memory half is in
[2026-11-13-gpu-soak-heap-trim.md](2026-11-13-gpu-soak-heap-trim.md).

A 150-minute live soak with `scripts/bench/soak.sh` on a private headless
sway (`SOAK_SESSION=sway`), Ultty on the discrete GPU: AMD Radeon RX 7900 XT
(RADV NAVI31, render node renderD128). Two tabs of `agent-stream.py`, a
resize every 5 minutes through four window sizes. The run waited for a
quiet machine first (1-minute load under 3 with no compiles for 10
minutes); over the run the host load was 0.9 to 18, median 4.6.

Build: main at `fad7c6b` with `--features heap-stats` (a counting allocator,
two atomic adds per allocation). That build predates #148, which only
changes how keys wait on the PTY reader under a flood; this workload is not
a flood.

## Results

| | |
|---|---|
| frame time, median of per-minute medians | 0.95 ms |
| frame time, median of per-minute p99s | 1.50 ms |
| minutes with p99 over 5 ms | 2 of 150 (5.5 and 7.7 ms) |
| worst minute p99 | 7.68 ms |

Draw time per window size, median of per-minute p99s, first hour after the
30-minute warm-up against the last hour:

| window | first hour | last hour | change |
|---|---:|---:|---:|
| 1000x700 | 1.21 ms | 1.03 ms | -15% |
| 1200x800 | 1.35 ms | 1.10 ms | -19% |
| 1600x1000 | 1.52 ms | 1.26 ms | -17% |
| 1920x1150 | 2.58 ms | 1.44 ms | -44% |

The two slow minutes (52 and 53) coincide with an unrelated cargo build on
the host: 1-minute load went from 6 to 18 at 13:01 and was back under 8 by
13:05. No other minute exceeded 5 ms.

D5's frame-time criterion is met on this GPU: p99 is well under 5 ms, and
draw time per window size fell rather than rose from the first measured
hour to the last. The fall is in line with the host settling; it is not a
claimed speed-up.

Memory in the same run, for reference: anonymous RSS plus swap +0.34
MiB/hour after warm-up (under the 1 MiB/hour limit), live Rust heap -0.26
MiB/hour.

Not measured here: the integrated GPU (the heap-trim soaks used it, but on
a loaded host), and X11.

## Command

```bash
cargo build --release -p ultty --bins --features heap-stats
env -u WAYLAND_DISPLAY SOAK_NO_BUILD=1 SOAK_SESSION=sway \
  WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128 \
  MESA_VK_DEVICE_SELECT=1002:744c MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE=1 \
  scripts/bench/soak.sh 150 <out>
python3 scripts/bench/soak-report.py <out>/samples.csv
```
