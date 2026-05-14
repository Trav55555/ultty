# Wrap And Reflow Investigation - 2026-05-14

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-14T17:45-04:00 |
| Commit | `a0ecf3f609f71d7f58562f92791061b957a235b0` |
| OS | Linux 6.17.0-23-generic x86_64 |
| CPU | AMD Ryzen 5 7600 6-Core Processor |
| Benchmark backend | Criterion with plotters fallback; `gnuplot` was not installed |

## Commands

```bash
timeout 900s cargo bench -p ultty --bench input_latency -- wrap_hot_path --quick --noplot
scripts/stress-repeat.sh --runs 3 --out docs/perf/2026-05-14-live-stress-repeat.md
```

## Summary

| Workload | Result |
|----------|--------|
| Live tab `500-lines-4000-cols` | mean 214.0 ms, stddev 18.2 ms |
| Internal parser/grid `500_lines_4000_cols_soft_wrap` | 75.611 ms to 77.149 ms |
| Internal parser/grid `25000_lines_80_cols_hard_breaks` | 108.84 ms to 109.16 ms |
| Resize `80 -> 120` after 500 wrapped lines | 9.6816 ms to 10.072 ms |

## Detail

The live stress workload writes 500 lines of 4000 ASCII `x` characters, forcing
soft wrap repeatedly at terminal width. The repeat runner reported:

```text
`6. 500-lines-4000-cols` | 3 | 214.0 ms | 18.2 ms | 203 ms | 235 ms | 32 ms
```

The new Criterion group in `crates/ultty/benches/input_latency.rs` isolates the
same payload inside the parser/grid path:

```text
wrap_hot_path/500_lines_4000_cols_soft_wrap
                        time:   [75.611 ms 76.841 ms 77.149 ms]
wrap_hot_path/25000_lines_80_cols_hard_breaks
                        time:   [108.84 ms 108.90 ms 109.16 ms]
wrap_hot_path/resize_reflow_500_wrapped_lines_80_to_120
                        time:   [9.6816 ms 9.7596 ms 10.072 ms]
```

## Interpretation

The live long-line workload is not dominated by resize reflow. Reflowing an
already-wrapped 500-line buffer from 80 to 120 columns costs roughly 10 ms, which
is small compared with the 214 ms live-tab wall clock.

The pure parser/grid soft-wrap case is also much faster than the live number at
roughly 77 ms. That leaves about 137 ms in PTY scheduling, tab/process launch,
renderer/compositor pacing, dirty presentation, and measurement overhead.

The hard-break control is slower than soft-wrap for the same byte scale. That
points away from soft-wrap bookkeeping as the main local bottleneck and toward
scroll/history work as the heavier internal path when every 80 cells becomes an
explicit line break.

## Recommendation

Do not optimise resize reflow first. The measured cost is already bounded enough
for interactive window resizes.

For throughput work, keep the next slices focused on:

1. Parser/character-width cost on steady ASCII input.
2. Scroll/history append cost for newline-heavy workloads.
3. Live pipeline frame/drain pacing, using `scripts/stress-repeat.sh` to guard
   against scheduler noise.

## Caveats

- Criterion setup for the resize benchmark constructs the wrapped buffer outside
  the timed resize path, so the resize number is specifically the reflow cost.
- The live run includes generator, PTY, renderer, compositor, and scheduler
  effects; compare it to Criterion as localisation evidence, not as identical
  workload timing.
- The Criterion run used `--quick`; re-run without it before using these values
  as long-lived release claims.
