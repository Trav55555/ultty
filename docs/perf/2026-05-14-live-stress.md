# Live-Tab Stress Analysis - 2026-05-14

Companion to [`2026-05-14-baseline.md`](./2026-05-14-baseline.md). The baseline
covers Criterion microbenchmarks of isolated components; this note pushes
workloads through a **real ultty tab** end-to-end (PTY → ANSI parser → grid →
GPU compositor) and compares the result to the parser micro-numbers to localise
where time goes in the live pipeline.

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-14T17:10-04:00 |
| Commit | `fd6f90177a1b54ea1733104d9605bda7f8d1d900` |
| OS | Linux 6.17.0-23-generic x86_64 |
| CPU | AMD Ryzen 5 7600 6-Core Processor |
| Live tab size | 83 × 34 cells |
| Driver | `ulttyctl new-tab bash scripts/stress.sh` (no focus steal) |

## Reproduce

```bash
# Microbenchmark side (parser ceiling)
cargo bench -p ultty --bench parser -- --quick

# Live-tab side (end-to-end through GPU pipeline)
ulttyctl new-tab bash scripts/stress.sh
# Or, with a timing log for an external watcher:
ULTTY_STRESS_LOG=/tmp/stress.log ulttyctl new-tab bash scripts/stress.sh
```

The script source is at [`scripts/stress.sh`](../../scripts/stress.sh).

## Headline Numbers

End-to-end timings, full pipeline, single run:

| # | Workload | Time | Notes |
|---|----------|------|-------|
| 1 | 4 MiB plain ASCII throughput | 289 ms | ~14 MiB/s end-to-end |
| 2 | Truecolor 64×64 grid (4096 cells) | 31 ms | full SGR per cell |
| 3 | 50k random absolute cursor moves | 170 ms | 3.4 µs / move |
| 4 | 50k truecolor SGR changes | 298 ms | 6.0 µs / change |
| 5 | 2000 mixed-unicode lines (~172 KiB) | 19 ms | see below |
| 6 | 500 lines × 4000 cols (wrap) | 213 ms | reflow-heavy |
| 7 | 200 alt-screen toggles | 6 ms | 30 µs / toggle |
| 8 | 20k scrollback lines | 39 ms | ring append |

Total ≈ 1.07 s for the full battery. Nothing hung; nothing crashed.

## Parser Ceiling vs Live Pipeline

Parser microbenchmark (this commit, `--quick`):

| Input class | Parser throughput |
|-------------|-------------------|
| Escape-heavy (CSI/SGR storms) | ~510 MiB/s |
| Realistic mix | ~340 MiB/s |
| Plain text | ~320 MiB/s |

Live-pipeline plain throughput (test 1): **~14 MiB/s**.

That is a **~23× gap** between the parser ceiling and the live tab. The gap is
*not* parsing — it is PTY read scheduling, grid mutation, damage tracking, and
GPU compositor frame pacing. This is the expected shape for a v-synced GPU
renderer: draining at parser speed would mean producing frames nobody sees.
The relevant question is whether the gap is dominated by useful work
(coalescing into v-sync intervals) or by recoverable overhead — the numbers
below suggest the former.

Cross-check the micro-ops:

| Op | Per-op live cost | Parser bench cost (1 KiB sequence) |
|----|------------------|------------------------------------|
| Cursor move | 3.4 µs | 34 µs / 1 KiB ≈ <1 µs per move |
| Truecolor SGR change | 6.0 µs | 30 µs / 1 KiB ≈ ~2 µs per change |
| Alt-screen toggle | 30 µs | n/a (not benched in isolation) |

The live cost is a few µs higher than the parser-only cost, consistent with
"parser does its work, then the grid mutation + dirty-marking + occasional
flush adds a small constant".

## Unicode Investigation

Test 5 ("2000 unicode lines in 19 ms") looked anomalously fast at first.
First instinct was that off-screen rows were being skipped. Both the data
and the code refute that:

**Code path (no elision):**

- `BufferWithHistory::write_char` (`crates/ultty/src/grid/buffer.rs:416`)
  calls `c.width()` and `self.grid.write_char(c)` unconditionally. There is
  no "is this row visible?" short-circuit.
- The only off-screen-aware logic in the row layer is
  `Row::insert_cells` (`crates/ultty/src/grid/row.rs:145`), which clears
  orphaned wide-char spacers at the edge during insert/delete — that is
  correctness, not elision.
- `BufferWithHistory::newline` and `scroll_grid_up` move displaced rows to
  history but do not change the per-cell write cost.

**Controlled experiment** — same byte volume (~172 KiB) across complexity
classes:

| Payload | Time | Cells per byte (approx) |
|---------|------|-------------------------|
| ASCII (control) | 24 ms | 1.00 |
| Mixed unicode (orig test 5) | 15 ms | ~0.50 |
| CJK only (wide chars) | 23 ms | ~0.65 |
| Emoji only (4-byte, wide) | 25 ms | ~0.50 |
| Combining marks heavy | 23 ms | ~0.20 |

Two conclusions:

1. The original 19 ms was not elision — it was a **24× smaller workload**
   (172 KiB vs the 4 MiB throughput test). Scaling test 5 to 4 MiB would
   land near ~440 ms, in the same ballpark as test 1.
2. **Per-byte cost is roughly flat across unicode complexity.** Combining
   marks at ~0.2 cells/byte cost the same per byte as ASCII at 1.0 cell/byte.
   The pipeline is **parser/byte-bound, not grid-cell-bound** — UTF-8 decode
   plus `unicode-width::UnicodeWidthChar::width()` per char is the dominant
   work in the steady-state hot path. Grid mutation cost is in the noise at
   this volume.

The mixed-unicode 15 ms outlier is within run-to-run jitter at the ~20 ms
scale (a single PTY scheduling hiccup is several ms).

## Implications

For future throughput work, the lever is **parser-side**, not grid-side:

- A char-width lookup table (or precomputed width on UTF-8 decode) would
  remove `unicode-width` dispatch per char.
- SIMD UTF-8 validation / decode (e.g. `simdutf`, `bstr` fast paths) would
  reduce the per-byte cost on the dominant ASCII case.
- Conversely, grid micro-optimisations (cell-write inlining, dirty-region
  packing) are unlikely to move the needle until the per-byte cost is
  lowered first — the cell-write path is already cheap enough that varying
  cells-per-byte by 5× barely moves wall clock.

Wrap (test 6, 213 ms for ~2 MiB of x's) is the only test that looks
"heavier than throughput per byte" and is a candidate hot path for separate
investigation — reflow-on-write is doing real work there.

## Caveats

- Single run per workload; numbers are wall-clock with PTY/scheduler jitter
  in the µs–ms range. Re-run for variance before regressing on individual
  cells.
- The 14 MiB/s end-to-end figure includes whatever frame pacing the GPU
  compositor applies under the test's I/O pattern. Different window
  occlusion / refresh rates will move it.
- `awk` generator overhead is included in wall-clock for tests 2–8. For
  ASCII throughput (test 1) the generator is `yes | head` (kernel-fast)
  so the test 1 number is the cleanest pipeline measurement.
