# Test your TUI with ulttyctl

`ulttyctl` lets a test script do what a person at the keyboard does: start
your program in a pane, press keys, wait for the screen to change, and check
what it shows. Each test gets its own pane, so tests can run side by side
without stealing each other's focus.

[`scripts/e2e/counter-app.sh`](../scripts/e2e/counter-app.sh) is a complete
example against `examples/counter-app`. Run it from a shell inside Ultty:

```sh
scripts/e2e/counter-app.sh        # prints "counter-app: ok", or FAIL and the screen
```

## The loop

```sh
# Start the program in its own tab; new-tab prints the new pane's id.
pane=$(ulttyctl new-tab env -u ULTTY_SOCKET ULTTY_IN_BAND=0 "$PWD/target/debug/my-app")

# Wait for it to draw, then act.
ulttyctl --pane "$pane" wait-for "Counter" --timeout 10000
ulttyctl --pane "$pane" send-text +
ulttyctl --pane "$pane" send-key enter
ulttyctl --pane "$pane" send-key c ctrl

# Wait for the screen to settle, then check it.
ulttyctl --pane "$pane" wait-for --quiet 300 --timeout 5000
ulttyctl --pane "$pane" text | grep -Eq '^│ *1 *│' || echo "wrong count"
```

- **Give `new-tab` an absolute path.** A new tab does not start in your
  current directory.
- **Wait for something, never `sleep`.** `wait-for <text>` returns as soon
  as the text is on screen or in scrollback; `wait-for --quiet <ms>` returns
  once the pane has been silent that long. Both fail (exit 1) at
  `--timeout`.
- **`text`** prints the pane's screen, one line per row; `text
  --scrollback` adds the scrollback. `search <pattern>` finds text with its
  position.
- **`screenshot <file.png>`** captures the window, for visual checks or for
  a failure report.
- **Exit status:** `ulttyctl` exits 1 when a request fails or a wait times
  out, so `set -e` scripts stop at the first problem.

## Native programs

A program built with `ulttui` detects Ultty and draws through its native
protocol. Native content is not in the terminal's text grid, so `text`
and `wait-for` cannot see it. `regions` can: it prints the program's
region tree, one region per line, with its content kind, accessibility
role (spec §10.5), position and size in cells, and its text. A widget with an id is named by it, and the
focused region is marked.

```sh
$ ulttyctl --pane "$pane" regions
session 1 (socket), focus #12
...
        #10 rich_text (text) 44,18 1x1 "1"
        #11 button (button) 26,22 10x1 id=dec "-1"
        #12 button (button) 38,22 9x1 id=inc "+1"
```

`--json` gives every region's full text, metadata and accessibility
`role`, `label` and `live`, for assertions:

```sh
regions() { ulttyctl --json --pane "$pane" regions; }
# the region whose widget id is "status" says "saved"
regions | jq -e '.scenes[0].regions[] | select(.metadata.id == "status")
                 | .text == "saved"'
# the "inc" button has focus
regions | jq -e '.scenes[0] as $s | $s.regions[]
                 | select(.id == $s.focused) | .metadata.id == "inc"'
```

An app's inline surfaces ([ADR 0020](adr/0020-inline-surfaces.md)) come
after its main tree, each as a `surface #id (rows, state at line N)` line
followed by its regions; in JSON they are `.scenes[0].surfaces[]`, each
with `id`, `rows`, `state` (`unplaced`, `placed` or `hidden`), once placed
`anchor_line` (the line of its first row, as `text --scrollback` numbers
them from 0), and its own `regions`, positioned from the surface's top
left. `scripts/e2e/inline-surface.sh` checks a card this way and in
screenshots. A live or closed card's rows hold its text, so `text
--scrollback` reads it in order with the output around it;
`scripts/e2e/agent-transcript.sh` checks a whole transcript that way, in
Ultty and under tmux ([inline cards](inline-cards.md)).

There is no `wait-for` on regions yet; poll `regions` in a loop, or wait
for something the program prints to the grid.

To assert on the screen text instead, run the program in ANSI mode:
remove the tab's native socket and turn off the in-band probe.

```sh
env -u ULTTY_SOCKET ULTTY_IN_BAND=0 my-app
```

Any other program (ratatui, Bubble Tea, Textual, ncurses) draws with escape
sequences and can be tested as it is.

## Programs with a shell

To test a command-line program rather than a full-screen one, run it in a
shell pane and use the shell integration (see
[Drive Ultty from an agent](agents.md#1-shell-integration)):

```sh
pane=$(ulttyctl new-tab)
ulttyctl --pane "$pane" wait-for --command-done --timeout 60000 &
ulttyctl --pane "$pane" run my-tool --check
wait $!                                     # my-tool's exit status
ulttyctl --pane "$pane" last-command        # its command line and output
```

## In CI or over ssh

[`scripts/e2e/run-headless.sh`](../scripts/e2e/run-headless.sh) builds
Ultty, starts it on a virtual X display, and runs a command as if from a
shell inside it:

```sh
scripts/e2e/run-headless.sh scripts/e2e/counter-app.sh
```

It needs `Xvfb` and a Vulkan or GL driver; Mesa's software drivers
(lavapipe, llvmpipe) work without a GPU. Linux only, for now.

The command runs with `ULTTY_PID` naming that Ultty and a private
`XDG_RUNTIME_DIR` holding only its control socket, so `ulttyctl` cannot
reach a window on your desktop. A script that starts its own Ultty should do
the same, and wait for `$XDG_RUNTIME_DIR/ultty-ctl-<pid>.sock` before its
first command; `ulttyctl --pid <pid>` makes each command talk to that
instance or fail, never another one.
