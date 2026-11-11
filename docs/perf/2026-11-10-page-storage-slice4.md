# Page storage slice 4: the screen page (2026-11-10)

[ADR 0022](../adr/0022-page-storage.md) migration slice 4: `Grid.rows`
becomes a `ScreenPage`, and scrolling, scroll regions, IL and DL become
slot rotations. Its gate is the scroll and scroll-region instruction cuts,
with a kill rule on `pty_flood`: stop if the cumulative gain is under half
of what the [prototype](2026-11-09-page-storage-prototype.md) predicted for
these levers. This note is those measurements, plus the memory bench.

**Verdict.** vt/scroll is −11.1% and vt/scroll-region −6.5% against the
committed baseline (slice 3 plus the CJK print path), and −23.2% and
−32.6% against the last baseline before slice 3. The prototype's C
reached −37% on scroll; most of what is left is history encoding, which is
slice 5. Four corpora that write a few cells per row access pay for the
slot lookup: cjk +1.7%, cursor-redraw +2.1%, alt-screen +1.8%, sgr-dense
+1.0%. `pty_flood` on a saturated
workstation puts the cumulative ascii gain since before slice 3 at +24.7%
(paired median, IQR +8% to +53%): the kill rule, at its strictest reading
(half of C's +44.8%), does not fire, but the spread is too wide to call
that confirmed.

## What was built

- `grid/screen.rs`: `ScreenPage` holds every screen row's packed cells in
  one `Vec` (slot `s` is cells `s * cols..(s + 1) * cols`), a slot table
  (`slots[r]` is the slot screen row `r` shows) and, per slot, a
  `RowHead` (`used`, `uniform` and its style key, wrap, marks, anchor; 16
  bytes) next to a `GraphemeStore`. `ScreenRow` and `ScreenRowMut` borrow
  one slot with its head and store, and carry the row operations
  `PackedRow` had (`write_ascii`, `write_char_in_place`, `erase`,
  `erase_range`, `insert_cells`, `delete_cells`, grapheme pushes, the
  integrity check). `PackedRow` is gone; reflow's scratch row is a one-row
  page.
- Scrolling (`scroll_up`, `scroll_down`), regions, IL and DL call
  `rotate_up` / `rotate_down`: rotate `slots[top..=bottom]` and erase the
  rows that come in. A rotation by one, which every line feed on the
  bottom margin is, moves the slots with `copy_within` instead of
  `slice::rotate_left`: the generic rotate cost about 100 instructions on
  40 slots, the `memmove` about 20. Clearing a whole region is unchanged
  (each row erased and versioned on its own, as before).
- Dirty flags, row versions and `content_version` stay per screen row and
  are bumped exactly where they were. `LastPrint`, marks (ADR 0019) and
  anchors (ADR 0020) are unchanged; marks and anchors live in the heads and
  move with their slot.
- The style table's collection walks the page's heads and used cells
  (`for_each_key`, `remap_keys`).
- Height and width resizes without reflow build a new page, copying rows
  in screen order (`ScreenPage::resized`); `set_rows` and `prepend_rows`
  write owned `Row`s into slots; `remove_top_rows` is a rotation.
- Readers outside the grid did not change: `scene/raster.rs`,
  `a11y/tree.rs` and the renderer's `text_cache` read through `RowView`
  and `Grid` accessors, whose signatures are the same. The renderer cache
  fast path (ADR open question 3) is not part of this slice.

### Graphemes: one store per slot, not per page

ADR §4 drew one `GraphemeStore` per page, an index of (cell, start, len)
sorted by cell index. That was built first and measured: vt/combining
+105%, emoji-zwj +47%, unicode-mixed +36%. After rotation a slot's cells
can sit anywhere in cell-index order, so printing into the bottom row
inserted into the middle of the index, and erasing a scrolled-in row
removed from the middle; each moved every entry after it. Appends also
never reached the compaction check, so `chars` only grew.

What shipped is a `GraphemeStore` per slot: a column-sorted index of
(col, len, start) and one `Vec<char>` of runs. A codepoint is appended in
place while its cluster is the last run; otherwise the run moves to the
end, and garbage is compacted in place (sort by start, `copy_within`,
sort back) once it passes 32 codepoints and half the store. Erasing the
slot clears both vectors and keeps their capacity, so a slot that has held
graphemes holds them again without allocating. The head's `GRAPHEMES` flag
lets writes skip the store entirely on rows that never had a cluster, and
a check of the index's last column skips it for writes past every
cluster, as `GraphemeMap::any_in` did. Against the committed baseline:
combining −10.3%, emoji-zwj −7.2%, unicode-mixed −0.3%.

## Instructions

`scripts/bench/icount-gate.sh --threshold 1` against
`bench/baselines/icount.json` (slice 3 plus the CJK print path, `da60b34`),
1 MiB, 120x40. "Before slice 3" is the baseline committed at `ad6a334`,
the same toolchain.

| case | baseline | this branch | change | before slice 3 | change since |
|---|---:|---:|---:|---:|---:|
| vt/ascii | 31,453,447 | 29,733,916 | −5.47% | 41,411,348 | −28.2% |
| vt/sgr-dense | 91,805,516 | 92,750,038 | +1.03% | 91,608,930 | +1.2% |
| vt/unicode-mixed | 178,304,940 | 177,720,424 | −0.33% | 231,339,899 | −23.2% |
| vt/cjk | 175,295,352 | 178,288,650 | +1.71% | 229,814,050 | −22.4% |
| vt/emoji-zwj | 165,009,377 | 153,187,098 | −7.16% | 199,684,131 | −23.3% |
| vt/combining | 422,922,185 | 379,263,475 | −10.32% | 432,156,702 | −12.2% |
| vt/cursor-redraw | 29,414,242 | 30,040,064 | +2.13% | 34,965,334 | −14.1% |
| vt/scroll | 162,581,570 | 144,591,432 | **−11.07%** | 188,247,015 | **−23.2%** |
| vt/scroll-region | 24,082,012 | 22,516,106 | **−6.50%** | 33,418,613 | **−32.6%** |
| vt/alt-screen | 67,446,319 | 68,683,873 | +1.83% | 66,722,562 | +2.9% |
| vt/wide-wrap | 26,870,232 | 25,879,899 | −3.69% | 36,729,770 | −29.5% |

Every parse case is 0.00%.

### Against the prototype

The prototype's C, second round, 1 MiB at 120x40: scroll −37.3% against
its reference. This branch is at −23.2% from the pre-slice-3 baseline, 62%
of C's cut. Where the rest is, from callgrind on vt/scroll (inclusive,
per 1 MiB, about 132,900 line feeds):

| part | instructions | per line feed |
|---|---:|---:|
| `Buffer::save_top_rows` (history: `CompactRow` encode, `VecDeque`, anchors) | 39.1 M | 294 |
| `Grid::scroll_up` (rotation, erase, dirty and version marking) | 34.8 M | 262 |
| of which `ScreenPage::rotate_up` | 16.8 M | 127 |
| printing the line (`write_ascii_span`) | 26.9 M | 202 |

History is the largest part and is slice 5's. Of `scroll_up`, about 55
instructions per line feed are the region's 40 row versions and 40 dirty
flags, which the slice keeps as they are; the rotation's `memmove` is
about 22 and erasing the incoming row most of the rest. The prototype had no scroll
region, anchors or per-row versions to keep, and no history encoder of
today's shape.

scroll-region has no prototype number (the prototype's region was always
the full screen).

### What the four regressions are

cjk, cursor-redraw, alt-screen and sgr-dense print one character, or a
short span, per row access. A row access now loads the slot from the slot
table and indexes the head array and the cell range, where it used to
index one `Vec<PackedRow>`: two more bounds checks and loads, about 10
instructions per access. For cjk that is about 300,000 prints per MiB.
Measured, not guessed: replacing the three checks with unchecked indexing
(an experiment, not kept; the house rule is no `unsafe` without need)
took cjk from +2.6% to +0.95% at the time, and alt-screen by 0.3%. Putting
the head and the grapheme store in one array, so one check covers both,
is what is kept of that. All four are under the CI job's 3% limit.

## Allocation

`crates/ultty-vt/tests/screen_alloc.rs` installs a counting global
allocator (the pattern from `crates/ultty/benches/memory.rs`, counting per
thread) and checks that these allocate nothing on a 40x12 screen full of
styled text, wide characters, combining marks, a ZWJ sequence and a shell
mark: IL and DL at three rows, IL past the bottom, IL and DL inside a
region, scroll up by 1 and 5, scroll down, scrolling everything off,
region scrolls both ways, and 30 line feeds on the bottom margin of a
region that keeps no history. On `main` the first IL allocated 48 times.

## Memory

`cargo test -p ultty --bench memory` (live heap bytes, counting
allocator). "main" is slice 3's column, which the CJK print path left
unchanged.

| case | main | this branch | change | budget |
|---|---:|---:|---:|---:|
| empty 80x24 | 115,254 | 116,886 | +1,632 | 185,000 |
| empty 300x75 | 450,044 | 455,144 | +5,100 | 1,229,000 |
| typical 10k lines, 80x24 | 2,715,254 | 2,716,886 | +1,632 | 2,915,000 |
| typical 10k lines, 120x40 | 3,143,254 | 3,145,974 | +2,720 | 3,461,000 |
| typical 10k lines, 300x75 | 3,430,044 | 3,435,144 | +5,100 | 4,358,000 |
| typical 120x40, resized (live) | 3,141,474 | 3,144,194 | +2,720 | 3,461,000 |
| typical 120x40, resized (peak) | 4,567,640 | 4,570,094 | +2,454 | 4,935,000 |
| sgr-dense 10k lines, 120x40 | 17,923,254 | 17,925,974 | +2,720 | 18,980,000 |
| scene tree, 1000 regions | | 923,664 | | 953,000 |

Every case is within budget. Each case grows by 34 bytes per screen row
(main and alternate screens): a slot's head and grapheme store are 72
bytes, plus 2 for its slot entry, against the 40-byte `PackedRow`; the
cells themselves, 8 bytes each, are the same bytes in one allocation
instead of one per row. History is still `CompactRow` (slice 5).

## Wall clock: `pty_flood`

`cargo bench -p ultty --bench pty_flood` (32 MiB through a raw PTY,
120x40): three bench binaries, built from `ad6a334` (before slice 3),
`da60b34` (`main`, slice 3 plus the CJK print path) and this branch, run
in turn by a script, every arm once per corpus per round, the arm order
reversed on even rounds, criterion's 10 samples per run (1 s warm-up, 3 s
measurement). Two batches, 6 and 14 rounds, 20 in all.

The workstation was saturated by other work: the 1-minute load was 19.6
to 51 on 12 threads during the runs, and every arm ran at about a sixth of
its quiet-machine rate (`main` ascii 38 MiB/s here, about 250 on the same
box at load 3–6). These numbers are what the box gave; they are not good
enough to settle a 10% question.

| corpus | arm | median MiB/s | IQR / median | range |
|---|---|---:|---:|---:|
| ascii | before slice 3 | 32.5 | 39% | 22.3–58.8 |
| ascii | main | 38.3 | 32% | 22.4–89.2 |
| ascii | this branch | 41.5 | 40% | 28.4–94.0 |
| cjk | before slice 3 | 17.8 | 27% | 14.3–31.3 |
| cjk | main | 20.2 | 25% | 15.1–37.1 |
| cjk | this branch | 21.3 | 34% | 14.2–37.2 |

Paired per-round ratios (median, IQR):

| corpus | branch / before slice 3 | branch / main | main / before slice 3 |
|---|---|---|---|
| ascii | 1.247 (1.078–1.533) | 1.157 (0.984–1.392) | 1.077 (1.011–1.348) |
| cjk | 1.117 (1.054–1.241) | 0.966 (0.915–1.094) | 1.173 (1.062–1.250) |

Per batch, branch over before-slice-3 was 1.218 and 1.340 on ascii, 1.134
and 1.117 on cjk.

### The kill rule

The rule: stop if the cumulative `pty_flood` gain is under half of what
the prototype predicted for these levers. The prototype measured packed
cells alone (A, ascii +27.0%) and the whole design (C, +44.8% on the
quiet machine, +42.4% on this workstation); it did not measure the screen
page without history pages, so the prediction for slices 3 and 4 lies
between A and C. Taking the strictest reading, half of C: +22.4%.

The cumulative ascii gain measured here is +24.7% (paired median over 20
rounds), so **the kill rule does not fire on the median**, under any
reading. It does not clear the strictest threshold with any margin: the
interquartile range of the ratio runs from +7.8% to +53%, and the first
batch alone gave +21.8%. Under the looser readings (half of A, +13.5%, or
anywhere between that and C) the median clears it comfortably. The
honest summary is "not fired, not confirmed": the instruction counts
(ascii −28%, scroll −23% since before slice 3) are the reliable signal,
and the wall clock should be rerun on a quiet machine or on the T4 before
slice 5, as the ADR's risk section already plans for slice 6.

The incremental slice-4 ratio (branch over `main`, +15.7% ascii, −3.4%
cjk) is inside the noise in both directions; slice 4's instruction cut on
ascii is 5.5% and its cost on cjk 1.7%, which would put the expected
wall-clock change at a few percent either way.

## Correctness

- `cargo test --workspace`, including both differential-oracle tests
  (`live_storage_matches_reference` and `..._under_constant_collection`)
  at `ULTTY_ORACLE_CASES=30000`, the integrity proptests on both feed
  paths, conformance, the scorecard, `command_marks.rs` and
  `inline_anchors.rs`.
- `ScreenPage::check_integrity` adds to the row checks: the slot table is
  a permutation, the heads and cells match the page's size, and each
  grapheme store is sorted, its runs inside `chars`, and its garbage
  counted exactly; a grapheme bit in a row whose head says it has none is
  an error.
- New unit tests: rotation moves graphemes with their row and clears the
  rows that come in; inserting and deleting cells moves graphemes; two
  clusters grown in turn are compacted and stay bounded; resizing keeps
  rows in screen order. `PackedRow`'s row tests now run on one-row pages.

## Environment

Ryzen 5 7600 (6 cores, 12 threads), Ubuntu 26.04.1, Linux 7.0.0-42,
rustc 1.98.1, valgrind 3.27.1, release profile. Shared workstation; the
1-minute load was 35–60 throughout.

## Quiet-machine rerun

The `pty_flood` comparison above, rerun on an idle machine to settle the
kill rule. Same method, with sgr-dense added.

- AWS c7a.2xlarge (AMD EPYC 9R14, 8 vCPUs, no SMT), us-east-1, on demand,
  as in the prototype's [third round](2026-11-09-page-storage-prototype.md#third-round-quiet-machine).
  Ubuntu 26.04.1, kernel 7.0.0-1016-aws, rustc 1.98.1. No cpufreq
  interface; not pinned.
- Three bench binaries, built on the instance with
  `cargo bench --locked -p ultty --bench pty_flood --no-run` from `git
  archive` of `ad6a334` (before slice 3), `da60b34` (slice 3 plus the CJK
  print path) and `1837151` (`main`, this slice merged).
- The harness as above: every arm once per corpus per round, the order
  reversed on even rounds, criterion's 10 samples per run (1 s warm-up,
  3 s measurement). Three batches of 15 rounds over ascii, cjk and
  sgr-dense, 45 rounds in all, after one discarded trial round.
- Quiet: the timers that could fire (sysstat, apt, man-db, fwupd, fstrim,
  tmpfiles, update-notifier, motd-news, dpkg-db-backup, logrotate,
  e2scrub), snapd and unattended-upgrades were stopped. The 1-minute load
  was 0.82 before the first batch and 0.82 to 1.83 during the runs, which
  is the harness itself.

| corpus | arm | median MiB/s | IQR / median | range |
|---|---|---:|---:|---:|
| ascii | before slice 3 | 164.6 | 0.0% | 164.6–164.7 |
| ascii | slice 3 + CJK | 202.3 | 2.6% | 197.3–214.3 |
| ascii | `main` | 219.1 | 0.0% | 217.8–219.1 |
| cjk | before slice 3 | 60.1 | 0.0% | 55.1–60.1 |
| cjk | slice 3 + CJK | 73.1 | 0.7% | 70.5–73.4 |
| cjk | `main` | 73.4 | 0.0% | 73.1–73.4 |
| sgr-dense | before slice 3 | 86.1 | 0.0% | 83.2–86.1 |
| sgr-dense | slice 3 + CJK | 82.4 | 0.0% | 82.4–82.5 |
| sgr-dense | `main` | 82.5 | 0.0% | 80.8–82.5 |

"0.0%" is under 0.05%: those arms ran within a few hundredths of a MiB/s
of each other in nearly every round. The slice-3 arm on ascii is the only
one that wanders, between about 197 and 214 MiB/s from round to round.

Paired per-round ratios, 45 rounds: median and 95% percentile bootstrap
interval over rounds (10,000 resamples).

| corpus | `main` / before slice 3 | `main` / slice 3 + CJK | slice 3 + CJK / before slice 3 |
|---|---|---|---|
| ascii | **1.331** (1.330–1.331) | 1.077 (1.066–1.088) | 1.229 (1.223–1.248) |
| cjk | 1.222 (1.222–1.222) | 1.004 (1.000–1.004) | 1.217 (1.217–1.222) |
| sgr-dense | 0.958 (0.958–0.958) | 1.000 (1.000–1.000) | 0.958 (0.958–0.958) |

| batch | 1-min load before → after | `main` / before slice 3: ascii, cjk, sgr-dense |
|---|---|---|
| 1 | 0.82 → 1.34 | 1.330, 1.222, 0.958 |
| 2 | 1.34 → 1.34 | 1.331, 1.222, 0.958 |
| 3 | 1.34 → 1.69 | 1.330, 1.222, 0.958 |

### The kill rule, settled

The cumulative ascii gain since before slice 3 is **+33.1%** (95% CI
+33.0% to +33.1%), against the strictest threshold of +22.4% (half of
C's +44.8%). **The kill rule does not fire**, and the whole interval
clears the threshold by more than ten points; every batch agrees to
0.1%. Slices 3 and 4 have delivered 74% of the gain the prototype's full
design predicted, with history pages (slice 5) still to come.

The workstation's +24.7% understated the gain, and its slice-4 increment
(+15.7% ascii, −3.4% cjk) was noise in both directions. On the quiet
machine slice 4 is worth +7.7% on ascii (CI +6.6% to +8.8%), more than its
5.5% instruction cut, and nothing measurable on cjk (+0.4%) or sgr-dense
(0.0%), where the instruction counts had predicted +1.7% and +1.0%.

sgr-dense is 4.2% slower than before slice 3, all of it from slice 3.
That is the prototype's C result again (about 4% slower in time on
sgr-dense), whose causes are in the prototype note's sgr-dense section.
It does not bear on this kill rule, which is about ascii.
