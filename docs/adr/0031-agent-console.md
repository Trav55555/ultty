# ADR 0031: The Agent Console (B1)

## Status

Accepted 2026-11-21. Plan item B1 of `docs/agent-tui-plan.md`, which builds on
[ADR 0008](0008-agent-workshop-console-flagship.md). No protocol change
(gate: none).

## Context

ADR 0008 names an agent console as Ultty's flagship native app but says no
more. B1's acceptance asks for four numbers, measured for the same app in
native and ANSI mode:

1. scroll position held while output streams;
2. copy fidelity: no stray indentation or trailing spaces;
3. resize without corruption;
4. flat memory over a 10-hour session.

These are the agent-CLI complaints the plan's market research found. The ANSI
model causes them, and synchronized output does not fix them.

The pieces B1 needs are in place:

- **Inline surfaces** (ADR 0020, `ulttui::inline::Console`): cards anchored in
  scrollback that freeze into text when closed, with an ANSI fallback in the
  same binary.
- **Append-only streamed text and per-region scrolling** (B2, D5).
- **Accessibility roles** (ADR 0021).
- **Control plane:** `ulttyctl text`, `selection` and `regions`, plus events.
- **`examples/agent-transcript`:** a 141-line scripted demo of one tool card
  and one diff card.

Porygon's client was the nearest real agent console, but it moved to
porygon-ai (#717) and needs a private daemon. A flagship that anyone can run
and measure has to live here and run without an account or a network.

## Decision

### 1. The app: `examples/agent-console`

`examples/agent-console` is an agent console built on `ulttui`. It renders a
stream of agent events and makes no model calls itself. The transcript lives
in the main screen and its scrollback, in the order the session happened:

| Element | Native | ANSI |
|---|---|---|
| User prompt, assistant prose | Markdown streamed into an append-only region; frozen into scrollback when the turn ends | Printed line by line as it streams |
| Tool call | Inline card (ADR 0020) with live output, status and duration; frozen when done | Live region redrawn in place with cursor-up, then printed |
| Diff | Inline card with a styled hunk | Printed hunk |
| Approval request | Card with choices; keys answer it | Same choices as a redrawn prompt |
| Status line (model, tokens, state, elapsed) | A pinned region below the transcript | Redrawn bottom line |

Panes, a command palette and forms from ADR 0008's list come later. They
don't affect the four B1 numbers.

### 2. Input: a JSON Lines event stream

The app reads agent events, one JSON object per line, from a file or stdin:

- `prompt`, `text_delta`, `turn_end`;
- `tool_start`, `tool_output`, `tool_end`;
- `diff`, `approval`, `status`.

There are two sources:

- **Fixtures** (`examples/agent-console/fixtures/*.jsonl`), replayed with
  recorded timing or as fast as possible. They are deterministic, so they are
  what the benchmarks use.
- **Live adapters** (slice 4): small translators from agent CLIs' machine
  output to these events, one for Claude Code (`--output-format stream-json`)
  and one for Codex (`exec --json`). Each is pinned to a CLI version and tested
  against recorded output, because both schemas change between versions.

### 3. ANSI mode is a fair baseline

The "before" numbers only mean something if ANSI mode is a competent ANSI
agent UI, not a strawman. It does what current agent CLIs do:

- synchronized output (mode 2026) around each redraw;
- a live region redrawn in place with cursor-up while it streams;
- finished output printed into normal scrollback;
- no alternate screen.

It is the same binary with `ULTTY_IN_BAND=0`, or run outside Ultty, so only
the transport differs.

### 4. Measurement: `scripts/bench/b1-agent-console.sh`

The harness runs a fixture in each mode, in Ultty, under Xvfb and the private
headless sway the other benchmarks use, and drives the app with `ulttyctl`:

| Number | How |
|---|---|
| Scroll held | Scroll up 20 lines while the fixture streams, then sample the first visible line (`ulttyctl text`) every 100 ms. The result is the number of rows the view moved before the user scrolled back. Target: 0 native. |
| Copy fidelity | Select whole prose paragraphs, code blocks and a diff after the stream ends. Compare `ulttyctl selection` with the fixture's source text: count characters wrong, lines with trailing spaces, and indentation that isn't in the source. Target: 0 native. |
| Resize | Resize through 120, 80, 150 and 100 columns during and after streaming. Compare the final screen and scrollback with a fresh render at the final width; count rows that differ. Target: 0 native. |
| Memory | Extend D5's `agent_session_10h` memory-bench case to the console's event mix, plus a live soak on `soak.sh`'s infrastructure. Target: flat live heap and anonymous memory under 1 MiB/hour after warm-up, as D5. |

ANSI mode is measured the same way in Ultty. As reference points, it is also
measured in Alacritty and Ghostty for the numbers that can be measured from
outside: copy via the X selection, and resize via xdotool. The result is a perf note with
the four numbers in both modes, which is B1's acceptance.

### 5. Slices

1. **Events and fixtures.** The event schema, three fixtures (short session,
   long tool output, a 2-hour session), and a replay reader with tests.
2. **The app in both modes.** Rendering for every element in the table above,
   snapshot tests for the native regions and the ANSI output, and
   `agent-transcript` replaced or kept as the minimal demo.
3. **The harness and the perf note.** The four numbers, both modes; B1 done.
4. **Live adapters.** Claude Code's and Codex's output, translated to events,
   so the console can sit in front of a real session.

Each slice is one PR with its own tests. Slices 1 to 3 need no new protocol
and no control-plane change. If one of the measurements needs a new
`ulttyctl` query, that becomes a gated change and stops for sign-off.

## Slice 2 as built (2026-11-21)

Three things differ from the table in section 1:

- **Prose is printed, not streamed into a region.** Inline cards have a fixed
  height (ADR 0020), so there is no native region that can grow with a turn.
  In both modes, finished prose lines go into the normal transcript as styled
  Markdown source, unwrapped, so the terminal wraps and reflows them and a
  copy returns the source. Only the line being typed is live.
- **The status line is the bottom row of the live card.** The card follows
  the transcript and can include open blocks above the typing and status
  lines. Before printing finished text, the console closes the card and
  erases its rows.
- **Each running tool has up to eight rows in the live preview.** Both modes
  print completed blocks into scrollback with their full text, in completion
  order. The terminal can reflow their text at any width.

Inside an Ultty tab, `ULTTY_IN_BAND=0` alone does not force ANSI mode,
because the tab's socket is tried first. The ANSI measurements also unset
`ULTTY_SOCKET`, as `scripts/e2e/agent-console.sh` does.

## Consequences

- Ultty gets a flagship that anyone can run and measure.
- The four numbers become a regression suite for the transcript use case,
  like the D-track benchmarks.
- `examples/agent-transcript` stays as the minimal inline-card demo; the
  console does not replace it.
- `examples/agent-console` is larger than the other examples (an estimated 1.5
  to 2.5k lines). It stays an example, not a supported product, unless a later
  decision says otherwise.
- ANSI mode doubles as a reference for what `ulttui`'s fallback has to
  handle well.

## Decisions on the open questions (2026-11-21)

1. Live adapters for both Claude Code and Codex.
2. ANSI reference terminals: both Alacritty and Ghostty.
3. `agent-transcript` stays as the minimal demo.
