# Headless VT throughput over the CRLF corpora - 2026-11-07

> **Correction:** the gate's callgrind filter dropped instructions below the
> parser's closure instantiations, so the escape-heavy counts here are low
> (`vt/sgr-dense` was 86 instructions per byte, not 74). See
> [`2026-11-07-unicode-throughput.md`](./2026-11-07-unicode-throughput.md).

First numbers from `ultty-bench vt` (slice 1 of the benchmark suite). Claim
classes: `headless-e2e` (parse + grid) and `micro` (parser only). The wall-clock
numbers were taken on a heavily loaded workstation and are **not** a release
baseline; the instruction counts are load-independent and are the reliable
part of this artifact.

## Environment

| Field | Value |
|---|---|
| Revision | `36f9301` (docs uncommitted, so samples are flagged dirty) |
| CPU | AMD Ryzen 5 7600, pinned to CPUs 5,11 (one core and its SMT sibling) |
| Governor / EPP / boost | `powersave` / `balance_performance` / on |
| Kernel | 7.0.0-42-generic |
| Toolchain | rustc 1.98.1 (Homebrew), release profile (LTO, codegen-units 1) |
| Load average | **35 to 54** during the wall-clock run (the scripts mark anything above 12 as noisy) |
| Build | isolated `RUSTFLAGS="--cfg wt_bench"` so the shared target dir could not supply another checkout's crates |
| Corpus size | 8 MiB per corpus (wall clock), 1 MiB (instruction counts); 64 KiB chunks |

## Commands

```bash
RUSTFLAGS="--cfg wt_bench" cargo build --release -p ultty-bench
scripts/bench/run.sh vt --aa --runs 8 --allow-noisy --bin <copy of ultty-bench> -- \
  --corpus ascii,sgr-dense,unicode-mixed,cjk,emoji-zwj,scroll,cursor-redraw \
  --size-mib 8 --suite vt,parse
scripts/bench/icount-gate.sh --update --bin <copy of ultty-bench> --skip-memory
cargo test -p ultty --bench memory
```

## Summary

Parse + grid at 120x40, main screen with 10k history. Instructions per byte
are from callgrind (exact for the parser, within 0.35% run to run for
parse + grid). MiB/s is the median of 16 samples (8 blocks x 2 arms of the
same binary); "best" is the fastest sample.

| Corpus | Instr/byte (vt) | Instr/byte (parse only) | MiB/s median | MiB/s best |
|---|---:|---:|---:|---:|
| `cursor-redraw` | 28 | 20 | 212 | 237 |
| `ascii` | 84 | 9.6 | 141 | 146 |
| `sgr-dense` | 74 | 44 | 98 | 104 |
| `scroll` (short lines) | 549 | 25 | 17 | 20 |
| `cjk` | 824 | 76 | 10 | 13 |
| `unicode-mixed` | 1011 | 72 | 7.4 | 9.5 |
| `emoji-zwj` | 1403 | 73 | 5.7 | 6.7 |
| `combining` | 1575 | 81 | - | - |
| `alt-screen` | 50 | 39 | - | - |
| `scroll-region` | 39 | 14 | - | - |
| `wide-wrap` | 71 | 8.1 | - | - |

Noise floor (A/A, same binary as both arms): the 95% CI half-width on the
ratio of medians was **21% to 95%** per case at this load, and every CI
contained 1.0. At load 35-54 no wall-clock A/B on this machine can detect
anything smaller than about 20%; use the instruction-count gate instead.

Heap bytes (`cargo test -p ultty --bench memory`, counting allocator; budgets
are these plus 5%):

| Case | Live heap bytes |
|---|---:|
| empty terminal 80x24 | 160,000 |
| empty terminal 300x75 | 990,342 |
| 10k typical lines at 80x24 | 2,600,000 |
| 10k typical lines at 120x40 | 3,097,632 |
| 10k typical lines at 300x75 | 3,810,342 |
| 10k SGR-dense lines at 120x40 | 15,477,632 |
| scene tree, 1000 regions | 907,280 |

## Detail

- Corpus hashes at 1 MiB are pinned in `tools/ultty-bench/src/corpus.rs`
  (`one_mib_hashes_are_pinned`); every sample in `results.jsonl` carries the
  SHA-256 of the exact bytes it ran.
- The instruction baseline is `bench/baselines/icount.json`.
- Parser-only throughput is 120-985 MiB/s across corpora, so for every
  non-trivial corpus the grid side dominates: for `unicode-mixed` the parser
  is 72 of 1011 instructions per byte.

## Interpretation

- **Non-ASCII text is the largest unmeasured cost until now**: 10-17x more
  instructions per byte than ASCII (CJK 824, mixed 1011, emoji 1403, combining
  marks 1575, against 84). This matches the 2026-11-07 probe (about 11x slower
  in wall time) and is almost entirely grid-side work (grapheme clustering and
  width handling per character), not parsing.
- **Short-line scrolling** (`scroll`, numbers of up to six digits per line) costs
  549 instructions per byte: per-line history work dominates when lines are
  short. Long lines amortise it (`ascii` 84, `wide-wrap` 71).
- Cursor-addressed redraws and the alternate screen are cheap (28 and 50),
  as expected with no history.
- The `ascii` corpus here (84-column word lines) runs at 141 MiB/s median under
  load; the probe's 199-207 MiB/s used a different file and a quieter machine,
  so the two are not comparable.

## Caveats

- Load 35-54 on a 6-core machine; wall-clock numbers are noisy, as the A/A
  floor shows. Treat MiB/s as order of magnitude only.
- `powersave` governor and boost on; EPP was not changed (needs root).
- Instruction counts are per toolchain (Homebrew rustc here); the gate refuses
  to compare across rustc or valgrind versions.
- Headless only: no PTY, renderer or present. Live and comparative numbers come
  from `scripts/bench/compare.sh` and need a real GPU to mean anything.
