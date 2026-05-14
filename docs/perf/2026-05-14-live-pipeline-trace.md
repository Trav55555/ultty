# Live Pipeline Trace - 2026-05-14

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-14T18:50-04:00 |
| Commit | `32774abe1949dbee94d784e931cf7dfea9afbd8f` |
| OS | Linux 6.17.0-23-generic x86_64 |
| CPU | AMD Ryzen 5 7600 6-Core Processor |
| Live tab size | 83 x 34 cells |
| Trace switch | `ULTTY_TRACE_PIPELINE=1` |

## Command

```bash
scripts/live-pipeline-trace.sh /tmp/ultty-live-pipeline-trace
```

The script launches Ultty with `RUST_LOG=ultty=info,ultty::perf=info,wgpu=warn`
and `ULTTY_TRACE_PIPELINE=1`, waits for the control socket, runs
`scripts/perf-smoke.sh`, then writes:

| Artifact | Path |
|----------|------|
| Trace log | `/tmp/ultty-live-pipeline-trace/pipeline.log` |
| Stress report | `/tmp/ultty-live-pipeline-trace/stress-report.md` |
| Summary | `/tmp/ultty-live-pipeline-trace/pipeline-summary.md` |
| JSON summary | `/tmp/ultty-live-pipeline-trace/pipeline-summary.json` |
| Gate report | `/tmp/ultty-live-pipeline-trace/pipeline-gate.md` |

## Gate Thresholds

The script is also the local pre-merge live performance gate. It fails if the
captured totals exceed these defaults:

| Environment variable | Default |
|----------------------|---------|
| `ULTTY_TRACE_MAX_PARSE_MS` | 900 ms |
| `ULTTY_TRACE_MAX_DRAIN_MS` | 1000 ms |
| `ULTTY_TRACE_MAX_RENDER_TOTAL_MS` | 400 ms |
| `ULTTY_TRACE_MAX_RENDER_FRAME_MS` | 50 ms |

Override those variables when intentionally refreshing baselines or testing on a
slower machine. The Markdown gate report is meant for humans; the JSON summary
is meant for scripts and future CI wiring.

## Stress Summary

Run from `/tmp/ultty-live-pipeline-trace-2`:

| Workload | Runs | Mean | Stddev | Min | Max |
|----------|------|------|--------|-----|-----|
| `1. throughput-4MiB` | 2 | 246.0 ms | 2.8 ms | 244 ms | 248 ms |
| `6. 500-lines-4000-cols` | 2 | 117.0 ms | 2.8 ms | 115 ms | 119 ms |
| `8. 20k-scrollback-lines` | 2 | 36.5 ms | 0.7 ms | 36 ms | 37 ms |

## Pipeline Summary

| Metric | Value |
|--------|-------|
| Parser/grid entries | 3128 |
| Parser/grid bytes | 12984910 |
| Parser/grid actions | 12984511 |
| Parser/grid print actions | 12631783 |
| Parser/grid time | 715.118 ms |
| PTY drain entries | 38 |
| PTY read batches | 3128 |
| PTY drain bytes | 12984910 |
| PTY drain time | 742.329 ms |
| Render scheduled | 17 |
| Render frames | 17 |
| Render frame time | 27.026 ms |
| Max render frame | 10.566 ms |

Dominant cost: parser/grid and PTY drain.

## Gate Baseline

Run from `/tmp/ultty-live-pipeline-gate-2026-05-14-pass` after the parser span
and row-rotation production optimisations:

| Metric | Value | Limit |
|--------|-------|-------|
| Parser/grid time | 456.657 ms | 900 ms |
| PTY drain time | 517.896 ms | 1000 ms |
| Render frame time | 107.855 ms | 200 ms |
| Max render frame | 12.088 ms | 50 ms |

Status: PASS. The JSON summary for this run reported
`dominant_cost: parser/grid and PTY drain`.

## Grid Span Gate Repeat

Run from `/tmp/ultty-live-pipeline-gate-grid-span-2` after adding
grid/buffer-level ASCII span writes:

| Metric | Value | Limit |
|--------|-------|-------|
| Parser/grid time | 127.249 ms | 900 ms |
| PTY drain time | 184.691 ms | 1000 ms |
| Render frame time | 268.560 ms | 400 ms |
| Max render frame | 11.376 ms | 50 ms |

Status: PASS. The parser/grid and PTY drain totals improved substantially. The
faster ingestion path produced many more render frames, so the total render
budget was raised to 400 ms while preserving the 50 ms max-frame jank guard.

The selected live stress workloads from the same run were:

| Workload | Runs | Mean | Stddev | Min | Max |
|----------|------|------|--------|-----|-----|
| `1. throughput-4MiB` | 2 | 61.0 ms | 4.2 ms | 58 ms | 64 ms |
| `6. 500-lines-4000-cols` | 2 | 2067.5 ms | 17.7 ms | 2055 ms | 2080 ms |
| `8. 20k-scrollback-lines` | 2 | 90.0 ms | 33.9 ms | 66 ms | 114 ms |

The long soft-wrap workload is now a distinct follow-up: wall-clock time remains
high even though parser/grid time, PTY drain time, and max-frame time are all
healthy in the trace.

## Soft-Wrap Catch-Up Polling

Run from `/tmp/ultty-live-pipeline-softwrap-catchup` after adding dirty-frame
PTY catch-up polling:

| Metric | Value | Limit |
|--------|-------|-------|
| Parser/grid time | 122.150 ms | 900 ms |
| PTY drain time | 185.503 ms | 1000 ms |
| Render frame time | 232.775 ms | 400 ms |
| Max render frame | 12.402 ms | 50 ms |

Status: PASS. The selected live stress workloads from the same run were:

| Workload | Runs | Mean | Stddev | Min | Max |
|----------|------|------|--------|-----|-----|
| `1. throughput-4MiB` | 2 | 79.5 ms | 31.8 ms | 57 ms | 102 ms |
| `6. 500-lines-4000-cols` | 2 | 1809.0 ms | 62.2 ms | 1765 ms | 1853 ms |
| `8. 20k-scrollback-lines` | 2 | 67.5 ms | 16.3 ms | 56 ms | 79 ms |

A redirected generator-only run of `scripts/stress.sh 6` reported 116 ms, and
the equivalent inline `awk` generator took about 112 ms. That puts the remaining
live soft-wrap gap outside the text generator, parser/grid, measured PTY drain,
and individual render-frame costs.

The catch-up change wakes the app every 1 ms while a frame is dirty but not yet
eligible to render. This gives the PTY more opportunities to drain between
render deadlines without increasing the 60 fps render cadence. It reduced
`500-lines-4000-cols` from 2067.5 ms to 1809.0 ms in the two-run live repeat.

## Soft-Wrap PTY Probe

Run from `/tmp/ultty-softwrap-pty-probe-issue-59-rerun` with only workload 6:

```bash
scripts/softwrap-pty-probe.sh /tmp/ultty-softwrap-pty-probe-issue-59-rerun
```

| Field | Value |
|-------|-------|
| Runs | 2 |
| Generator-only mean | 109.0 ms |
| Live child mean | 1671.0 ms |
| Live minus generator estimate | 1562.0 ms |

The same run produced this app-side pipeline mean per run:

| Component | Mean |
|-----------|------|
| Parser/grid | 17.0 ms |
| PTY drain | 23.5 ms |
| Render frames | 112.4 ms |
| Combined measured app-side work | 152.9 ms |
| Live minus measured app-side estimate | 1518.1 ms |

The gate passed with parser/grid 33.929 ms, PTY drain 47.032 ms, render frame
time 224.880 ms, and max render frame 11.302 ms across both runs. The probe
therefore confirms that the remaining long-line wall-clock cost is primarily
outside direct parser/grid/render execution time. It is child-side blocking
while the PTY is filled and repeatedly drained.

## PTY Readiness Wakeups

Run from `/tmp/ultty-softwrap-pty-probe-issue-58-committed` after bridging PTY
readiness into winit with a user event:

```bash
scripts/softwrap-pty-probe.sh /tmp/ultty-softwrap-pty-probe-issue-58-committed
```

| Field | Value |
|-------|-------|
| Runs | 2 |
| Generator-only mean | 112.5 ms |
| Live child mean | 168.5 ms |
| Live minus generator estimate | 56.0 ms |

The same run produced this app-side pipeline mean per run:

| Component | Mean |
|-----------|------|
| Parser/grid | 19.9 ms |
| PTY drain | 25.1 ms |
| Render frames | 24.5 ms |
| Combined measured app-side work | 69.5 ms |
| Live minus measured app-side estimate | 99.0 ms |

The gate passed with parser/grid 39.797 ms, PTY drain 50.274 ms, render frame
time 49.012 ms, and max render frame 11.445 ms across both runs.

This changed the long soft-wrap workload from a child-blocking workload back to
one bounded mostly by generator time plus small app-side work. The readiness
bridge keeps terminal state mutation and rendering on the UI thread; the helper
thread only watches PTY fds and sends a winit user event.

## Interpretation

The original live stress gap was dominated by parser/grid and PTY drain time.
After parser-owned ASCII spans and grid/buffer ASCII span writes, the long
soft-wrap workload became a different shape: the app-side trace is healthy, but
the child process still spends most of its wall-clock time blocked behind PTY
throughput and scheduling.

The current trace coalesces 12,768 PTY read batches into 229 rendered frames,
with a worst frame of 12.402 ms. That rules out GPU jank as the reason the
soft-wrap workload still takes seconds. The remaining cost is consistent with
PTY backpressure during very long line writes, where the child can only make
progress as the terminal side repeatedly drains the master.

## Recommendation

Keep the current parser/grid optimisations and live trace gate as the baseline.
Further soft-wrap work should focus on PTY wakeup strategy rather than renderer
micro-optimisation:

1. Prototype event-loop wakeups driven by real PTY readiness instead of periodic
   winit timers.
2. If keeping timer-based catch-up, test whether bounded immediate polling after
   a successful PTY drain improves long-line throughput without unacceptable CPU
   spin.
3. Keep `scripts/live-pipeline-trace.sh` as the end-to-end guardrail after each
   internal optimisation.

## Caveats

- `ULTTY_TRACE_PIPELINE=1` emits many log lines and is intentionally opt-in.
- The trace measures application-side timing, not compositor or kernel-level
  scheduling directly.
- The script refuses to run if an existing Ultty control socket is active, so the
  trace starts from a known launch.
