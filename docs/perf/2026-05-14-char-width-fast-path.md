# Character Width Fast-Path Prototype - 2026-05-14

## Environment

| Field | Value |
|-------|-------|
| Date | 2026-05-14T18:05-04:00 |
| Commit | `d8b822b729289786b36fe5760f731b1cd0507e22` |
| OS | Linux 6.17.0-23-generic x86_64 |
| CPU | AMD Ryzen 5 7600 6-Core Processor |
| Benchmark backend | Criterion with plotters fallback; `gnuplot` was not installed |

## Commands

```bash
timeout 900s cargo bench -p ultty --bench input_latency -- char_width --quick --noplot
timeout 900s cargo bench -p ultty --bench input_latency -- input_latency/single_char_echo --quick --noplot
cargo check -p ultty --benches
```

## Prototype

The candidate fast path was intentionally kept in the benchmark harness instead
of the production parser/grid path:

```rust
fn ascii_width_candidate(c: char) -> u16 {
    let codepoint = c as u32;
    if (0x20..=0x7e).contains(&codepoint) {
        1
    } else if codepoint < 0x20 || codepoint == 0x7f {
        0
    } else {
        c.width().unwrap_or(0) as u16
    }
}
```

This preserves the current production behavior while keeping the measurement
repeatable.

## Results

| Benchmark | Result |
|-----------|--------|
| `char_width/ascii_candidate_fast_path` | 36.485 ns to 36.588 ns |
| `char_width/ascii_unicode_width_direct` | 24.254 ns to 24.531 ns |
| `char_width/mixed_fallback_path` | 39.821 ns to 40.232 ns |
| `input_latency/single_char_echo` | 32.752 ns to 32.923 ns |

## Interpretation

The ASCII candidate is slower than the existing direct `unicode-width` call in
this microbenchmark. The likely explanation is that `unicode-width` already has
a highly optimised ASCII path, and adding another branch in front of it does not
remove meaningful work.

The parser/grid single-character echo benchmark also shows no statistically
significant change from the earlier baseline. That confirms there is no obvious
production win to take from this candidate.

## Decision

Do not adopt the ASCII width fast path in production.

Keep the `char_width` Criterion group as a regression probe for future width
experiments. Future work should try a different shape before touching the hot
path, for example:

1. Batch ASCII spans before per-character width checks.
2. Store width alongside decoded parser actions.
3. Measure UTF-8 decode and action dispatch before revisiting single-character
   width lookup.

## Caveats

- The run used Criterion `--quick`; this is enough to reject the candidate, but
  not enough for a durable release claim.
- The candidate benchmark measures width lookup directly. It does not model all
  branch-prediction effects in full terminal workloads.
