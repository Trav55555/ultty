# ADR 0025: Native Sessions Belong to Panes

## Status

Proposed 2026-11-11. Follows A1 (stable pane ids) in
`docs/agent-tui-plan.md` and ADR 0024, whose `lua open --split` falls back
to ANSI because of the gap this ADR closes.

## Context

A native session is bound to a **tab**: the terminal listens on one socket
per tab (`ULTTY_SOCKET`, spec §1.2), a session carries the `TabId` whose
socket accepted it, and the renderer draws a tab's sessions over the tab's
one terminal. The multi-pane frame (`render_multi_pane`, `encode_panes`)
draws grids only, so a native app in a split tab connects, lays out and
streams frames that nobody sees. Hence:

- `ulttyctl lua open --split` starts `ulttui-lua --ansi` and says so.
- `ulttyctl split <cmd>` with a ulttui app gives a blank pane.
- Accessibility exposes scenes only in a one-pane tab; screenshots of a
  split tab leave them out; A3's "native session connected/dropped" events
  have no pane to name.

What the terminal already knows, per pane, is enough to fix this without a
wire change:

- Every pane is a PTY with a foreground process group (`tcgetpgrp`), and a
  socket session knows its peer's pid (`SO_PEERCRED`), so its process group
  (`getpgid`). Keys are already routed on exactly this: a session gets
  keystrokes only while its peer is in the focused pane's foreground group
  (§1.2). Every pane's environment also carries `ULTTY_PANE`, but the
  terminal never sees a client's environment, and reading it would need a
  new `ClientHello` field.
- An in-band session arrives on one pane's PTY.
- `TerminalManager::layout_panes` already sizes each pane's grid from the
  split tree; sessions are the only thing still sized from the window.
- `encode_panes` is shared by the window and `Screenshot`, and
  `accessibility_snapshot` already walks the active tab's pane rectangles.

## Decision

### 1. A session binds to a pane

`NativeSession` gains `pane: Option<TerminalId>`. The protocol server does
not know panes, so the app binds after each `poll`:

- A **socket** session binds to the pane of its tab whose PTY has the peer's
  process group in the foreground. A tab with one pane binds the session to
  that pane whatever the group (today's behaviour: a background job's scene
  shows in a one-pane tab, keys withheld). A session in a split tab whose
  peer is in no pane's foreground (`app &`) stays unbound: not drawn, not
  announced, no input; binding is retried when it next sends a frame.
- An **in-band** session binds to the pane whose PTY carried its hello.
  Lifting §1.3's one-pane restriction is its own slice.

The binding is for the session's life. Splitting the tab, moving focus,
switching tabs and dragging dividers leave it alone. Closing the pane
disconnects its sessions (as closing the tab does today); closing the tab
is unchanged. A pane that loses its session (the app exits) shows its grid
again, as today.

### 2. The viewport is the pane

A bound session's viewport is its pane's grid (cols x rows), set when it
binds and after every `layout_panes` (window resize, split, close, divider
drag, tab bar height), each through the same `Resize` + `LayoutComputed`
the app gets today for a window resize. `TerminalManager` counts layout
generations; the app compares one integer per update and syncs viewports
only when it changed, so an idle window does no work. Unbound sessions keep
the window's grid, as today.

### 3. Drawing

`encode_panes` takes, per pane, the full-scene sessions bound to it and
draws them where it draws the pane's grid text: origin at the pane's top
left (under the tab bar), clipped to the pane's rectangle, before the pane's
border so borders stay on top. The clip is a GPU scissor rectangle per pane
(one vertex batch per pane with scenes), so widget content (inputs,
buttons, progress, images, markdown, code), which is built unclipped, stays
inside the pane as well as the clipped quads and glyphs. The one-pane frame
is unchanged in effect (its one pane is the window) and keeps its path for
now; it draws only the sessions bound to that pane or not bound yet, so a
session whose pane closed under it never paints over the pane that remains.

Per-frame cost: the native vertex cache (`cached_native_buffers`) is kept;
each pane with scenes is one batch (two small render passes, background and
text, plus its images), rebuilt only when a session is dirty, the atlas
changed, or a pane rectangle or surface moved (the cache key grows from
surfaces to pane rectangles plus surfaces). The scroll geometry cache
becomes per session, keyed by origin, clip bounds, cell size and viewport:
today two full-scene sessions in one tab share one `RegionId`-keyed map and
the second reuses the first's geometry, which splits would make common.

Inline surfaces (ADR 0020) in split panes stay off until pairing takes a
pane's terminal instead of a `single_pane` flag; a separate slice.

### 4. Input

Keys: unchanged in mechanism, now stated as a pane rule: recipients are
full-scene sessions bound to the focused pane whose peer is in its PTY's
foreground group. Mouse and scroll: the pane under the pointer is found
with `pane_at`, the position becomes that pane's cells, and only sessions
bound to it are hit-tested. While a button is held, moves and the release
go to the pane that took the press, with the position clamped into it, so
a drag that leaves the pane or the window still ends where it began.

### 5. Accessibility, screenshots, `regions`

- `accessibility_snapshot`: each `PaneSnapshot` carries the scenes of the
  sessions bound to its pane at the pane's bounds; the focused pane's
  scenes replace its grid as today. Live-region text is announced for a
  session bound to a pane of the shown tab (`scenes_shown` per session,
  not per tab).
- Screenshots come through `encode_panes`, so a split tab's native panes
  appear as the window shows them. A per-pane crop stays a later slice.
- `ulttyctl regions --pane N` lists the sessions bound to pane N plus the
  unbound sessions of N's tab (so a background job in a split is still
  inspectable). No field is added to the JSON; a `pane` field can follow
  additively if tests want it.

### 6. Events (A3)

`native_session_connected { pane, session }` and `native_session_dropped`
are unblocked: emitted when a session binds and when a bound session ends.
A later slice, additive to the event JSON.

### 7. Protocol

No wire change: no message, field or capability bit. Spec text: §1.2 gains
the pane binding and viewport rule; §1.3's one-pane restriction and §10.6's
surface rule stay until their slices. One version-history row records the
text change.

### 8. `lua open --split`

Once slices 1 and 2 land, `ulttyctl lua open --split` drops the forced
`--ansi` and its note, and `scripts/e2e/lua-open.sh` checks the split
pane's regions natively. ADR 0024 open question 1 is then resolved: native.

## Alternatives considered

- **Pane id in `ClientHello`** (from `ULTTY_PANE`). Exact, but a wire
  change that every client must adopt, and a client can name a pane it
  does not run in. The process group is what the kernel says and is what
  key routing already trusts.
- **Controlling tty of the peer** (`/proc/<pid>/stat`). Binds background
  jobs too, but Linux-only and needs the pane's slave device number; the
  group rule reuses existing code and matches the key rule, so what is
  drawn and what gets keys agree.
- **A socket per pane.** Clean binding, but a pane split from a shell
  inherits the shell's `ULTTY_SOCKET`, so the new pane's apps would connect
  to the old pane's socket anyway; the env route cannot work for splits.
- **Draw a split tab's sessions over the focused pane.** Simplest, wrong:
  an app in the left pane would jump when focus moves.

## Consequences

- A ulttui app draws natively in any pane; `ulttyctl split <cmd>` and
  `lua open --split` become first-class launchers.
- Resize semantics change for sessions in split tabs: they get their
  pane's size, not the window's. Sessions in one-pane tabs see no change.
- Two full-scene sessions in one tab render correctly (per-session scroll
  geometry), including the one-pane case where this was latent.
- `regions --pane` becomes pane-precise in split tabs.
- One integer compare per update; no new per-frame work when idle.

## Slices

Each slice: `cargo test --workspace`, clippy `-D warnings`, `fmt --check`,
plus its own check.

1. **Bind, size, draw, keys, resize.** `NativeSession::pane`,
   `ProtocolServer::bind_unbound_sessions`, per-pane viewport sync on
   layout generation, `encode_panes` drawing bound full scenes with
   per-session scroll geometry and a pane-aware cache key. Checks: unit
   tests (a session binds by foreground group and in a one-pane tab;
   a bound session's viewport follows its pane's grid and the app gets
   `Resize` + `LayoutComputed`; layout generation bumps on split and
   resize; sessions of a closed pane are dropped); e2e
   `scripts/e2e/native-split.sh`: a headless Ultty, `ulttyctl split right
   counter-app`, `regions --json --pane N` shows a scene whose root is the
   pane's size, `send-text +` counts to 3, `resize` changes the root's
   width, a screenshot is written. Spec §1.2, ADR 0024 cross-reference.
   *Done 2026-11-11.* As built: binding is retried on every poll while a
   session is unbound (a few syscalls per unbound session, rare); the
   pane-precise `regions` of slice 4 came with it.
2. **Mouse per pane.** `pane_at` → pane cells → bound sessions. Check:
   server unit test that a click in the right pane reaches only the right
   pane's session with pane-local coordinates. *Done 2026-11-11.*
3. **Accessibility per pane.** Scenes under their pane's node; live text
   per bound session. Check: a tree test with a scene in an unfocused pane
   (under that pane's node, focus stays on the focused pane's screen);
   `scripts/e2e/accessibility-*.sh` keep passing. *Done 2026-11-11.*
4. **`regions --pane` and screenshots.** Pane-precise `regions`; e2e
   asserts the shell pane lists no scene. Screenshot comes with slice 1.
   *Done with slice 1.*
5. **`lua open --split` native.** Drop `--ansi` and the note; lua-open.sh.
6. **Later, each its own slice:** in-band sessions in split panes; inline
   surfaces in split panes; A3 connected/dropped events; per-pane
   screenshot crop.

## Open questions

1. Whether an unbound session in a split tab should bind to the focused
   pane after a timeout rather than stay invisible. Staying invisible is
   honest; `regions` still shows it.
2. Whether `NativeScene` should carry `pane` in JSON. Cheap and additive;
   deferred until a test needs it.
