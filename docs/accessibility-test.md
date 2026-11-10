# Accessibility manual test

The script a person runs with a screen reader, from ADR 0021 §5. Each run
is recorded below with the date, the Orca version and the distribution.
Steps 1 to 3 cover the terminal grid (slice 4), step 6 native scenes
(slice 5) and step 5 live regions (slice 6); step 4 waits on ulttui
semantics (slice 7) and is added with it.

**Status: not yet run with Orca.** What has been checked without a screen
reader is under "Automated stand-in" at the end.

## Setup

1. Ubuntu 24.04 or Debian 13, a GNOME session (Wayland or X11).
2. Build Ultty: `cargo build --release -p ultty --bins`.
3. The bridge is on by default; check that `~/.config/ultty/config.toml`
   does not turn it off:

   ```toml
   [behavior]
   accessibility = true
   ```

4. Turn Orca on: `Super+Alt+S`. Note its version (`orca --version`).

## Steps

### 1. Start Ultty

Run `target/release/ultty`.

Expect: Orca names the window and the focused pane's title (the shell's
title, else "Terminal 1"), and says "terminal". Flat review
(`Insert+Up` on desktop layout, `CapsLock+I` on laptop layout) reads the
current screen line, the prompt.

Record: what Orca says on start; what flat review reads.

### 2. `echo hello`

Type `echo hello` and press Enter.

Expect: each typed character is echoed as you type. Record what Orca says
about the output line `hello` (spoken at once, after a pause, or not at
all) and whether the prompt that follows is read.

### 3. `ls -la`

Run `ls -la` in a directory with more files than fit on the screen.

Record whether and how the output is spoken: all of it, the last screen,
nothing, or repeated lines. This is the baseline for ADR 0021 §2: Orca's
terminal heuristics on an AccessKit `Terminal` node are untested, and the
fix for redraw noise (per-command nodes) is a later slice.

Also try, and record: line review (`KP_7`/`KP_9` or `CapsLock+U`/`O`)
across the screen, word review on a line with wide characters (`echo
"a 中文 b"`), and whether the caret position is announced after moving
with the arrow keys at the prompt.

### 5. A streaming live region

First the protocol example. Build it with
`cargo build -p ultty-client --example live-region`, then in Ultty run
`target/debug/examples/live-region 12 30` and do nothing. It shows one
`Log` region labelled "Reply", marked polite and busy while it streams,
and appends twelve lines five characters at a time, every 20 ms.

Expect: each line announced once as it completes, in order, with no line
re-read; at most four announcements a second, so lines that complete
faster are read together; no cut inside a word except where a line took
more than 250 ms to arrive (not the case here). Flat review then reads
the whole reply line by line. Before Orca 49, politeness is ignored:
note whether an announcement interrupts speech.

Record: what Orca says while it streams and after; whether "busy" is
spoken; whether any line is read twice.

Then the B4 done criterion: `cargo run -p chat`, type a message and press
Enter. The transcript is a polite `Log` labelled "Messages"; the bot's
reply streams word by word into a `MarkdownView::streaming(true)`, a busy
polite `Log` labelled "Bot reply" until it ends. Expect "Bot reply" once
when it appears, then the reply, each line once as it completes. Your own
message is not announced (it is a new region under the log, ADR 0021
slice 6). `scripts/e2e/accessibility-chat.sh` checks the same over AT-SPI
without a screen reader.

### 6. Tab through a native app

Build and run the counter: `cargo build -p counter-app`, then in Ultty
`target/debug/counter-app`. It talks the native protocol and covers the
pane with its own scene.

Expect: Orca says the focused button, "-1 button" (role and name). Press
Tab: "+1 button", then "Reset button", then back to "-1". Flat review
reads the scene, not the shell underneath: the title "Counter", the
count, the three buttons and the key hint. Press Enter on "+1" and review
the count again: it reads 1. Press `q` to quit: Orca is back on the
terminal and its prompt.

Record: what Orca says on start and on each Tab; whether the role is
spoken; what flat review reads (dividers currently read as "-" until
ulttui marks them hidden, slice 7); whether the count is re-read on its
own when it changes (it should not be: it is not a live region).

## Runs

| Date | Orca | Distribution | Step 1 | Step 2 | Step 3 | Step 5 | Step 6 | Notes |
|---|---|---|---|---|---|---|---|---|
| (none yet) | | | | | | | | |

## Automated stand-in

What can be checked without Orca, and was on 2026-11-08 (slice 4):

- `cargo test -p ultty --lib a11y`: the tree builder (rows as text runs
  with character lengths, positions and widths for ASCII, wide and
  combining cells; the caret; tabs and panes; stable ids) and the rebuild
  coalescing. One test feeds the tree to `accesskit_consumer`, the code
  the platform adapters read it with, and checks the document text and
  caret offset it reports.
- `scripts/e2e/accessibility-atspi.sh`: Ultty on Xvfb with a private
  session bus and AT-SPI bus, `org.a11y.Status.IsEnabled` set; a Python
  AT-SPI client (`gi.repository.Atspi`) finds the `terminal` role object,
  reads its text, sends `echo hello` with `ulttyctl send-text`, and checks
  that the text then contains `hello` and the caret offset moved. This is
  what a screen reader sees through AT-SPI; it says nothing about what Orca
  chooses to speak.

And on 2026-11-09 (slice 5):

- `cargo test -p ultty --lib a11y::scene`: native regions to nodes, the
  role table, derived roles, labels and text lines, value and position
  clamping, state and flags, hidden subtrees, relationships with dangling
  targets dropped, focus, bounds with scrolling, an inline surface after
  its anchor row, ids across sessions; each tree is fed to
  `accesskit_consumer`.
- `scripts/e2e/accessibility-atspi.sh` goes on to run `counter-app` in
  the shell and checks that AT-SPI lists `button` objects named -1, +1
  and Reset, that the focused object is the button `ulttyctl regions`
  reports as focused, and that `ulttyctl send-key tab` moves both. It
  also counts idle wakeups with the AT-SPI bus enabled: none at the
  prompt, and with the app only the 250 ms maintenance tick native
  sessions always have, with no tree rebuilt.

And on 2026-11-09 (slice 6):

- `cargo test -p ultty --lib a11y::live`: the queue with a scripted
  clock: a line break flushes complete lines at once, a partial line
  after 250 ms from its first byte, at most one flush per 250 ms per
  session with the rest accumulating, the 2 KiB cut at a line break, a
  blank or a character, eight announcements kept, a flood bounded.
  `a11y::scene` feeds consecutive trees to `accesskit_consumer` and
  checks that each flush adds exactly one live node, with the flushed
  text, and that kept ones are not added again.
- `scripts/e2e/accessibility-live.sh`: the `live-region` example streams
  twelve lines into a polite log; a Python AT-SPI listener records the
  `object:announcement` events. First run: the log's name "Reply" when it
  appeared, then ten announcements over 2.3 s carrying all twelve lines
  in order (two of them two lines each), at most four in any second, all
  polite. Then 20 wakeups in 5 s of idle with the app connected, none of
  them a tree rebuild.
