# Page storage slice 5: history pages (2026-11-10)

[ADR 0022](../adr/0022-page-storage.md) migration slice 5: scrollback
moves from `VecDeque<CompactRow>` to history pages. Its gate is every GO
criterion of the ADR's §Prototype, measured on the real code. This note is
those measurements, the memory bench (with a new CJK case) and one
indicative `pty_flood` run.

**Verdict.** Every instruction criterion and the heap criterion are met
on the real code: vt/ascii is 19.9–20.9 instructions per byte at 1 and
8 MiB (limit 27), scroll is −26.7% to −36.1% and wide-wrap −38.7% to
−51.0% against the code before slice 3 (limit −25%), sgr-dense is −1.1%
to +0.0% and cjk −38.0% to −45.5% (limit +3%). Live heap falls 44–73% in
every scrollback case, to a third of the budgets or less (sgr-dense,
half). The wall-clock
criterion (`pty_flood` ascii +15% or better with spread under 5%) is not
settled here. On the shared workstation ascii is level with `main`
(paired median 0.998, IQR 0.957–1.032) because the harness is at a
PTY ceiling there, about 240 MiB/s with more time in the kernel than in
user space; cjk, which is not, gains +16.5%. The criterion needs the
quiet-machine rerun that slice 6 plans.

## What was built

`grid/history.rs` is new; `CompactRow` is gone from the live storage
(the frozen copy in `grid/reference/` stays until slice 6).

- `History`: a `VecDeque` of pages of 256 rows, `front` (rows of the
  first page already evicted), the row count, one spare page, and the
  style remap for the newest page. Row `i` is in page
  `(front + i) / 256`: every page but the last is full. Eviction is per
  row; a page whose last row goes is cleared and kept as the spare, which
  is the next page started, so a full scrollback allocates nothing per
  row (`tests/screen_alloc.rs` now checks this, and fails with 328
  allocations when freed pages are dropped instead). A page that fills is
  shrunk to fit, so pages carry no growth slack.
- Each row has a 24-byte head (form, offset, length, width, wrap, marks,
  anchor, and either its style key or where its grapheme extras are) and
  is stored in one of three forms, picked per row:
  - **Ascii**, a byte a cell, one style: a uniform screen row is a byte
    copy trimmed of its blank tail. A row printed a character at a time
    (which clears `uniform`) that is still ASCII in one style lands here
    too.
  - **Chars**, four bytes a cell (the codepoint and the width bits), one
    style, no grapheme extras. This is a third form beyond the ADR's two;
    see "CJK memory" below.
  - **Cells**, the packed cells with page style ids, and their grapheme
    extras in a per-page list.

  Choosing among the second and third takes one pass of ORs over the
  cells (`packed::Summary`).
- Styles: inline keys are stored as they are. A table id is remapped into
  the page's own style list through a map from the grid table's ids,
  valid for one page and one table stamp (a per-table id plus the
  collection generation), so a lookup per style run, not per cell. The
  stamp's table id is new: the grid's table and the packer's (used for
  owned rows) both start at generation 0, and a remap keyed on the
  generation alone would have mixed their ids.
- `RowView` reads history rows in place (`HistoryRow`): `cell(col)` from
  the byte, the codepoint or the packed cell with the style looked up
  once per row for the one-style forms, `grapheme_extra` by binary search,
  and `text`, `text_trimmed`, `copy_text`, `eq_row`, `to_row` and
  `copy_into` without building a `Row`. Search, `wait`, `text
  --scrollback`, `last-command` and the cursor-line read in `input.rs`
  already went through `Buffer::row_at` and `RowView::text` /
  `text_trimmed` (`app/mod.rs`, `app/control.rs`), so they now decode
  nothing; each still builds its `String`. `RowView::owned` and the
  `Cow<Row>` inside `RowView` are gone; a view of an owned `Row` borrows
  it.
- Marks and anchors are read from and written to the heads
  (`marks_at`, `anchor_at`, `strip_anchor_rows`, `drop_all_anchors`).
- Reflow drains the old pages: each row is read as a view into
  `ReflowCell`s, and a page is dropped once its rows are read. Rows older
  than the new screen are packed into a one-row screen page (`RowPacker`,
  as before) and appended to new pages.
- Height resize: shrinking appends the top rows through the scroll-off
  path, as it did; growing reads the newest rows as views into owned rows
  for `Grid::prepend_rows` and takes them off the end of history, giving
  their payload back when nothing was written after them.
- `Buffer::replace_row` on a scrollback row (an inline surface, ADR 0020)
  appends the new cells to the row's page and leaves the old ones as
  garbage, which is compacted away once it passes 4 KiB and half the
  page. Its styles are found again in the page's list rather than through
  the remap, so repeated rewrites do not grow it.

### The scroll path

The first working version left vt/scroll at −25.0% at 1 MiB and −21.9% at
8 MiB (120x40), short of the −25% criterion: the history push still cost
about 180 instructions a line, about what `CompactRow::encode_from` had.
Three changes, each measured, took it to −29.6% and −26.7%:

- the non-uniform forms moved out of line (`Page::append_cells`), so the
  uniform byte copy is not compiled around them;
- `save_top_rows` reads the screen row once instead of twice, and
  `pop_front` keeps freeing a page out of line;
- the uniform copy trims the blank tail on the packed cells before
  copying, instead of copying and truncating, and skips the remap for
  the default style; `push` finds the newest page with one deque lookup.

Two others were tried and dropped: writing into a pre-sized slice (a
`memset` call per row cost more than it saved) and copying eight cells
per store (the capacity check stopped the loop from vectorising). Of the
rest of a scroll line, the screen's rotation, erase and per-row
dirty/version marks (`Grid::scroll_up`, about 260 instructions a line)
are slice 4's and unchanged.

## Instructions: the GO criteria

callgrind inside the timed pass, as `icount-gate.sh` does, on the five
corpora of the prototype gate. "Before slice 3" is `ad6a334`, the
reference the slices have been measured against, built and run here with
the same toolchain. "C" is the prototype's variant C against its own
reference, from the [prototype note](2026-11-09-page-storage-prototype.md)
(second round).

| corpus | MiB | geometry | before slice 3 | this branch | change | instr/B | C vs its ref | C instr/B |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| ascii | 1 | 120x40 | 41,411,348 | 20,889,241 | -49.6% | 19.9 | -53.1% | 18.7 |
| ascii | 1 | 300x75 | 44,583,052 | 21,755,366 | -51.2% | 20.7 | -54.7% | 19.5 |
| ascii | 8 | 120x40 | 270,878,595 | 168,840,584 | -37.7% | 20.1 | -42.8% | 18.7 |
| ascii | 8 | 300x75 | 295,643,706 | 175,594,032 | -40.6% | 20.9 | -45.5% | 19.5 |
| wide-wrap | 1 | 120x40 | 36,729,682 | 17,982,169 | -51.0% | 17.1 | -55.9% | 15.6 |
| wide-wrap | 1 | 300x75 | 26,628,313 | 13,518,253 | -49.2% | 12.9 | -52.2% | 12.3 |
| wide-wrap | 8 | 120x40 | 239,293,496 | 146,677,559 | -38.7% | 17.5 | -46.0% | 15.6 |
| wide-wrap | 8 | 300x75 | 197,086,791 | 110,717,148 | -43.8% | 13.2 | -48.2% | 12.3 |
| scroll | 1 | 120x40 | 188,247,015 | 132,575,895 | -29.6% | 126.4 | -37.3% | 115.1 |
| scroll | 1 | 300x75 | 221,978,236 | 141,777,058 | -36.1% | 135.2 | -43.0% | 123.8 |
| scroll | 8 | 120x40 | 1,446,004,896 | 1,060,149,675 | -26.7% | 126.4 | -34.8% | 115.1 |
| scroll | 8 | 300x75 | 1,715,140,941 | 1,133,524,851 | -33.9% | 135.1 | -41.1% | 123.8 |
| sgr-dense | 1 | 120x40 | 91,609,236 | 91,137,929 | -0.5% | 86.9 | +1.1% | 88.5 |
| sgr-dense | 1 | 300x75 | 91,986,219 | 91,000,796 | -1.1% | 86.8 | +0.5% | 88.3 |
| sgr-dense | 8 | 120x40 | 724,066,671 | 723,501,356 | -0.1% | 86.2 | +1.4% | 87.7 |
| sgr-dense | 8 | 300x75 | 728,360,093 | 728,570,827 | +0.0% | 86.9 | +1.4% | 88.2 |
| cjk | 1 | 120x40 | 229,814,459 | 126,610,378 | -44.9% | 120.7 | -42.3% | 126.8 |
| cjk | 1 | 300x75 | 234,120,868 | 127,686,712 | -45.5% | 121.8 | -43.0% | 127.6 |
| cjk | 8 | 120x40 | 1,640,407,168 | 1,017,400,377 | -38.0% | 121.3 | -35.8% | 125.9 |
| cjk | 8 | 300x75 | 1,670,122,777 | 1,025,907,505 | -38.6% | 122.3 | -36.5% | 126.8 |

| criterion | needed | measured | met |
|---|---|---|---|
| vt/ascii instructions per byte, 1 and 8 MiB | ≤ 27 | 19.9 / 20.1 at 120x40, 20.7 / 20.9 at 300x75 | yes |
| scroll | −25% or better | −26.7% to −36.1% | yes |
| wide-wrap | −25% or better | −38.7% to −51.0% | yes |
| sgr-dense | no worse than +3% | −1.1% to +0.0% | yes |
| cjk | no worse than +3% | −38.0% to −45.5% | yes |
| live heap | within the current budgets | 1.17 / 1.45 / 9.96 MB against 3.46 / 4.36 / 18.98 MB | yes |
| `pty_flood` ascii median, 120x40 and 300x75 | +15% or better, spread under 5% | not settled: ascii level with `main` at a PTY ceiling on this box; `pty_flood` runs 120x40 only | not judged |

Against C, the real code is within 1.2 instructions per byte on ascii and
1.5 on wide-wrap, and ahead on cjk and sgr-dense; scroll is the furthest
behind (about 11 per byte), for the reasons above: C's core had no scroll
region, row versions or anchors to keep. cjk beats C because C stored
CJK rows as 8-byte cells, where this stores them as 4-byte chars.

### The gate's own run

`scripts/bench/icount-gate.sh` against `bench/baselines/icount.json`
(slice 4, `1f70ce4`), 1 MiB, 120x40:

| case | baseline | this branch | change |
|---|---:|---:|---:|
| vt/ascii | 29,733,916 | 20,889,241 | −29.75% |
| vt/sgr-dense | 92,750,038 | 91,137,909 | −1.74% |
| vt/unicode-mixed | 177,720,424 | 155,752,193 | −12.36% |
| vt/cjk | 178,288,650 | 126,610,247 | −28.99% |
| vt/emoji-zwj | 153,187,098 | 128,336,120 | −16.22% |
| vt/combining | 379,263,475 | 356,320,860 | −6.05% |
| vt/cursor-redraw | 30,040,064 | 30,060,501 | +0.07% |
| vt/scroll | 144,591,432 | 132,575,905 | −8.31% |
| vt/scroll-region | 22,516,106 | 22,589,101 | +0.32% |
| vt/alt-screen | 68,683,873 | 69,040,962 | +0.52% |
| vt/wide-wrap | 25,879,899 | 17,979,896 | −30.53% |

Every parse case is 0.00%. Nothing is over the CI job's 3%; the three
small regressions (alt-screen, scroll-region, cursor-redraw) keep no
history at the gate's size and pay for code layout only. The baseline is
regenerated in its own commit.

## Memory

`cargo test -p ultty --bench memory` (live heap bytes, counting
allocator). "main" is slice 4 as merged, with the new CJK case added to
measure it.

| case | main | this branch | change | budget | budget at +5% |
|---|---:|---:|---:|---:|---:|
| empty 80x24 | 116,886 | 36,886 | −68% | 185,000 | 38,700 |
| empty 300x75 | 455,144 | 375,144 | −18% | 1,229,000 | 393,900 |
| typical 10k lines, 80x24 | 2,716,886 | 731,210 | −73% | 2,915,000 | 767,800 |
| typical 10k lines, 120x40 | 3,145,974 | 1,165,110 | −63% | 3,461,000 | 1,223,400 |
| typical 10k lines, 300x75 | 3,435,144 | 1,454,280 | −58% | 4,358,000 | 1,527,000 |
| typical 120x40, resized (live) | 3,144,194 | 1,163,142 | −63% | 3,461,000 | 1,221,300 |
| typical 120x40, resized (peak) | 4,570,094 | 1,448,974 | −68% | 4,935,000 | 1,521,500 |
| sgr-dense 10k lines, 120x40 | 17,925,974 | 9,959,158 | −44% | 18,980,000 | 10,457,200 |
| CJK 10k lines, 120x40 (new) | 4,594,320 | 2,323,958 | −49% | 4,824,000 | 2,440,200 |
| scene tree, 1000 regions | 923,664 | 923,664 | 0 | 953,000 | |

The last column is what each budget would be at this branch's
measurement + 5%; the budgets stay as they are until slice 6, which
lowers them. The new CJK budget is main's measurement + 5%, like the
others. The empty cases drop by the 80 KB `VecDeque` of `CompactRow`
headers each buffer used to reserve up front. The prototype's C measured
1.20, 1.34 and 9.99 MB for typical 120x40, typical 300x75 and sgr-dense;
this is 1.17, 1.45 and 9.96. The 300x75 difference is the screen page's
per-slot heads and grapheme stores, which C's screen did not have.

A typical line costs about 105 bytes of history (84 bytes of text, a
24-byte head, and the page's share of its style list), against about 300
before.

### CJK memory (ADR Risks; open question 2)

§9 estimated that a CJK row of 60 wide characters would cost about
976 bytes as cells, 1.7 times its `CompactRow`. The Chars form stores any
one-style row without grapheme extras at four bytes a cell, which is
how CJK output arrives, so the new case measures 2.32 MB against 4.59 MB
on main: about half, not 1.7 times. Emoji rows with ZWJ sequences or
modifiers carry grapheme extras and take the Cells form, at 8 bytes a
cell plus the extras; no budget case covers them, and they are rarer
than CJK text. Open question 2 (a UTF-8 form with a wide bitmap) is
answered for CJK by Chars; for emoji it is still open.

## Wall clock: `pty_flood`

`cargo bench -p ultty --bench pty_flood` (32 MiB through a raw PTY,
120x40): bench binaries built from `fc17653` (`main`, slice 4 merged;
the docs-only merges since change no code) and this branch, each in its
own target directory, run in turn by a script, every arm once per corpus
per round, the arm order reversed on even rounds, criterion's 10 samples
per run (1 s warm-up, 3 s measurement). 10 rounds over ascii, cjk and
sgr-dense; the 1-minute load was 5.1 to 11.6 on 12 threads.

| corpus | arm | median MiB/s | IQR / median | range |
|---|---|---:|---:|---:|
| ascii | main | 240.0 | 13.2% | 186.4–248.2 |
| ascii | this branch | 227.4 | 15.4% | 200.3–248.8 |
| cjk | main | 75.1 | 19.2% | 69.4–90.0 |
| cjk | this branch | 98.8 | 20.8% | 57.3–107.0 |
| sgr-dense | main | 93.7 | 15.1% | 83.3–102.5 |
| sgr-dense | this branch | 96.9 | 6.9% | 85.8–104.1 |

Paired per-round ratios:

| corpus | branch / main, median | IQR | range |
|---|---:|---|---|
| ascii | 0.998 | 0.957–1.032 | 0.846–1.153 |
| cjk | 1.165 | 1.121–1.214 | 0.798–1.469 |
| sgr-dense | 1.036 | 0.984–1.055 | 0.941–1.139 |

**ascii is at a ceiling here, not slower.** Both arms run at about
240 MiB/s in the quiet rounds, which is where slice 3's branch already
was on this box (245–254 MiB/s in its note). Three more runs of each arm
under `/usr/bin/time` spent 2.4–4.0 s of system time against 1.6–2.4 s
of user time per run, at the same throughput in both: the PTY transfer
in the kernel, not the parse and grid work this slice cuts, sets the
rate. (`perf` is not available to this user, so this is rusage, not a
profile.) cjk, which runs at a third of that rate, shows the cut:
+16.5%, against −29% instructions; sgr-dense +3.6% is inside its spread.

So this run cannot judge the GO wall-clock criterion (ascii +15% or
better at 120x40 and 300x75, spread under 5%) for the cumulative design:
its arms are at the ceiling, its spread is 13–15% per arm, and
`pty_flood` has no 300x75 case (the prototype's `proto-flood` had). The
quiet AWS c7a.2xlarge, where slice 4's `main` ran ascii at 219 MiB/s and
the prototype's C at 291, is not at that ceiling; that rerun is slice
6's.

## Correctness

- `cargo test --workspace`, including the integrity proptests on both
  feed paths, conformance, the scorecard, `command_marks.rs`,
  `inline_anchors.rs`, `selection.rs` and the renderer's `text_cache`
  and control tests.
- Both differential-oracle tests (`live_storage_matches_reference` and
  `..._under_constant_collection`) at `ULTTY_ORACLE_CASES=30000`: pass
  (138 s). During development they found one real bug: a row with no
  cells, rewritten after a later row, kept an offset past the end of its
  page once that later row was taken back off. Rows with no payload now
  point at none; the two shrunk cases are in the regression file.
- `History::check_integrity`, run by `Buffer::check_integrity` after
  every oracle step: the row count, full pages before the last, payloads
  inside their vectors, keys valid in each page's style list, ASCII rows
  ASCII, Chars rows free of grapheme bits, extras sorted and only on
  cells with the bit. Each history row is also turned into a `Row` and
  given `Row::check_integrity`, as before.
- The round-trip proptest (`history::tests`) pushes rows from the
  generator the `CompactRow` test used, including the wide, spacer and
  combining flags only direct edits leave, and checks
  `view().to_row() == row` and, column by column, that `cell`,
  `grapheme_extra` and `text` read in place agree with the row. A second
  proptest checks that the uniform shortcut stores exactly what the
  general path stores.
- Unit tests: the form each kind of row takes; per-row eviction across a
  page boundary, the spare page, truncation and draining; styles
  remapped into each page; rewriting a row 100 times keeps its place,
  compacts and does not grow the page's styles; taking rows off the end
  gives their payload back; the head stays 24 bytes.
- `tests/screen_alloc.rs`: scrolling into a full scrollback of plain,
  styled, CJK and truecolor-plus-grapheme rows allocates nothing.
- The fuzz targets (`fuzz/`, including `vt_terminal`) build with
  `rustup run nightly-2026-08-21 cargo build --bins`. cargo-fuzz is not
  installed here, so they were not built with its sanitizer flags or run;
  CI's fuzz job does both.

## What remains for slice 6

- `grid/reference/` (the frozen `Grid`, `Row`, `Buffer` and `CompactRow`,
  and the second compile of `terminal.rs` and `term_handler.rs`) and the
  oracle that uses it. No live code uses `CompactRow` now.
- `RowView`'s owned-`Row` variant stays: the renderer's cache tests, reflow
  of the screen rows and tests build views of owned rows. `RowPacker`
  (an owned `Row` packed into a one-row page) stays for reflow,
  `push_to_history` and `replace_row`.
- The `page-proto` prototype (`grid/proto/`, `ultty-bench --suite
  proto`), if slice 6 removes it with the rest.
- Lowering the memory budgets to the last column above, the quiet-machine
  `pty_flood` and the T4 comparison, and the ADR's status.

## Open questions this bears on

1. **Byte budget for scrollback.** Rows now cost from 24 bytes (an empty
   line) to 24 + 8 × width (a styled line with emoji). Nothing here
   needed a byte budget: every case shrank. Still open; it would change
   configuration semantics.
2. **A third row form.** Done for one-style rows without extras (Chars,
   above), which covers CJK; emoji with extras still take 8 bytes a cell.
4. **Page size and the free list.** 256 rows and one spare page. A
   scrollback limit far below 256 still holds up to two pages (a 30-line
   limit keeps up to 256 rows' payload alive in the page being filled),
   bounded at about 600 KB at 300 columns of cells. `clear_history`
   frees every page but the spare.
7. **Converting cell rows when idle.** Not needed for the budget cases;
   sgr-dense rows are the only large ones, at 9.96 MB for 10k lines.

## Environment

Ryzen 5 7600 (6 cores, 12 threads), Ubuntu 26.04.1, Linux 7.0.0-42,
rustc 1.98.1, valgrind 3.27.1, release profile. Shared workstation; the
1-minute load was 15–30 during the work.
