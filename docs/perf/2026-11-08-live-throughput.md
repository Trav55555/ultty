# Live throughput: where a flood goes (2026-11-08)

On the AWS T4 box (g4dn.xlarge, 120x40, DejaVu Sans Mono 11,
`scripts/bench/compare.sh`), Ultty trailed Ghostty on plain and mixed text:

| MiB/s | Ultty X11 | Ghostty X11 | Ultty Wayland | Ghostty Wayland |
|---|---:|---:|---:|---:|
| ascii | 71 | 130 | 70.5 | 126 |
| unicode-mixed | 16 | 26 | 16.4 | 24.6 |
| cursor-redraw | 91 | 85 | | |
| sgr-dense | 48 | 27 | | |

The working assumption was that most of the loss was outside the VT core
(headless parse+grid on a c7a ran ascii at 247 MiB/s): PTY reads, lock
hand-off, per-frame render cost, frames per MiB. On this workstation the
profile says otherwise: **the I/O thread is the bottleneck and it is busy
parsing.** This note records the profile, the five changes kept, the ones
measured and dropped, and what is left.

## Setup

- Ryzen 5 7600 (6 cores, SMT), RX 7900 XT, Mesa 26.0.8 RADV, load 5 to 7
  during the runs (other agents' builds).
- Wayland: a private headless GNOME Shell 50.1 (`--virtual-monitor
  1920x1080@60`, private `XDG_RUNTIME_DIR` and session bus), Vulkan on the
  real GPU. X11 (Xvfb + lavapipe) was used only as a sanity check.
- `ubench-feed` as Ultty's shell (16 MiB CRLF corpora, 64 KiB writes, CPR
  fence), the `compare.sh` Ultty config (ligatures off, 10,000 lines of
  scrollback), default window size (114x50).
- Per-thread CPU from `/proc/<pid>/task/*/stat` at the end of each launch;
  instruction counts from callgrind.
- Ghostty could not be run here (the snap refuses to start outside a
  systemd user session), so there is no local Ghostty comparison.

## Profile

### The I/O thread is saturated

Three 16 MiB passes per launch, per-thread CPU over the whole launch
(10 ms ticks):

| corpus | MiB/s | `ultty-pty` thread | main thread |
|---|---:|---:|---:|
| ascii | 170 | 26 (18 user, 6 sys) | 9 |
| cursor-redraw | 207 | 19 | 10 |
| sgr-dense | 114 | 40 | 11 |
| unicode-mixed | 38 | 124 (111 user, 11 sys) | 14 |

48 MiB of ascii at 170 MiB/s is about 0.28 s of flooding; the I/O thread
spent 0.26 s of CPU in it. For unicode-mixed it is 1.24 s of CPU for about
1.26 s of flooding. It hardly ever waits for data (under `strace -c` in the
end-to-end bench below, 58 of 20,778 reads returned EAGAIN), so throughput
is the I/O thread's CPU rate:
parse plus grid plus scrollback in user space (75% for ascii, 90% for
unicode-mixed), and the kernel's PTY reads in sys (25% and 10%). Each read
returns at most 4 KiB (the n_tty buffer), whatever the buffer size.

The main thread is not competing for much. With the pipeline trace on, a
whole ascii + unicode-mixed run (3 passes each, about 2 s of flooding)
drew 10 frames on the base build and 50 on the final one, with a median
`draw_us` of 1.0 ms and 0.73 ms: the terminal lock is held for drawing
well under 3% of the flood. Where frames do follow each other, they are 17
to 21 ms apart on average, so pacing holds. (The low frame rate during
floods is its own issue, see the end.)

### Where the I/O thread's instructions go

`ultty-bench vt`, 1 MiB, 120x40 (the same per-byte cost as the live I/O
thread: callgrind on the end-to-end bench below gave 69 instructions per
byte for ascii against 68 headless):

| ascii, base | share |
|---|---:|
| `CompactRow::encode_from` (scrollback encoding) | 39% |
| `Grid::write_ascii_span` | 17% |
| parser's printable-run scan (inlined into the caller) | 13.5% |
| `Row::erase` | 6% |
| memcpy, malloc (history still filling at 1 MiB) | 8% |

| unicode-mixed, base | share |
|---|---:|
| `Grid::write_measured_char` | 19% |
| `Grid::continues_cluster` incl. the UAX #29 fallback (37k calls, 1,000 instructions each) | 23% |
| `CompactRow::encode_from` | 8.6% |
| `print_utf8_run` + its `from_utf8` | 10% |
| `Buffer::write_char` | 5.4% |

### Frame cost

`render_scrolling` (120x40, a fresh screen every frame, ligatures on), on
RADV under callgrind with collection inside `render_to_view`: 5.69 M
instructions per ascii frame. SipHash in the glyph and shaping caches was
31% of it, memcpy 16% (vertices gathered into one vector, then copied
again into the staging buffer), and every row built a throwaway index
vector.

## Changes kept

One commit each; instruction counts are deterministic, wall-clock numbers
are best-of or median as noted.

| commit | what | measured |
|---|---|---|
| Scan printable ASCII runs eight bytes at a time | word-at-a-time (SWAR) test for bytes outside 0x20..0x7E in `feed_with_ascii_spans` | vt/ascii -8.6%, cursor-redraw -10.2%, wide-wrap -10.1%, scroll-region -14.0%; scroll +1.1%, sgr-dense +1.1% |
| Hash the glyph and shaping caches with foldhash | `foldhash` (seeded per process, like `RandomState`) for the atlas, shaping and run caches; new direct dependency of `ultty`, already built for hashbrown | frame: ascii 5.69 M -> 3.93 M (-31%), unicode-mixed 3.91 M -> 2.67 M (-32%) |
| Stop building a throwaway index list for every grid row | grid quads emit vertices only; the draw uses the shared quad index list | frame: ascii -5.8%, unicode-mixed -8.7% |
| Upload grid vertices from the row cache into staging memory | rows copied straight into a `write_buffer_with` view; index buffers rewritten only when they hold too few quads | frame: ascii -13%, unicode-mixed -15% |
| Copy plain rows into scrollback without the per-cell loop | a one-style, all-ASCII row is checked in one pass and its text copied in one go | vt/ascii -7.7%, wide-wrap -8.9%; scroll +1.0%; others within 0.2% |

The baseline in `bench/baselines/icount.json` is regenerated in its own
commit: vt/ascii 71.3 M -> 60.2 M (-15.6%), cursor-redraw -10.4%,
wide-wrap -18.1%, scroll-region -14.1%, scroll +2.1%, sgr-dense +1.0%,
the rest within 0.4%. The memory budgets pass unchanged.

Render frame, wall clock on RADV (criterion, two interleaved runs per
arm, base -> final): ascii 572-594 us -> 333-356 us, unicode-mixed
397-443 us -> 284-303 us, cjk 272-277 us -> 208-219 us.

### New bench: `pty_flood`

`cargo bench -p ultty --bench pty_flood` has `dd` write a 32 MiB corpus
into a raw-mode PTY (as `ubench-feed` does) and times it through the real
I/O thread, with the bench thread taking the terminal lock once per frame
interval and copying the visible rows. No GPU. It reproduces the live
numbers (ascii 152 MiB/s against 150-175 live) and runs under callgrind with
`--separate-threads=yes`. At 8 MiB, spawning and reaping the child hid
most of the work (about 97 MiB/s for ascii whatever the build), hence the
size.

## Before and after, locally

Interleaved launches, base (`d4c3f26`) and final, 3 passes per corpus per
launch, median MiB/s with min-max.

Wayland, all 12 CPUs, 4 launches per arm (n = 12):

| corpus | before | after |
|---|---:|---:|
| ascii | 153 (108-197) | 165 (120-203) |
| cursor-redraw | 201 (169-239) | 214 (164-242) |
| sgr-dense | 114 (81-121) | 115 (84-123) |
| unicode-mixed | 35.2 (29.3-39.6) | 34.7 (29.6-38.7) |

Wayland, Ultty and the compositor pinned to two cores with their SMT
siblings (CPUs 4, 5, 10, 11; a stand-in for the 4-vCPU g4dn.xlarge), n = 12:

| corpus | before | after |
|---|---:|---:|
| ascii | 159 (151-185) | 177 (161-186) |
| cursor-redraw | 202 (156-227) | 189 (160-233) |
| sgr-dense | 106 (77-118) | 105 (75-116) |
| unicode-mixed | 34.8 (29.3-38.0) | 34.7 (31.5-38.5) |

`pty_flood`, 3 interleaved criterion runs per arm (median of the point
estimates): ascii 152 -> 159, cursor-redraw 185 -> 183, sgr-dense
100 -> 100, unicode-mixed 36.3 -> 35.2.

X11 sanity (Xvfb + lavapipe, one launch each, 2 passes): ascii 84/62 ->
84/53, unicode-mixed 20.0/17.7 -> 19.0/17.4. Lavapipe rasterizes on the
same CPUs, so these only show that nothing broke.

So: ascii gains 5 to 11%, the other corpora are unchanged within the noise
(spreads are 20 to 40% at this load). The frame-cost cuts are real (-40%
wall per frame) but frames were already a few percent of a flood here; they
should matter more on the T4 box, where the main thread, the compositor and
the I/O thread share four vCPUs. That needs the AWS re-run to confirm.

Correctness: `ulttyctl screenshot` of ASCII, CJK, emoji (ZWJ, flags, skin
tones), combining marks, RTL scripts and SGR styles after 120 lines of
scrolling is byte-identical between base and final on Wayland and on X11,
and `ulttyctl text --scrollback` (the same content scrolled into history,
so through the new encoder) is identical too.

## Measured and dropped

- **Gathering PTY reads before parsing.** The I/O thread read up to 64 KiB
  (several 4 KiB reads) before taking the lock, notified the condvar only
  when someone waits in `EventLoop::wait` (std's `notify_all` is a futex
  syscall even with no waiters, and it ran per chunk), and skipped the
  second lock of `flush_outbound` when nothing is queued. No change: base
  vs gathered, interleaved, n = 9: ascii 151 vs 150, unicode-mixed 34.9 vs
  33.3; pinned to two cores, ascii 172 vs 174, unicode-mixed 36.5 vs 34.5.
  Per-chunk overhead is small next to parsing 4 KiB.
- **Splitting every style run into an ASCII copy and a per-cell tail** in
  `encode_from`: ascii -20%, but CJK +9 to 10% (a wide character and its
  spacer differ in style, so CJK rows are one-cell runs) and sgr-dense +4%.
  The row-level fast path keeps most of the ascii gain without that.
- **Hybrid 8-byte prefix scan** before the word test: ascii -7.7% instead of
  -8.9%, for smaller regressions on short runs; the two-byte prefix kept is
  in between.

Not tried, because the profile ruled them out on this box: fewer frames per
MiB (already far below one per 16 ms during floods), shifting cached row
vertices on scroll (during a flood every frame is a new screen; it would
only help slow scrolling), and bigger reads (the kernel hands out 4 KiB).

## What is left

The gap to Ghostty is the I/O thread's per-byte cost. In order of size
(items 1 to 3 are addressed in
[2026-11-08-vt-core-throughput.md](2026-11-08-vt-core-throughput.md)):

1. **Scrollback encoding.** Plain one-style rows now take the copy path,
   but every other row (prompts, colored output, any non-ASCII) still goes
   through the per-cell style loop, which is 8.6% of unicode-mixed and was
   39% of plain text before. Ghostty writes cells into pages that become
   scrollback without a copy. A cheaper encoding, or encoding off the I/O
   thread, is the largest lever left for styled and plain text.
2. **Grapheme joins** fall back to the UAX #29 segmenter for 37k of the
   codepoints in 1 MiB of unicode-mixed, about 1,000 instructions each (10%
   of the corpus, 23% with `continues_cluster` itself). Regional-indicator
   pairs and emoji ZWJ sequences are the likely bulk; `cluster::joins`
   could answer them directly.
3. **`print_utf8_run` validates with `from_utf8`** and then decodes again
   (6% of unicode-mixed).
4. **Frames during floods.** The UI got 10 to 50 frames in about 2 s of
   flood (5 to 25 per second). The I/O thread re-takes the terminal lock
   right after releasing it, so the UI thread rarely wins it. Fixing that
   (a hand-off when the UI is waiting) would cost some throughput and
   should be measured as latency and smoothness, not MiB/s.

The AWS re-run (`compare.sh --session x11` and `--session wayland` with
Ghostty) is still needed to state the effect on the T4 box.
