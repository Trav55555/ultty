# Unicode write path: the VT core's per-character cost (2026-11-08)

Follows [2026-11-08-vt-core-throughput.md](2026-11-08-vt-core-throughput.md).
Live throughput is bound by the PTY I/O thread parsing at full CPU
([2026-11-08-live-throughput.md](2026-11-08-live-throughput.md)), and on the
AWS T4 box Ultty ran unicode-mixed at 18 MiB/s against Ghostty's 25 (ASCII
95 against 131). That note left non-ASCII text at about 300 instructions
per byte, spread over writing the character (`write_measured_char`, about
235 per character), deciding whether it joins the previous grapheme
cluster (`continues_cluster`, about 130 per call), two style runs per wide
character in scrollback, and Indic conjuncts through the UAX #29
segmenter. This note records what was changed for those and what the
profiles showed after each step.

Same workstation (Ryzen 5 7600), shared with other builds. Instruction
counts are from `scripts/bench/icount-gate.sh` (callgrind inside the timed
pass of `ultty-bench vt`, 1 MiB, 120x40) and are deterministic; profiles
are `callgrind_annotate` on a release build with line tables
(`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`).

## Changes kept

One commit each, deltas in instructions against the commit before (vt
suite, 1 MiB).

| commit | what | instructions |
|---|---|---|
| Keep wide characters and their spacers in one scrollback style run | WIDE_CHAR, WIDE_CHAR_SPACER and HAS_COMBINING follow from a row's structure, so the per-cell encoder leaves them out of the styles and records them in the per-column marks it kept for grapheme extras; a row whose flags do not follow from its marks keeps them in its styles | cjk -5.9%, unicode-mixed -2.4%, emoji-zwj -1.9%, combining -1.3%; sgr-dense +1.7%, scroll +1.4% (fixed below), ascii +0.5% |
| Look up a printed character's width and cluster class in one table | a build script tabulates unicode-width's width and the cluster class for every codepoint below U+40000 in a two-level table | emoji-zwj -4.9%, combining -3.5%, unicode-mixed -2.9%, cjk -2.6% |
| Remember how the last printed cluster ends instead of reading it back | the grid records the class of a cluster's last codepoint when it prints, so the join check does not read the cell and its extras back | cjk -2.9%, emoji-zwj -2.5%, unicode-mixed -2.0%, combining -1.3% |
| Join Indic conjuncts and spacing marks without the segmenter | spacing marks (GB9a), Indic_Conjunct_Break consonants and linkers and the Indic extends become classes, and the recorded end tracks an open conjunct (GB9c) | unicode-mixed -8.8%; emoji-zwj +1.7%, combining +1.1%, cjk +0.8% |
| Track emoji sequences and flags in the recorded cluster end | pictograph and regional-indicator classes, and ends for a pictograph with extends, the ZWJ after one, and half a flag (GB11, GB12/13) | emoji-zwj -13.0%, unicode-mixed -1.0%; combining +1.6% |
| Measure a widening cluster without building its string | the VS16/flag/ZWJ widening check measures the cell's codepoints with `unicode_width::char_iter_width` instead of encoding them and validating the UTF-8 | combining -14.0%, emoji-zwj -3.8%, unicode-mixed -2.4% |
| Inline the cheap half of the cluster check | the check against the recorded end is inlined, the walk over the stored cluster stays out of line | unicode-mixed -1.8%, emoji-zwj -1.8%, cjk -1.6%, combining -1.3%; ASCII corpora +0.1 to +0.5% (placement, undone below) |
| Write a character that fits at the cursor without the general path | `Row::write_char_in_place` writes a one- or two-column character and its spacer when no wrap is pending, insert mode is off, it fits and it overwrites no part of a wide character; the grid and the buffer try it first | cjk -15.0%, unicode-mixed -12.1%, emoji-zwj -9.6%, combining -3.0% |
| Keep the per-cell scrollback encoder out of line | inlined, the per-cell encoder's register saves were paid by every row, plain ones too | scroll -1.4%, ascii -0.5%, wide-wrap -0.4%; unicode corpora +0.1 to +0.4% |

Plus: the `pty_flood` bench gains the CJK corpus, and the baseline in
`bench/baselines/icount.json` is regenerated in its own commit.

### Instructions per byte, before and after

`ultty-bench vt` and `--suite parse`, 1 MiB, 120x40; before is
`origin/main` (d37645b).

| corpus | vt before | vt after | change | parse |
|---|---:|---:|---:|---:|
| ascii | 39.0 | 39.0 | 0 | 3.9 |
| sgr-dense | 85.7 | 87.2 | +1.7% | 48.7 |
| unicode-mixed | 309.1 | 219.6 | -29.0% | 36.3 |
| cjk | 289.3 | 217.9 | -24.7% | 38.8 |
| emoji-zwj | 276.0 | 189.8 | -31.2% | 24.1 |
| combining | 517.2 | 411.6 | -20.4% | 53.2 |
| cursor-redraw | 33.2 | 33.2 | +0.1% | 18.2 |
| scroll | 174.3 | 174.5 | +0.1% | 28.5 |
| scroll-region | 31.4 | 31.4 | 0 | 10.1 |
| alt-screen | 63.4 | 63.5 | +0.1% | 42.7 |
| wide-wrap | 34.6 | 34.6 | +0.1% | 1.6 |

The parse suite is unchanged: nothing here touches the parser.

At 1 MiB the 10,000-line history is still filling, so scrollback encoding
allocates for every row. At 8 MiB (history full, allocations reused, as in
a long flood): cjk 262.7 -> 194.3 (-26.0%), unicode-mixed 288.2 -> 203.7
(-29.3%), emoji-zwj 259.0 -> 177.9 (-31.3%), combining 496.8 -> 395.8
(-20.3%), sgr-dense 84.7 -> 86.1 (+1.7%).

The memory budgets (`cargo test -p ultty --bench memory`) pass with the
same live bytes as before: the marks reuse the per-column list scrollback
already had, and the budget corpora have no wide characters.

### `pty_flood`, end to end

`cargo bench -p ultty --bench pty_flood` (32 MiB through the real I/O
thread, no GPU), with the CJK corpus added to both builds. Base and final
benches alternated, which went first swapping each round, 6 rounds each;
criterion's point estimate per round, MiB/s. Load 2.8 to 4.3 during the
runs, so the rounds were steady (spread under 2%).

| corpus | before: best | median | after: best | median | change (median) |
|---|---:|---:|---:|---:|---:|
| ascii | 199.2 | 197.5 | 200.0 | 199.2 | +0.9% |
| unicode-mixed | 45.4 | 44.9 | 63.2 | 62.9 | +40.2% |
| cjk | 54.1 | 53.9 | 76.4 | 76.0 | +41.1% |
| cursor-redraw | 198.0 | 195.1 | 197.5 | 196.3 | +0.6% |
| sgr-dense | 106.7 | 106.4 | 105.1 | 104.8 | -1.5% |

The live gain is larger than the instruction cut (-29%); why was not
measured (the cut removed calls and branches on every character, which
may cost more cycles per instruction than the rest).
sgr-dense moves with its +1.7% instructions. The AWS T4 comparison with
Ghostty has not been rerun.

## Details

**Scrollback styles.** The per-cell encoder compared every cell's style
with the run's, and a wide character's style differs from its spacer's
only in WIDE_CHAR and WIDE_CHAR_SPACER: a CJK row was one run per cell.
Those two flags and HAS_COMBINING follow from the row (a wide cell is
followed by its spacer; a cell with extra codepoints has the flag), so the
encoder leaves them out of the styles and keeps a wide bit in the mark it
already stored for a column with grapheme extras. A row they do not
follow from (only reachable by editing cells directly; `Row` integrity
forbids it) is encoded again with the flags in its styles, so encoding
stays exact. The round-trip proptest now generates wide pairs and rows
with those flags set directly. Masking the flags costs every styled cell
an instruction or two, which is the sgr-dense +1.7%.

**One table.** `char_class.rs` holds the cluster classes (plain, extend,
linker, spacing mark, consonant, regional indicator, pictograph) and a
function that packs a codepoint's width and class into a byte. `build.rs`
compiles the same file and tabulates it for U+0000..U+3FFFF in 128-codepoint
blocks (176 distinct, 24 KB with the index); codepoints above compute the
byte directly. A test checks the table against unicode-width and the class
definitions for every codepoint. unicode-width gives some codepoints width
3 (U+17D8), so the width takes two bits.

**The recorded cluster end.** `continues_cluster` used to read the cell
and its extras back and classify the last codepoint. The grid now records
how the cluster ends when it prints (`Tail`): settled (plain, extend,
linker or spacing mark), a pictograph with extends, the ZWJ after one,
another ZWJ, a consonant, an open conjunct, an unpaired regional
indicator, or other. With the new codepoint's class that answers GB9,
GB9a, GB9c, GB11 and GB12/13 in one table lookup; anything else (Prepend,
Hangul jamo, controls, extends after a ZWJ that may sit in a conjunct)
falls back to the walk over the stored cluster and then the segmenter. A
codepoint dropped at the 16-per-cell cap leaves the recorded end alone
(test). The consonant and linker sets are exactly the codepoints the
segmenter treats as such, checked for every codepoint: unicode-segmentation
1.13 has Indic_Conjunct_Break for Myanmar, Khmer, Balinese, Javanese and
others besides the six scripts of Unicode 15.1. Two tests check the
recorded end against the segmenter: every codepoint of a known class at
the end of each kind of cluster, followed by samples of every class; and
every cluster of up to four samples. The existing exhaustive class tests
cover the new extends (each also keeps a conjunct going) and the spacing
marks.

**Writing in place.** After the cluster check, nearly every printed
non-ASCII character starts a one- or two-column cluster that fits at the
cursor. `Row::write_char_in_place` writes its cell and spacer when the
columns hold no part of a wide character, and the grid then marks the row
dirty once, advances the cursor and records the print. `Buffer::write_char`
tries it before working out whether the write scrolls (it cannot), and a
codepoint that joins the previous cluster skips that too. A proptest drives
random characters (narrow, wide, joining, zero-width, width three), cursor
moves, insert mode and autowrap through both paths and compares cursor,
last print, cells and grapheme extras after every step.

## Measured and dropped

- **Handling every wide or combined cell out of line** in the per-cell
  scrollback encoder (one `#[inline(never)]` function for any cell with an
  implied flag): cjk +3.2% and sgr-dense +4.3% against `origin/main`,
  where keeping the wide pair and the combining case inline gave cjk -5.9%.
  Several shapes of the loop were tried. Part of the sgr-dense cost, on
  rows that have neither, was `push_char` no longer being inlined once it
  had more callers (fixed with `#[inline]`); the +1.7% left is the masking
  itself.
- **Classifying the GB11 walk through the table** before the emoji ends
  were tracked: emoji-zwj +2.3% against the range searches, which check the
  cheap ZWJ and regional-indicator cases first. Once the recorded end
  covers emoji the walk is rare and it does use the table.
- **Skipping the widening check for combining marks.** Measuring the
  cluster after every joining codepoint is still about 10% of combining.
  Whether a mark can widen a narrow cluster depends on unicode-width's
  contextual string rules, which it does not expose per codepoint, so
  there is no check of a shortcut against it; not done.

## What is left

Profile of unicode-mixed after (vt, 1 MiB, 220 instructions per byte):

- Scrollback encoding is now the largest part, 24% inclusive, about a
  third of it allocation while the 1 MiB run fills the history (a long
  flood reuses rows). The per-cell loop still compares five style fields
  per cell for styled and non-ASCII rows.
- The in-place write and its join check (`write_in_row`, `print_width`)
  are about 25% together, a third of it the dirty/version bookkeeping and
  `Row::drop_grapheme_entries`, which is a call per character to learn
  that the row has no clusters after the cursor.
- Codepoints that join a cluster still take the general path
  (`write_measured_char_slow`, 10%): mostly `extend_cluster`, whose
  widening check measures the whole cluster with unicode-width each time.
- `decode_utf8` and the handler dispatch are about 8%; a run-at-a-time
  parser output would remove the dispatch (see above for why it was not
  built).
- sgr-dense is 1.7% worse in instructions and 1.5% in `pty_flood`; leaving
  the flags in the styles for rows with no wide or combined cell would win
  it back at the cost of a pass to find out.

Not built: a batched run writer fed by a new parser output for runs of
multi-byte characters. The parser hands the grid one `Print` per
character, and a run would need either a second UTF-8 validation in the
consumer or a large by-value variant that every `AnsiOutput` would pay
to move. Writing in place took most of the per-character cost without
changing the parser's API.

## Correctness

Besides the tests above, all of `cargo test -p ultty-vt` (including
`conformance.rs`, `integrity.rs` on both feed paths and `selection.rs`)
and `cargo test -p ultty` pass, and the fuzz targets build (nightly; the
VT target ran 100k uninstrumented inputs without a crash).

Debug builds of `origin/main` and of this branch, under a private Xvfb
with lavapipe, printed a sample of ASCII, CJK and halfwidth katakana,
emoji (skin tone, family and rainbow-flag ZWJ sequences, VS16 and text
hearts, flags, technologist), combining sequences, Devanagari, Bengali,
Gujarati, Oriya, Telugu, Malayalam and Tamil conjuncts and vowel signs,
Greek, Cyrillic, Hebrew, Arabic, Thai, Hangul jamo, repeated clusters and
SGR-styled clusters, then 60 lines, then the sample again. The
screenshot of the screen, the screenshot after scrolling the view back to
the first sample (drawn from rows that went through scrollback encoding
and decoding), `ulttyctl text` and `ulttyctl text --scrollback` are
byte-identical between the two builds. Both show the rendering issues
noted in [2026-11-07-live-unicode.md](2026-11-07-live-unicode.md) (some
second combining marks offset into the next cell, the family sequence
drawn as a box) and Indic text drawn cell by cell rather than shaped;
this change neither causes nor fixes them.
