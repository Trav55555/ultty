# Page storage prototype: ADR 0022's go/no-go gate (2026-11-09)

[ADR 0022](../adr/0022-page-storage.md) proposes 8-byte packed cells with
interned styles, a screen page with slot rotation, and append-only history
pages. It says not to refactor until a prototype has passed a gate. This
note is that prototype's measurements and the gate applied to them.

The prototype's code (`grid/proto/`, the `proto` features and
subcommands, `ultty-proto-mem`, `scripts/bench/proto-icount.sh`) was
removed in migration slice 6; check out `3f97d11` to rerun the commands
below.

**First-round verdict: not GO.** The page design (variant C) passes every criterion but
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

That was the first round. A second round, below, changed the prototype's
style handling and re-ran the whole gate.

**Second-round verdict: GO on every criterion but the wall-clock spread.**
Every criterion that measures the design now passes, sgr-dense
included (+0.5% to +1.4% instructions). The one clause still not met is
the spread of the PTY medians: under 5% is required, and this shared
workstation gives 8–10% per arm (4–7.6% in the first round); pinning the
harness did not help. The medians themselves are +42% and +49%, with 95%
intervals of about ±2–3%, three times the +15% bar. Read literally, the
gate is therefore still not GO, on that clause alone; the design question
it guards (is the gain real and well above +15%?) is answered. "A only"
does not qualify and the NO-GO rule does not fire. Whether to accept the
ADR on these numbers, or to rerun the wall clock on a quiet machine
first, is for the ADR's owners.

**Third-round verdict (quiet machine): GO.** The second round's wall clock
was rerun unchanged on an idle AWS instance. Per-arm IQR/median is 0.3–2.6%
(C 0.3–0.7%), under the 5% limit, so the spread clause is met and with it
every GO criterion. The medians match the workstation's: C runs ascii
+44.8% and +50.9% (95% CI ±0.2%). "A only" still does not qualify (A
carries 60% and 49% of C's gain) and NO-GO does not fire. One thing the
quiet machine shows that the workstation could not: sgr-dense is about 4%
slower in C on the wall clock, not level (see below).

## Third round: quiet machine

The wall-clock half of the second round, rerun on a machine with nothing
else on it. The binary and the method are the second round's; nothing in
the prototype changed.

- AWS c7a.2xlarge (AMD EPYC 9R14, 8 vCPUs, one thread per core, so no SMT
  sibling to share), us-east-1, on demand. Ubuntu 26.04.1, kernel
  7.0.0-1016-aws, rustc 1.98.1. The instance exposes no cpufreq interface,
  so there is no governor or turbo to set. Not pinned (`pin` 0-7).
- Branch `adr-0022-sgr` at `1c91725`, built on the instance with
  `cargo build --release --locked -p ultty-bench --features proto`.
  `proto-check` passed on ascii, sgr-dense and cjk before the runs.
- Quiet: nothing ran but the harness and the SSH session that started it.
  Timers that could fire during the run (sysstat, apt, man-db, fwupd,
  fstrim, tmpfiles, update-notifier) and snapd were stopped first. The
  1-minute load average was 0.20 before the first batch; during the runs it
  was 1.0–1.5, which is the harness itself (`dd`, the reader and the frame
  thread on 8 cores).

The method is the Commands section's: three batches of
`proto-flood --corpus ascii --rounds 40`, then
`proto-flood --corpus sgr-dense,cjk --rounds 12`, both geometries, plus one
extra `--corpus sgr-dense --rounds 40` to check the sgr-dense result. Paired
medians are the median of per-round arm/ref ratios; the 95% intervals are a
percentile bootstrap over rounds (10,000 resamples).

| case | arm | median MiB/s | IQR / median | paired median vs ref | 95% CI (bootstrap) |
|---|---|---:|---:|---:|---:|
| ascii 120x40 | ref | 200.5 | 0.7% | | |
| ascii 120x40 | A | 255.0 | 2.6% | 1.270 | 1.268–1.274 |
| ascii 120x40 | C | 290.6 | 0.7% | 1.448 | 1.447–1.451 |
| ascii 300x75 | ref | 187.5 | 0.7% | | |
| ascii 300x75 | A | 234.6 | 0.4% | 1.251 | 1.249–1.252 |
| ascii 300x75 | C | 283.2 | 0.3% | 1.509 | 1.507–1.511 |

| batch | 1-min load before → after | C vs ref 120x40 / 300x75 | A vs ref 120x40 / 300x75 | largest arm IQR/median |
|---|---|---|---|---:|
| 1 | 0.20 → 1.02 | 1.450 / 1.511 | 1.270 / 1.251 | 2.7% (A, 120x40) |
| 2 | 1.02 → 1.42 | 1.454 / 1.507 | 1.274 / 1.252 | 2.6% (A, 120x40) |
| 3 | 1.42 → 1.24 | 1.445 / 1.507 | 1.269 / 1.249 | 2.6% (A, 120x40) |

The batches agree within 0.6%. Every arm's IQR is under 1% of its median
except A at 120x40 (2.6%), whose runs fall in two clusters about 3% apart.
The paired ratios are as tight (IQR/median 0.6–0.8% for C). So the
workstation's 8–10% was the shared machine, not the harness. Absolute
rates are about 15% below the workstation's (a 3.7 GHz server core against
a desktop Ryzen), and the ratios are within 2.5 points of the second
round's 1.424 and 1.492.

A's share of C's gain is 60% at 120x40 and 49% at 300x75 (per batch 60 /
60 / 60% and 49 / 50 / 49%), below two thirds as before.

Other corpora (12 rounds at load 1.24 → 1.30; the sgr-dense rows with 40
rounds at 0.78 → 1.22). Every arm's IQR/median is 1.2% or less.

| case | ref | A | C | A vs ref | C vs ref (95% CI) |
|---|---:|---:|---:|---:|---:|
| sgr-dense 120x40, 12 rounds | 98.1 | 93.9 | 94.1 | 0.952 | 0.957 (0.953–0.962) |
| sgr-dense 300x75, 12 rounds | 97.3 | 92.8 | 93.7 | 0.956 | 0.964 (0.963–0.966) |
| sgr-dense 120x40, 40 rounds | 98.8 | 93.9 | 94.4 | 0.952 | 0.956 (0.953–0.957) |
| sgr-dense 300x75, 40 rounds | 97.4 | 93.1 | 93.8 | 0.956 | 0.963 (0.960–0.965) |
| cjk 120x40 | 67.4 | 84.7 | 102.3 | 1.256 | 1.519 (1.514–1.523) |
| cjk 300x75 | 65.3 | 81.5 | 100.7 | 1.244 | 1.541 (1.534–1.543) |

**sgr-dense is not level on the wall clock.** C runs it 3.6–4.4% slower
than the reference (4–5% more time), and A 4.4–4.8% slower. The second
round called C level, but its intervals there were 0.951–1.102 and
0.966–1.039; the quiet machine resolves the difference. The gate's
"no worse than +3%" for sgr-dense is an instruction criterion (§Prototype,
measurement 1; the first round already reported the other corpora's wall
clock for information only), and on instructions C is +0.5% to +1.4%, so
the gate passes. Taken as time, the same 3% would not be met. Why +1%
instructions costs about 4% of time is in the next section. For real
colorful output, which repeats a handful of styles, the instruction counts
above show C ahead of the reference.

### The gate, third round

Only the wall-clock rows change; the instruction and heap rows are the
second round's.

| criterion (C) | needed | measured | met |
|---|---|---|---|
| PTY ascii median, 120x40 and 300x75 | +15% or better | +44.8% and +50.9% (95% CI 1.447–1.451 and 1.507–1.511) | yes |
| spread of those medians | under 5% | IQR/median 0.3–2.6% per arm (C 0.3–0.7%); batches agree within 0.6% | **yes** |
| every other GO criterion | as before | as in the second round | yes |
| "A only": A's share of C's wall-clock gain | two thirds or more | 60% and 49% | no |
| NO-GO: C's wall-clock gain on ascii | fires under +10% | +44.8% / +50.9% | does not fire |

### sgr-dense wall clock

Why C is about 4% slower in time on sgr-dense while executing only
+0.5% to +1.4% more instructions. Measured on the workstation (load 4–12),
with the third-round binary (`1f451d0`, nothing in the prototype changed).
`perf` is not available there (`perf_event_paranoid` 4), so the hardware
side comes from paired wall clock and getrusage, and the counts from
callgrind with `--cache-sim=yes --branch-sim=yes`. The PTY is not needed:
`ultty-bench vt --suite vt,proto-c --corpus sgr-dense --size-mib 8 --runs
150` gives C/ref paired medians of 0.945–0.957, the same gap in process.

There are two causes, and the style cache is not one of them.

**1. Fresh memory every round (a harness effect, about 1 point at 32 MiB
and 2.5 at 8 MiB).** Every round builds a new terminal in the same
process. When a C terminal is dropped, glibc gives its large history page
buffers back to the kernel, so the next round faults them in again. The
reference's history is many small blocks, which stay in the heap and are
reused.

| 8 MiB passes, 120x40 | ref | C |
|---|---:|---:|
| minor faults, first pass (above a parse-only run) | 4,092 | 2,988 |
| minor faults, each later pass | 731 | 2,857 |
| system time, 40 passes | 0.09–0.11 s | 0.20–0.26 s |
| C/ref paired median | | 0.949, 0.952 |
| the same with `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=268435456:glibc.malloc.trim_threshold=1073741824` | | 0.978, 0.978 |
| 32 MiB passes, without / with the tunables (one run each) | | 0.968 / 0.976 |

With the tunables both arms fault about 100 pages a pass and C's gap
shrinks by 2.5 points at 8 MiB and about 1 point at 32 MiB (the size the
PTY runs use; history fills once per pass, so the cost is fixed per round).
A long-lived terminal faults its history in once, and C needs fewer pages
than the reference for it, so this part does not apply outside the
harness.

**2. Steady state: C adds slow instructions and saves fast ones (about 2–3
points).** With the history already full (8 MiB fed untimed, 16 MiB timed,
paired over 40–60 rounds) C runs 0.968–0.986 of the reference. Turning
scrollback off in both arms separates the two halves:

| per 8 MiB, history full | ref | C | C − ref |
|---|---:|---:|---:|
| instructions, whole parse | | | +17.7 M (+2.4%) |
| instructions, with scrollback 0 (screen and style path only)\* | | | +33.6 M (+5%) |
| simulated mispredicted branches, scrollback 0\* | 9.20 M | 9.84 M | +0.64 M (+7%) |
| instructions in the history copy | 79 M | 54 M | −25 M |
| C/ref wall clock, scrollback 0\* | | | 0.886, 0.891 |
| C/ref wall clock, full\* | | | 0.968, 0.978 |

\* Measured with the `set_attrs` copy removed (see What was tried), which
saves 9.3 M; the third-round code is about +43 M here.

The 8 MiB gate pass is about half history fill, where C is cheaper; the
steady state alone is +2.4%. On the screen side, C's packed writes save
instructions, but the per-template style path costs more than they save:
`Style::of` and the `Cell` copy in `set_attrs`, `inline_key`, then for the
47% of templates that need the table, `intern` and the cache probe in
`StyleTable::insert` (together about 45 M instructions per 8 MiB). Those
branch on the style itself (truecolor or not, cache hit or not), which in
sgr-dense is a coin flip per template. In time, the screen side is 11%
slower and the history side's 25 M saved instructions buy back only about
two thirds of it. The reference's history copy is about 20% of its time
on this corpus (212 MiB/s without scrollback, 169 with).

**What was tried.** Dropping the `Cell` copy in `set_attrs` (C keeps only
the `Style`) cut 9.3 M instructions (−1.3%). Writing `inline_key`'s tests
and `remap_key_to_page`'s with `|` instead of `||`, so that a coin-flip
style costs one mispredict rather than up to four, gained 1.5–2 points in
the steady-state loop. Together they put sgr-dense at −0.5% to +0.5%
instructions with every other corpus within 1.2% of the second round. In
`ultty-bench vt`, the gate's own binary, the same source gave 0.938–0.943
against the third-round binary's 0.947–0.957: a code-layout effect of the
same size as the gain. A branch-free style comparison in the cache probe
cut 6.8 M instructions and was slower. None of these is kept: without
cycle counters, a 1–2% change cannot be told apart from layout here.

**What this means for the gate.** The instruction criterion stands. Of the
quiet machine's 4%, about 1 point is the harness, and the rest is the
style path on a stream where nearly every template is new. A real
implementation would apply SGR to the style key directly instead of
through a scratch `Terminal` and `Style::of`, which removes most of that
path; the prototype cannot show what is left without being rewritten.
To confirm the harness share on the quiet machine, rerun
`proto-flood --corpus sgr-dense` with and without the `GLIBC_TUNABLES`
above.

## Second round: style handling

Branch `adr-0022-sgr`, on top of the first round. Only the prototype's
style handling changed; the reference, the harnesses and the method are the
same. Every number in this section is from the second-round build, with
the reference re-measured in the same binary.

### What the cost was

The first round's profile (Detail, below) put the gap in interning: a
foldhash `HashMap` lookup and insert for nearly every SGR change, plus a
collection every few thousand styles that scanned the screen and rehashed
the survivors. sgr-dense's 8 MiB, 120x40 pass computes 637,560 template
styles; 343,188 of them are distinct. A cache of recent styles cannot help
those, and lazy interning cannot either: the corpus prints one to four
characters after every SGR, so no style is replaced before it is used.

### What changed

1. **Inline styles.** A cell's style field (bits 24..64, 40 bits) now holds
   either a table id or, with its top bit set, the whole style: foreground
   and background as a 2-bit kind (default, named, indexed) plus an 8-bit
   value each, and the 16 flag bits, 37 bits in all. Any style with no
   truecolor, no underline color and no hyperlink is stored this way and
   never touches the table. A style that fits inline is always stored
   inline, so equal keys still mean equal styles. History pages store
   inline keys as they are; only table ids are remapped.
2. **Approximate interning.** The table's hash index is replaced by a
   256-slot direct-mapped cache of recent styles (a multiplicative hash of
   the style's words). A hit returns the cached id; a miss appends a new id
   without looking further. A style can then have more than one id. That
   only splits a uniform row or a history run where it need not; equality
   of ids still implies equality of styles, and collection drops the
   duplicates no longer on screen.
3. **Collection sized to the screen.** The next collection runs once the
   table has grown by max(4096, 2 × the screen's cells) since the last one
   (the first round used max(4096, 2 × live)). A collection scans every
   screen cell, so its cost per new style is now a few instructions at any
   screen size. The table can reach about three times the screen's cells,
   about 1.4 MB at 300x75 under truecolor churn.
4. **History cell copy.** The cells-form copy resizes the page buffer once
   and writes into it, instead of an `extend_from_slice` per cell, and the
   style remap's miss path is out of line. This is the same kind of tuning
   as the first round's; it also helps cjk.

The foldhash dependency is gone. `proto-check` passes 20 of 20 cases at 1
and at 8 MiB, and the unit tests add an inline round trip and a stream
that mixes inline and table styles through repeated collections (both fail
if the key remap or the inline layout is broken).

### Each step on sgr-dense (8 MiB, instructions)

| build | 120x40 | vs ref | 300x75 | vs ref |
|---|---:|---:|---:|---:|
| reference | 725.7 M | | 730.2 M | |
| C, first round | 841.2 M | +15.9% | 895.1 M | +22.6% |
| + approximate interning | 798.6 M | +10.0% | 829.6 M | +13.6% |
| + collection sized to the screen | 787.6 M | +8.5% | 792.7 M | +8.6% |
| + inline styles | 753.1 M | +3.8% | 757.2 M | +3.7% |
| + history cell copy (final) | 735.9 M | +1.4% | 740.1 M | +1.4% |

Each step was kept because it won on its own. With inline styles, 53% of
the template styles in this pass skip the table (302,508 of 637,560 go to
it). The cache-and-append insert costs about 60 instructions (37.9 M over
637,574 calls) against about 100 for the hash table's (63.8 M), which also
re-inserted every survivor at each collection (16.9 M more).

### The gate, second round

| criterion (C) | needed | measured | met |
|---|---|---|---|
| vt/ascii instructions per byte, 1 and 8 MiB | ≤ 27 | 18.7 / 18.7 at 120x40, 19.5 / 19.5 at 300x75 | yes |
| scroll | −25% or better | −34.8% to −43.0% | yes |
| wide-wrap | −25% or better | −46.0% to −55.9% | yes |
| cjk | no worse than +3% | −35.8% to −43.0% | yes |
| sgr-dense | no worse than +3% | **+0.5% to +1.4%** | **yes** |
| PTY ascii median, 120x40 and 300x75 | +15% or better | +42.4% and +49.2% (paired medians, 120 rounds; 95% CI 1.395–1.454 and 1.464–1.515) | yes |
| spread of those medians | under 5% | IQR/median 8.2–10.0% per arm (7.7–9.8% for the reference and A); batch medians agree within 3% | no (as in the first round; see Wall clock) |
| live heap | within the current budgets | 1.20 / 1.34 / 9.99 MB against 3.46 / 4.36 / 18.98 MB | yes |

| "A only" test | needed | measured | met |
|---|---|---|---|
| A's share of C's instruction cut | two thirds or more | ascii 42–74%, scroll 30–53%, wide-wrap 54–80%, cjk 52–72% | no |
| A's share of C's wall-clock gain (ascii) | two thirds or more | 66% at 120x40, 55% at 300x75 | no |

| NO-GO test | fires when | measured | fires |
|---|---|---|---|
| C's wall-clock gain on ascii | under +10% | +42.4% / +49.2% | no |

Instructions, callgrind inside the timed pass (`scripts/bench/proto-icount.sh
--jobs 6`, 60 runs):

| corpus | MiB | geometry | ref | A | C | ref instr/B | A instr/B | A vs ref | C instr/B | C vs ref | A's share of C's cut |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii | 1 | 120x40 | 41,922,329 | 31,096,467 | 19,644,816 | 40.0 | 29.7 | -25.8% | 18.7 | -53.1% | 49% |
| ascii | 1 | 300x75 | 45,238,481 | 34,744,525 | 20,482,817 | 43.1 | 33.1 | -23.2% | 19.5 | -54.7% | 42% |
| ascii | 8 | 120x40 | 274,973,864 | 187,836,747 | 157,201,705 | 32.8 | 22.4 | -31.7% | 18.7 | -42.8% | 74% |
| ascii | 8 | 300x75 | 300,907,611 | 217,174,847 | 163,928,790 | 35.9 | 25.9 | -27.8% | 19.5 | -45.5% | 61% |
| wide-wrap | 1 | 120x40 | 37,113,057 | 25,844,517 | 16,379,398 | 35.4 | 24.6 | -30.4% | 15.6 | -55.9% | 54% |
| wide-wrap | 1 | 300x75 | 26,907,941 | 17,626,927 | 12,849,629 | 25.7 | 16.8 | -34.5% | 12.3 | -52.2% | 66% |
| wide-wrap | 8 | 120x40 | 242,360,275 | 153,428,561 | 130,964,340 | 28.9 | 18.3 | -36.7% | 15.6 | -46.0% | 80% |
| wide-wrap | 8 | 300x75 | 199,338,275 | 125,248,668 | 103,306,001 | 23.8 | 14.9 | -37.2% | 12.3 | -48.2% | 77% |
| scroll | 1 | 120x40 | 192,511,141 | 158,470,297 | 120,646,710 | 183.6 | 151.1 | -17.7% | 115.1 | -37.3% | 47% |
| scroll | 1 | 300x75 | 227,835,402 | 198,465,096 | 129,828,534 | 217.3 | 189.3 | -12.9% | 123.8 | -43.0% | 30% |
| scroll | 8 | 120x40 | 1,480,128,698 | 1,207,446,049 | 965,168,643 | 176.4 | 143.9 | -18.4% | 115.1 | -34.8% | 53% |
| scroll | 8 | 300x75 | 1,762,023,231 | 1,527,507,465 | 1,038,551,928 | 210.0 | 182.1 | -13.3% | 123.8 | -41.1% | 32% |
| sgr-dense | 1 | 120x40 | 91,814,931 | 94,680,907 | 92,843,394 | 87.6 | 90.3 | +3.1% | 88.5 | +1.1% | n/a |
| sgr-dense | 1 | 300x75 | 92,218,274 | 94,438,942 | 92,641,680 | 87.9 | 90.1 | +2.4% | 88.3 | +0.5% | n/a |
| sgr-dense | 8 | 120x40 | 725,710,475 | 747,249,996 | 735,907,626 | 86.5 | 89.1 | +3.0% | 87.7 | +1.4% | n/a |
| sgr-dense | 8 | 300x75 | 730,214,780 | 750,500,034 | 740,128,548 | 87.0 | 89.5 | +2.8% | 88.2 | +1.4% | n/a |
| cjk | 1 | 120x40 | 230,295,321 | 178,972,888 | 132,918,106 | 219.6 | 170.7 | -22.3% | 126.8 | -42.3% | 53% |
| cjk | 1 | 300x75 | 234,772,241 | 182,460,639 | 133,791,459 | 223.9 | 174.0 | -22.3% | 127.6 | -43.0% | 52% |
| cjk | 8 | 120x40 | 1,644,263,963 | 1,222,810,539 | 1,055,912,910 | 196.0 | 145.8 | -25.6% | 125.9 | -35.8% | 72% |
| cjk | 8 | 300x75 | 1,675,354,788 | 1,256,825,225 | 1,063,715,913 | 199.7 | 149.8 | -25.0% | 126.8 | -36.5% | 68% |

Against the first round, C's other corpora moved by under 1% except cjk
(−3% to −5%, from the history copy). ascii and scroll are 0.5–0.9% worse:
the wider style key in the row heads and the inline branch in the lookup.
A is 1–2% worse on ascii and scroll for the same reasons. The reference's
counts moved by up to 0.7% (cjk) between the two builds, as the first round
saw the feature move it; each comparison is within one build.

### Wall clock, second round

Same harness and method as the first round: `proto-flood`, 32 MiB, three
batches of 40 interleaved rounds for ascii, 12 rounds for sgr-dense and
cjk. The batches ran right after a period of load 20–47 from other agents'
builds, at load 5.8–11 (ascii) and 6.5–8.7 (sgr-dense and cjk). Absolute
rates are about 10% below the first round's for every arm, the reference
included.

| case | arm | median MiB/s | IQR / median | paired median vs ref | 95% CI (bootstrap) |
|---|---|---:|---:|---:|---:|
| ascii 120x40 | ref | 233.1 | 9.8% | | |
| ascii 120x40 | A | 302.1 | 7.7% | 1.278 | 1.261–1.292 |
| ascii 120x40 | C | 336.5 | 10.0% | 1.424 | 1.395–1.454 |
| ascii 300x75 | ref | 216.7 | 9.7% | | |
| ascii 300x75 | A | 276.5 | 9.5% | 1.270 | 1.251–1.291 |
| ascii 300x75 | C | 327.2 | 8.2% | 1.492 | 1.464–1.515 |

Per batch, C over the reference was 1.416 / 1.427 / 1.448 at 120x40 and
1.496 / 1.461 / 1.500 at 300x75; A over the reference 1.274 / 1.273 /
1.291 and 1.262 / 1.270 / 1.278. A's share of C's gain is 66% and 55%,
below two thirds. A fourth batch pinned to cores 8–11 (not part of the
method, only to test whether pinning tightens it) gave 1.452 and 1.462
with IQR/median 9.6–12.6%: the spread comes from the shared machine, not
from scheduling across cores. The paired per-round ratios are as spread as
the arms (IQR/median 10–12%), so pairing does not get under 5% either.

| case | ref | A | C | A vs ref | C vs ref (95% CI) |
|---|---:|---:|---:|---:|---:|
| sgr-dense 120x40 | 112.1 | 111.2 | 117.0 | 0.955 | 1.061 (0.951–1.102) |
| sgr-dense 300x75 | 119.8 | 113.3 | 118.7 | 0.952 | 1.001 (0.966–1.039) |
| cjk 120x40 | 84.3 | 104.6 | 124.7 | 1.271 | 1.497 (1.463–1.530) |
| cjk 300x75 | 79.7 | 100.3 | 121.6 | 1.253 | 1.518 (1.431–1.591) |

sgr-dense is now level with the reference on the wall clock (it was 12–16%
slower), in line with the instruction counts; A is still about 5% slower.

### Live heap, second round

| case | reference | A | C | budget |
|---|---:|---:|---:|---:|
| empty 300x75 | 1,172,860 | 268,983 | 265,709 | 1,229,000 |
| typical 10k lines, 120x40 | 3,297,990 | 3,215,268 | 1,195,364 | 3,461,000 |
| typical 10k lines, 300x75 | 4,152,860 | 3,359,703 | 1,338,189 | 4,358,000 |
| sgr-dense 10k lines, 120x40 | 18,077,990 | 17,995,268 | 9,989,412 | 18,980,000 |

C's typical cases grew by about 40 KB (about 4 bytes per history row, the
wider style key in the row head); sgr-dense fell by 0.4 MB, because inline
styles take no page style entries. All stay at a third of the reference or
less, except sgr-dense at 55%.

### How representative sgr-dense is, and real colorful output

sgr-dense draws each SGR from seven kinds at random (256-color fg, truecolor
fg, 256-color bg, a bold/dim/italic plus basic fg pair, a dark truecolor bg,
reset, and bold-italic-underline) and prints one to four letters after each.
The template accumulates until a reset, so about half of the printed
templates carry a truecolor and almost every one is new. Real output was
captured and replayed through the same passes for comparison: `ls -l
--color=always` over three large directories, `git log -p --color=always`
over 400 commits, `rustc` diagnostics for 400 broken functions, and the
VT core's Rust source through `pygmentize` (monokai) as truecolor and as
256-color. Each was reduced to text, SGR and CRLF (tabs expanded, other
escapes dropped, which the prototype would ignore), repeated to about
8 MiB, and passed `proto-check` cell for cell at both geometries.

| input | SGR per KiB | cells per SGR | distinct templates | truecolor share |
|---|---:|---:|---:|---:|
| sgr-dense (8 MiB) | 80 | 2.4 | 343,188 | 47% |
| ls --color | 33 | 24.8 | 7 | 0% |
| git log -p | 68 | 10.6 | 7 | 0% |
| rustc diagnostics | 80 | 8.1 | 6 | 0% |
| pygmentize, 256-color | 108 | 2.0 | 7 | 0% |
| pygmentize, truecolor | 75 | 2.0 | 7 | 100% |

So sgr-dense is about as SGR-dense as highlighted source, but its styles
almost never repeat, where real output repeats a handful. It stands for
gradient and image output (lolcat, chafa, truecolor TUI gradients), not
for `ls`, diffs, diagnostics or highlighters. Its worst case is still the
right gate, because the table path is what such output takes.

C against the reference on the captures (instructions; first-round C
for comparison):

| input | ref instr/B 120x40 | C first round | C now | A now | C now, 300x75 |
|---|---:|---:|---:|---:|---:|
| ls --color | 76.4 | -29.1% | -38.3% | -25.3% | -39.7% |
| git log -p | 72.5 | -25.6% | -29.4% | -21.5% | -32.1% |
| rustc diagnostics | 91.0 | -14.7% | -24.1% | -12.0% | -26.2% |
| pygmentize, 256-color | 86.7 | -0.5% | -8.3% | -2.7% | -9.5% |
| pygmentize, truecolor | 67.9 | -1.2% | -3.9% | +1.1% | -5.1% |

Real colorful output was never as bad for C as sgr-dense, because the
first round's hash table hit on repeated styles. The second round helps
all of it: inline styles remove the table from the 256-color and
basic-color cases, and the cache handles the truecolor highlighter's seven
styles. Truecolor highlighting gains least: every token carries a
truecolor SGR, so it takes the table path on every change, as sgr-dense
does, though nearly always as a cache hit. (The first-round column is the
first-round binary against the second-round reference; the reference moves
by under 1% between builds.)

The sections from here on are the first round's. Environment, What was
built and Commands hold for both rounds, with the style handling replaced
as described above.

## Environment

- 2026-11-09. Branch `adr-0022-prototype`: `38c1ff5` (the ADR) plus
  `cd31763` (the prototype). Second round: branch `adr-0022-sgr`, the
  same plus the style-handling commit.
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

## First round: the gate

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

## First round: detail

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

## First round: interpretation

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
