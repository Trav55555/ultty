# 2026-05-29 Current Performance And Live Readiness

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-29 |
| Commit | `b00dd225bff4967149f174e6dfcab4f85af7ef60` plus the stress driver fix committed with this artifact |
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

scripts/live-pipeline-trace.sh /tmp/ultty-issue103-live-pipeline-trace-fixed
scripts/softwrap-pty-probe.sh /tmp/ultty-issue103-softwrap-pty-probe-fixed
```

The benchmark commands completed. The sequential live smoke completed. The
live pipeline trace and soft-wrap PTY probe completed after fixing the live
stress driver to use absolute `env` and `bash` paths.

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

The first live pipeline attempt timed out because the live stress driver used
`new-tab env ... bash ...`, which relies on PATH lookup inside the PTY child.
In this launch environment that exited with status 127 before producing the
`DONE` sentinel. `scripts/stress-repeat.sh` now resolves absolute paths for
`env` and `bash`, and preserves its temp directory when a timeout occurs.

After that fix, the live pipeline trace passed at:

```text
/tmp/ultty-issue103-live-pipeline-trace-fixed
```

Pipeline summary:

| Metric | Value |
|--------|-------|
| Parser/grid entries | 14302 |
| Parser/grid bytes | 12984913 |
| Parser/grid actions | 529837 |
| Parser/grid print actions | 12631786 |
| Parser/grid time | 150.425 ms |
| PTY drain entries | 382 |
| PTY read batches | 14302 |
| PTY drain bytes | 12984913 |
| PTY drain time | 228.296 ms |
| Render scheduled | 33 |
| Render frames | 33 |
| Render frame time | 54.056 ms |
| Max render frame | 12.591 ms |

Pipeline gate:

| Metric | Value | Limit | Status |
|--------|-------|-------|--------|
| Parser/grid time | 150.425 ms | 900.000 ms | PASS |
| PTY drain time | 228.296 ms | 1000.000 ms | PASS |
| Render frame time | 54.056 ms | 400.000 ms | PASS |
| Max render frame | 12.591 ms | 50.000 ms | PASS |

Live stress repeat summary:

| Workload | Runs | Mean | Stddev | Min | Max | Spread |
|----------|------|------|--------|-----|-----|--------|
| `1. throughput-4MiB` | 2 | 68.5 ms | 12.0 ms | 60 ms | 77 ms | 17 ms |
| `6. 500-lines-4000-cols` | 2 | 145.0 ms | 1.4 ms | 144 ms | 146 ms | 2 ms |
| `8. 20k-scrollback-lines` | 2 | 37.0 ms | 1.4 ms | 36 ms | 38 ms | 2 ms |

The soft-wrap PTY probe passed at:

```text
/tmp/ultty-issue103-softwrap-pty-probe-fixed
```

Soft-wrap summary:

| Field | Value |
|-------|-------|
| Runs | 2 |
| Generator-only mean | 123.0 ms |
| Live child mean | 147.5 ms |
| Live minus generator estimate | 24.5 ms |
| Parser/grid mean per run | 21.9 ms |
| PTY drain mean per run | 27.9 ms |
| Render-frame mean per run | 23.5 ms |
| Combined measured app-side work | 73.3 ms |
| Live minus measured app-side estimate | 74.2 ms |
| PTY read batches | 1002 |
| Render frames | 24 |
| Max render frame | 13.146 ms |

## Idle Evidence

Methodology: launch a GUI Ultty instance, wait for the control socket to become
ready, allow the process to settle, then sample the Ultty process with:

```bash
ps -p <ultty-pid> -o pid=,pcpu=,rss=,etime=,stat=,comm=
```

The sample used 12 readings spaced 5 seconds apart. The process had been up
for more than seven minutes before the first recorded sample.

| Sample | `%CPU` | RSS |
|--------|--------|-----|
| 1 | 0.3 | 107960 KB |
| 2 | 0.3 | 107960 KB |
| 3 | 0.3 | 107960 KB |
| 4 | 0.3 | 107960 KB |
| 5 | 0.3 | 107960 KB |
| 6 | 0.3 | 107960 KB |
| 7 | 0.3 | 107960 KB |
| 8 | 0.3 | 107960 KB |
| 9 | 0.3 | 107960 KB |
| 10 | 0.3 | 107960 KB |
| 11 | 0.3 | 107960 KB |
| 12 | 0.3 | 107960 KB |

This is process-level idle evidence only. It does not measure GPU utilization.
The RSS is higher than the synthetic memory estimate because it includes the
debug build process image, renderer/windowing allocations, loaded libraries,
and runtime state, not only terminal data structures.

## Interpretation

The benchmark side of the refresh is healthy for the README-level targets:
layout remains below 1 ms for the sampled cases, render remains below 5 ms for
the sampled CPU-side render benches, and memory remains below the 50 MB target
estimate.

The live side is now healthy enough for current public-alpha readiness evidence:
the sequential renderer/native smoke passed, the live pipeline gate passed,
the soft-wrap PTY probe passed, and process-level idle CPU stayed at 0.3% for
the sampled minute.

## Caveats

- Criterion values are CPU-side benchmark evidence, not end-to-end interactive
  latency.
- The fixed live stress driver should be kept because relative `env`/`bash`
  lookup in spawned PTY commands can fail depending on the child environment.
- The idle sample measures process CPU and RSS only, not GPU utilization.
- The RSS sample came from a debug build and should not be used as a release
  package memory claim.

## Recommendation

Use this artifact as the current dated benchmark and live-readiness refresh for
public-alpha decisions. Repeat it for release candidates or after changes to
PTY draining, parser/grid hot paths, renderer scheduling, or live smoke
orchestration.
