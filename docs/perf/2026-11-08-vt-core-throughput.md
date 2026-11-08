# VT core throughput: the I/O thread's per-byte cost (2026-11-08)

Follows [2026-11-08-live-throughput.md](2026-11-08-live-throughput.md),
which found live throughput bound by the PTY I/O thread parsing at full
CPU, and listed what was left in the VT core: scrollback encoding,
grapheme joins that fell back to the UAX #29 segmenter, and
`print_utf8_run` decoding UTF-8 twice. This note records the changes made
for those three and two more the profile showed afterwards.

Same workstation as before (Ryzen 5 7600), load 6 to 17 from other
builds during the wall-clock runs. Instruction counts are from
`scripts/bench/icount-gate.sh` (callgrind inside the timed pass of
`ultty-bench vt`, 1 MiB, 120x40) and are deterministic.

## Changes kept

One commit each, deltas against the commit before.

| commit | what | instructions |
|---|---|---|
| Copy plain rows into scrollback without looking at their styles | `Row` records whether every cell is ASCII in the default style, the way it keeps its used mark; encoding such a row is a copy of its characters with trailing spaces trimmed | ascii -29.9%, wide-wrap -31.8%, scroll -9.5%; alt-screen +1.0%, sgr-dense +0.9% |
| Validate and decode UTF-8 runs in one pass | `decode_utf8` checks the well-formedness table and assembles the code point in one pass, replacing `from_utf8` then `chars()` | parse/unicode-mixed -25.7%, parse/cjk -24.3%, parse/emoji-zwj -33.1%; vt/unicode-mixed -3.6%, vt/cjk -3.9% |
| Join emoji ZWJ sequences and flags without the segmenter | `cluster::joins` applies GB9, GB11 and GB12/13 itself to clusters of plain, extend, ZWJ and regional-indicator codepoints | emoji-zwj -35.0%, unicode-mixed -4.5% |
| Keep scrollback grapheme extras in the row's text | extra codepoints follow their cell's character in the compact row's UTF-8 text instead of a boxed slice per cluster | combining -9.9%, emoji-zwj -5.7%, unicode-mixed -2.5% |
| Write a printed character's row once instead of cell by cell | `write_measured_char` marks the row dirty once and writes cells directly instead of through `cell_mut` for each | cjk -4.3%, unicode-mixed -2.4% |
| Build each ASCII cell in one store | `write_ascii` builds the cell with its character instead of copying the template and overwriting the character | ascii -2.6%, cursor-redraw -2.5%, scroll-region -3.3% |

The baseline is regenerated in its own commit.

### Instructions per byte, before and after

`ultty-bench vt` and `--suite parse`, 1 MiB, 120x40; base is
`origin/live-throughput` (7f9b98f).

| corpus | vt before | vt after | change | parse before | parse after | change |
|---|---:|---:|---:|---:|---:|---:|
| ascii | 57.4 | 39.0 | -32.0% | 3.9 | 3.9 | 0 |
| sgr-dense | 85.5 | 85.7 | +0.3% | 48.7 | 48.7 | 0 |
| unicode-mixed | 352.2 | 309.2 | -12.2% | 48.9 | 36.3 | -25.7% |
| cjk | 312.9 | 289.6 | -7.5% | 51.2 | 38.8 | -24.3% |
| emoji-zwj | 470.2 | 275.9 | -41.3% | 36.1 | 24.1 | -33.1% |
| combining | 587.2 | 516.9 | -12.0% | 66.9 | 53.2 | -20.5% |
| cursor-redraw | 33.8 | 33.2 | -2.0% | 18.2 | 18.2 | 0 |
| scroll | 195.4 | 174.3 | -10.8% | 28.5 | 28.5 | 0 |
| scroll-region | 32.2 | 31.4 | -2.4% | 10.1 | 10.1 | 0 |
| alt-screen | 63.5 | 63.4 | -0.2% | 42.7 | 42.7 | 0 |
| wide-wrap | 52.6 | 34.6 | -34.1% | 1.6 | 1.6 | 0 |

The memory budgets (`cargo test -p ultty --bench memory`) pass with the
same live bytes as before: the plain flag fits in `Row`'s padding, and the
budget corpora have no grapheme clusters.

### `pty_flood`, end to end

`cargo bench -p ultty --bench pty_flood` (32 MiB through the real I/O
thread, no GPU). Base and final binaries alternated, 8 rounds each,
criterion's mean per round, MiB/s. The box was busy: in some rounds one
arm lost more than half its throughput to other work, so the best round is
the steadier number.

| corpus | before: best | median | after: best | median |
|---|---:|---:|---:|---:|
| ascii | 158.2 | 151.7 | 181.5 | 175.6 |
| unicode-mixed | 34.1 | 32.1 | 39.5 | 39.0 |
| cursor-redraw | 178.1 | 172.0 | 175.8 | 173.4 |
| sgr-dense | 96.7 | 94.0 | 95.0 | 93.1 |

ascii +15%, unicode-mixed +16%; cursor-redraw and sgr-dense are unchanged
within the noise (their per-byte cost moved by 2% or less).

## Details

**Plain rows.** The profile put `CompactRow::encode_from` at 38% of ascii
even with the one-style fast path: about 1,900 instructions per row, most
of it comparing each cell's style with the first (`#[derive(PartialEq)]`
on five fields, short-circuiting). The writer already knows when a row is
plain, so the row keeps a flag with the same contract as its used mark:
it may understate, never overstate. `write_ascii` with a default-style
template and erasing to the default background keep it; `get_mut`,
`IndexMut`, `cells_mut`, grapheme writes, insert/delete and colored erases
clear it. `Row::check_integrity` verifies it, so the integrity proptests
(both feed paths) check every row they touch. A proptest compares the
shortcut with the per-cell encoder on random plain rows.

**Grapheme joins.** In unicode-mixed, 37k codepoints per MiB went to the
segmenter. For clusters made only of the classes `cluster.rs` already
knew (plain, extend) plus ZWJ and regional indicators, only GB9, GB11 and
GB12/13 can hold, so `joins` walks the cluster with those three rules.
GB11 needs Extended_Pictographic: a generated table of the plain
codepoints that have it, checked against the segmenter for every plain
codepoint in both GB11 positions. A test enumerates every cluster of up
to four codepoints over samples of each known class and of the classes it
leaves to the segmenter (SpacingMark for GB9a, InCB consonant and linker
for GB9c, Prepend, Hangul jamo, controls, ZWNJ), and checks `joins`
against the segmenter for every next codepoint. Devanagari is what still
reaches the segmenter in unicode-mixed (23k calls per MiB, about 6%).

**UTF-8.** `print_utf8_run` stays a run loop; only the per-character step
changed. The decoder is tested against `str::from_utf8` on every scalar
value, whole and cut short, and on every lead byte with every second
byte; the span-vs-bytewise proptest with its CSI fragments covers it in
the parser.

## Measured and dropped

- **Branch-free style comparison in the per-cell encoder** (XOR and OR of
  every field instead of the derived `==`): cjk +2.3%, sgr-dense +2.6%,
  unicode-mixed +1.5% against the derived comparison, which exits early on
  the first differing field and so is cheaper for the short runs styled
  and wide text produce.
- **Lazy encoding** (keeping recently scrolled rows as `Row`s in a ring and
  compacting them later) was not built: every row is encoded once either
  way, and the cost per row was the comparison, which the plain flag
  removes for plain text. Ghostty avoids the copy by storing scrollback in
  the same 8-byte cells as the screen; our cells are 24 bytes, which is
  what the compact form exists to avoid.

## What is left

- **unicode-mixed** is now spread thin: `write_measured_char` (about 235
  instructions per character), `continues_cluster` (about 130 per call,
  mostly the class lookups), the per-cell encoder for styled and wide rows
  (a wide character and its spacer are two style runs), and Devanagari
  through the segmenter. Indic conjunct rules (GB9a, GB9c) in
  `cluster.rs` would remove the last.
- **ascii**: writing cells (12 instructions per byte), erasing the
  recycled row, and history allocation while it fills (at 1 MiB the
  10,000-line history is still filling; a long flood reuses rows).
- **sgr-dense** was not touched: parsing SGR and applying attributes is
  most of it.
