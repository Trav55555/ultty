# Accessibility manual test

The script a person runs with a screen reader, from ADR 0021 §5. Each run
is recorded below with the date, the Orca version and the distribution.
Steps 1 to 3 cover the terminal grid (slice 4) and step 6 native scenes
(slice 5); the rest wait on live regions and ulttui semantics (slices 6
and 7) and are added with them.

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

| Date | Orca | Distribution | Step 1 | Step 2 | Step 3 | Step 6 | Notes |
|---|---|---|---|---|---|---|---|
| (none yet) | | | | | | | |

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
