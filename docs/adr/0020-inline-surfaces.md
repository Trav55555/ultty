# ADR 0020: Inline Surfaces Anchored In Scrollback

## Status

Accepted 2026-11-08. Plan item B3 of `docs/agent-tui-plan.md` (gate:
protocol change). Amends spec §1.3, §2.1, §3.2, §4 and §10.

## Context

A native session is full-scene: as soon as a tab has one, the renderer stops
drawing the PTY grid and draws the session's scene over the whole pane
(`render_with_native_sessions`; `ulttui` creates one opaque root region
under `ROOT` that covers the viewport). That is the right model for a TUI.
It is the wrong model for an agent CLI, whose output is a transcript: prose,
tool calls, diffs, more prose, scrolling away as the shell's does. Such an
app has to choose between the shell's scrollback (ANSI, cursor-up redraws,
copy and resize bugs) and native regions with no scrollback at all.

Tern's TSP has `flow` surfaces: anchored to one grid row, drawn over it while
live, left in scrollback after close, gone when the row leaves scrollback.
It is the one thing TSP does that Ultty cannot
(`research_notes/.../tern_competitor.md`).

Two facts about the code shape the design. First, over the socket the app's
stdout and its frames are two channels with no ordering between them, so
"anchor at the cursor row when the frame arrives" is a race. Second, the VT
parser classifies APC strings by their first byte (`U` is in-band protocol
data) and hands them to the I/O thread without the VT handler seeing them, so
the handler never knows where in the text a frame sat. Shell marks solved a
similar problem by living on rows ([ADR 0019](0019-command-marks-on-rows.md)).

## Decision

An app can open **inline surfaces**: native region trees anchored to rows of
the main screen, each `rows` high and the pane's width wide, that scroll with
the text, into scrollback, and freeze into plain styled text when closed.
Full-scene mode is unchanged and the two coexist in one session.

### 1. Protocol

**Capability.** `CAP_INLINE_SURFACES = 0x0100`. The terminal advertises it;
an app that did not negotiate it gets `Error` 400 for the messages below.

**Anchor, in the PTY stream.** The place of a surface in the text is set by
one APC string the app writes to its stdout, on both transports:

    ESC _ u <session_id> ; <surface_id> ; <rows> ESC \

Decimal ASCII, at most 64 bytes. `session_id` is the `u64` from
`SessionReady`; `surface_id` is the `RegionId` the app will use for the
surface root; `rows` is 1 to 128. The VT handler (`ultty-vt`) applies it
where it sits in the text, like a shell mark: it scrolls in `rows` rows as
`rows - 1` line feeds would, tags each of them with a per-buffer anchor tag
(§2), and puts the cursor back on the first. The app then prints exactly
`rows` lines of **fallback text**, each at most the pane width, as if the
anchor were not there. In a terminal that drops the APC string (tmux, any
non-Ultty terminal, a replayed recording) the result is the fallback text at
the same place, which is the point: one byte stream, enhanced where
understood. Apps emit the anchor at column 0. An anchor received on the
alternate screen tags nothing and the surface is refused (400).

**Messages, app to terminal.** Both are gated and both are frame-batched.

| Message | Code | Payload | Effect |
|---|---|---|---|
| `CreateSurface` | `0x0015` | `id: u32`, `rows: u16`, `constraints`, `style` | Creates region `id` as a second root of the scene, laid out at `cols × rows` cells. Children attach with ordinary `CreateRegion { parent: id }`. |
| `CloseSurface` | `0x0016` | `id: u32` | Freezes the surface (§2) and deletes its subtree. `DeleteRegion` on a surface root does the same. |

The surface is **placed** when its anchor and its `CreateSurface` have both
arrived; the orders may differ over the socket. An anchor pairs only with a
session in the tab whose PTY carried it, and a second anchor for a surface
already placed is ignored, so another program in the tab cannot move or
claim an app's surface. The two `rows` must agree,
otherwise the surface is closed with 400. A `CreateSurface` whose anchor has
not appeared after 2 s, or an anchor no session claims in 2 s, is dropped
(400 to the app in the first case; the fallback text stays in both). Limits:
16 live surfaces per session, 1024 anchor tags per buffer, region text bounds
as today (spec §8.2). `SetConstraints`, `SetStyle`, `SetContent`,
`AppendContent`, streams and metadata work on surface regions unchanged.
The surface root's `width`/`height` constraints are ignored: it is always
`cols × rows`. Height is fixed for the surface's life (growth: open
question 1).

**Messages, terminal to app.** `SurfaceEvent { id: u32, kind: u8 }`
(`0x0026`): `Placed = 0`, `Closed = 1` (app asked), `Evicted = 2` (anchor
row left scrollback or was erased), `Hidden = 3` / `Shown = 4` (scrolled off
or back onto the screen, so an app can stop streaming work nobody sees;
sent at most once per change, never per frame).

**Coexistence.** A session is full-scene while `ROOT` has children; the grid
and all surfaces are hidden under it, as under an alternate screen. With no
children under `ROOT` the grid is drawn and the surfaces over it. Key events
go to a native session only while it is full-scene; an inline-only app reads
keys from its tty like any CLI, and `Subscribe` with `EVENT_KEY` on a surface
region is refused (400). Mouse clicks inside a live surface hit-test as
today, with surface-relative coordinates; drags are the terminal's selection
and never reach the app. Session end (disconnect, idle timeout, app gone)
freezes every live surface where it stands.

### 2. Grid and scrollback

Rows carry an **anchor tag**: a `u16`, 0 for none. `CompactRow` has two bytes
of padding for it (measured: 80 bytes, 78 used); `Row` is exactly 56 bytes
and grows to 64, which is screen rows only. `history.rs` pins both sizes. Every row of a live surface carries
its tag; a frozen surface's rows carry none. The `Buffer` keeps a small table
`tag -> (session_id, surface_id, rows, first_line)` where `first_line` is a
monotonic line number (rows evicted so far plus absolute index) used as a
hint and verified against the row's tag on lookup; the row is the truth.

- **Scroll and history.** Tags move with their rows into `CompactRow` and
  out again, as marks do. Scroll regions, insert and delete line move rows
  within the grid; the table re-scans grid rows (a screenful) after those.
- **Eviction.** When the first tagged row of a live surface leaves
  scrollback the surface is frozen and the app gets `Evicted`.
- **Erase.** `ED 2`, `ED 3`, `RIS` and any erase of a row drop its tag with
  its text; a live surface whose first row lost its tag is frozen with
  `Evicted`. Clear means clear.
- **Alternate screen.** Surfaces live on the main screen. While the alternate
  screen shows, they are not drawn and not rasterised; they come back with
  the main screen. Anchors on the alternate screen are refused (§1).
- **Reflow.** `Rewrap` streams logical lines. A tagged row is one logical
  line that does not wrap: it is clipped or padded to the new width and keeps
  its tag, so a live surface stays `rows` rows and its scene is laid out
  again at the new width and re-rasterised. Frozen rows are plain text and
  re-wrap like any text (a wide card wraps like a wide table would).
- **Rasterisation and memory.** While a surface is live, the UI loop's
  pass after any change to it (a frame, a scroll or animation step, a new
  width) writes its scene into its rows as styled cells: text per
  region with the renderer's line breaking (`wrap_line` moves to a shared
  module so GPU and cells agree), `RichText` spans to cell colours and
  bold/italic/underline/dim, borders as box-drawing characters per
  `BorderStyle` (`Ascii` gives `+-|`), backgrounds as cell `bg`, `Image` as
  blank cells, widgets as their label or value. Rows outside the screen are
  re-encoded in place (`Buffer::replace_row`). Closing a surface (or its
  session ending) is the last rasterisation, if a change is still unwritten,
  plus dropping the scene and tags, so a frozen surface costs
  exactly what `rows` lines of text cost and there is nothing to bound.
  Live cost per changed frame is, for each region, at most its visible
  cells (`rows × cols`) plus its text up to the surface's last row, however
  large its layout; borders are drawn last, over content, since they take
  no room in the layout. A row is written only when its cells changed, and
  a write drops only a selection that covers the row, as streaming output
  does; a card ticking leaves selections elsewhere, scrolled back or not,
  alone.

### 3. Copy, `text`, `last-command`, search

Nothing changes: the rows hold the text the user sees. Selection
(`selected_text`), `ulttyctl text --scrollback` (`text_of_rows`),
`last-command` (ADR 0019 marks sit on the same rows) and `Search` read the
rasterised cells, live or frozen, and a card between two commands' output
copies in order with them. Box-drawing borders are copied too, as from any
terminal; an app that wants clean copy uses `Ascii` or no border. Images in a
surface are lost on copy and on freeze (kitty graphics is a separate gap).

### 4. Rendering

For each placed surface of the visible tab's sessions the renderer looks up
the first tagged row and maps it to a visual row through the same arithmetic
as `visible_row` (history length, scroll offset). The surface is drawn at
`(pane_x, pane_y + visual_row × cell_height)` with the subtree's computed
layout offset by that origin and clipped to the intersection of its own rect
and the pane's grid area, through the existing per-region clip rects.
A surface with no visible row builds no vertices. Scrolling the viewport
moves surfaces, so the cached native vertex buffers are rebuilt when any
anchor's visual row changed (`scroll_dirty`), not only when a scene did.
The grid text under a live surface is drawn and the surface's opaque root
background covers it, as the full-scene root does today. Surfaces follow
the in-band rule for panes: a tab with one pane. Lifting that waits for
sessions mapping to panes (A1 follow-up).

### 5. Transport

The anchor is in the PTY stream and the frames take either transport, so
inline surfaces work over the socket, in-band, and over ssh (anchor and
frames share the ssh stream, so they stay ordered). Under tmux the probe
already fails and the app never opens a session; if a session exists
elsewhere and the APC is dropped, the fallback text is what shows. A
`cat`ed recording reserves and prints its fallback rows, claims no session,
and loses its tags after 2 s. In-band apps keep raw mode without echo for the
session (spec §1.3); an agent CLI with a line editor already does.

### 6. `ulttui` API sketch

A transcript app does not run the Elm loop; the screen is the shell's. It
gets an inline console that prints like a CLI and opens surfaces:

```rust
let mut console = ulttui::inline::Console::connect()?; // Ultty or Fallback
console.println("Running tests...")?;                 // stdout, ANSI as usual
let card = console.open(8, &ToolCallCard::new("cargo test"))?;
// Emits the anchor, the view's 8 fallback lines (render_ansi into an
// 8 x cols buffer), then CreateSurface + a frame. Returns a handle.
console.update(&card, &ToolCallCard::new("cargo test").output(&log))?;
// Diffs the view against the retained tree (native_diff) and sends a frame.
console.close(card)?;                                  // CloseSurface
```

`Console` wraps `ultty-client` (`UlttyClient::connect`/`connect_in_band`,
which already run the probe and know `session_id`). Views are ordinary
`View`s, so one card type renders native and ANSI. In `Fallback` (not Ultty,
tmux, probe failed) `open` prints the view's ANSI rows and `update` redraws
them with cursor-up only while the card is the last thing the console
printed; after that it is final. Widths come from the terminal size; lines
are clipped to it so the fallback is exactly `rows` rows. The example for
B3 is a small agent transcript: prose, a tool-call card streaming `cargo`
output, prose, a diff card, while a loop prints shell output under it.

## Alternatives considered

- **Keep full-scene only.** Agent CLIs stay on ANSI with cursor-up redraws,
  which is the complaint list the plan opens with. Rejected.
- **Kitty-graphics-style placement.** Images placed at the cursor move with
  text and Unicode placeholders mark their cells. Surfaces take the anchoring
  and the "stream is self-describing" idea, but the cells hold fallback text
  rather than placeholder glyphs, so copy, search and other terminals get
  text, not holes.
- **TSP-style JSON flow surfaces.** Same anchoring; TSP's wire is JSON with
  74 widget kinds, and what a closed surface is in scrollback is not
  documented. We keep the binary codec and primitive regions (ADR 0018) and
  make the frozen form plain text, which also answers "survive copy".
- **Keep frozen surfaces as scenes.** Richer (re-layout on resize), but
  unbounded memory, a second reflow path, and copy would still need a text
  rendering. Rejected; rasterised text is the frozen form.
- **Anchor by cursor row at `CreateSurface` time, no APC.** Correct in-band,
  a race over the socket, and invisible to recordings. Rejected.

## Consequences

- An agent CLI keeps the shell's scrollback and gets native cards; the
  terminal pays `rows × cols` cells per frame per changed card and nothing
  for frozen ones.
- The fallback text is written by the app, so in Ultty it is visible only
  until the first frame; the app must still get it right for every other
  terminal and for recordings. `ulttui` generates it from the same view.
- The anchor is a new sequence in the VT layer; `ultty-vt` stays free of the
  protocol crate (it parses three decimal fields).
- Row grows by eight bytes on screen; scrollback rows do not grow.
- A card that streams while the user selects over it drops the selection.
- A surface's height cannot change after creation (see open questions).
- Surfaces inherit the single-pane limit of in-band sessions until sessions
  map to panes.

## Slices

1. **VT anchor.** `ultty-vt`: `ApcKind::Anchor` for `u`,
   `Action::InlineAnchor`, tags on `Row`/`CompactRow` with size assertions,
   the anchor table in `Buffer`, row reservation, tag handling on scroll,
   eviction, `ED 2/3`, `RIS`, IL/DL, alternate screen and reflow (tagged rows
   clip, never wrap). Unit tests for each, mutation-checked like ADR 0019.
2. **Codec and session.** `CAP_INLINE_SURFACES`, `CreateSurface`,
   `CloseSurface`, `SurfaceEvent` in `ultty-protocol` with proptest
   roundtrips and the spec drift test; second roots in `SceneTree` and
   per-root layout in `LayoutEngine`; `NativeSession` applies the messages
   and enforces limits; `ulttyctl regions` shows surfaces with anchor line,
   rows and placement. Snapshot test in `protocol/session.rs`.
3. **Pairing and drawing.** The UI thread matches a session's surfaces to
   the terminal's anchors; `SurfaceEvent` placement, hidden/shown and
   eviction; renderer draws at the anchor row with clipping and skips
   off-screen surfaces; vertex cache invalidation on viewport scroll.
   Headless test: open a surface, `seq 1 200`, screenshot before and after
   scrolling back shows the card where its text is.
4. **Rasterisation.** Scene to cells on changed frames and on close;
   `Buffer::replace_row`; `wrap_line` shared. Tests: `ulttyctl text
   --scrollback` returns the card's text in order with the shell output,
   identical before and after close, after eviction of older lines, and
   after a resize; selection over prose + card + prose.
5. **Client and framework.** `ultty-client` gains `open_surface`/`close_surface`
   (anchor plus message, one call); `ulttui::inline::Console` with the
   `Fallback` path; the agent transcript example; an e2e script under
   `scripts/e2e/` that runs it headless, prints shell output under it,
   and checks `text --scrollback`, then runs it under `tmux` and checks the
   same text appears. This is the plan's acceptance example.

   *Done 2026-11-09.* As built: `open_surface(id, rows, fallback,
   constraints, style, contents)` takes the surface's child messages so the
   surface and its regions arrive in one frame, and its fallback text is
   plain (control characters dropped, so it cannot carry a second anchor).
   `Console::connect()` returns a `Console` rather than a `Result`: every
   failure to reach Ultty is the `Fallback`, which is also chosen when
   stdout is not a terminal. A fallback card is final once the width
   changes or when it is as tall as the screen, as well as after other
   output. `examples/agent-transcript` and `scripts/e2e/agent-transcript.sh`
   pass in a headless Ultty and under tmux.
6. **Later, each its own slice:** growth of the bottom-most live surface,
   surfaces in split panes, mouse actions on surface regions.

## Open questions

1. **Growth.** A tool-call card does not know its height up front. A safe
   rule exists (re-send the anchor with more `rows` while the cursor is on
   the row right after the surface and nothing was printed after it; the
   terminal reserves the extra rows) but it adds a second code path in the VT
   handler. Start fixed-height; decide after the example shows how often
   cards guess wrong.
2. **Rasterising every changed frame versus on close only.** Every frame
   keeps copy and search right while streaming at `rows × cols` writes; on
   close only would be cheaper and make selection over a streaming card
   stable, at the cost of copying stale fallback text while live. The ADR
   picks every frame; measure in slice 4.

   *Resolved in slice 4: every changed frame, no cadence.* Rasterisation
   runs in the UI loop's pairing pass, so frames an app sends faster than
   the loop turns are already coalesced into one write, and a pass with no
   change writes nothing (a per-surface changed flag, set by the messages,
   scrolls, animations and resizes that touch its tree). Measured with
   `cargo bench -p ultty --bench layout -- surface_raster` (release, LTO;
   a bordered 8 × 120 card, title plus five streamed lines of output, a new
   line each frame): rasterising 9.5 µs, rasterising and writing the eight
   rows on the screen 11.0 µs, in scrollback (decoding each row to skip an
   unchanged one, re-encoding the rest) 14.9 µs. Under 0.1 % of a 16 ms frame, so a ≤ 100 ms cadence would buy
   nothing and would make copy lag what is drawn. Closing, or the session
   ending, writes the last frame if a pass has not, so the frozen text is
   never stale; an eviction does not (its rows are gone or erased).
3. **Multi-pane.** Whether surfaces should wait for pane-mapped sessions or
   the single-pane rule is acceptable for the agent console flagship.
