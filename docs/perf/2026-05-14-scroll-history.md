# Scroll And History Investigation - 2026-05-14

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-14T18:35-04:00 |
| Commit | `bad48f2e18e1eb0e8b48a2adcd0e45144f789afa` |
| OS | Linux 6.17.0-23-generic x86_64 |
| CPU | AMD Ryzen 5 7600 6-Core Processor |
| Benchmark backend | Criterion with plotters fallback; `gnuplot` was not installed |

## Commands

```bash
timeout 900s cargo bench -p ultty --bench input_latency -- scroll_history --noplot
```

## Summary

| Workload | Result |
|----------|--------|
| `25000_lines_80_cols_main_scrollback` | 105.10 ms to 106.77 ms |
| `25000_lines_80_cols_alt_no_scrollback` | 89.644 ms to 91.347 ms |
| `25000_lines_12_cols_main_scrollback` | 49.605 ms to 51.583 ms |

## Interpretation

The same 25k hard-line payload is roughly 15 ms slower on the main screen than
on the alternate screen. That isolates a measurable scrollback/history cost
inside the parser/grid path, separate from PTY and compositor noise.

The short-line control is much faster at the same line count, so byte volume and
per-character parser/grid work still matter. The result is not "history is the
only bottleneck"; it is "history append and row movement are real enough to
measure and worth a focused optimisation pass."

## Recommendation

The next implementation slice should look at full-screen scrollback append and
row movement before resize reflow. Good candidates:

1. Avoid cloning more row state than needed when saving scrolled-off rows.
2. Reuse row storage when rotating visible rows during full-screen scroll.
3. Separate dirty-row marking from history append so off-screen history work does
   not inflate visible damage.

Use `scroll_history/25000_lines_80_cols_main_scrollback` as the regression probe
and compare it against the alternate-screen no-scrollback case after each change.

## Caveats

- The alternate-screen benchmark includes the enter-alt-screen escape sequence,
  but that overhead is tiny relative to the 25k-line payload.
- This is still an internal parser/grid benchmark. Live stress remains necessary
  before claiming user-visible throughput improvement.
