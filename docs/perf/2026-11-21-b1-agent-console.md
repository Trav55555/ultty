# B1: the agent console's four numbers (2026-11-21)

Plan item B1 asks for four numbers for `examples/agent-console`, measured in
native mode and in ANSI mode: scroll position held while output streams,
copy fidelity, resize without corruption, and flat memory
([ADR 0031](../adr/0031-agent-console.md) section 4).
[`scripts/bench/b1-agent-console.sh`](../../scripts/bench/b1-agent-console.sh)
took them on a private Xvfb, for Ultty in both modes and, as references, for
the same ANSI output in Alacritty and Ghostty.

Native mode meets the copy target and misses the scroll and resize
targets. Ultty does not hold the scroll position in either mode, and in
native mode a resize corrupts the frozen cards. Alacritty and Ghostty hold
the scroll position and pass resize. Memory is not judged in this note:
12 minutes of replay cannot show a trend under 1 MiB/hour.

## Results

| Number | Native (Ultty) | ANSI (Ultty) | ANSI (Alacritty) | ANSI (Ghostty) | Target, native |
|---|---|---|---|---|---|
| 1. Scroll held, short.jsonl | back at the bottom before the first shot (0.1 s) | moved by 0.7 s, at the bottom at the end | 0 rows moved | 0 rows moved | 0 |
| 1. Scroll held, long tool output | held 23 s, then moved; at the bottom at the end | back at the bottom before the first shot (0.1 s) | 0 rows moved | 0 rows moved | 0 |
| 2. Copy errors (11 blocks) | 0 | 0 | 0 | 0 | 0 |
| 3. Resize: transcript lines differing | 4 of 85 | 0 of 80 | not measurable (no text access) | not measurable (no text access) | 0 |
| 3. Resize: screen rows differing, bottom / top of scrollback | 9 / 1 of 30 | 3 / 0 of 30 | 0 / 0 of 30 | 0 / 0 of 30 | 0 |
| 4. Memory trend, last 4.5 min (anonymous / Rust heap) | -0.59 / +0.51 MiB/h | +2.84 / -0.33 MiB/h | -0.48 MiB/h | -20.1 MiB/h | under 1 MiB/hour; not judged here |

Copy fidelity counts four things per block, summed over 9 prose blocks, the
fenced code block and the diff of `short.jsonl`: characters wrong
(Levenshtein distance to the source), lines with trailing spaces, lines
indented differently from the source, and line breaks that are not in the
source. All four are 0 in every column.

## What the numbers show

**Ultty drops the scroll position on every character printed.**
`Buffer::write_char`, `write_ascii_span` and `write_text_with_width` in
`crates/ultty-vt/src/grid/buffer.rs` set `scroll_offset = 0`. Any printed
character returns the view to the bottom. Native mode holds only while
nothing is printed to the grid. During the long tool call, output went
into the inline card, and the view stayed 20 rows up for 23 s. It returned
to the bottom when the closing prose was printed. In `short.jsonl`, prose
lines are printed every few hundred milliseconds, so the view returned to
the bottom before the first sample. ANSI mode in Ultty redraws its live
region every frame, so the view returned to the bottom almost at once.
Alacritty and Ghostty keep the view anchored to the scrollback content
while output arrives. This is a terminal behaviour, not an app one.
Inline cards do not fix it.

**Native resize corrupts frozen cards.** The run resized through 80, 150
and 100 columns during the stream, then 120, 80, 150 and 100 after it. The
transcript was compared with a fresh run at 100 columns:

- A tool card frozen while the window was 80 columns wide kept its 80-column
  clip (``... assertion `left == right` f`` became `... asserti`). The
  card's next line (`test result: FAILED ...`) was missing.
- Three blank lines between blocks were missing.
- On screen, card backgrounds were drawn as bands across rows that no longer
  belong to the card. The approval card's choices were spread over five
  rows, with blank rows between them.

ANSI mode in Ultty reflowed the same session to the fresh transcript
exactly. The ANSI screen differed by three rows: the same text sat one row
higher, with blank rows under the status line. No row held wrong text.

**Memory: the heap is flat once scrollback is full, but this run is too
short to judge.** At 2,000 lines of scrollback, Ultty's live Rust heap
rose from 3.1 to 3.9 MiB (native) over the first 7 minutes, while
scrollback filled. It then stayed between 3.86 and 3.91 MiB for the last
5.5 minutes. The trends above cover only the 4.5 minutes after the 8-minute
warm-up (9 samples). Anonymous memory moves by up to 0.7 MiB between samples
there, so a trend under 1 MiB/hour cannot be shown or ruled out in that
window. Ghostty's anonymous memory fell by 20 MiB/hour. The cause
was not investigated. The flat-memory number needs the 10-hour
soak on `soak.sh`'s infrastructure, which this slice does not run. A first
memory replay with 10,000 lines of scrollback never filled it (the session
is about 4,850 rows at 120 columns). Every terminal grew while it stored the
transcript: native +9.2, ANSI +4.7, Alacritty +71.5 MiB/hour. Ghostty fell
by 9.5 MiB/hour. These numbers measure storing the transcript, not leaks.

**Copy is clean everywhere.** The ANSI console prints finished lines
whole and leaves wrapping to the terminal. It prints no trailing spaces
and no hard breaks. Its fallback mode is therefore not the "before" case
for copy that agent CLIs are: this baseline is careful (ADR 0031 section 3),
and Ultty's copy joins soft-wrapped rows (#195), as Alacritty and Ghostty
do. The native frozen cards copy as clean text as well.

## Method

Every number used the same app binary, fixture, 120-column window and
DejaVu Sans Mono 11. Only the terminal and the transport differed. Each
launch ran a wrapper script in the terminal. Before the stream, the wrapper
reported `stty size` after every resize and drew two inverse cells on
request. The harness used them to find the cell grid in screenshots and
to fit each window to exact column and row counts. The wrapper then
cleared the screen and scrollback, replayed the fixture with
`--auto-approve 1`, and left the result on screen.

1. **Scroll held.** Each terminal's lines per wheel click were measured
   before the stream, from how far a calibration cell moved: Ultty and
   Alacritty 2, Ghostty 1. While the fixture streamed in a 120×30 window,
   the harness wheeled up 20 lines through xdotool on the X display. It
   screenshotted the cell grid just before the wheel, 0.1 s after it, and
   then as often as it could until the stream ended. The target was every
   100 ms. The loaded machine managed about every 320 ms: 37 to 40 shots
   in 12 s for `short.jsonl`, and 169 to 179 in 31 s for the long fixture.
   Each shot was aligned with the post-wheel shot by matching identical
   pixel rows from its top half. The vote-winning offset, in cell rows, is
   how far the view moved. At the end, the harness wheeled the view far
   down. If the screen did not change, the view had been yanked to the
   bottom. The first scenario was `short.jsonl` at recorded pace, wheeled
   5 s in. The second was `short.jsonl --fast` followed by
   `long-tool-output.jsonl` at recorded pace, wheeled 4 s into the long
   call. Ghostty's scrollbar (the rightmost 24 pixels) was left out because
   it moves while the view holds.
2. **Copy fidelity.** `short.jsonl` was replayed at 4× in a 120×100 window,
   so the whole transcript fit on screen. Each block's screen rows came from
   Ultty's own text of the screen (`ulttyctl text`, logical lines), wrapped
   at 120 columns with East Asian widths. The reference terminals reused
   Ultty ANSI's rows, because the console writes the same bytes to them.
   Each block was selected with a mouse drag (xdotool): press at the end
   of its last row, release at the start of its first. The selection was
   read with `ulttyctl selection` in Ultty, and from the X PRIMARY selection
   through libX11 in Alacritty and Ghostty (xclip and xsel are not
   installed). It was compared with the block as assembled from the
   fixture's events: text deltas joined and split at blank lines outside
   fences, or the diff event's hunk.
   `scripts/bench/test_b1_agent_console.py` checks that each counter
   reports the fault it measures.
3. **Resize.** `short.jsonl` at recorded pace in a 120×30 window. The
   window went to 80, 150 and 100 columns at 2.4 s intervals during the
   stream, and to 120, 80, 150 and 100 at 0.6 s intervals after it (xdotool
   `windowsize`, to pixel sizes fitted with `stty` before the stream). A
   fresh run at 100 columns was the reference. In Ultty, the two
   `ulttyctl text --scrollback` outputs were diffed by line. In every
   terminal, the bottom screen and the top of scrollback were screenshotted,
   cut into cell rows and diffed as sequences of rows. A screen that is one
   row higher therefore counts one row, not all 30. Text is not readable
   from outside Alacritty or Ghostty, so they have only the screen numbers.
4. **Memory.** All four terminals ran at once on the same display at
   120×40, replaying `two-hours.jsonl` at `--speed 10`. That is 12 minutes
   of real time for two hours of session, which is about 4,850 rows of
   transcript at 120 columns. Each terminal had 2,000 lines of scrollback,
   so it reached its cap during the warm-up. Every 30 s, the harness
   sampled RSS, anonymous RSS, swap and memfd-backed GPU memory from
   `/proc`. For Ultty, it also took the live Rust heap from its
   `heap-stats` log. The samples were written to a `soak.sh`-style
   `samples.csv` and summarized by `scripts/bench/soak-report.py`. The
   table uses `--warmup 8`, now the harness default. The run itself
   reported with a 4-minute warm-up, which falls before the cap and gives
   +1.79 (native) and +2.62 (ANSI) MiB/hour, rising with scrollback. That
   run is separate from the run behind numbers 1 to 3. It was repeated
   after fixing the harness's heap parsing (the log is colored) and
   adding the smaller scrollback.

## Environment

- Ultty `64526a9` (branch `b1-slice3-measure` on #195), release build with
  `heap-stats`; agent-console from the same tree.
- Alacritty 0.16.1 and Ghostty 1.4.0 (snap rev 1016), on the same Xvfb
  (2560×2000, 24-bit, no window manager). Ultty used Mesa 26.0.8 lavapipe
  for Vulkan. Alacritty and Ghostty used Mesa's software GL.
- Ryzen 5 7600, kernel 7.0.0-42. The machine was shared and busy: load
  average 15 to 30 during the run, and a separate 10-hour soak of two
  Ultty instances was running at the same time.

## Caveats

- **Claim class `live-swrast`.** Software rendering on a loaded machine.
  Every number here concerns behaviour, not speed, but sampling was about
  3× slower than the 100 ms the ADR asks for. A view that moved and came
  back between two shots would be missed. The yank check at the end does
  not depend on sampling.
- **Cells are not the same size.** At `size = 11`, Ultty's cells were 7×11
  pixels, Alacritty's 9×18 and Ghostty's 9×17. Every comparison uses each
  terminal's own geometry, so this changes no number, but Ultty's font size
  is not equivalent to the others' font size.
- **Copy needed a workaround in Ultty.** Ultty draws its own window frame,
  and a press within 8 pixels of the window edge starts a window resize. At
  7-pixel cells, that band covers all of column 0. A drag that starts there
  does not select text. The harness starts each drag at the right end
  instead. A user who starts a selection at the left edge gets a resize.
- **Resize in Alacritty and Ghostty is measured on screen only**, at the
  bottom and at the top of scrollback. Rows in between were not compared.
- **One run each.** One run per scenario, no repeats. Numbers 1 to 3 come
  from one harness run; number 4 comes from a second (`--numbers memory`).

To reproduce, run `scripts/bench/b1-agent-console.sh`. It writes
`target/bench-results/<date>-b1/` (`results.json`, `report.md`, the compared
screenshots, each launch's wrapper and logs). It took 23 minutes here, 12.5
of them for memory. `/usr/bin/python3 -I scripts/bench/test_b1_agent_console.py`
tests the comparison helpers without a display.

## Filed

- #196: output snaps a scrolled-back view to the bottom (scroll held, both modes).
- #197: native resize corrupts frozen inline cards and the blank lines around them.
- #198: the left-edge resize band swallows clicks in column 0.
- #199: cells are much smaller than other terminals' at the same font size.
- #200: ANSI-mode resize leaves the screen one row off.
