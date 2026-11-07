# Non-ASCII VT throughput - 2026-11-07

Headless parse + grid cost for non-ASCII text was 10 to 20 times the ASCII
cost per byte. This change set brings CJK, mixed scripts, emoji sequences,
combining marks and short-line scrolling down by 62-67% each, with ASCII 18%
cheaper and SGR-dense text unchanged. Claim class: `headless-e2e`
instruction counts (deterministic). No wall-clock numbers are claimed: the
machine was at load 30-50, where A/B timing cannot resolve less than about
20%.

## Environment

| Field | Value |
|---|---|
| Base | `integration-wave1` at `ad15511`; rebased onto `16d02e5`, where every count is within 0.01% |
| Toolchain | rustc 1.98.1 (Homebrew), release profile (LTO, codegen-units 1) |
| Build | isolated `RUSTFLAGS="--cfg wt_unicode"` |
| Measurement | `scripts/bench/icount-gate.sh`, callgrind (valgrind 3.27.1), 1 MiB per corpus, 120x40, main screen |

## The gate was undercounting

The gate's callgrind filter `--toggle-collect='*run_vt_pass*'` also matched
the parser's generic instantiations over the pass's closures, such as
`process_byte::<...feed_with_ascii_spans<ultty_bench::run_vt_pass::{closure#0}>...>`.
Entering one toggled collection off until it returned, so everything below
those frames went uncounted. How much depended on inlining: on the
integration tree `vt/cjk` read 184M instructions against 870M real, and even
the committed baseline (from the bench-suite tree) was low for escape-heavy
corpora (`vt/sgr-dense` 77M recorded, 90M real). The gate now names the two
functions exactly. "Before" below is the base tree measured with the fixed
gate; the "2026-11-07 baseline" column is the published figure, for
reference.

The fixed counts also show that the integration merges made escape-heavy
input dearer than the bench-suite tree measured (same gate, both trees):
`vt/sgr-dense` 86 -> 131 instructions per byte, `parse/sgr-dense` 51 -> 93,
`vt/cursor-redraw` 44 -> 60, `vt/alt-screen` 64 -> 104. That is parser-side
and not addressed here.

## Results

Instructions per byte, `ultty-bench vt`, 1 MiB, 120x40.

| Corpus | 2026-11-07 baseline (vt) | Before (vt) | After (vt) | Change | Before (parse only) | After (parse only) |
|---|---:|---:|---:|---:|---:|---:|
| `ascii` | 84 | 84.5 | 69.2 | -18% | 11.0 | 11.0 |
| `sgr-dense` | 74 | 131.0 | 129.1 | -1% | 92.7 | 92.6 |
| `unicode-mixed` | 1011 | 1019.6 | 352.6 | -65% | 79.9 | 49.7 |
| `cjk` | 824 | 829.7 | 313.8 | -62% | 86.1 | 52.6 |
| `emoji-zwj` | 1403 | 1411.2 | 470.6 | -67% | 84.6 | 36.5 |
| `combining` | 1575 | 1578.6 | 586.3 | -63% | 88.0 | 66.4 |
| `scroll` (short lines) | 549 | 591.0 | 204.6 | -65% | 40.3 | 40.3 |
| `cursor-redraw` | 28 | 59.8 | 51.0 | -15% | 35.3 | 35.3 |
| `scroll-region` | 39 | 58.3 | 41.9 | -28% | 19.8 | 19.8 |
| `alt-screen` | 50 | 103.9 | 101.9 | -2% | 81.2 | 81.2 |
| `wide-wrap` | 71 | 69.5 | 64.2 | -8% | 8.1 | 8.1 |

`bench/baselines/icount.json` holds the new counts.

## What changed

One commit each, in order; every commit message carries its own deltas.

1. **Blank-tail scan.** History encoding found a row's last non-default cell
   with the derived `==`, a branch per field per cell. A branch-free check
   over blocks of eight cells (`Cell::is_default`): scroll -21%.
2. **One dirty mark per scroll.** Scrolling, IL and DL bumped the content
   version once per row of the region (40 per line of output); the moved
   rows now share one new version: scroll -13%, ascii -7%.
3. **Used mark per row.** A row remembers a column past which every cell is
   default. Erasing a row and encoding it for history skip the tail, so a
   short line no longer costs two passes over 120 columns. Every cell write
   raises the mark; the integrity checker verifies it, and the hostile-stream
   proptest now runs through the ASCII-span path the terminal really uses
   (dropping the raise in `Row::write_ascii` passed the old proptest and
   fails the new one): scroll -48%.
4. **Grapheme joins without the segmenter.** With mode 2027 on, every
   codepoint from U+0300 copied the previous cluster into a `String` and ran
   UAX #29 over it. Two classes answer most cases directly: nothing joins a
   *plain* codepoint (Other, Extended_Pictographic, Hangul syllables, outside
   Indic conjuncts) to a cluster ending in plain or Extend, and Extend marks
   and ZWJ always join a plain codepoint followed by more of them. The tests
   check every codepoint in those ranges against the segmenter, so a Unicode
   update that moves one fails instead of changing behaviour: cjk -46%.
5. **Decide once.** `Buffer::write_char` and `Grid::write_char` each asked
   whether the codepoint joined and how wide it was; the buffer now hands
   its answer down: emoji-zwj -31%, unicode-mixed -20%.
6. **Binary search** for the plain class instead of a chain of range
   comparisons: cjk -7%.
7. **Grapheme map updates in place** instead of splitting and re-joining
   the map on every span write: combining -13%, unicode-mixed -15%.
8. **Cluster under the cursor on the stack** for the segmenter and the
   widening check, instead of a new `String` each time: emoji-zwj -21%.
9. **Sorted vector for a row's clusters.** Output goes left to right, so the
   cluster being extended is the last entry and new writes land past every
   entry; checking the end first replaces three `BTreeMap` searches per mark.
   A proptest compares it with the ordered map: combining -22%.
10. **Wider cheap classes:** variation selectors, emoji modifiers, tags and
    ZWJ join without the segmenter. Emoji sequences reach it only for the
    pictograph after a ZWJ and for flags: emoji-zwj -11%.
11. **Whole UTF-8 characters in the span feeder.** The parser decoded each
    non-ASCII byte through the state machine; runs of whole, well-formed
    characters are now decoded at once, out of line so the escape-heavy loop
    keeps its code layout (an inline version cost sgr-dense 4-8%). A
    proptest holds span feeding to byte-wise feeding over split and
    ill-formed input: cjk -9%, parse-only cjk 86 -> 53.
12. **Erase a span with one fill.** Through `get_mut`, each EL raised the used
    mark to the end of its row (+7% on cursor-redraw after step 3); one fill
    that respects the mark fixes that and skips the blank tail:
    cursor-redraw -20%, scroll-region -24%.

## Where the time goes now

- `cjk`: the parser's byte loop and UTF-8 decode (53 of 314), the cell write
  for wide characters, and history encoding (~50, one UTF-8 encode and style
  compare per cell including spacers).
- `combining`: per-mark cluster bookkeeping; each cluster still owns a
  `Vec<char>` and is copied into a boxed slice when it scrolls into history.
  A per-row codepoint pool would remove those allocations.
- `emoji-zwj`: the segmenter for pictographs after ZWJ (about a quarter).
  The stored cluster is always a single grapheme when it can still be
  extended, so a boundary check at the end would do; that relies on an
  invariant worth its own change.
- `scroll`: rotating the 40 rows (a 2 KiB memmove per line) and encoding the
  line for history.

## Verification

- `cargo test -p ultty-vt`: unit, conformance, integrity (now both feed
  paths), selection; the new class tests cover every codepoint in the cheap
  ranges.
- `cargo check --offline --manifest-path fuzz/Cargo.toml --bins`.
- `scripts/bench/icount-gate.sh --update --skip-memory` regenerated the
  baseline. The heap-bytes bench (`cargo test -p ultty --bench memory`) was
  not run, to stay within the free disk space (it builds all of `ultty`).
  `Row` stays 56 bytes and `CompactRow` is unchanged; only rows holding
  grapheme clusters store them differently.
