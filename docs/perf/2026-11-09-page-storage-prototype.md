# Page storage prototype: ADR 0022's go/no-go gate (2026-11-09)

[ADR 0022](../adr/0022-page-storage.md) proposes 8-byte packed cells with
interned styles, a screen page with slot rotation, and append-only history
pages. It says not to refactor until a prototype has passed a gate. This
note is that prototype's measurements and the gate applied to them.

**Verdict: not GO.** The page design (variant C) passes every criterion but
one. It cuts ASCII to 18.6–19.4 instructions per byte and runs the PTY flood
45–50% faster, but sgr-dense costs 16–23% more instructions and 12–16% more
wall-clock time, against a limit of +3%. Packed cells alone (variant A) does
not qualify for "A only": it carries 38–81% of C's instruction cut and 61–71%
of its wall-clock gain, against the two thirds needed for both, and it
regresses sgr-dense too. The NO-GO kill rule does not fire either, because
C's wall-clock gain is far above +10%. As written, the gate does not pass,
so the ADR is not accepted. The one failing criterion is the cost of
interning styles, and a fix for it would need its own measurement (see
Interpretation).

## Environment

- 2026-11-09. Branch `adr-0022-prototype`: `38c1ff5` (the ADR) plus
  `cd31763` (the prototype).
- Ryzen 5 7600 (6 cores, 12 threads), Ubuntu 26.04.1, Linux 7.0.0-42,
  governor `powersave`, EPP `balance_performance`, no pinning.
- The workstation was shared with other agents' builds. The 1-minute load
  average was 5.5–18.5 during the wall-clock runs kept here, and 6.7–9.3
  during the sgr-dense and cjk ones. A first ascii run at load 22–25 was
  thrown away (see Caveats).
- rustc 1.98.1 (Homebrew), valgrind 3.27.1, release profile (LTO,
  codegen-units 1).

## What was built

All of it sits behind non-default features: `page-proto` in `ultty-vt` and
`proto` in `ultty-bench`. Nothing that ships compiles it.

- `crates/ultty-vt/src/grid/proto/`: the two variants share one cursor and
  write core (`Core`), copied from `Buffer` and `Grid` for the actions the
  ADR lists. That is printable ASCII spans; `Print` through the in-place
  write, with the slow path for wraps and wide orphans; CR; LF; autowrap;
  and SGR. Every other action is ignored. SGR goes through the existing
  attribute code (a scratch `Terminal` and `TerminalHandler::handle`), and
  the resulting template is interned once per change, when the next
  character is printed. Both variants use the `PackedCell` layout from ADR
  §1, a `StyleTable` (foldhash) with collection at max(4096, 2 × live),
  and a `uniform` row flag in place of `plain`. They keep the `CharProps`
  width lookup, the cluster-join check, `LastPrint`, and the dirty and
  version arrays.
  - **A** (`a.rs`): `Vec<PackedRow>`, one cell allocation per row. History
    is a `VecDeque` of `CompactRow`-like rows encoded from packed cells:
    a uniform row is a byte copy, and other rows find their runs by
    comparing style ids.
  - **C** (`c.rs`): one `ScreenPage` (all cells in one `Vec<u64>`, `slots`
    rotated on scroll, per-slot heads) and `History` pages of 256 rows with
    per-row eviction and a free list. Rows are stored in the uniform-bytes
    form or the remapped-cells form (§3).
- `ultty-bench vt --suite proto` runs the reference (`run_vt_pass`, today's
  `Terminal`), `run_proto_a_pass` and `run_proto_c_pass` on the same corpora
  and in the same 64 KiB chunks.
- `ultty-bench proto-check` feeds a corpus into the reference, A and C. It
  then compares the cursor, `history_len`, and every history and screen
  row, cell by cell (character, colors, flags, wide structure) together
  with its wrap flag.
- `ultty-bench proto-flood` is a `pty_flood`-style harness, described under
  Detail.
- `ultty-proto-mem` copies the counting allocator and workloads from
  `crates/ultty/benches/memory.rs`.

## Commands

From the repository root, with `CARGO_TARGET_DIR` pointing at a private
target directory:

```bash
cargo build --release -p ultty-bench --features proto
target/release/ultty-bench proto-check --corpus ascii,wide-wrap,scroll,sgr-dense,cjk --size-mib 1
target/release/ultty-bench proto-check --corpus ascii,wide-wrap,scroll,sgr-dense,cjk --size-mib 8
scripts/bench/proto-icount.sh --jobs 6          # 60 callgrind runs
target/release/ultty-bench proto-flood --corpus ascii --rounds 40 --dir target --out flood-ascii.jsonl   # three times
target/release/ultty-bench proto-flood --corpus sgr-dense,cjk --rounds 12 --dir target --out flood-other.jsonl
target/release/ultty-proto-mem
cargo test -p ultty-vt --features page-proto --lib proto
```

## Summary: the gate

| criterion (C) | needed | measured | met |
|---|---|---|---|
| vt/ascii instructions per byte, 1 and 8 MiB | ≤ 27 | 18.6 / 18.6 at 120x40, 19.4 / 19.4 at 300x75 (reference 40.0 / 32.8 and 43.1 / 35.9) | yes |
| scroll | −25% or better | −35.4% to −43.5% | yes |
| wide-wrap | −25% or better | −46.2% to −55.7% | yes |
| cjk | no worse than +3% | −32.8% to −39.9% | yes |
| **sgr-dense** | no worse than +3% | **+15.9% to +22.6%** (wall clock −12% / −16%) | **no** |
| PTY ascii median, 120x40 and 300x75 | +15% or better | +45.5% and +49.9% (paired medians, 120 rounds) | yes |
| spread of those medians | under 5% | IQR/median 4.0–7.6% per arm; 95% CI of the paired ratio ±1–1.5% | not quite (see Detail) |
| live heap | within the current budgets | 1.16 / 1.30 / 10.38 MB against 3.46 / 4.36 / 18.98 MB | yes |

| "A only" test | needed | measured | met |
|---|---|---|---|
| A's share of C's instruction cut | two thirds or more | ascii 45–76%, scroll 38–57%, wide-wrap 55–81%, cjk 53–74%; sgr-dense regresses in both | no |
| A's share of C's wall-clock gain (ascii) | two thirds or more | 71% at 120x40, 61% at 300x75 | no |

| NO-GO test | fires when | measured | fires |
|---|---|---|---|
| C's wall-clock gain on ascii | under +10% | +45.5% / +49.9% | no |

## Detail

### Instructions (callgrind inside the timed pass)

The method is `icount-gate.sh`'s: `--toggle-collect` on exactly the pass
function, 1 run, 0 warmup. `scripts/bench/proto-icount.sh` runs it. Every
case was also run through `proto-check` at the same size, and A and C were
identical to the reference on every row (20 of 20 cases at 1 MiB and at
8 MiB).

| corpus | MiB | geometry | ref | A | C | ref instr/B | A instr/B | A vs ref | C instr/B | C vs ref | A's share of C's cut |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii | 1 | 120x40 | 41,924,871 | 30,756,800 | 19,547,803 | 40.0 | 29.3 | -26.6% | 18.6 | -53.4% | 50% |
| ascii | 1 | 300x75 | 45,239,374 | 33,970,994 | 20,383,758 | 43.1 | 32.4 | -24.9% | 19.4 | -54.9% | 45% |
| ascii | 8 | 120x40 | 274,976,486 | 185,449,060 | 156,422,367 | 32.8 | 22.1 | -32.6% | 18.6 | -43.1% | 76% |
| ascii | 8 | 300x75 | 300,908,504 | 211,280,691 | 163,147,223 | 35.9 | 25.2 | -29.8% | 19.4 | -45.8% | 65% |
| wide-wrap | 1 | 120x40 | 36,893,925 | 25,519,752 | 16,342,892 | 35.2 | 24.3 | -30.8% | 15.6 | -55.7% | 55% |
| wide-wrap | 1 | 300x75 | 26,909,171 | 17,368,604 | 12,860,665 | 25.7 | 16.6 | -35.5% | 12.3 | -52.2% | 68% |
| wide-wrap | 8 | 120x40 | 242,362,940 | 151,115,460 | 130,365,733 | 28.9 | 18.0 | -37.6% | 15.5 | -46.2% | 81% |
| wide-wrap | 8 | 300x75 | 199,338,095 | 123,203,606 | 103,079,509 | 23.8 | 14.7 | -38.2% | 12.3 | -48.3% | 79% |
| scroll | 1 | 120x40 | 192,513,852 | 155,232,811 | 119,582,931 | 183.6 | 148.0 | -19.4% | 114.0 | -37.9% | 51% |
| scroll | 1 | 300x75 | 227,836,233 | 190,447,813 | 128,770,329 | 217.3 | 181.6 | -16.4% | 122.8 | -43.5% | 38% |
| scroll | 8 | 120x40 | 1,480,131,429 | 1,181,876,875 | 956,663,541 | 176.4 | 140.9 | -20.2% | 114.0 | -35.4% | 57% |
| scroll | 8 | 300x75 | 1,762,024,032 | 1,463,662,142 | 1,030,052,075 | 210.0 | 174.5 | -16.9% | 122.8 | -41.5% | 41% |
| sgr-dense | 1 | 120x40 | 91,746,137 | 106,010,774 | 106,404,187 | 87.5 | 101.1 | +15.5% | 101.5 | +16.0% | n/a |
| sgr-dense | 1 | 300x75 | 92,225,174 | 113,209,273 | 112,952,419 | 88.0 | 108.0 | +22.8% | 107.7 | +22.5% | n/a |
| sgr-dense | 8 | 120x40 | 725,715,001 | 832,400,899 | 841,398,667 | 86.5 | 99.2 | +14.7% | 100.3 | +15.9% | n/a |
| sgr-dense | 8 | 300x75 | 730,233,792 | 891,108,213 | 895,144,419 | 87.1 | 106.2 | +22.0% | 106.7 | +22.6% | n/a |
| cjk | 1 | 120x40 | 230,976,032 | 180,886,697 | 138,760,638 | 220.3 | 172.5 | -21.7% | 132.3 | -39.9% | 54% |
| cjk | 1 | 300x75 | 233,205,303 | 184,276,491 | 140,105,566 | 222.4 | 175.7 | -21.0% | 133.6 | -39.9% | 53% |
| cjk | 8 | 120x40 | 1,644,944,537 | 1,244,127,928 | 1,105,308,608 | 196.1 | 148.3 | -24.4% | 131.8 | -32.8% | 74% |
| cjk | 8 | 300x75 | 1,673,788,066 | 1,274,132,551 | 1,113,582,806 | 199.5 | 151.9 | -23.9% | 132.7 | -33.5% | 71% |

The reference's vt/ascii count at 1 MiB, 120x40, is 41.92 M. Built without
the feature it is 41.82 M, so the feature moves the reference by 0.3%. The
committed baseline (`bench/baselines/icount.json`, 40.93 M) predates this
tree. The gate compares arms measured together, so the baseline is not used.

**The ADR's expectations.** It expected ascii at about 20–27 instructions
per byte; C measured 18.6–19.4, below the range. It expected scroll −35% to
−60%; C measured −35% to −44%. It expected wide-wrap in line with ascii, and
C was. It expected cjk +5% to +15% throughput; C measured −33% to −40%
instructions, because the in-place 8-byte write and the remapped-cells
history row also replace the per-cell encoder for wide characters. It
expected sgr-dense at about ±5%; C measured +16% to +23%. The 8 MiB ascii
case, which the ADR had not measured, is 32.8 instructions per byte for the
reference, and C's relative cut there is 10 points smaller than at 1 MiB, as
the ADR guessed.

**Why sgr-dense regresses.** Inclusive callgrind costs, 8 MiB, 120x40:

| | reference | A | C |
|---|---:|---:|---:|
| total | 725.7 M | 832.4 M | 841.4 M |
| `apply_attribute` (shared code) | 29.9 M | 29.9 M | 29.9 M |
| screen interning (`Storage::intern`) | none | 119.3 M | 120.7 M |
| of which the hash lookup or insert | | 80.7 M | 80.7 M |
| of which collection | | 29.8 M | 29.8 M |
| scrollback save | 92.3 M (`save_scrolled_rows`) | 90.6 M | 90.5 M (page remap 37.2 M) |

Interning accounts for almost all of the difference. The corpus changes SGR
every one to three characters, and most of the resulting styles are new
(random 256-color and truecolor foreground and background combinations), so
nearly every change hashes and inserts into the screen table. The table
then fills and collects every few thousand styles. At 300x75 a collection
scans 22,500 cells and rehashes every live style, so
collection rises to 62.6 M and interning to 172.8 M (19% of the pass).
Scrollback saves cost no less than today: C's remapped-cells form pays a
page-table insert for each style that is new to the page, and that is what
the per-cell encoder used to cost. A, whose history stores `Style` values,
pays the same interning. The ADR's mitigation, caching the last few
template-to-id pairs, cannot help styles that never repeat.

**First pass, and the changes since.** The first build followed the ADR's
text literally: two hash lookups to intern a new style, five fields hashed,
and a hash-indexed `StyleTable` on every history page. It measured
sgr-dense at +36% to +45% for C and +22% to +31% for A. Three changes,
kept in the committed prototype, brought that to the numbers above:

- interning with one hash lookup (an `entry`), and collecting before the
  lookup when the table is due;
- hashing a style as three words;
- history pages that keep a plain list of styles. The remap from screen ids
  already makes each style appear once per page, so the page index only
  cost a second hash.

No other corpus moved by more than 0.5%. These are the obvious
implementation tunings, not design changes, and the gate still fails by a
wide margin after them.

### Wall clock (`proto-flood`, a `pty_flood`-style harness)

`dd if=<32 MiB corpus> bs=65536` writes into a PTY whose slave the harness
set to raw mode. The harness thread does blocking 4 KiB reads from the
master, then parses and feeds each read under a `Mutex`. A bench thread
wakes every 16 ms, takes the mutex, and copies every visible row. The
reference copies with `Row::copy_from` from `Buffer::visible_row`, as
`pty_flood` does; A and C unpack into `Vec<Cell>`, the copy ADR §5 plans.
Time runs from spawning `dd` to EOF on the master. In each round every
arm runs once per geometry, and the arm order is reversed on odd rounds.
About 8,200–8,400 reads per run (4 KiB each), 15–160 frames.

ascii, 32 MiB: three batches of 40 rounds, at load 14–18.5, 10–12 and
5.5–6.4. The medians agree across batches within 1.5%.

| case | arm | median MiB/s | IQR / median | range / median | paired median vs ref | 95% CI (bootstrap) |
|---|---|---:|---:|---:|---:|---:|
| ascii 120x40 | ref | 258.7 | 4.0% | 38% | | |
| ascii 120x40 | A | 340.0 | 6.0% | 87% | 1.316 | 1.309–1.334 |
| ascii 120x40 | C | 376.1 | 7.6% | 43% | 1.455 | 1.442–1.471 |
| ascii 300x75 | ref | 244.2 | 5.5% | 84% | | |
| ascii 300x75 | A | 317.2 | 5.8% | 85% | 1.309 | 1.296–1.316 |
| ascii 300x75 | C | 365.3 | 4.5% | 31% | 1.499 | 1.484–1.518 |

Per batch, C over the reference was 1.461 / 1.448 / 1.454 at 120x40 and
1.510 / 1.511 / 1.484 at 300x75. A over the reference was 1.336 / 1.312 /
1.315 and 1.312 / 1.302 / 1.302. A's share of C's gain, taken as the median
of A's per-round gains over the median of C's, was 72 / 69 / 71% at 120x40
and 62 / 58 / 62% at 300x75.

**Spread.** Each arm's IQR stays at about 4–7.6% of its median whatever the
load. It was 5.5–5.8% even in the quietest batch, so this harness on this
box does not get under 5%. More rounds did not narrow the IQR, which is a
property of the samples, but they did narrow the uncertainty of the median.
The paired-ratio confidence intervals are about ±1–1.5%, far from the +15%
threshold. Read literally, the spread criterion is not met for C at 120x40
(7.6%). That does not decide the verdict, because sgr-dense fails on
deterministic instruction counts.

Other corpora, for information only (32 MiB, 12 rounds, load 6.7–9.3):

| case | ref | A | C | A vs ref | C vs ref |
|---|---:|---:|---:|---:|---:|
| sgr-dense 120x40 | 133.0 | 114.6 | 117.6 | 0.867 | 0.881 |
| sgr-dense 300x75 | 133.9 | 108.0 | 112.4 | 0.799 | 0.838 |
| cjk 120x40 | 90.7 | 114.0 | 134.4 | 1.278 | 1.476 |
| cjk 300x75 | 87.6 | 110.0 | 133.8 | 1.249 | 1.526 |

The wall clock confirms the instruction counts: sgr-dense is 12–16% slower
in C and 13–20% slower in A.

### Live heap (`ultty-proto-mem`, the memory bench's counting allocator)

| case | reference | A | C | budget | C + alternate screen page (projection) |
|---|---:|---:|---:|---:|---:|
| empty 300x75 | 1,172,860 | 267,567 | 264,293 | 1,229,000 | 444,293 |
| typical 10k lines, 120x40 | 3,297,990 | 3,214,132 | 1,155,684 | 3,461,000 | 1,194,084 |
| typical 10k lines, 300x75 | 4,152,860 | 3,358,287 | 1,298,229 | 4,358,000 | 1,478,229 |
| sgr-dense 10k lines, 120x40 | 18,077,990 | 18,016,992 | 10,381,024 | 18,980,000 | 10,419,424 |

The reference column matches `memory.rs`'s own measurements (budget ÷ 1.05).
The prototype arms include the scratch 1x1 `Terminal` used for SGR (about
82 KB, close to the roughly 93 KB of non-grid state a real `Terminal`
holds). They have no alternate screen, so the last column adds one screen
page (cols × rows × 8 bytes). Every budget case fits with room to spare:
C uses 35%, 34% and 55% of the reference. These match the ADR §9 estimates
(1.1–1.3, 1.3–1.5 and about 10.3 MB). A saves little, because its history
is today's `CompactRow` format.

## Interpretation

- **The page design delivers what the ADR said for plain output, and more.**
  C reaches 18.6–19.4 instructions per byte on ascii, below the ADR's
  20–27, and runs +45% to +50% end to end through a PTY harness. That is
  three times the +15% bar and, if the T4 behaves like this box, would
  close the gap to Ghostty's ASCII throughput. Scroll, wide-wrap and cjk
  improve by 33–56%, and memory falls to a third.
- **Packed cells are not most of it.** A carries about half of C's cut at
  1 MiB and two thirds to three quarters at 8 MiB, where the reference has
  stopped allocating history. On the wall clock, which is what the gate
  weighs, A carries 61–71%. The rest is the pages: slot rotation,
  history appended without per-row allocation, and the narrowed copy.
- **Interning styles is the cost the ADR underestimated.** With most SGR
  changes producing a new style, interning on the screen costs more than
  the per-cell encoder it replaces. This hits A and C equally, so it is a
  cost of packed cells with a style table, not of pages. sgr-dense is a
  synthetic worst case, but it is the case the gate names, and truecolor
  output (syntax highlighters, `ls --color` with LS_COLORS, TUI gradients)
  behaves partly like it.
- **What would change the verdict.** A fix for interning unique styles,
  measured by this same prototype, with sgr-dense brought to within +3%.
  Possible directions, none of them measured:
  - store a run's full `Style` inline on the screen row when the table is
    churning;
  - intern lazily per row instead of per SGR change;
  - stop collecting the screen table in place and give it a
    generation-scoped arena.

  Until then the ADR stays unaccepted, and the decision about whether to
  pursue that fix, or to relax the sgr-dense criterion in view of the
  ascii, scroll, cjk and memory results, belongs to the ADR's owners and
  is not this note's to make.
- Nothing here supports a release claim. These are prototype numbers on a
  shared workstation.

## Caveats

- **The prototypes do less bookkeeping than the reference.** They have no
  scroll-region lookup (the region is always the full screen), no
  alternate-screen check per action, no `linefeed_newline`, insert-mode
  or autowrap-mode checks, no `scroll_offset` reset, no anchor
  (`note_row_to_history`) or mark handling, and no grapheme store: the
  write paths test the `has_graphemes` flag, but nothing ever sets it.
  The reference's `TerminalHandler::handle` dispatch and `Buffer::newline`
  together cost about 35 instructions per CR/LF action on scroll, and the
  prototypes keep most of that (a closure match and their own newline). The
  omitted checks are estimated at tens of instructions per line, at most
  about 3 instructions per byte on scroll (about 8 bytes per line) and well
  under 1 on ascii. That is small against the measured cuts, and it would
  make sgr-dense worse, not better.
- The corpora have no grapheme clusters, so neither variant's grapheme path
  is measured. `proto-check` would have caught any cluster, because the
  prototypes drop them.
- SGR is applied through a scratch `Terminal`, so the prototype arms pay
  the same `apply_attribute` as the reference (29.9 M on both sides above)
  plus copying the template out (small).
- The wall-clock harness is not `pty_flood` itself, which drives the full
  `EventLoop` and could not take the prototype buffers. It reproduces its
  shape (`dd`, raw PTY, 4 KiB reads, parse under a lock, a frame copy
  every 16 ms), and all three arms run through the same code. The first
  ascii attempt, at load 22–25, gave runs from 0.23 s to 2.5 s
  (IQR/median 220–520%) and was discarded before the three batches above.
  Every batch ran in full.
- Instruction counts depend on the compiler; all arms were built together
  in one binary.
