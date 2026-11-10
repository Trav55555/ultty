# CJK print path (2026-11-10)

Follows [slice 3](2026-11-10-page-storage-slice3.md), which left cjk at
−11% instructions against the pre-slice-3 reference where the
[prototype](2026-11-09-page-storage-prototype.md)'s variant A reached −22%
to −26%. Slice 3 traced the gap to the per-character print path: A's
`Core::print` is called straight from the parser closure, while the real
path goes `TerminalHandler::handle` → `Buffer::write_char` →
`Grid::print_width` → `Grid::write_in_row`. This note is what removing
that dispatch cost, without changing behaviour, gave.

**Result.** cjk is −23.8% against the reference (A: −22.7%), and printing
non-ASCII costs 74.1 M instructions per MiB of cjk (A: 77.4 M, reference:
109.3 M). Every other case in the gate is level or better.

## Changes kept

One commit each. Instructions are the vt suite of `ultty-bench`
(callgrind inside the timed pass, 1 MiB, 120x40), against the commit
before.

| commit | what | cjk | unicode-mixed | emoji-zwj | ascii | sgr-dense |
|---|---|---:|---:|---:|---:|---:|
| print without entering the handler's large match | `handle` decides `Print` inline; every other action goes to an out-of-line match | −1.11% | −1.19% | −1.28% | 0.00% | −0.99% |
| print a character that fits in one call with one lookup | `Grid::print_in_row` does the cluster check, the width, the in-place write and the recorded cluster end from one `CharProps` lookup and hands the width back when the general path is needed; `Buffer::write_char` inlines it and keeps the scroll handling out of line | −7.96% | −7.09% | −3.94% | −0.12% | +0.15% |
| do not call `Action`'s drop glue for a printed character | the glue covers every variant and is not inlined, so dropping a `Print` cost a call per character | −3.32% | −2.94% | −2.54% | 0.00% | +0.79% |
| rule out joining from the recorded cluster end first | a CJK character cannot join a cluster ending in a plain character, which the recorded tail answers before the cursor, wrap and content-version comparisons | −2.04% | −1.78% | −0.48% | +0.46% | +0.03% |

The sgr-dense +0.79% is the SGR parameter loop's code moving in the
parser's instantiation over the handler closure, not work added; the
ascii +0.46% is all in libc `memcpy` (the Rust functions' counts are
identical). Both are level or better against the baseline (below).

## The gate

`scripts/bench/icount-gate.sh --threshold 1` against the baseline
committed with slice 3:

| case | baseline | this branch | change |
|---|---:|---:|---:|
| vt/ascii | 31,489,882 | 31,453,447 | −0.12% |
| vt/sgr-dense | 91,859,037 | 91,805,516 | −0.06% |
| vt/unicode-mixed | 203,852,166 | 178,304,940 | −12.53% |
| vt/cjk | 203,562,768 | 175,295,352 | −13.89% |
| vt/emoji-zwj | 179,395,161 | 165,009,377 | −8.02% |
| vt/combining | 427,039,233 | 422,922,185 | −0.96% |
| vt/cursor-redraw | 29,476,142 | 29,414,242 | −0.21% |
| vt/scroll | 162,848,022 | 162,581,570 | −0.16% |
| vt/scroll-region | 24,114,934 | 24,082,012 | −0.14% |
| vt/alt-screen | 67,496,010 | 67,446,319 | −0.07% |
| vt/wide-wrap | 26,871,664 | 26,870,232 | −0.01% |

Every parse case is 0.00%; nothing here touches the parser. The memory
bench passes with the same live bytes.

### Against the reference and A

cjk, 1 MiB, 120x40, with the slice 3 note's "ref" and "A" (one binary
from `ad6a334` with the `proto` feature):

| | ref | A | slice 3 | now |
|---|---:|---:|---:|---:|
| total | 230,148,601 | 177,789,922 | 204,276,231 | 175,295,352 |
| vs ref | | −22.7% | −11.2% | −23.8% |
| printing non-ASCII (`print_utf8_run` inclusive) | 109.3 M | 77.4 M | 102.6 M | 74.1 M |
| history encode | 66.4 M | 59.3 M | 55.8 M | 54.9 M |

The prototype has no cluster extension or zero-width attachment, so it
has no comparable unicode-mixed or emoji-zwj numbers; those two are
compared with the baseline only.

## Measured and dropped

- Inlining the whole print path into `handle` (no out-of-line
  `TerminalHandler::print`): cjk −8.7%, but the parser's closure stopped
  being inlined into the generic parse loop, so every CSI action paid a
  call: sgr-dense +4.7%, ascii +2.9%.
- `#[inline(always)]` on `Grid::print_in_row`: cjk +3.0%.
- `ManuallyDrop` around the whole action instead of forgetting a `Print`:
  the same counts.
- A batched non-ASCII span (`AnsiOutput` carrying a run of printable
  UTF-8, as `PrintAscii` does for ASCII) was not tried. cjk's runs are
  about three characters between single spaces, the target was met
  without it, and it changes every consumer of `AnsiOutput`.

## `pty_flood`

Not run. Building the two bench binaries (this branch and `main`) was
stopped when the shared workstation ran short of memory, and the
wall-clock check was left for a quieter moment. The instruction cut is
the measured result here; slice 3's `pty_flood` gave cjk +12% for a −11%
instruction cut, so a gain is expected, not shown.

## Correctness

Behaviour is unchanged by construction (the same writes, reordered checks
whose answers do not depend on the order) and checked by:

- `cargo test --workspace`, with both differential-oracle tests at
  `ULTTY_ORACLE_CASES=30000` (`live_storage_matches_reference` and
  `..._under_constant_collection`), the integrity proptests on both feed
  paths, `tests/conformance.rs` and the scorecard;
- `printing_in_place_matches_the_general_path`, which now drives
  `Grid::print_in_row` and also checks that the width it hands back is
  the one the general path is given.

## Environment

Ryzen 5 7600, Ubuntu 26.04.1, Linux 7.0.0-42, rustc 1.98.1, valgrind
3.27.1, release profile. Shared workstation.
