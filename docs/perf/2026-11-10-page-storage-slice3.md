# Page storage slice 3: packed cells (2026-11-10)

[ADR 0022](../adr/0022-page-storage.md) migration slice 3: screen rows
store 8-byte packed cells with a grid style table, in place of 24-byte
`Cell`s. Its gate is that the instruction cut lands in the range the
[prototype](2026-11-09-page-storage-prototype.md) measured for variant A
(packed cells only). This note is that comparison, plus the memory bench
and one `pty_flood` run, which the ADR asks for at slice 3.

**Verdict.** ascii, scroll and sgr-dense are inside A's range or better,
and wide-wrap is inside it at 300x75 and 3–4 points short at 120x40. cjk
gets about half of A's cut (−11% to −13% against −22% to −26%). The
storage side of cjk does better than A; what A saved beyond that is in the
per-character print path, which the prototype's core shortened in ways
that have nothing to do with packing (below). `pty_flood` gives ascii
+22% to +26% and cjk +12% to +13%, close to A's +27% and +26%; sgr-dense
is level within the noise. Every memory case shrinks.

## What was built

- `grid/packed.rs`: `PackedCell` (ADR §1, with the two width bits kept
  independent so every `Cell` round-trips), `StyleKey` (inline or a table
  id, round 2's layout), `StyleTable` (256-slot approximate cache, append
  on a miss, collection after max(4096, 2 × screen cells) new styles, a
  generation counter) and `PackedRow`, the screen row: a boxed slice of
  cells, the grapheme map behind one pointer, `used`, `uniform` with its
  style key, wrap, marks and anchor. 40 bytes; `Row` was 56.
- `Grid` holds `Vec<PackedRow>` and the table. The cursor template's key
  is cached with the generation it was made in. Collection only runs where
  no key is held outside the rows: when the template key has to be made,
  or when an erase needs a non-default blank. Collection changes ids, not
  content, so it neither dirties rows nor touches `LastPrint`.
- `RowView` reads screen rows in place and hands out `Cell`s by value.
  Writers use `Grid::set_cell` / `update_cell` and the `RowMut` handle;
  `cell_mut`, `IndexMut` and `&Row` access are gone. `Row` stays the owned
  snapshot (renderer cache, reflow, tests); its `plain` flag is gone.
- `CompactRow::encode_from` reads packed cells: a uniform row is a byte
  copy with its style looked up once; other rows find runs by comparing
  keys and record the structure bits per cell, so encoding is exact for any
  row. UTF-8 is written straight from the codepoint.

## Instructions

callgrind inside the timed pass, as `icount-gate.sh` does. "ref" and "A"
come from one binary built from `main` (`ad6a334`) with the `proto`
feature, as the prototype note measured them; "new" is this branch. The
feature moves the reference by under 0.5% against the gate's own baseline.

| corpus | MiB | geometry | ref | A | new | A vs ref | new vs ref | new / A's cut |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| ascii | 1 | 120x40 | 41,523,510 | 31,097,872 | 31,637,176 | -25.1% | -23.8% | 95% |
| ascii | 1 | 300x75 | 44,693,850 | 34,747,648 | 33,887,021 | -22.3% | -24.2% | 109% |
| ascii | 8 | 120x40 | 271,758,832 | 187,838,201 | 192,308,640 | -30.9% | -29.2% | 95% |
| ascii | 8 | 300x75 | 296,522,489 | 217,177,970 | 210,006,386 | -26.8% | -29.2% | 109% |
| wide-wrap | 1 | 120x40 | 36,862,460 | 25,845,860 | 26,980,786 | -29.9% | -26.8% | 90% |
| wide-wrap | 1 | 300x75 | 26,760,480 | 17,630,214 | 17,640,707 | -34.1% | -34.1% | 100% |
| wide-wrap | 8 | 120x40 | 240,335,076 | 153,429,939 | 162,739,601 | -36.2% | -32.3% | 89% |
| wide-wrap | 8 | 300x75 | 198,128,640 | 125,250,168 | 125,373,289 | -36.8% | -36.7% | 100% |
| scroll | 1 | 120x40 | 188,116,557 | 158,471,798 | 164,443,857 | -15.8% | -12.6% | 80% |
| scroll | 1 | 300x75 | 221,846,198 | 198,468,198 | 188,562,272 | -10.5% | -15.0% | 142% |
| scroll | 8 | 120x40 | 1,444,944,278 | 1,207,447,558 | 1,255,299,810 | -16.4% | -13.1% | 80% |
| scroll | 8 | 300x75 | 1,714,078,703 | 1,527,510,567 | 1,447,825,887 | -10.9% | -15.5% | 143% |
| sgr-dense | 1 | 120x40 | 91,653,461 | 94,685,982 | 91,887,203 | +3.3% | +0.3% | |
| sgr-dense | 1 | 300x75 | 92,035,080 | 94,455,079 | 91,457,104 | +2.6% | -0.6% | |
| sgr-dense | 8 | 120x40 | 724,355,943 | 747,253,902 | 725,323,886 | +3.2% | +0.1% | |
| sgr-dense | 8 | 300x75 | 728,651,478 | 750,507,853 | 726,412,555 | +3.0% | -0.3% | |
| cjk | 1 | 120x40 | 230,148,601 | 177,789,922 | 204,276,231 | -22.7% | -11.2% | 49% |
| cjk | 1 | 300x75 | 233,777,419 | 182,060,101 | 206,836,160 | -22.1% | -11.5% | 52% |
| cjk | 8 | 120x40 | 1,642,105,465 | 1,221,923,239 | 1,428,953,138 | -25.6% | -13.0% | 51% |
| cjk | 8 | 300x75 | 1,671,143,090 | 1,256,424,446 | 1,449,691,770 | -24.8% | -13.3% | 53% |

| corpus | A's range | new | in range |
|---|---|---|---|
| ascii | −22.3% to −30.9% | −23.8% to −29.2% | yes |
| wide-wrap | −29.9% to −36.8% | −26.8% to −36.7% | at 300x75; 120x40 is 3–4 points short |
| scroll | −10.5% to −16.4% | −12.6% to −15.5% | yes |
| sgr-dense | +2.6% to +3.3% | −0.6% to +0.3% | better (the ADR allows +3%) |
| cjk | −22.1% to −25.6% | −11.2% to −13.3% | no: half of A's cut |

The gate's own run (`scripts/bench/icount-gate.sh`, 1 MiB, 120x40, against
`bench/baselines/icount.json`): ascii −23.96%, wide-wrap −26.84%, scroll
−13.49%, sgr-dense +0.28%, cjk −11.02%, unicode-mixed −11.88%, emoji-zwj
−10.16%, combining −1.18%, cursor-redraw −15.69%, scroll-region −27.84%,
alt-screen +1.16%; every parse case 0.00%.

### Where cjk's other half is

Inclusive instructions per 1 MiB of cjk at 120x40 (callgrind with line
tables, same binaries):

| part | ref | A | new |
|---|---:|---:|---:|
| history encode (`encode_from`, A's `save_top_row`) | 66.4 M | 59.3 M | 55.8 M |
| printing non-ASCII (`print_utf8_run`, decode included) | 109.3 M | 77.4 M | 102.6 M |

The storage half (encode) is cheaper than A's. The print path is where A
is 25 M ahead, about 80 instructions per character. A's `Core::print` is
called from the parser closure and does the width lookup, the cluster
check and the in-place write in one function. The real path, in the
reference and here alike, goes `TerminalHandler::handle` →
`Buffer::write_char` → `Grid::print_width` → `Grid::write_in_row`, with the
scroll-offset reset, insert-mode and scroll-region checks and the
cluster-join state on the way. Packing removed what it could from that path
(the 24-byte template copy and cell writes: ref 109.3 M → 102.6 M); the
rest is dispatch, which no storage slice changes. The same kind of
difference, smaller, is behind wide-wrap at 120x40 and scroll at 120x40:
the reference's scroll and region bookkeeping, which A's core does not
have.

Two changes found while profiling carried most of the cut and are worth
keeping in mind for slices 4 and 5:
- reading a cell's character with `char::from_u32` in the encoder cost
  ascii 7 M per MiB: the uniform copy now takes the low byte, and
  non-ASCII is encoded straight from the codepoint (that and the smaller
  row below saved cjk 11 M);
- a scroll moves the screen's rows, so their size matters: with 64-byte
  rows scroll was −6.4% at the gate's geometry, with 40-byte rows −13.5%.

## Memory

`cargo test -p ultty --bench memory` (live heap bytes, counting allocator):

| case | main | this branch | change | budget |
|---|---:|---:|---:|---:|
| empty 80x24 | 175,374 | 115,254 | −34% | 185,000 |
| empty 300x75 | 1,170,356 | 450,044 | −62% | 1,229,000 |
| typical 10k lines, 80x24 | 2,775,374 | 2,715,254 | −2% | 2,915,000 |
| typical 10k lines, 120x40 | 3,296,046 | 3,143,254 | −5% | 3,461,000 |
| typical 10k lines, 300x75 | 4,150,356 | 3,430,044 | −17% | 4,358,000 |
| typical 120x40, resized (live) | 3,294,322 | 3,141,474 | −5% | 3,461,000 |
| typical 120x40, resized (peak) | 4,699,129 | 4,567,640 | −3% | 4,935,000 |
| sgr-dense 10k lines, 120x40 | 18,076,046 | 17,923,254 | −1% | 18,980,000 |

The screens shrink threefold; history is still `CompactRow`, so the
history-heavy cases move by the screen's share only. The budgets are left
as they are until slice 6 (ADR: "update the memory budgets downward").

## Wall clock: `pty_flood`

`cargo bench -p ultty --bench pty_flood` (32 MiB through a raw PTY,
120x40), the bench binaries of `main` and this branch run in turn, four
rounds with the arm order alternating, criterion's 10 samples per run. The
workstation was shared (1-minute load 3–6 during the runs); round 3 ran
through a burst of other work and is shown but left out of the medians.
Indicative only.

| corpus | round | main MiB/s | branch MiB/s | branch / main |
|---|---:|---:|---:|---:|
| ascii | 1 | 196.7 | 245.6 | 1.248 |
| ascii | 2 | 203.5 | 248.5 | 1.222 |
| ascii | 3 | 160.7 | 168.5 | 1.048 (load) |
| ascii | 4 | 201.5 | 254.1 | 1.261 |
| cjk | 1 | 75.9 | 85.0 | 1.120 |
| cjk | 2 | 78.2 | 87.5 | 1.119 |
| cjk | 3 | 49.1 | 71.7 | 1.461 (load) |
| cjk | 4 | 76.7 | 86.9 | 1.133 |
| sgr-dense | 1 | 107.9 | 107.2 | 0.994 |
| sgr-dense | 2 | 107.3 | 104.7 | 0.976 |
| sgr-dense | 3 | 106.4 | 108.7 | 1.022 |
| sgr-dense | 4 | 109.1 | 105.0 | 0.962 |

Without round 3: ascii 1.222–1.261 (median 1.248), cjk 1.119–1.133
(median 1.120). sgr-dense over all four rounds is 0.962–1.022 (median
0.985), inside the spread of a single run here. The prototype's A, on the
quiet machine: ascii 1.270, cjk 1.256, sgr-dense 0.952.

## Correctness

- `cargo test --workspace`, including the differential oracle
  (`live_storage_matches_reference`), which now also runs
  `Terminal::check_integrity` on the live terminal after every step.
- A second oracle test runs the same streams with every live style table
  collected after 1 to 8 new styles. It found a collection that
  renumbered the default style when a blank uniform row marked it; the
  unit test `collection_renumbers_the_uniform_style` now covers it, and
  dropping the generation bump fails three tests.
- Both oracle tests at `ULTTY_ORACLE_CASES=30000`.
- `check_integrity` adds: every key valid against the table and in
  canonical form, uniform rows uniform, grapheme data behind every
  grapheme bit, and the cached template key naming the attributes' style.

## Environment

Ryzen 5 7600, Ubuntu 26.04.1, Linux 7.0.0-42, rustc 1.98.1, valgrind
3.27.1, release profile. Shared workstation.
