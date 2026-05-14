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
| `ULTTY_TRACE_MAX_RENDER_TOTAL_MS` | 200 ms |
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

## Interpretation

The live stress gap is not dominated by GPU render time in this workload. Render
frames accounted for roughly 27 ms total, while parser/grid work accounted for
roughly 715 ms and total PTY drain accounted for roughly 742 ms.

The trace also shows frame pacing doing the right broad thing: thousands of PTY
read batches and parse/grid chunks are coalesced into 17 rendered frames. That
is why live wall-clock time remains much higher than isolated parser
microbenchmarks without implying the renderer is the main bottleneck.

## Recommendation

Prioritise parser/grid throughput before renderer work:

1. Add a production parser-owned ASCII span action or callback, using
   [`2026-05-14-ascii-span.md`](./2026-05-14-ascii-span.md) as the proof target.
2. Optimise main-screen scrollback append and row movement, using
   [`2026-05-14-scroll-history.md`](./2026-05-14-scroll-history.md) as the probe.
3. Keep `scripts/live-pipeline-trace.sh` as the end-to-end guardrail after each
   internal optimisation.

## Caveats

- `ULTTY_TRACE_PIPELINE=1` emits many log lines and is intentionally opt-in.
- The trace measures application-side timing, not compositor or kernel-level
  scheduling directly.
- The script refuses to run if an existing Ultty control socket is active, so the
  trace starts from a known launch.
