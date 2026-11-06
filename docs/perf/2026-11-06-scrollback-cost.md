# Where VT throughput goes: scrollback, not the render lock (2026-11-06)

[2026-11-05-io-thread.md](2026-11-05-io-thread.md) found that a large
window cut `cat` throughput by about a third, and backlog item 6 blamed the
renderer holding the terminal lock. These measurements say otherwise. The
cost is in the VT core, and most of it is scrollback living outside the
fastest caches.

All numbers come from `crates/ultty-vt/examples/throughput` (release, no
renderer, no PTY) on the 64 MiB test file of 84-character lines, unless
noted otherwise. Ryzen 5 7600: 1 MiB L2 per core, 32 MiB L3. Runs vary by
about ±10%.

## 1. The render lock is not the bottleneck

- **Live, 2560x1440 window.** `cat` ran at 72-78 MiB/s with
  `ULTTY_TRACE_PIPELINE=1`.
- **Frame time.** 163 frames, 1.25 ms at p50 and 3.7 ms at p99.
- **Lock held.** 216 ms in total, about 8% of the run.
- **Headless.** At the same grid size (300x75), with no renderer and no
  lock, throughput was already 80 MiB/s.

Rendering from a snapshot could win back at most the ~8% of time the lock
is held, not the gap.

## 2. Scrollback is the cost

| run | 120x40 | 300x75 |
|---|---:|---:|
| main screen (10,000 rows of scrollback) | 106-124 MiB/s | 73-86 MiB/s |
| alternate screen (no scrollback) | 301-303 MiB/s | 259-271 MiB/s |

With history enabled there are 45% more instructions (1.66 billion
against 1.15 billion on an 8 MB sample), but the run takes 2.7x as long.
The difference is memory stalls. A cache simulation puts 94% of last-level
write misses in the `memcpy` that copies the top row into history.

Shrinking history isolates it (120x40, temporary experiment):

| scrollback rows | throughput |
|---:|---:|
| 100 | 211-225 MiB/s |
| 1,000 | 217-220 MiB/s |
| 10,000 (default) | 106-110 MiB/s |

Each new line is written into the oldest history row, which was last
touched 10,000 lines earlier. With history in L2 that costs little; once
history outgrows L2, every line stalls on L3.

## 3. Things tried that did not help

These were measured and reverted:

- **A ring buffer for grid rows** (`VecDeque`, O(1) scroll). No change:
  moving 75 `Row` headers was never the cost.
- **A `memcpy`-based row fill.** Fewer instructions (`slice::fill` writes
  20-byte cells one at a time), but no change in wall-clock time.
- **Scrollback rows without trailing blanks.** History bytes fell from
  2.4 KB to 1.7 KB per line at 120 columns, and from 6 KB to 1.7 KB at 300.
  Throughput did not move: 1.7 KB x 10,000 rows is still far beyond L2.

## What would help

A compact scrollback format: text bytes plus style runs instead of 20-byte
cells. That is roughly 100 bytes for a typical line, so 10,000 lines fit in
about 1 MiB. Readers would decode the few rows they show. Backlog item 6
replaces the former items 6 and 7 with this.

## Result: compact scrollback (2026-11-06)

Scrollback rows are now stored as one UTF-8 character per cell plus style
runs, without trailing blanks (`crates/ultty-vt/src/grid/history.rs`).

**The benchmark above was misleading.** Its file uses bare `\n`, with no
carriage return, so fed headless each line started where the last one
ended and filled whole rows. A real PTY turns `\n` into `\r\n`. The
figures below use a CRLF copy of the same file, so lines are 84 cells as
they are live.

The machine was under load (load average about 8 on 12 cores), so these are
the best of 8 interleaved runs per configuration. MiB/s:

| | before | compact | alternate screen (ceiling) |
|---|---:|---:|---:|
| 120x40 | 202 | 223 | ~420 |
| 300x75 | 101 | 125 | ~255 |

- **Memory.** 10,000 lines of typical output at 300 columns: 2.4 MB in
  history, down from 57 MB (test `a_full_scrollback_of_typical_output_stays_small`).
- **History size no longer matters.** With compact rows, 1,000 and
  100,000 rows of history run at about the same speed; the cache effect is
  gone.
- **What is left.** Encoding now costs about 3,500 instructions a line,
  mostly comparing each cell's style, because `Color` is an enum and is
  compared field by field. Closing the rest of the gap to the alternate
  screen would mean a different `Color` representation. That reaches
  through the whole codebase and is not done here.
