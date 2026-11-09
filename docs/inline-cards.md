# Inline cards

A command-line program that prints a transcript (an agent CLI, a build
tool, a test runner) can show live cards in it: a tool call streaming its
output, a diff, a progress panel. In Ultty a card is drawn natively in the
shell's scrollback, scrolls away with the text around it, and is left there
as text when it closes, so copying, searching and `ulttyctl text` read it in
order with the rest. In any other terminal, inside tmux, or when stdout is
not a terminal, the same card is printed as ANSI text. The design is
[ADR 0020](adr/0020-inline-surfaces.md); the protocol is spec §10.6.

## With ulttui

`ulttui::inline::Console` prints like a CLI and opens cards from ordinary
views:

```rust
use ulttui::inline::Console;
use ulttui::prelude::*;

let mut console = Console::connect(); // Ultty, or the ANSI fallback
console.println("Running tests...")?;
let card = console.open(3, &Text::new("cargo test"))?;
console.update(&card, &Text::new("cargo test\n  running 12 tests"))?;
console.close(card)?;
```

- `open(rows, &view)` starts a card `rows` lines high (1 to 128) on the next
  line, as wide as the terminal. Its height never changes.
- `update(&card, &view)` shows a new view. In Ultty only the changes are
  sent. In the fallback, the card is redrawn in place with cursor movement
  only while it is the last thing printed; once anything is printed after
  it, it stays as it is.
- `close(card)` ends it. In Ultty its rows keep what it last showed, as
  text.
- `poll_events()` returns what the terminal reported (placed, hidden, shown,
  evicted, closed).
- The console answers the terminal's keepalive from a thread of its own, so
  the app can block (on a model, say) for as long as it likes. If the
  session is lost anyway, the console carries on as the fallback for good:
  open cards stay as they were last drawn, and new ones are printed as
  text. A single reply that takes longer than 5 s is enough to lose it.
- Print through the console, not `println!`. In-band (over ssh) the
  terminal is in raw mode, and the console ends lines with `\r\n` and keeps
  them in order with the cards. Text that another thread prints to stdout
  while a card opens can land between its anchor and its fallback lines;
  the card then covers the wrong rows.
- In-band, the console reads the app's own tty for the terminal's replies,
  so keys typed while it is native are kept for `take_input()`: read them
  there, not from stdin. Over the socket the console never reads stdin.

**Redraws in the fallback** move the cursor up over the card, so they assume
the console printed everything on the screen since the card. Output that
bypasses it (stderr, `tracing`, a subprocess) does not make the card final,
and the next redraw lands on the wrong lines; send such output through
`print`. When stdout is not a terminal (a pipe, a log file), cards are
never redrawn: each is written once, as plain text in its last state, when
something is printed after it, when it closes, or when the console is
dropped, and lines end with `\n`. A card still waiting is lost if the app
ends with `std::process::exit`, which skips `Drop`; close the card, or drop
the console, first.

**Width.** A card takes the terminal's width when it opens. In Ultty a live
card is laid out again when the window is resized, and a closed one is text
that re-wraps like any other. In the fallback a card drawn at one width is
final once the width changes, since the terminal may have re-wrapped its
lines.

**What other terminals see.** The card's view is rendered with its ANSI
renderer into `rows` lines of the terminal's width. In the fallback those
lines are the card. In Ultty they are written as plain text under the
anchor and replaced by the card's own text as soon as it is drawn, so a
recording replayed elsewhere shows them.

## With ultty-client

Without the framework, `UlttyClient::open_surface(id, rows, fallback,
constraints, style, contents)` writes the anchor and exactly `rows` lines of
fallback text to stdout, then sends `CreateSurface` and `contents` (the
messages that build the surface's regions) in one frame. `close_surface(id)`
closes it. `poll_events` returns `SurfaceEvent`s.
[`inline-surface.rs`](../crates/ultty-client/examples/inline-surface.rs) is a
small example.

## Example and test

[`examples/agent-transcript`](../examples/agent-transcript/src/main.rs) is a
made-up agent session: prose, a tool card streaming `cargo` output, prose, a
diff card, prose. Run it in Ultty and in tmux to compare:

```sh
cargo run -p agent-transcript
```

[`scripts/e2e/agent-transcript.sh`](../scripts/e2e/agent-transcript.sh) runs
it in a headless Ultty with shell output printed under the live cards and
checks the screenshot and `ulttyctl text --scrollback`, then runs it under
tmux and checks the same text appears.
