# Batched ASCII Span Prototype - 2026-05-14

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-14T18:55-04:00 |
| Commit | `af166ac7b7f143cd78ac50ef8a72d4cf923a7692` |
| OS | Linux 6.17.0-23-generic x86_64 |
| CPU | AMD Ryzen 5 7600 6-Core Processor |
| Benchmark backend | Criterion with plotters fallback; `gnuplot` was not installed |

## Commands

```bash
timeout 900s cargo bench -p ultty --bench input_latency -- ascii_span --noplot
```

## Prototype

The candidate is benchmark-only. It scans contiguous printable ASCII bytes and
writes those characters to the terminal directly, while delegating controls,
escapes, and non-ASCII bytes back through the existing `AnsiParser`.

This intentionally does not change production behavior. A production version
should add a parser-level span action instead of duplicating parser decisions in
the grid layer.

## Results

| Benchmark | Result |
|-----------|--------|
| `ascii_span/current_parser_4m_ascii` | 202.21 ms to 223.29 ms |
| `ascii_span/candidate_4m_ascii_span` | 186.46 ms to 203.46 ms |
| `ascii_span/current_parser_25k_hard_lines` | 135.48 ms to 150.17 ms |
| `ascii_span/candidate_25k_hard_lines` | 137.10 ms to 150.12 ms |

## Production Follow-Up

The production slice added a parser-owned borrowed ASCII span path:
`AnsiParser::feed_with_ascii_spans`. The regular `feed` API remains unchanged.
The terminal event loop now uses the span-aware path for PTY input, and the span
benchmark compares the unchanged per-action path against the production path.

Command:

```bash
timeout 900s cargo bench -p ultty --bench input_latency -- ascii_span --noplot
```

| Benchmark | Result |
|-----------|--------|
| `ascii_span/current_parser_4m_ascii` | 151.93 ms to 155.31 ms |
| `ascii_span/production_4m_ascii_span` | 144.22 ms to 146.90 ms |
| `ascii_span/current_parser_25k_hard_lines` | 108.18 ms to 111.21 ms |
| `ascii_span/production_25k_hard_lines` | 103.56 ms to 105.96 ms |

The production path improves the same-run 4 MiB ASCII benchmark by roughly 5%
at the midpoint. The 25k hard-line guardrail also improves slightly, which means
the borrowed parser span dispatch does not introduce the scroll-heavy regression
risk seen in the benchmark-only shortcut caveats.

## Grid Span Follow-Up

The next production slice added `Grid::write_ascii_span` and
`Buffer::write_ascii_span`, so parser-owned ASCII spans no longer loop through
the public single-character write path in the handlers. The fast path writes
row-local ASCII runs directly, while falling back to `write_char` for insert
mode and no-autowrap cases.

Command:

```bash
timeout 900s cargo bench -p ultty --bench input_latency -- ascii_span --noplot
```

| Benchmark | Result |
|-----------|--------|
| `ascii_span/current_parser_4m_ascii` | 100.86 ms to 102.70 ms |
| `ascii_span/production_4m_ascii_span` | 26.207 ms to 26.888 ms |
| `ascii_span/current_parser_25k_hard_lines` | 55.173 ms to 55.690 ms |
| `ascii_span/production_25k_hard_lines` | 18.774 ms to 19.540 ms |

This is the first span result that removes most of the per-cell overhead from
plain ASCII output. The current-parser benchmark also improved because the
previous row-rotation work reduced scroll cost, but the production span path is
now roughly 3.8x faster than the per-action parser path on the 4 MiB ASCII
workload in the same run.

## Interpretation

The span candidate improves the pure ASCII throughput shape by roughly 8-10% at
the midpoint. That suggests `Action::Print(char)` callback/action dispatch is a
real cost on long printable streams.

The hard-line case does not improve. That workload is dominated by newline,
scroll, history append, and row movement; batching printable spans does not move
the cost once every 80 cells becomes a scrollback operation.

## Recommendation

Adopt the idea, but not this benchmark implementation.

The next production slice should add a parser-owned action such as
`Action::PrintAsciiSpan` or a borrowed callback path that emits printable ASCII
runs only when the parser is in the normal printable state. It must fall back to
per-character actions for:

1. DEC special graphics character sets.
2. UTF-8 and combining marks.
3. Escape, CSI, OSC, DCS, and C0 control handling.
4. Insert mode, wide characters, and wrap-pending edge cases.

Use `ascii_span/current_parser_4m_ascii` and `ascii_span/candidate_4m_ascii_span`
as the proof target. Use the hard-line case as a guardrail: a span implementation
that helps throughput but regresses scroll-heavy input is not worth landing.

## Caveats

- The candidate is a benchmark shortcut, not a production parser design.
- It only proves that batching has headroom on long printable ASCII runs.
- It does not remove grid per-character work, so the measured win is a lower
  bound for a deeper parser/grid batch API.
