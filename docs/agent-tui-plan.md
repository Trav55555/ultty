# Plan: the agent- and TUI-friendly terminal

Status: proposed 2026-11-07. Needs sign-off on the items marked **gate**
before they start (protocol, schema or control-plane changes; see
[North Star](NORTH_STAR.md#constraints)).

## Position

Ultty is the terminal that AI agents and TUI developers can build on:

- **Agents can drive it and watch it.** A local control plane addresses every
  pane, reports what happened in it, and lets an agent or a test wait on
  events instead of scraping the screen.
- **TUIs can hand it their structure.** Apps send a layout tree; the terminal
  owns layout, per-region scrolling, streamed text, selection and, next,
  accessibility. Apps written with `ulttui` still run as full ratatui UIs in
  every other terminal.
- **It is fast enough that nobody has to forgive it.** Zero idle work,
  startup and memory at or under Alacritty's on the same machine, measured
  keystroke-to-pixel latency, and long agent sessions that stay flat.
- **It stays open and local.** No account, no telemetry, no network calls.

The market evidence behind this is in
[the market research report](../reports/Ultty%20terminal%20market%20research.md)
(untracked; move into `docs/research/` if it should be kept). In short:

- AI coding agents are now the flagship TUIs. Their biggest complaints
  (scroll jumping, copy fidelity, resize corruption, long-session memory,
  the alt-screen versus scrollback trade-off) are built into the ANSI model.
  Synchronized output fixes only flicker.
- Tern's Tern Surface Protocol (Stencil Labs, closed beta, October 2026) has
  the same core idea and ships with the omp agent. It is flex-only, falls
  back to plain text and has thin accessibility. Ultty's opening is the open,
  full-application protocol with a real fallback.
- Protocols spread through frameworks and graceful fallback, not through
  other terminals adopting them. tmux blocks in-band APC; control mode is the
  only way through it.
- Users pick terminals for speed and for working well with agent CLIs. Speed
  is table stakes, so it has its own track here, not a footnote.

## ulttyctl today

`ulttyctl` talks to a per-process, token-authenticated Unix control socket
([ADR 0017](adr/0017-runtime-socket-and-control-plane-semantics.md)). The
requests are in `crates/ultty-protocol/src/control.rs`:

| Group | Requests |
|---|---|
| Drive | `SendText`, `SendKey`, `SendRaw`, `RunCommand`, `NewTab`, `CloseTab`, `SwitchTab`, `Resize` |
| Read | `GetText` (with scrollback), `GetSelection`, `Search`, `ListTabs`, `GetInfo`, `Screenshot` |
| Synchronise | `WaitFor` (pattern with timeout), `Batch` |
| Agent | `SetAgentStatus` (cost, budget, pressure, model, step, agent name) |

`mcp/ultty-mcp-server.py` wraps ten of these as MCP tools by shelling out to
the CLI.

This already covers two real jobs: end-to-end tests for TUIs
(send keys, wait for text, read text or screenshot) and letting an agent run
commands in a terminal. Neither Tern nor Ghostty pitches its control surface
this way.

### Gaps

1. **Everything targets the focused tab.** No request takes a tab or pane id,
   so an agent cannot watch one pane while you type in another, and parallel
   tests collide. Panes in splits have no identity at all.
2. **No events.** Each connection carries one request and one reply. Callers
   poll, and a pending `wait-for` keeps the 250 ms maintenance tick running.
   Ultty already parses OSC 133 prompt marks, the bell and OSC 9/777
   notifications, but nothing outside the window can hear them.
3. **No command boundaries.** With OSC 133 the terminal knows where each
   command's output starts and ends and its exit code; `ulttyctl` can only
   return screen text.
4. **Native state is invisible from outside.** The region tree, computed
   layout and focus show only in the `Ctrl+Shift+I` overlay, so tests assert
   on cells or pixels, not structure.
5. **`SetAgentStatus` is one agent's schema.** Cost, budget, model and
   "context quality" fit one tool. A generic per-pane status fits any agent.
6. **The MCP server is a side script.** Python, shelling out, not packaged,
   not tested in CI.

## Tracks

Each item is one vertical slice with its own check. Items within a track run
in order; tracks run in parallel where people allow. "Gate" means it changes
a protocol, schema or public contract and needs sign-off first.

### A. Control plane v2 (agents drive and watch)

| # | Slice | Done when | Gate |
|---|---|---|---|
| A1 | Stable pane ids: every pane in every tab gets an id that survives splits, moves and tab switches. `ListPanes` returns id, tab, title, cwd, foreground process, size, focused. | Integration test opens splits, moves focus, and ids stay put; `ulttyctl panes --json` lists them. | gate (approved 2026-11-07; done) |
| A2 | Optional `target` on every pane-scoped request (`GetText`, `GetSelection`, `SendText`, `SendKey`, `SendRaw`, `RunCommand`, `Search`, `WaitFor`). Omitted means focused pane, as today. `Screenshot` stays window-wide; a per-pane crop is a later slice. | Existing control tests pass unchanged; new tests drive an unfocused pane while another has focus. | gate (approved 2026-11-07; done) |
| A3 | Event subscription: a long-lived connection (`subscribe`) that streams newline-delimited JSON events: OSC 133 prompt/command start/finish with exit code, bell, OSC 9/777 notification, title change, pane opened/closed, process exited, native session connected/dropped. Amends ADR 0017 (one request per connection). Native session connected/dropped events are deferred until sessions map to panes. | Test subscribes, runs a command in a pane with shell integration, and receives start and finish with the exit code; idle window with a subscriber makes zero wakeups. | gate (approved 2026-11-07; done) |
| A4 | `WaitFor` becomes event-driven on top of A3 (pattern, command-finished, bell, quiet-for-N-ms). Drops the maintenance tick while a wait is pending. | Idle wakeup bench shows zero switches/s with a pending `wait-for`. | — (done 2026-11-07) |
| A5 | Command output query: `last-command [--target]` returns the command line, output and exit code from OSC 133 marks. | Test with bash and zsh shell integration. | gate |
| A6 | Generic pane status replaces `SetAgentStatus`: `label`, `state` (idle/working/waiting/error), optional `progress`, optional `attention` flag, free-form `detail`. Shown in the tab bar; `attention` also marks the window urgent like `bell_urgent`. Old request kept one release as a deprecated alias. | Snapshot of tab bar per state; deprecation noted in CHANGELOG. | gate |
| A7 | MCP server as a supported binary (`ulttyctl mcp`, Rust, stdio), exposing A1-A6. Retire the Python script. | MCP conformance test in CI: list tools, run a command in a target pane, wait for finish, read output. | — |
| A8 | Guide: "Test your TUI with ulttyctl" and "Drive Ultty from an agent", with a worked example for a ratatui app and for Claude Code via MCP. | Examples run in CI headless where possible. | — |

### B. Native protocol for agents and TUIs

| # | Slice | Done when | Gate |
|---|---|---|---|
| B1 | Flagship agent console ([ADR 0008](adr/0008-agent-workshop-console-flagship.md)) with a measured before/after against the same app in ANSI mode: scroll position held while streaming, copy fidelity (no stray indentation or trailing spaces), resize without corruption, flat memory over a 10-hour soak. | A published perf note with the four numbers in both modes. | — |
| B2 | Read-only native inspection over `ulttyctl`: `regions [--target]` returns the region tree, computed layout, content kind and focus. Tests can assert "region `log` contains X". | Snapshot test of `regions --json` for an example app. | gate (control schema) |
| B3 | Inline surfaces anchored in scrollback, next to full-scene native mode, so an agent CLI can keep native scrollback and still get native regions. Matches the one thing TSP does better. | ADR plus an example where native blocks scroll away with shell output and survive copy. | gate (protocol + ADR) |
| B4 | Accessibility semantics in the protocol before 1.0: `role`, `label`, `live` (off/polite/assertive) per region; bridge to AT-SPI on Linux and NSAccessibility on macOS (AccessKit). | Orca reads a streaming region as a live region in the agent console; manual test script recorded. | gate (protocol + ADR) |

### C. Reach (break the chicken-and-egg)

| # | Slice | Done when | Gate |
|---|---|---|---|
| C1 | `ratatui-ultty` bridge crate: an existing ratatui app opts specific areas (transcript, log) into native regions, keeping ratatui for the rest. | A small ratatui app gets native scroll and copy in its log pane with a diff under ~50 lines. | — |
| C2 | Versioned public spec with a conformance test suite that a second implementation can run. | Suite runs against Ultty in CI; spec tagged. | gate (spec freeze) |
| C3 | TypeScript client (Ink and OpenTUI already build flex trees). | An Ink-style example renders natively in Ultty and falls back elsewhere. | — |
| C4 | Verify how Tern runs under tmux (`tmux list-clients -F '#{client_control_mode}'` during a session), then scope tmux control-mode client support as its own project. | A written scope with risks; no build yet. | — |
| C5 | Spike a TSP receiver to price being its second open implementation. | A go/no-go note with effort estimate. | — |

### D. Damn fast

Speed is measured on the comparison bench (`scripts/bench/compare.sh`) against
Alacritty and GNOME Terminal on the same hardware, on both the development
workstation and the T4 runner, and published per release. Targets are
relative to Alacritty because absolute numbers depend on the driver.

Where things stand ([2026-11-07 idle and footprint](perf/2026-11-07-idle-footprint.md)):

| Measure | Ultty | Alacritty | Status |
|---|---:|---:|---|
| Idle wakeups, focused, no blink | 0/s (workstation, after fixes) | 0/s | T4 re-run pending |
| RSS after feed | 130 MB workstation / 216 MB T4 (before fixes) | 115 MB T4 | behind; README target is < 50 MB |
| Exec to titled window | 123 ms workstation / 476 ms T4 (before fixes) | 197 ms T4 | behind on T4 |
| Keystroke to pixels | probe exists, no published run | — | unknown |
| VT throughput | 120-985 MiB/s parse, ~141 MiB/s parse+grid on ASCII | — | ahead of need; scrollback is the cost |

| # | Slice | Done when |
|---|---|---|
| D1 | Re-run `compare.sh` on the T4 with the idle, GL and startup fixes; publish the table. | Dated perf note with all three terminals. |
| D2 | Keystroke-to-pixel latency: `compare.sh` already runs the X11 probe (`ubench-k2p`, XTest injection plus GetImage polling) and a coarse Wayland one, but no results are published. Run it idle and under output flood on both machines for all three terminals. Replace the README's "not measured". | Perf note with median and p99 per case; README row updated. |
| D3 | Memory: shrink the glyph atlas to start at 1024x1024 and grow on demand (16 MiB of the 22-24 MiB GPU memory today); check the font database cost; set a CI heap budget per window. | RSS after feed at or under Alacritty's on the T4; `cargo test -p ultty --bench memory` covers the new budget. |
| D4 | Startup: find where the T4's extra ~350 ms goes with the `ultty::startup` stage log; overlap `FontSystem::new` with GPU bring-up. | Exec to titled window at or under Alacritty's on the T4. |
| D5 | Agent workload speed: append-only streaming markdown cost per token, and a 10-hour Claude-Code-shaped soak (streaming, resize, scrollback churn) with RSS and frame time sampled every minute. | Flat RSS (no upward trend after warm-up) and frame time p99 under 5 ms over the soak. |
| D6 | Control plane speed: `ulttyctl` round trip and event delivery latency (A3). | p99 under 1 ms for a local request; documented. |
| D8 | Title refresh without a full `/proc` scan: naming the foreground program (`process_name_for_group`, `pids_in_group` in `event_loop.rs`) reads `/proc/*/stat` for every process on the machine, and with the shell in the foreground it finds nothing and scans in full each time. Found 2026-11-07 tracing an idle window: thousands of syscalls a second for several seconds after output. Read `/proc/<pgrp>/stat` first and walk only the leader's children. | A trace of a title refresh with the shell in front shows a handful of `/proc` reads, not one per process. Done 2026-11-07: 7,805 opens down to 150 over five commands. |
| D7 | Guard it: every slice above adds or extends a bench in the instruction-count gate (`scripts/bench/icount-gate.sh`) or the memory budget test, so speed cannot regress silently. | CI fails on a deliberate regression in a test branch. |

### E. Baseline and trust (runs throughout)

| # | Slice | Done when |
|---|---|---|
| E1 | Public VT compatibility scorecard: mode 2026, kitty keyboard and graphics, OSC 8/52/133, mode 2031, OSC 9;4, grapheme width, against what agent CLIs need. Close gaps found. | Scorecard page generated from the conformance suite. |
| E2 | Fix the ssh-from-tmux false positive: a remote app probing from inside a local tmux pane renames that pane. Document multiplexer behaviour honestly. | Test reproduces the rename and passes after the fix. |
| E3 | Say "no account, no telemetry, no network calls" in the README and site; open GitHub Sponsors. | README section. |

## Order

1. **Now (no gate):** D1, D2, E1, E2, E3, C4. They need no sign-off, give the
   speed and compatibility baseline every later claim rests on, and answer the
   tmux question that could change track C.
2. **First gated batch:** A1, A2, A3, then A4-A8. The control plane is the
   cheapest way in for both audiences and works with plain ANSI apps, before
   anyone adopts the native protocol.
3. **In parallel once A3 lands:** D3, D4, D5, B1.
4. **Second gated batch:** B2, B3, B4 (needs ADRs), then C1, C2, C3.
5. **Decide:** C5 after C4 and B3, when Tern's trajectory is clearer.

## Not in this plan

Windows, desktop GUI embedding, canvas APIs and widget breadth beyond the
agent console stay deferred per [ADR 0014](adr/0014-next-phase-exclusions.md).
No AI features ship in the terminal itself; agents integrate through the
control plane and the protocol.

## Open questions

- Pane ids: opaque strings or numbers? Stable across a restart if session
  restore lands later?
- Event stream transport: same control socket with a `subscribe` upgrade, or
  a second socket? (A3's ADR amendment decides.)
- Should A6's `attention` flag also raise a desktop notification, or leave
  that to the existing OSC 9/777 path?
- Does the T4 or another hardware GPU become the reference machine for
  published speed numbers?
