# ADR 0021: Accessibility Semantics in the Protocol and an AccessKit Bridge

## Status

Accepted 2026-11-08. Plan item B4 of `docs/agent-tui-plan.md`; gated on
protocol and ADR sign-off. Written alongside ADR 0020 (inline surfaces in
scrollback), which takes app code `0x0015`/`0x0016` and capability bit
`0x0100`; this ADR uses the next free ones. §6 covers where the two meet.

## Context

Ultty draws with the GPU and exposes nothing to assistive technology: a
screen reader sees an empty window. GPU terminals are criticised for exactly
this, and grid-redrawing TUI frameworks for being unreadable even where the
terminal exposes text (market research, `structured_protocol_history.md`,
`ai_agent_demand.md` §5). Agent CLIs answer with a linear "screen reader
mode" that abandons their UI. Tern carries an `aria` label and a few roles
and documents no screen-reader behaviour.

Ultty already holds what a screen reader needs: a retained region tree with
typed content, a widget id per keyed region (`SetRegionMetadata` key `id`),
keyboard focus (`Focus`/`Blur`), and append-only streaming (`AppendContent`,
`StreamStart`/`StreamEnd`). What is missing is a way for an app to say what
a region *is*, and a bridge from that tree to the platform.

Facts this decision rests on (checked 2026-11-08 against the crate sources
in the cargo registry and the Orca source on GNOME GitLab):

- `accesskit_winit` 0.34.2 needs `winit ^0.30.5` (workspace: 0.30.13) and
  pulls `accesskit` 0.25.2, `accesskit_unix` 0.24.1 (AT-SPI) on Linux and
  `accesskit_macos` 0.27.2 on macOS. MSRV 1.87. The adapter must exist
  before the window is first shown, or it panics; Ultty's window is created
  visible today.
- On Linux the adapter connects to the session D-Bus and watches
  `org.a11y.Status.IsEnabled`; only when that is true does it join the
  AT-SPI bus and ask for a tree. `update_if_active` is a no-op until then.
- Both platform adapters announce a node whose `live` is not `Off` **when
  the node is added** (AT-SPI `object:announcement`, macOS
  `NSAccessibilityAnnouncementRequested`); relabelling a node does not.
  (Corrected in slice 6: both do announce a live node whose name
  changes, `accesskit_atspi_common` 0.21.1 `notify_property_changes` and
  `accesskit_macos` 0.27.2 `node_updated`, which also announces a node
  that turns live or comes back into the filtered tree, and liveness is
  inherited by descendants in `accesskit_consumer`. Announcing by adding nodes still
  holds; what changed is that nothing else under a live region may be
  named by the streamed text.)
  Text-change events come from diffing a text node's content between
  updates; a text node has role `Terminal`, `Document`, `Label` or a text
  input, and `TextRun` children with `character_lengths`.
- Orca's default script speaks `object:announcement` (`_on_announcement`);
  politeness is read since 49.beta (August 2025). Ubuntu 24.04 ships Orca
  46.1 and Debian 13 ships 48.1; whether they have the handler is
  **unverified**. Orca speaks text inserted into a focused `TERMINAL` object
  only when it can attribute a cause; unsolicited output goes through
  terminal heuristics we did not run. AccessKit sets no `container-live`
  attribute, so that Orca path is unavailable.
- Ultty runs no tokio runtime on its main thread (`tokio` is declared in
  `crates/ultty/Cargo.toml` and unused), so the adapter keeps its default
  `async-io` executor thread.

## Decision

### 1. Protocol: one `SetSemantics` message, one capability

Semantics travel in a new app-to-terminal message, not in metadata keys and
not as fields on `CreateRegion`:

- Metadata is opaque by contract (spec §10.4) and `SetRegionMetadata`
  replaces the whole list, so tools' keys and semantics would collide.
- `CreateRegion` is a fixed struct; semantics change at run time (busy,
  checked, value, label), so a setter is needed anyway. One full-replacement
  message matches `SetConstraints` and `SetStyle`.

```text
SetSemantics (0x0026), gated by CAP_SEMANTICS = 0x0200
  id: u32
  role: u8                       (table below; 0 = Auto)
  label: Option<string>          accessible name
  description: Option<string>    longer help text
  live: u8                       Off=0, Polite=1, Assertive=2
  flags: u8                      busy=1, hidden=2, read_only=4, disabled=8,
                                 atomic=16, modal=32, selected=64
  toggled: u8                    Unset=0, False=1, True=2, Mixed=3
  expanded: u8                   Unset=0, Collapsed=1, Expanded=2
  value: Option<(f32 now, f32 min, f32 max)>
  value_text: Option<string>     spoken instead of the number when present
  position: Option<(u32 index, u32 size)>   position in set (lists, tabs)
  level: u8                      heading or tree level, 0 = none
  labelled_by: Option<u32>       region id
  described_by: Option<u32>      region id
  controls: Option<u32>          region id
```

Unknown `role`, `live`, `toggled` or `expanded` values and NaN floats are
decode errors (`Error` 400), like other enums. `SetSemantics` for an unknown
region is `Error` 404; without `CAP_SEMANTICS` it is `Error` 400 "missing
capability", like `SetRegionMetadata`. `DeleteRegion` drops the semantics.
Relationship targets that do not exist are ignored at bridge time, not
refused, since apps create regions in any order inside a frame.

Role vocabulary, closed, each with the AccessKit role it maps to and the
`ulttui` widget that needs it:

| Role | AccessKit | Why it is in the set |
|---|---|---|
| Auto (0) | derived, see below | apps that send nothing |
| Group | `Group` | `Column`, `Row`, `Stack`, `Card`, `Section` |
| Text | `Label` (one line) or `Paragraph` | `Text`, `RichText`, `Body` |
| Heading | `Heading` + `level` | `Title`, `Section` headers |
| Log | `Log` | append-only transcripts: `MarkdownView` streaming, tool output |
| Status | `Status` | `Spinner`, `LoadingState`, `Badge`, status lines |
| Alert | `Alert` | `ErrorState`, `Modal` error text |
| Dialog | `Dialog` + `modal` | `Modal` |
| Input | `TextInput`, `MultilineTextInput`, `PasswordInput` by `Input` flags | `TextInput` |
| Button | `Button` | `Button` |
| Checkbox | `CheckBox` + `toggled` | `Checkbox` |
| Switch | `Switch` + `toggled` | `Toggle` |
| Progress | `ProgressIndicator` + `value` | `ProgressBar` |
| List | `List` | `List` |
| ListItem | `ListItem` + `position` | their rows |
| TabList | `TabList` | `Tabs` |
| Tab | `Tab` + `position` | each tab label |
| Tree | `Tree` | `Tree` |
| TreeItem | `TreeItem` + `expanded`, `level` | `TreeNode`, `Collapsible` |
| Image | `Image` | `Image` (label is the alt text) |
| Link | `Link` | OSC 8 style links in rich text, later |
| Table (21) | `Table` + row and column counts | `Table` |
| Row | `Row` + row index, `selected` | `Table` rows, the header row first |
| Cell | `Cell` + column index | `Table` cells |
| ColumnHeader | `ColumnHeader` + column index | `Table` header cells |
| ComboBox | `ComboBox` + `value`, `expanded` | `Select` |
| Option (26) | `ListBoxOption` + `position`, `selected` | `Select` options while expanded |

Not in the set: `Terminal` (the grid's), menus, sliders, a listbox apart
from a combo box. Adding a role is additive under ADR 0011; removing one
is not. The table and combo box roles were added after slice 7, before
semantics shipped in a release, so they need no capability (see open
questions).

**Defaults when an app sends nothing** (`role = Auto`, or no `SetSemantics`
at all), derived from content: `None` → Group; `Text`/`RichText`/`Markdown`/
`Code` → Text, or Log once the region has received `AppendContent`; `Input`
→ Input with `value` = its text and `placeholder`; `Button` → Button with
its label; `Progress` → Progress with its value, or `busy` when
indeterminate; `Spinner` → Status with `busy`; `Image` → Image, unlabelled.
`live` defaults to `Off`: the terminal never infers liveness, because a
wrongly live region floods speech. Regions with no label take their text as
name when it is one line, else none. A region with `hidden` and its subtree
are left out of the tree (decorative dividers, spacers).

The spec gains a §10.5 "Semantics" and the drift test gains the new code
and bit. `ulttyctl regions --json` reports `role`, `label` and `live` per
region, so tests can assert semantics without a screen reader.

### 2. The plain terminal grid

The focused pane's grid is one node with role `Terminal`, labelled with the
pane title, whose children are one `TextRun` per visible screen row with
`character_lengths`, `character_positions` and `character_widths` from the
cells (wide cells span two). The caret is a `TextSelection` at the cursor.
Rows are re-sent only when the grid is dirty; AccessKit diffs the document
text and emits the insert and delete events itself.

A screen reader can then review the screen by line and word, follow the
cursor, and apply its own terminal heuristics, as it does for VTE. Not
promised: that unsolicited output is spoken the way a user wants. Orca's
rules are its own and untested with AccessKit, and alternate-screen TUIs
redraw the whole grid, which no throttle makes pleasant. The manual script
(§5) records what Orca actually does.

Deferred, explicitly: scrollback as part of the document; per-command
nodes from the OSC 133 marks on rows (ADR 0019), which would give
"jump to previous command" and is the real fix for redraw noise; the
selection; braille table tuning; unfocused panes' text. Unfocused panes
appear as labelled `Group` nodes with no children, and the tab bar as a
`TabList` of `Tab` nodes with `selected`, so the structure is navigable.

### 3. The bridge

A new module `crates/ultty/src/a11y/` with two parts:

- `tree.rs`: a pure function from `(tabs, focused pane title, grid
  snapshot, native scenes of the focused pane, pending announcements)` to
  an `accesskit::TreeUpdate`. No platform types beyond `accesskit`. This is
  the unit under test.
- `adapter.rs`: owns the `accesskit_winit::Adapter`, created in `App::new`
  after the window is created invisible and before it is shown. It hands
  AccessKit events to the event loop through `EventLoopProxy<AppEvent>`
  (`AppEvent::Accessibility(accesskit_winit::Event)`), calls
  `adapter.process_event` first in `handle_window_event`, and runs
  `update_if_active` from the existing update pass.

Node ids: tab `t<id>`, pane `p<id>`, grid row `p<id>r<n>`, region
`s<session>:<region>`, announcement `a<counter>`; hashed into `NodeId(u64)`.

**When it updates.** Only while AccessKit is active (between
`InitialTreeRequested` and `AccessibilityDeactivated`) and only when
something shown changed: grid dirty, native scene dirty, focus moved, tabs
changed. Rebuilds coalesce to one per 50 ms through `app/pacing.rs`. Every
update is a full tree of a few hundred nodes; incremental updates are a
later optimisation, measured first.

**Focus.** ulttui's `Focus { id }` becomes `TreeUpdate.focus` on that
region's node; `Blur` puts focus on the app's root region; with no native
session, focus is the `Terminal` node. Native sessions of other tabs are not
in the tree at all (they are not drawn either).

**Live regions and streaming.** A region whose `live` is not `Off` keeps a
small queue of unannounced text. `AppendContent` to it adds to the queue.
The bridge flushes the queue as one child node (role `Label`, `live`
inherited, label = the text) when a line break arrives or 250 ms have passed
since the first unflushed byte, whichever is first. Adding the node is what
makes the platform announce it. At most four flushes per second per
session; above that, text keeps accumulating into the next flush. A flush
larger than 2 KiB is cut at the last line break inside the limit and the
rest waits. Only the last eight announcement nodes are kept under a region,
so the tree does not grow with the transcript. The region's own `TextRun`
rows carry the full text for review. `busy` is set while `StreamStart` is
open on the region and cleared at `StreamEnd`, so a reader can say
"streaming" and a user can wait for it. `Assertive` is passed through; the
platform decides whether it interrupts.

**Actions.** `Action::Focus` on a region sends the app a `FocusEvent`;
on a pane it focuses the pane. `Action::Click` on a `Button` has no protocol
counterpart (there is no activate input event) and is deferred to a
follow-up protocol change. `Action::SetValue` and scroll actions are
deferred.

**Cost without assistive technology.** One D-Bus session connection at
startup, one executor thread asleep on the bus, a match per window event in
`process_event`. No tree is built, no text copied. Headless, the adapter is
a no-op. `accessibility = false` in `[behavior]` skips it entirely. Slice 3
measures startup (D4) and idle wakeups (D1) before and after; the D-Bus
connection leaves the startup path if it costs over a millisecond.

### 4. ulttui

`View` gains `fn semantics(&self) -> Semantics` (default: `Auto`, no
label). Each built-in widget overrides it with the role from the table and
its state: `Checkbox` reports `toggled`, `ProgressBar` reports `value`,
`Tree` nodes report `expanded` and `level`, `Tabs` report `position`.
Widgets that already have a label use it as the accessible name. Builders
gain `.label()`, `.description()` and `.live()`; a `Semantic` wrapper view
sets role, label and liveness on any child for containers and custom views.
`MarkdownView::streaming(true)` sets `Log`, `Polite` and `busy` by default,
which is what gives the agent console a live transcript with no app change.

The native backend sends `SetSemantics` after `CreateRegion` when the
widget's semantics differ from the content-derived default, and again when
they change, diffed in `RetainedTree` like content and constraints. Nothing
is sent for the default case, so plain apps cost no extra bytes.

ANSI mode: nothing to do. ratatui draws cells and the host terminal's
accessibility applies. A linear screen-reader renderer fed by the same
labels is a possible later use, outside this ADR. The done bar (ADR 0013)
gains one line: a widget declares its role and label.

### 5. Testing

Automated:

- Codec: proptest round trip for `SetSemantics`, rejection of unknown enum
  values, the drift test for code and bit.
- Session: capability gate, 404 on unknown region, semantics dropped with
  the region, `regions --json` snapshot with role, label, live.
- Tree builder (`a11y/tree.rs`), pure, no display: grid rows become
  `TextRun`s with correct lengths for ASCII, wide and combining cells; caret
  follows the cursor; derived roles per content kind; explicit roles
  override; focus for `Focus`, `Blur` and no session; flushing at line break
  and at 250 ms; the four-per-second cap; the 2 KiB cut; pruning to eight;
  `busy` across `StreamStart`/`StreamEnd`; hidden subtrees omitted.
- AT-SPI smoke test if it runs headless: `dbus-run-session` with
  `at-spi-bus-launcher`, `gsettings set org.gnome.desktop.interface
  toolkit-accessibility true`, Ultty under `scripts/e2e/run-headless.sh`, a
  Python `atspi` client asserting a `TERMINAL` object under the Ultty
  application and an `object:announcement` while the chat example streams.
  If it cannot be made reliable in CI within its slice, it becomes a local
  script and the manual test is the gate.

Manual, in `docs/accessibility-test.md`, recorded with Orca version and
distribution each run:

1. Ubuntu 24.04 or Debian 13, GNOME, Orca on (`Super+Alt+S`). Start Ultty.
   Expect: Orca names the window and the focused pane's title; flat review
   (`Insert+Up`) reads the current screen line.
2. `echo hello`, Enter. Expect typed characters echoed; record what Orca
   says about the output.
3. `ls -la`. Record whether and how the output is spoken: the baseline for §2.
4. `cargo run -p chat`, focus the input. Expect "text input" and its label.
   Send a message.
5. While the reply streams, do nothing. Expect each completed line announced
   once as it arrives, no re-reading, no mid-word cuts beyond one per flush.
   Before Orca 49, politeness is ignored; note it.
6. Tab through the app. Expect each focused widget named with role and
   state (button, checkbox, tab).
7. Second tab, split pane. Expect the tab list with the selected tab; the
   unfocused pane listed by title.
8. Orca off and on while streaming. Expect the tree back and announcements
   resumed; no crash, no stuck speech.
9. `accessibility = false`. Expect an inaccessible window and nothing else
   changed.

### 6. Inline surfaces (ADR 0020)

Where accessibility meets inline surfaces, for 0020 to settle or hand back:

- A surface anchored to a scrollback row must appear *in reading order*
  inside the pane, not as a sibling scene. The grid's text node holds only
  `TextRun`s, so the plan is a `Group` child of the pane after the anchor
  row's `TextRun`, `described_by` that row. ADR 0020 provides both inputs:
  a surface is `(session_id, surface_id)` and its rows carry an anchor tag
  the tree builder can find while they are on screen. As built (slice
  5): the surface's root region is that child, under the pane's
  `Terminal` node, after the last run of its anchor row and
  `described_by` the row's first run; its role comes from its semantics
  (`Group` by default). A surface whose anchor row is above the screen
  comes first, described by nothing. Text runs inside it are not used
  (slice 5), so the `Terminal`'s document stays the grid. While the pane
  is scrolled back its surfaces are left out of the tree: the grid's
  rows are the live screen (§2), so an anchor row in view would be
  matched to the wrong row.
- Surfaces take `SetSemantics` and defaults like any region. One that
  scrolls off the visible screen leaves the tree with its rows until
  scrollback is exposed (§2, deferred). Live queues are per region, so a
  streaming surface behaves as in a full-scene app.

## Alternatives considered

- **Reserved metadata keys** (`a11y.role`): stringly typed, unvalidated,
  and `SetRegionMetadata` is a full replacement ulttui already uses for
  `id`. Rejected.
- **Fields on `CreateRegion`/`SetContent`**: semantics change independently
  of layout and content, and every create would grow for apps that never
  set them. Rejected.
- **Infer everything, no protocol change**: content gives roles, not
  labels, liveness or state. Kept as the default layer only.
- **Announce by relabelling one node**: neither platform adapter announces
  on a label change, only on node addition. Hence announcement children.
- **AT-SPI `container-live` attributes**: Orca would honour them, but
  AccessKit does not set them and macOS has no equivalent.
- **Talk to AT-SPI directly, skip AccessKit**: full control over text
  events on one platform, every macOS line written twice. Rejected while
  AccessKit covers both.
- **A linear screen-reader mode in ulttui**: what Gemini CLI and Claude
  Code do; abandons the UI and does nothing for the grid. Possibly later,
  in addition.

## Consequences

- The protocol version stays 1; `CAP_SEMANTICS` is additive. Apps on old
  terminals get `Error` 400 and keep working without semantics.
- Ultty gains D-Bus and `async-io` dependencies on Linux, AppKit
  accessibility on macOS, and one idle thread. If the idle wakeup bench
  (D1) shows it waking, the adapter becomes opt-in until fixed upstream.
- The window is created invisible and shown once the adapter exists: a
  startup-order change to watch in the `ultty::startup` stage log.
- Orca behaviour before 49 and on unsolicited terminal output is recorded,
  not promised. The plan's done criterion ("Orca reads a streaming region
  as a live region in the agent console") is met on Orca 49 or later;
  older Orcas get the `Terminal` text model and whatever their heuristics
  speak.
- Platform scope: Linux (AT-SPI, both X11 and Wayland, since it is D-Bus)
  first; macOS second, hand-tested with VoiceOver; Windows stays excluded
  (ADR 0014).
- Widgets owe one more thing to be done (ADR 0013).

## Implementation slices

Each slice is independently testable and mergeable.

1. **Protocol**: `Semantics` types, `SetSemantics` codec, `CAP_SEMANTICS`,
   spec §10.5, drift test, proptest. Check: `cargo test -p ultty-protocol`.
2. **Session**: semantics on `scene::Region`, gate, 404, derived defaults as
   a pure function, `regions --json` fields. Check: session tests, snapshot.
3. **Adapter, no tree**: dependency, invisible-then-shown window,
   `AppEvent::Accessibility`, `process_event`, config key, a lone `Window`
   node. Check: starts with and without a session bus; startup and
   idle-wakeup numbers before and after. Done 2026-11-08: no startup cost,
   but zbus's `blocking` pool thread wakes twice a second, so the adapter
   was made opt-in per Consequences;
   `docs/perf/2026-11-08-accessibility-adapter.md`. Since then
   `Cargo.lock` pins `blocking` 1.6.2, whose idle threads exit, and the
   adapter is on by default (`accessibility = true`);
   `scripts/e2e/accessibility-no-bus.sh` fails if idle wakeups return.
4. **Grid as Terminal**: tabs, panes, focused grid rows and caret, coalesced
   rebuilds. Check: builder tests; manual steps 1 to 3 recorded. Done
   2026-11-08 except the Orca run: the steps are written in
   `docs/accessibility-test.md`, not yet run with Orca;
   `scripts/e2e/accessibility-atspi.sh` checks the same text and caret
   over AT-SPI without a screen reader. The panes sit in a `TabPanel`
   that the selected `Tab` `controls`, not under the `Tab` node (the ARIA
   tab pattern). The rows are the live screen even while the view is
   scrolled back (scrollback is deferred, §2).
5. **Native scenes**: regions to nodes, derived and explicit roles, focus.
   Check: builder tests; manual step 6. Done 2026-11-09 except the Orca
   run (`a11y/scene.rs`); `scripts/e2e/accessibility-atspi.sh` runs
   `counter-app` and finds its buttons as AT-SPI `button`s, the focused
   one focused, and Tab moving focus. Mappings chosen where this ADR was
   open:
   - Native scenes are exposed only when the active tab has one pane,
     the only time the renderer draws them. While a session is
     full-scene the grid is hidden under it, so the focused pane node is
     a `Group` (the pane title), not a `Terminal`, and its children are
     the `ROOT` nodes of every full-scene session, by session id. Focus
     is the focused region of the last of them that has one, else the
     last one's root (§3, `Blur`).
   - A region's text is not `TextRun`s: AccessKit gives text ranges only
     to `Label`, `Document`, `Terminal` and text inputs, and its run
     iterator descends through other nodes, so runs in a surface would
     join the `Terminal`'s document. One-line text is a `Label` (AccessKit
     names a label by its value); text the name does not already say is
     one `Label` child per non-empty line (`s<session>:<region>l<line>`),
     the last 200 lines, under the region's node (`Paragraph` for `Text`,
     also when the app set a `value_text`, which a `Label` cannot carry
     beside its name). Only those lines are copied out of the scene. An
     input's text is its `value`; its caret is not exposed yet.
   - Apps are not trusted with the tree's shape. A pane's native scenes
     add at most 5000 nodes, regions and lines together, ending with a
     "… truncated" label when cut. Node ids are unkeyed FNV-1a hashes of
     names the app partly chooses, so a collision can be found offline;
     the builder takes each id once and leaves out a native node (and its
     subtree) whose id is already in the tree, since AccessKit panics on
     a duplicate. Scenes are copied after the pane's terminal lock is
     released.
   - Bounds are the computed layout in cells, scaled by the cell size,
     from the pane's origin (a surface's from its first row), less the
     scroll offsets of the regions above; line labels have none.
   - Nodes advertise no actions: `Action::Focus` and `Click` are not
     handled yet (§3 Actions).
   - Sessions count changes to what they show (`shown_generation`:
     scene messages applied without error, except styles and streams;
     focus; scroll steps; relayout; surfaces placed, dropped or moved on
     or off screen), which the app hashes into its shown digest only
     while a screen reader listens. Idle wakeups with a screen
     reader listening and a still screen: 0 in 5 s at the prompt; 21 in
     5 s with `counter-app`, against 20 with accessibility off, all from
     the 250 ms maintenance tick native sessions already have, with no
     tree rebuilt.
6. **Live regions**: queue, flush rules, cap, pruning, `busy`. Check: builder
   tests with a scripted clock; manual step 5, the B4 done criterion.
   Done 2026-11-09 except the Orca run (`a11y/live.rs`);
   `scripts/e2e/accessibility-live.sh` streams twelve lines into a polite
   log with the `live-region` example of `ultty-client` and checks the
   AT-SPI `object:announcement` events: every line once, in order, never
   cut inside a line, at most four a second. Choices made where §3 was
   open or did not survive contact:
   - Only `AppendContent` feeds the queue, as §3 says; a `SetContent` on
     a live region is not queued. A live region never appended to is
     named by its one-line text, and on both platforms a renamed live
     node is announced, which covers a one-line status set whole. Once
     appended to, a live region is never named by its text again, not
     even after a later `SetContent`: a stated choice, so a stream is
     not spoken twice.
   - "At most four flushes a second" is a spacing of 250 ms per session;
     a flush is one announcement, of the region that has waited longest.
   - A flush with a line break in it takes the complete lines only. The
     line begun after them waits for its own break or 250 ms from its own
     first byte; flushing everything once the queue was 250 ms old cut a
     word in half whenever the cap had held lines back (seen over AT-SPI).
   - Over 2 KiB, the cut is after the last line break inside the limit,
     else after the last blank, else at a character boundary. An
     announcement is not cut to the 1024-character name cap.
   - Bounds: a region queues at most 16 KiB, cut back to 12 KiB from a
     line start near the cut when it passes that; a session's live
     regions hold at most 64 KiB, queued and announced together, cut back
     to 48 KiB oldest first (announcements, then the queues that have
     waited longest); a session holds at most 128 KiB of appended text
     between two passes of the event loop, cut back to 64 KiB. Text is
     kept only while AccessKit is active, only for sessions of the active
     tab, and only when that tab has one pane (the only time scenes are
     shown); inactive, appends cost one check.
   - Announcement nodes are `s<session>:<region>a<serial>`, the serial
     counting every flush since activation, so a kept node keeps its id
     and a new one never reuses one. A region that leaves the tree drops
     its queue and announcements, so that coming back adds no old node;
     so does activation, and so does deleting the region, reported in
     order with the appends so a region created again with the same id
     inherits nothing. Announcements are counted against the node budget
     before the region's text and children, so they are never held back.
   - Because names and liveness work as corrected above: a live region
     that has been appended to is not named by its text, and every node
     under a live region that is not live itself (its line `Label`s,
     child regions and theirs) is marked `live` `Off`, else each new or
     growing line would be spoken besides the announcements, unthrottled.
     Only a region's own `live` queues its appends. The region's own node is still
     announced (its label) when it is first added. A consequence for the
     "one region per message" pattern: a new child region added under a
     live log is not spoken when it appears (it is `Off`); an app that wants
     each message read streams it into the live region with `AppendContent`,
     or marks the message region live itself. Slice 7 (ulttui) should make
     `MarkdownView` streaming do the former.
   - `busy` follows the region's open stream, also when `StreamStart`
     came before the region was created.
   - Idle with a screen reader listening after the stream: 20 wakeups in
     5 s, the app's maintenance tick, and no tree rebuilt.
7. **ulttui**: `View::semantics`, widget defaults, builders, `Semantic`,
   backend diffing, done-bar line. Check: widget snapshots gain semantics;
   `regions --json` on `chat` shows a `Log` with `Polite`.
   Done 2026-11-10 except the Orca run. `snapshot_widget_semantics`
   records what each widget sends; `scripts/e2e/accessibility-chat.sh`
   finds `chat`'s transcript as a polite `Log` in `regions --json`,
   types a message, and hears the streamed reply over AT-SPI once, in
   order, polite; `accessibility-atspi.sh` also finds counter-app's
   buttons named in `regions --json`. Choices made where §4 was open:
   - The derivation from content moved into `ultty-protocol`
     (`derive_semantics`), so the app compares with exactly what the
     terminal derives: a view's semantics are sent only when they derive
     to something else for its content (taken as never appended to and
     not streaming). A button, a progress bar, a spinner, plain text and
     containers send nothing.
   - `SetSemantics` goes in the same frame as the region's create,
     after every content change of the frame; with no capability, never.
     A region whose view falls back to the default gets
     `Semantics::default()`, so the terminal derives again.
   - `Semantic` adds no region: it changes the semantics of the view it
     wraps and forwards everything else. It also takes `hidden`.
   - Defaults as built: `Button` button named by its label; `Checkbox`
     checkbox and `Toggle` switch, named by the label without the drawn
     prefix, `toggled`; `ProgressBar` progress, value 0–1; `Spinner`
     busy status; `Badge` status; `ErrorState` alert, hint as
     description; `Title` heading, level 1 (`Title::level`), a
     `Section`'s title level 2 and the section a group named by it;
     `Divider`, `Spacer` hidden; `Image` image, `.label()` is the alt
     text; `TextInput` input with the typed value and placeholder (not
     the focus and vim markers its content draws), nothing for a
     `.password()`; `Select` combo box, the selected option as
     `value_text`, its options child `Option` regions while expanded
     (see open questions); `List` list, rows list
     items with position; `Tabs` tab list, tabs named by their labels
     with position; `Tree` tree, rows tree items with level and
     expanded, the texts drawn in a row hidden; `Collapsible` tree item,
     expanded, chevron hidden; `Modal` modal dialog named by its title,
     hidden while not visible, its buttons buttons; `Table` table, rows
     rows, the header row's cells column headers. `LoadingState`,
     `EmptyState`, containers: the derived default.
   - The active tab, the selected `List` row, the selected `Tree` row
     and the selected `Table` row carry the `selected` flag (bit 64,
     added after this slice; see open questions), and only that one
     item does. The terminal exposes any `Tab`, `ListItem` or `TreeItem`
     without the flag as selectable but not selected, as ARIA does for
     those roles, so one bit is enough. A first version had no row role:
     a table row was a `Group` with the flag, and `Select` was one region
     whose options were lines of its text; both now have roles of their
     own (see open questions).
   - `MarkdownView` is one region per block, streaming or not. While it
     streams, the block still arriving is live itself (with the view's
     liveness) and the view is the busy log around it; a finished block
     stops being live. Streamed text so reaches a live region as
     `AppendContent`, each region holds one block (the codec's 10,000
     spans per text and the wire cost stay per block: one more item on a
     3,500-item list costs under 1 KiB), and a code block keeps its rule.
     No cursor is drawn natively while streaming: it would be appended
     and spoken; `busy` says it instead. A first version kept the whole
     document as one region's text; review found it past the span limit
     at about 40 KB of markdown and resending the document whenever a
     block or styled span began.
     Not done: finished blocks are still rendered and diffed every frame
     (CPU is O(blocks) per frame, as for any view); caching them is a
     follow-up. A first version appended a plain-looking chunk to the
     last block without parsing it again, so a link streamed in small
     chunks (`[te` `xt](ht` `tp://x)`) stayed literal on screen and its
     address was spoken when the stream ended, and a setext underline
     (`Title`, `\n`, `=====`) showed and was spoken (review probes). Now
     every `append_chunk` parses again from the line the last top-level
     block starts on, or a top-level list's last item, keeping the blocks
     before it: O(that block or item) per chunk, and the same blocks as
     parsing the whole source (two property tests stream random markdown,
     made of pieces and of whole lines of every kind of block, in random
     chunks and a character at a time, against a whole parse). A restart
     point is taken only after a blank line (spaces and tabs only, not
     U+00A0 or U+3000), not after a list (`1` can still become `1. x`,
     one of its items); in a list, at an item after its first once the
     one before is done and the item's first line keeps it an item
     (text after the marker, not only `-`, `*`, `_`), carrying the list's
     numbering. Parsed from its last item a list can be tight where the
     whole is loose, so text after another block of a tight item begins
     a paragraph as it would in a loose one. A source with a link
     reference definition, or a `[^label]:` footnote definition (not any
     `[^`: a regex is common), is parsed whole on every chunk, as those
     reach back to earlier blocks. Streaming in 4-byte chunks, release,
     in all (before, parsing the whole source on most chunks): 40 KB of
     paragraphs 13 ms (700 ms), the same with `[^a-z]` in a code span
     10 ms (660 ms; any `[^` once parsed it whole, seconds), a 2,000-item
     list 9 ms (6 s), ordered 15 ms (8 s). An open fenced code block is
     one block, parsed again whole: 40 KB of code 130 ms (70 ms, when
     code was appended without parsing).
   - Options: pulldown-cmark's metadata blocks are off (a reply that
     starts with `---` had the text to the next rule hidden as front
     matter, and only in a whole parse), and so are definition lists,
     heading attributes and its own task lists: 0.12.2 panics, in
     release too, on a definition list in a block quote (`> _\n> : \n\n`)
     and on a heading a task marker was carried into (it gives the marker
     of an item that starts with another block, `- [ ] # x`, to whichever
     paragraph comes next). Their syntax shows as the text it is; a task
     item is a list item whose own plain text starts with `[ ] `, `[x] `
     or `[X] ` and goes on.
   - A task list item (`- [ ] buy milk`) draws the `Checkbox` widget's
     ASCII `[ ]` / `[x]` in place of its bullet (after its number when
     ordered) and is a read-only `Checkbox` named by its text, `toggled`
     True or False: GitHub draws a task item as a disabled checkbox, and
     the region is the item, so no extra node is needed. The checkbox is
     drawn but never announced (the walk notes the unspoken bytes of the
     region, `FrameNotes`); an ordered item's number is said, and is in
     its name (`1. text`). While it arrives the item is an ordinary
     block region and becomes the checkbox in the frame it stops being
     live: the adapters announce a live node whenever its name changes,
     and a checkbox without a label is named by its growing text
     (`accessibility-chat.sh` heard `[x] a task that`). An arriving item
     whose text is empty or could still be a task marker (`[`, `[x`) is
     drawn empty until that is known: a region created live with text it
     has not appended is named by it and announced (`• `). A task item
     inside a block quote stays quoted text.
   - The backend keeps the text it has appended to each live region
     (announced), in the form it has now. What the region shows past
     that is appended, then a `SetContent` in the same frame sets the
     styled result when appending alone does not give it (a span turned
     bold). Where to resume: after the announced text if the new text
     starts with it (letter case ignored, so `Title` set as `TITLE` is
     not news); nothing if the text only got shorter; after the
     announced part of the same line when an earlier line was rewritten;
     else at the start of the word where the two part. Text is repeated
     rather than skipped; a first version counted bytes and lost text
     whenever announced text got shorter (review probes: a long link, a
     long bold phrase).
   - Only `MarkdownView`'s arriving prose block (paragraph, heading, list
     item; not code, not other live regions such as a plain log with
     `[INFO]`) holds back the end of its last line from the first inline
     opener still shown as characters, so `**bo` is not spoken before
     `bold` and a link's address is never spoken while it may resolve.
     `[`, `**`, `__` and `~~` hold until the line ends, which for a
     paragraph is when the block is finished, except a `[` whose `]` is
     already followed by something other than `(` or `[` (`x[i]`, `[1]`),
     which can no longer become a link. A lone `*`, `_` (not in a word)
     or `` ` `` holds only within 80 bytes of the end, past which it is
     taken as literal (`2 * 3`); a closed code span no longer shows its
     backticks. A last line of only `=` or `-`, which under a line of
     text is a setext underline, is held whole. The walk notes such
     regions beside the native keys (`FrameNotes`).
   - A region created live announces its first text the same way, and a
     region that turns live announces only what it grows by.
     `SetSemantics` that makes a region live goes before its content in
     the frame; any other change after, so the text that arrives in the
     frame a region stops being live (the last chunk, as the stream ends)
     is still appended and announced, without the hold-back.
   - App-sent strings are cut, at a character boundary, to what the codec
     decodes (4 KiB each, 8 KiB together): a long input value or label
     made the terminal refuse the message.
8. **AT-SPI smoke test in CI**, or the local script if headless AT-SPI is
   unreliable.
9. **macOS**: VoiceOver run of the manual script, differences recorded.

## Open questions

- Does Orca 46.1 (Ubuntu 24.04) and 48.1 (Debian 13) have
  `_on_announcement`? Settled by running manual step 5 on each.
- What does Orca speak for unsolicited output in an AccessKit-backed
  `Terminal` node, and does its 1 s repaint window make alternate-screen
  TUIs tolerable? Manual steps 2 and 3.
- Should a `Button` activation input event be added to the protocol so
  `Action::Click` works, and should it come with keyboard activation of
  `Button` regions in general?
- Does `accesskit_winit`'s "full tree" requirement apply to every update
  or only the first after activation? Answered by the source
  (`accesskit_consumer` 0.39.2 `Tree::update`): only the first. Later
  updates may carry just the changed nodes; nodes no longer reachable
  are dropped, and unchanged nodes in a full update raise no events.
  Slice 4 sends full trees (44 nodes for an 88x39 screen, built in under
  0.1 ms); incremental updates stay a later optimisation.
- Whether the unused `tokio` dependency of `crates/ultty` should go, now
  that a second executor arrives.
- How should a selected item be exposed? Answered: §1 gained a
  `selected` flag, bit 64 of `flags`, which the terminal maps to
  AccessKit `set_selected(true)` (AT-SPI state SELECTED). Semantics had
  not shipped in a release, so it needed no new capability. ulttui sets
  it on the active `Tabs` tab and the selected `List`, `Tree` and
  `Table` row. A `Tab`, `ListItem` or `TreeItem` without it is
  selectable but not selected; other roles say nothing about selection.
  `scripts/e2e/accessibility-atspi.sh` checks over AT-SPI that the
  `dashboard` example's selected list item is SELECTED, its others
  SELECTABLE only, and that the selection follows `j`.
- How should `Table` rows and `Select` options be exposed? Answered: §1
  gained `Table`, `Row`, `Cell`, `ColumnHeader`, `ComboBox` and `Option`
  (21 to 26), mapped to the AccessKit roles of the same names (`Option`
  to `ListBoxOption`). Semantics had still not shipped, so no capability.
  - `Table` maps to AccessKit `Table`, not `Grid`. AccessKit 0.25.2
    counts only item roles (list items, options, tree items, tabs) as a
    selection container's items, never a `Row` (its table support is
    AccessKit issue #29), so a `Grid` would offer a selection interface
    that reports no row selected. A row carries `selected`, which AT-SPI
    shows as SELECTED on a `table row`; the rows of a table one of whose
    rows is selected are SELECTABLE, as list items are, and a table that
    selects nothing says nothing.
  - A row's `position` is its row index (the header row first) and a
    cell's its column index: the terminal sets AccessKit's zero-based
    `row_index` and `column_index`, not `position_in_set`, which ARIA
    does not allow on rows and cells. The table's `row_count` and
    `column_count` are derived from its children: its `Row`s, and the
    most `Cell`s and `ColumnHeader`s in one of them, so an app need not
    send them. AccessKit 0.25.2's AT-SPI adapter does not expose the
    counts or indices (no `Table` interface yet); they are in the tree
    for adapters that do.
  - `Cell` and `ColumnHeader` are needed for a table to be navigable as
    one: a header cell names its column, which a screen reader with
    table support reads with each cell of it. `Table` already drew each
    cell as a region, so they cost no regions.
  - `Select` is a `ComboBox` valued by the chosen option. While
    expanded its region holds no text and has one `Option` child per
    option, named by the option, with its position, the drawn line
    (cursor, mark, option) a hidden child. The options are the combo
    box's own children rather than a listbox's: AccessKit treats
    `ComboBox` as a selection container and `ListBoxOption` as an item,
    so the combo box's selection interface lists them, with no listbox
    role. `selected` is on the option under the cursor, the one Enter
    would choose, as the ARIA combo box pattern puts `aria-selected` on
    the active option; the chosen one stays the combo box's value until
    Enter. With `selected` on the chosen one instead, moving through the
    options would change nothing a screen reader is told.
  - Focus stays on the combo box (the app's focused widget), so the
    terminal makes an expanded `ComboBox`'s selected `Option` child its
    `active_descendant`: AccessKit then reports the option as focused,
    and focus moves with the cursor. No protocol field is needed; an
    app's own active descendant would need one.
  - An option is two regions, the `Option` and a hidden child holding
    its drawn line. One region holding the line and labelled by the
    option would have its line read as well: the terminal gives a
    labelled region's text as `Label` children whenever the label is
    not the whole text, and the line ("> * Red") never is.
  - Cost: each `Row`, `Cell` and `ColumnHeader` now has semantics, which
    the terminal keeps boxed per region. The `scene_table_1000x4` cases
    of `benches/memory.rs` measure it: a table of 1000 rows of 4 cells
    (5001 regions) holds 3.73 MB without semantics and 4.41 MB with
    them, about 136 bytes a region. A session holds at most 10,000
    regions (`MAX_REGION_COUNT`), so semantics add at most about 1.4 MB
    to one, a table of some 2,000 rows of 4 cells.
- Positions and levels were off by one on every platform, and set sizes
  missing. AccessKit 0.25.2 counts `position_in_set` and `level` from 0
  and reads the size of a set from the items' container
  (`size_of_set_from_container`), while the first version passed the
  1-based wire values and set the size on each item: AT-SPI reported
  item 1 of 2 as `posinset` 2 with no `setsize`. The terminal now
  subtracts 1 and sets `size_of_set` on the container of items with a
  `position` (the largest size among them; rows and cells are indexed
  instead). `scripts/e2e/accessibility-atspi.sh` checks the dashboard
  list's items report `posinset` 1 and 2 of `setsize` 5 over AT-SPI.
