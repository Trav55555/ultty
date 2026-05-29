# 2026-05-29 Current Performance And Live Readiness

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-29 |
| Commit | `b3ab0ba77753f0d8e3d204c99dc7d78df94e6dd9` |
| Host | `Aurora` |
| OS | Linux 6.17.0-29-generic x86_64 |
| CPU | AMD Ryzen 5 7600, 6 cores / 12 threads |
| Session | GNOME Wayland with Xwayland available |

## Commands

```bash
timeout 900s cargo bench -p ultty --bench layout -- --noplot
timeout 900s cargo bench -p ultty --bench input_latency -- --noplot
timeout 900s cargo bench -p ultty --bench render -- --noplot
timeout 900s cargo bench -p ultty --bench memory -- --noplot

cargo run -p ultty --bin ultty
scripts/live-smoke-gate.sh /tmp/ultty-issue103-live-smoke-gate

scripts/live-pipeline-trace.sh /tmp/ultty-issue103-live-pipeline-trace
ULTTY_TRACE_TESTS=1,8 scripts/live-pipeline-trace.sh /tmp/ultty-issue103-live-pipeline-trace-throughput-scrollback
```

The benchmark commands completed. The sequential live smoke completed. The
live pipeline trace did not complete; see [Live Readiness](#live-readiness).

## Benchmark Summary

Criterion results are from `target/criterion/*/new/estimates.json`. Values are
mean with 95% confidence interval.

| Benchmark | Mean | 95% CI |
|-----------|------|--------|
| `layout_sync/flat_1000_regions` | 0.256 ms | 0.233-0.281 ms |
| `layout_compute/flat_1000_regions` | 0.038 ms | 0.037-0.039 ms |
| `layout_full_cycle/flexbox_20x10_221_regions` | 0.218 ms | 0.210-0.226 ms |
| `input_latency/single_char_echo` | 0.000029 ms | 0.000029-0.000030 ms |
| `input_latency/line_echo` | 0.000637 ms | 0.000614-0.000663 ms |
| `full_refresh/clear_and_redraw_80x24` | 0.045 ms | 0.044-0.046 ms |
| `wrap_hot_path/500_lines_4000_cols_soft_wrap` | 57.526 ms | 56.482-58.891 ms |
| `wrap_hot_path/25000_lines_80_cols_hard_breaks` | 62.863 ms | 61.816-63.898 ms |
| `ascii_span/current_parser_4m_ascii` | 101.287 ms | 99.877-102.537 ms |
| `ascii_span/production_4m_ascii_span` | 26.741 ms | 26.012-27.482 ms |
| `scroll_history/25000_lines_80_cols_main_scrollback` | 57.550 ms | 56.948-58.177 ms |
| `render_empty/terminal/80x24` | 0.107 ms | 0.105-0.108 ms |
| `render_filled/terminal/200x60` | 1.331 ms | 1.323-1.340 ms |
| `render_large/300x100_filled` | 3.052 ms | 3.031-3.075 ms |
| `memory_usage/terminal_80x24_10k_scrollback` | 0.003 ms | 0.003-0.003 ms |
| `memory_usage/scene_tree_1000_regions` | 0.065 ms | 0.065-0.065 ms |

The memory benchmark also printed these estimates:

| Estimate | Value |
|----------|-------|
| Terminal 80x24 with 10k scrollback | 12.83 MB |
| Terminal 200x60 with 10k scrollback | 32.19 MB |
| Scene tree with 1000 regions | 0.86 MB |
| Glyph atlas 2048x2048 R8 | 4.19 MB |
| Total estimate, terminal + scene + atlas | 17.89 MB |

## Live Readiness

The sequential live smoke gate passed at:

```text
/tmp/ultty-issue103-live-smoke-gate
```

It ran:

- chrome screenshot smoke
- terminal surface smoke
- native rich text smoke
- AppendContent smoke

The full live pipeline trace did not complete. With the default 60s
`stress-repeat` timeout, workload 6 timed out before the `DONE` sentinel. After
raising the traced-live timeout to 180s, the same workload still timed out:

```text
timed out waiting for run 1 to finish; log: /tmp/ultty-stress-repeat.UdUpbY/run-1.log
```

The trace log for the timed-out run is:

```text
/tmp/ultty-issue103-live-pipeline-trace/pipeline.log
```

It captured 346 render frames with mean render-frame work of 0.739 ms and max
render-frame work of 13.579 ms. Near the timeout window, Ultty was scheduling a
render about every 530 ms with frame work mostly between 0.5 ms and 0.9 ms.
That is useful renderer-cadence evidence, but it is not a substitute for
process-level CPU/GPU idle sampling.

A follow-up attempt to run a smaller throughput/scrollback-only trace failed at
launch because Winit could not connect to the Wayland compositor:

```text
Ultty control socket did not become ready.
Trace log: /tmp/ultty-issue103-live-pipeline-trace-throughput-scrollback/pipeline.log
```

The trace log ended with:

```text
Could not find wayland compositor
```

## Interpretation

The benchmark side of the refresh is healthy for the README-level targets:
layout remains below 1 ms for the sampled cases, render remains below 5 ms for
the sampled CPU-side render benches, and memory remains below the 50 MB target
estimate.

The live side is not healthy enough to claim public-alpha live readiness from
this artifact. The renderer smoke path passed, but the live pipeline/soft-wrap
stress path timed out before producing a completed stress report, and explicit
process-level idle CPU/GPU evidence was not captured in this run.

## Caveats

- Criterion values are CPU-side benchmark evidence, not end-to-end interactive
  latency.
- The live trace timeout may represent a workload/runtime regression, an
  environment problem, or a stress harness problem. It should be reduced before
  making a release claim from live pipeline evidence.
- The failed Wayland launch prevented a clean follow-up process-idle sample.
- The logged render cadence during the timed-out run is not the same as GPU
  utilization.

## Recommendation

Keep the benchmark numbers as the current dated benchmark refresh. Do not use
this artifact as public-alpha live-readiness proof until the live pipeline
timeout is diagnosed and a process-level idle sample is captured.
