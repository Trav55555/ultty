# Performance Artifact Conventions

Use this directory for measurements that support Ultty performance work. Every
artifact should make the result reproducible, say what kind of claim the data
supports, and preserve enough raw detail for later comparison.

## Artifact Types

| Type | Use for | Example |
|------|---------|---------|
| Baseline | Repeatable Criterion or benchmark-suite runs that support README or release claims. | [`2026-05-14-baseline.md`](./2026-05-14-baseline.md) |
| Live stress | End-to-end terminal runs through a real tab, PTY, parser, grid, and compositor. | [`2026-05-14-live-pipeline-trace.md`](./2026-05-14-live-pipeline-trace.md) |
| Investigation | A targeted hot-path note with evidence, hypotheses, and a recommendation. | `YYYY-MM-DD-<topic>.md` |
| Design note | A performance-sensitive design decision that is not yet benchmarked. | [`styled-native-text.md`](./styled-native-text.md) |

## Current Release Gates

Use these gates before publishing performance or chrome claims:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/live-pipeline-trace.sh /tmp/ultty-live-pipeline-trace
scripts/softwrap-pty-probe.sh /tmp/ultty-softwrap-pty-probe
scripts/screenshot-smoke.sh /tmp/ultty-screenshot-smoke
scripts/native-richtext-smoke.sh /tmp/ultty-native-richtext-smoke
scripts/append-content-smoke.sh /tmp/ultty-append-content-smoke
```

The live pipeline and soft-wrap probes require a GUI-capable Ultty run. The
renderer screenshot smokes also require a running Ultty control socket and
capture renderer output through `ulttyctl screenshot`; see
[`../visual-regression.md`](../visual-regression.md).

`scripts/live-pipeline-trace.sh` passes a longer stress timeout to
`scripts/perf-smoke.sh` because live PTY workloads can exceed the generic
60-second smoke default on slower or noisy workstations. Override it with
`ULTTY_TRACE_TIMEOUT_SECS=<seconds>` when collecting a targeted investigation.

The current dated evidence set is:

- [`2026-05-29-current-readiness.md`](./2026-05-29-current-readiness.md) for
  current benchmark refresh, fixed live-pipeline evidence, soft-wrap evidence,
  and process-level idle sample.
- [`2026-05-14-baseline.md`](./2026-05-14-baseline.md) for repeatable benchmark
  baselines.
- [`2026-05-14-ascii-span.md`](./2026-05-14-ascii-span.md) for parser/grid
  hot-path changes.
- [`2026-05-14-live-pipeline-trace.md`](./2026-05-14-live-pipeline-trace.md)
  for live PTY, parser/grid, render, and readiness wakeup evidence.

## Required Sections

New measurement artifacts should include these sections:

1. **Environment**: date, commit, OS, CPU, relevant GPU/window state when known,
   terminal size for live runs, and any benchmark backend details.
2. **Commands**: exact commands run from the repository root.
3. **Summary**: a compact table with the headline numbers.
4. **Detail**: raw benchmark names, timing ranges, repeat counts, variance, or
   the exact workload shape.
5. **Interpretation**: what the result means for the current performance
   question, including whether the measurement supports a release claim, a
   regression gate, or only a local investigation.
6. **Caveats**: scheduler noise, window focus or occlusion, generated input
   overhead, missing GPU instrumentation, benchmark shortcuts, and anything
   else that could move the number.

## Naming

Use date-prefixed filenames for time-bound measurements:

```text
docs/perf/YYYY-MM-DD-<short-topic>.md
```

Keep durable design notes under descriptive names without dates when the file is
expected to evolve with the design.

## Release Claims

Only baseline artifacts should be used directly for README or release claims.
Live stress and investigation notes are useful for localising work and catching
regressions, but they include scheduler, PTY, compositor, and window-manager
noise. Promote a live number to a release claim only after documenting repeat
runs, variance, and the exact hardware/window conditions.

## Raw Output

Prefer preserving raw output inside the artifact when it is short. For longer
runs, store the generated log in `/tmp` during collection and summarise the
stable values in the artifact. Do not commit machine-local Criterion output,
flamegraphs, or temporary logs unless the file is intentionally curated.
