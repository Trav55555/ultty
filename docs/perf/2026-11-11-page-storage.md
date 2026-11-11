# Page storage: the migration's results (2026-11-11)

[ADR 0022](../adr/0022-page-storage.md) replaced the VT core's screen and
scrollback storage in five slices (packed cells, the screen page, history
pages, and the read seam and oracle before them). This note is slice 6's
summary: the confirming measurements, what the cleanup removed, and what
is left of the gap to Ghostty. The per-slice notes have the detail:
[prototype](2026-11-09-page-storage-prototype.md),
[slice 3](2026-11-10-page-storage-slice3.md),
[slice 4](2026-11-10-page-storage-slice4.md),
[slice 5](2026-11-10-page-storage-slice5.md).

**Verdict.** Every GO criterion of the ADR's §Prototype now holds on the
real code. The last one open, `pty_flood` ascii +15% or better with
spread under 5% at 120x40 and 300x75, measures **+33.2% and +42.5%** on a
quiet AWS c7a.2xlarge (95% CI 1.332–1.332 and 1.414–1.430; per-arm
IQR/median 0.0–2.3%). On the T4, live ASCII throughput went from two
thirds of Ghostty's to level with it or slightly ahead: **143 against
133 MiB/s under X11 and 140 against 132 under Wayland** (unicode-mixed
33 against 26 and 32 against 25). Live heap is a third of the old
budgets or less (sgr-dense, about half), and the budgets are lowered to
match.

## Summary

| measure | before slice 3 | now | change |
|---|---:|---:|---:|
| vt/ascii instructions per byte, 1 MiB, 120x40 | 39.5 | 19.9 | −49.6% |
| vt/scroll, 1 MiB, 120x40 (instructions) | 188.2 M | 132.6 M | −29.6% |
| vt/cjk, 1 MiB, 120x40 (instructions) | 229.8 M | 126.6 M | −44.9% |
| vt/sgr-dense, 1 MiB, 120x40 (instructions) | 91.6 M | 90.9 M | −0.7% |
| `pty_flood` ascii 120x40, quiet c7a (MiB/s) | 164.7 | 219.3 | +33.2% |
| `pty_flood` ascii 300x75, quiet c7a (MiB/s) | 153.1 | 218.1 | +42.5% |
| `pty_flood` cjk 120x40 / 300x75 | 60.1 / 58.2 | 86.2 / 85.8 | +43.5% / +47.6% |
| `pty_flood` sgr-dense 120x40 / 300x75 | 85.3 / 82.3 | 82.5 / 82.2 | −3.3% / −0.1% |
| live heap, typical 10k lines, 120x40 | 3.15 MB | 1.17 MB | −63% |
| live heap, sgr-dense 10k lines, 120x40 | 17.9 MB | 9.96 MB | −44% |
| T4 X11 live ascii, Ultty / Ghostty (MiB/s) | 82.2 / 124 (2026-11-08) | 143 / 133 | 0.66 → 1.08 of Ghostty |
| T4 X11 live unicode-mixed, Ultty / Ghostty | 22.6 / 24.1 (2026-11-08) | 32.7 / 25.8 | 0.94 → 1.27 of Ghostty |

"Before slice 3" is `ad6a334`, the reference every slice was measured
against. The instruction and heap rows are slice 5's measurements; no
production code has changed since (the gate below confirms it). Heap
"before" is `main` before slice 5, with the old 24-byte screen cells
already gone; against the 2026-11-07 budgets the cut is larger.

## Wall clock: quiet-machine `pty_flood`

The GO criterion: `pty_flood` ascii median +15% or better at 120x40 and
300x75, spread under 5%. `pty_flood` ran 120x40 only, so slice 6 adds a
300x75 case (both sizes now run for every corpus).

- AWS c7a.2xlarge (AMD EPYC 9R14, 8 vCPUs, no SMT), us-east-1, on demand,
  as in the prototype's third round and slice 4's rerun. Ubuntu 26.04.1,
  kernel 7.0.0-1016-aws, rustc 1.98.1. No cpufreq interface; not pinned.
- Two bench binaries, built on the instance with `cargo bench --locked -p
  ultty --bench pty_flood --no-run` from `git archive` of `ad6a334` (before
  slice 3) and `5f6044f` (`main` at `3f97d11`, slice 5 merged, plus the
  300x75 case). `ad6a334`'s `pty_flood.rs` is identical to `main`'s before
  that change, so the before arm got the same new file.
- Every arm once per case per round, the arm order alternating by round
  and by batch, criterion's 10 samples per run (`--warm-up-time 1
  --measurement-time 3`); the run's point estimate is the sample. One
  discarded trial round, then three batches of 15 rounds, 45 in all, over
  ascii, cjk and sgr-dense at both sizes.
- Quiet: the timers that could fire (sysstat, apt, man-db, fwupd, fstrim,
  tmpfiles, update-notifier, motd-news, dpkg-db-backup, logrotate,
  e2scrub), snapd and unattended-upgrades were stopped. Nothing else ran.
  The 1-minute load was 0.89 to 2.06 during the batches, which is the
  harness itself.

| case | arm | median MiB/s | IQR / median | range |
|---|---|---:|---:|---:|
| ascii 120x40 | before slice 3 | 164.7 | 0.0% | 153.7–164.7 |
| ascii 120x40 | `main` | 219.3 | 0.0% | 213.4–219.3 |
| ascii 300x75 | before slice 3 | 153.1 | 2.3% | 148.4–161.8 |
| ascii 300x75 | `main` | 218.1 | 0.0% | 199.3–218.2 |
| cjk 120x40 | before slice 3 | 60.1 | 0.0% | 56.5–60.1 |
| cjk 120x40 | `main` | 86.2 | 0.0% | 81.9–86.2 |
| cjk 300x75 | before slice 3 | 58.2 | 0.0% | 55.2–58.2 |
| cjk 300x75 | `main` | 85.8 | 0.0% | 81.9–85.8 |
| sgr-dense 120x40 | before slice 3 | 85.3 | 2.6% | 79.5–86.1 |
| sgr-dense 120x40 | `main` | 82.5 | 0.0% | 77.6–82.5 |
| sgr-dense 300x75 | before slice 3 | 82.3 | 0.4% | 79.2–84.4 |
| sgr-dense 300x75 | `main` | 82.2 | 0.0% | 77.9–82.2 |

"0.0%" is under 0.05%. Paired per-round ratios, `main` / before slice 3,
45 rounds: median, 95% percentile bootstrap interval over rounds (10,000
resamples), and the median of each batch.

| case | ratio | 95% CI | IQR of the ratio | batches 1, 2, 3 |
|---|---:|---|---|---|
| ascii 120x40 | **1.332** | 1.332–1.332 | 1.332–1.332 | 1.332, 1.332, 1.332 |
| ascii 300x75 | **1.425** | 1.414–1.430 | 1.403–1.441 | 1.414, 1.425, 1.420 |
| cjk 120x40 | 1.435 | 1.434–1.435 | 1.434–1.435 | 1.435, 1.435, 1.435 |
| cjk 300x75 | 1.476 | 1.475–1.476 | 1.475–1.476 | 1.476, 1.475, 1.476 |
| sgr-dense 120x40 | 0.967 | 0.963–0.971 | 0.959–0.979 | 0.967, 0.975, 0.962 |
| sgr-dense 300x75 | 0.999 | 0.995–0.999 | 0.991–0.999 | 0.995, 0.995, 0.999 |

| criterion | needed | measured | met |
|---|---|---|---|
| `pty_flood` ascii median, 120x40 | +15% or better | +33.2% (CI +33.2% to +33.2%) | yes |
| `pty_flood` ascii median, 300x75 | +15% or better | +42.5% (CI +41.4% to +43.0%) | yes |
| spread | under 5% | per-arm IQR/median 0.0–2.3% on ascii (2.6% at most on any arm); ascii batches agree within 1.1 points | yes |

**The 219 MiB/s ascii figure was not a ceiling.** Slice 4's `main` ran
ascii 120x40 at 219.1 MiB/s on the same instance type and slice 5 gave
219.3 despite cutting vt/ascii instructions by another 29.8%, which looked
like a limit set outside the VT core. It was the bench: every `pty_flood`
time here is a whole number of 16 ms frames plus a small constant (see
[the frame quantum note](2026-11-11-pty-flood-frame-quantum.md)), and the
fixed bench puts `main` at 261.9 MiB/s for ascii 120x40 and 255.5 at
300x75 on a quiet c7a ([quiet rerun](2026-11-11-pty-flood-quiet-rerun.md)).
The `pty_flood` numbers in this note are frame-quantised and are lower
bounds, not rates; the ratios between arms in different frames hold only to
about a frame (11% at ascii's 146 ms). The criterion is met with the ceiling in place, so the
ascii ratios understate what the storage change is worth.

sgr-dense at 120x40 is 3.3% slower than before slice 3, as in slice 4's
rerun (4.2%) and the prototype's C (3.6–4.4%); the cause is in the
prototype note's sgr-dense section. At 300x75 it is level. The criterion
for sgr-dense is in instructions (−1.1% to +0.0%, slice 5), where it is
met.

The trial round (discarded) agrees with every batch: 1.332, 1.436, 1.434,
1.476, 0.959 and 0.995.

## Live: the T4 against Ghostty

`scripts/bench-aws.sh --role gpu` on an AWS g4dn.xlarge (Tesla T4, NVIDIA
580.178.04, 4 vCPUs, Xeon Platinum 8259CL) from the bench image
`ami-0fe200d7ed965618f` (Ubuntu 26.04.1, kernel 7.0.0-1016-aws, Ghostty
1.4.0 from source, sway 1.11). Ultty `3f97d11` (`main`, slice 5 merged;
this branch changes no production code). 120x40 cells, DejaVu Sans Mono
11, claim class `live-realgpu`. Three rounds, terminal order shuffled per
round, 16 MiB corpora fed three times per launch, 200 key presses per
launch and case. The Wayland probe arms the frame request before the key
(the default since #76).

```bash
scripts/bench-aws.sh --host <ip> --user ubuntu --key <key.pem> --known-hosts <file> \
  --role gpu --ref adr-0022-cleanup --skip-setup \
  --compare-args "--terminals ultty,ghostty --corpora ascii,unicode-mixed --idle-secs 0 --tabs 1"
```

### X11 (headless Xorg on the GPU, openbox)

| | Ultty | Ghostty | Ghostty / Ultty (95% CI) |
|---|---:|---:|---|
| throughput, ascii (MiB/s; n = 6 / 9) | **143** (IQR 3.2%) | 133 (IQR 3.5%) | 0.93 [0.87, 0.97] |
| throughput, unicode-mixed (MiB/s) | **32.7** (IQR 2.1%) | 25.8 (IQR 6.2%) | 0.79 [0.74, 0.81] |
| keypress to pixel, idle (p50 / p99, n=600) | **3.68 / 3.84 ms** | 22.1 / 30.2 ms | 6.0 [5.88, 6.17] |
| keypress to pixel, 10 MiB/s flood | **5.48 / 8.54 ms** | 14.0 / 25.9 ms | 2.56 [2.46, 2.73] |
| keypress to pixel, maximum flood | **22.1 / 25.1 ms** | 33.1 / 58.3 ms | 1.50 [1.45, 1.54] |
| RSS after the feed (n=2 / 3) | 163 MiB | 201 MiB | |
| exec to titled window (n=2 / 3) | 365 ms | 628 ms | |

Ultty's first launch of the run (round 0, throughput) failed with "window
never appeared", so its throughput rows have six samples from two
launches, not nine; it is the first window opened after the instance
booted. Every later launch worked. `analyze.py` calls the ascii row "no
detectable change" by its ±5% rule (the interval reaches 0.97); Ultty's
median is 7.5% higher.

### Wayland (headless sway, 60 Hz)

| | Ultty | Ghostty | Ghostty / Ultty (95% CI) |
|---|---:|---:|---|
| throughput, ascii (MiB/s; n=9) | **140** (IQR 3.5%) | 132 (IQR 3.3%) | 0.94 [0.91, 0.98] |
| throughput, unicode-mixed (MiB/s) | **31.9** (IQR 2.7%) | 25.2 (IQR 4.5%) | 0.79 [0.75, 0.82] |
| keypress to pixel, idle (p50 / p99) | **3.74 / 3.91 ms** | 25.7 / 28.3 ms | 6.88 [6.87, 6.89] |
| keypress to pixel, 10 MiB/s flood | 28.1 / 36.0 ms | 27.4 / 38.1 ms | 0.98 [0.94, 1.02] (no detectable change) |
| keypress to pixel, maximum flood | **29.8 / 52.3 ms** | 40.9 / 59.3 ms | 1.37 [1.34, 1.42] |
| RSS after the feed (n=3) | 179 MiB | 195 MiB | |
| exec to titled window (n=3) | 410 ms | 742 ms | |

One of Ultty's nine Wayland ascii samples is 53.4 MiB/s (the others
134–145); the median and IQR are robust to it.

### Against the earlier T4 runs

| | 2026-11-08 (v0.2.0 era) | 2026-11-09 Wayland note | now |
|---|---|---|---|
| X11 ascii, Ultty / Ghostty | 82.2 / 124 | | 143 / 133 |
| X11 unicode-mixed | 22.6 / 24.1 | | 32.7 / 25.8 |
| Wayland ascii | 79.3 / 122 | | 140 / 132 |
| Wayland unicode-mixed | 22.0 / 23.6 | | 31.9 / 25.2 |
| X11 k2p idle p50 | 3.56 / 22.4 ms | 3.41 / 22.5 ms | 3.68 / 22.1 ms |
| X11 k2p max flood p50 | 21.9 / 34.2 ms | 21.7 / 33.2 ms | 22.1 / 33.1 ms |
| Wayland k2p idle p50 | | 3.7 ms (corrected) / 26 ms | 3.74 / 25.7 ms |

ADR 0022 predicted 105–125 MiB/s on the T4 against Ghostty's 129, "most
of the gap, probably not all of it". Ultty's ascii rose 74% (82 to 143)
and unicode-mixed 45%, while Ghostty's numbers moved 2–8% between
instances; latency is unchanged, as expected from a storage change.

## Instructions

`scripts/bench/icount-gate.sh` against slice 5's baseline, before
regenerating it: every case within +0.02% (vt/sgr-dense −0.23% and
vt/unicode-mixed −0.18%, which move that much between runs). The baseline
is regenerated in its own commit with the same rustc 1.98.1 and valgrind
3.27.1. Slice 5's note has the full table against `ad6a334` for the five
gate corpora at 1 and 8 MiB and both sizes; in short, vt/ascii 19.9–20.9
instructions per byte (limit 27, from 39.5), scroll −26.7% to −36.1%,
wide-wrap −38.7% to −51.0%, sgr-dense −1.1% to +0.0%, cjk −38.0% to
−45.5%.

## Memory

`cargo test -p ultty --bench memory`, unchanged since slice 5. Every
budget except the scene tree's is lowered to the measurement + 5%,
as the originals were set.

| case | live bytes | old budget | new budget |
|---|---:|---:|---:|
| empty 80x24 | 36,886 | 185,000 | 38,800 |
| empty 300x75 | 375,144 | 1,229,000 | 394,000 |
| typical 10k lines, 80x24 | 731,210 | 2,915,000 | 767,800 |
| typical 10k lines, 120x40 | 1,165,110 | 3,461,000 | 1,223,400 |
| typical 10k lines, 300x75 | 1,454,280 | 4,358,000 | 1,527,000 |
| typical 120x40, resized (live) | 1,163,142 | 3,461,000 | 1,221,300 |
| typical 120x40, resized (peak) | 1,448,974 | 4,935,000 | 1,521,500 |
| sgr-dense 10k lines, 120x40 | 9,959,158 | 18,980,000 | 10,457,200 |
| CJK 10k lines, 120x40 | 2,323,958 | 4,824,000 | 2,440,200 |
| scene tree, 1000 regions | 923,664 | 953,000 | 953,000 |

## What the cleanup removed, and what the tests still cover

- **The differential oracle** (`grid/reference/`: the frozen `Grid`,
  `Row`, `Buffer` and `CompactRow`, the second compile of `terminal.rs`
  and `term_handler.rs`, and the oracle proptests). Before removing it,
  its three tests ran at `ULTTY_ORACLE_CASES=30000`, including a new
  variant with history pages of two to four rows (below): all passed
  (435 s on the shared workstation). `term_handler.rs` names grid items
  through `crate::grid::` again; the `super::` paths existed only for the
  second compile.
- **The prototype**: `grid/proto/`, the `page-proto` and `proto`
  features, `ultty-bench vt --suite proto`, `proto-check`, `proto-flood`,
  `ultty-proto-mem` and `scripts/bench/proto-icount.sh`. Nothing needed
  them once `pty_flood` had a 300x75 case. The code is at `3f97d11` for
  anyone who wants to rerun the gate's arms.
- **`CompactRow` and the `Cow<Row>` shims** were already gone from live
  code in slice 5; only `CellFlags::intersection`, used by the frozen
  copy, was left, and is removed.

**Coverage.** The oracle compared the new storage with the old after
every step of random streams: the cursor, every visible and scrollback
row (cells, extras, wrap, marks, anchors), `history_len` and anchors, on
both feed paths, with resizes, scrollback-limit changes, scrolling back
and inline-surface rewrites. Its value was catching storage bugs that
leave a structurally valid terminal showing the wrong thing, which
`check_integrity` cannot see. It found real bugs in slices 3 to 5, four
of them kept as shrunk cases.

Its replacement, `grid/differential.rs`, keeps the oracle's stream
generators, snapshots and comparison, and compares the storage with
itself under a changed parameter: history pages of two to four rows
instead of 256, and the style table collected after every one to eight
new styles. Neither may change what the terminal shows, and they are the
two places where the slices' bugs sat (page boundaries, eviction and
take-back; collection invalidating cached keys and remaps). The four
shrunk cases run as fixed streams under both. `check_integrity` runs
after every step of both runs. It costs one implementation, not two.

What is lost: a behaviour change that the storage shows the same way at
every page size and collection interval, which the old storage would not
have shown. The defence there is what tests behaviour directly:
`conformance.rs`, the scorecard, `command_marks.rs`, `inline_anchors.rs`,
`selection.rs`, the history round-trip proptest (`view().to_row() == row`
for every row the generator makes, flags only direct edits create
included) and the unit tests. A cheaper full replacement for that does
not exist; keeping a frozen implementation alive is what it would cost.

## Slice 5 follow-ups done here

- `History`'s page size can be set per test thread
  (`history::test_support::page_rows`), read when a `History` is made,
  like `StyleTable`'s `collect_every`; release builds keep the constant.
  The oracle ran with pages of two to four rows before its removal, and
  the differential test keeps that variant. The eviction unit test no
  longer assumes 11 rows a page and runs at 256, 2, 3, 4 and 7.
- `Page::compact` has a unit test on Cells rows with grapheme extras,
  checked to fail when the extras' offsets are not rewritten.
- `RowView::chars()` yielded nothing for a scrollback row. It now reads
  all three history forms; a unit test checks every kind of row.

## What is left of the gap

On ASCII the T4 gap to Ghostty is closed: Ultty is level to 8% ahead
under X11 and Wayland, and 27% ahead on unicode-mixed. What remains:

- **The I/O path above the VT core.** The apparent 219 MiB/s `pty_flood`
  ceiling was the bench's frame sleep; on the fixed bench `main` runs
  ascii at 262 MiB/s on the quiet c7a (the prototype's harness reached 291
  on the same hardware), so the gap is smaller than this note first read
  it. Profiling `EventLoop`'s read, wakeup
  and lock hand-off under a flood is the next step; ADR 0022's
  Consequences already named the parser and handler dispatch and the
  kernel's 4 KiB reads as what this ADR does not address.
- **sgr-dense**, 3.3% slower on the wall clock than before slice 3 at
  120x40 (level at 300x75), for the reasons in the prototype note.
- **Keypress-to-pixel under maximum flood**: 22 ms on X11 against
  Alacritty's 12 ms in the 2026-11-08 run; storage did not change it.

## Correctness

- `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D
  warnings`; `cargo fmt --all -- --check`; `cargo build --manifest-path
  examples/porygon-tui/Cargo.toml`.
- The differential tests (default 1024 cases each).
- `scripts/bench/icount-gate.sh` against the regenerated baseline, and the
  memory bench with the new budgets.

## Caveats

- The c7a and T4 instances ran nothing but the harness, but one run each:
  the c7a numbers repeat slice 4's to within 0.1% where nothing changed,
  and the T4 Ghostty numbers move 2–8% between instances.
- The X11 throughput rows have six Ultty samples (one launch failed to
  open a window), and the RSS and startup rows two or three.
- The T4's raw results stayed on the workstation (not committed); both
  instances are gone.
