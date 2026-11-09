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
                                 atomic=16, modal=32
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
| List | `List` | `List`, `Select` |
| ListItem | `ListItem` + `position` | their rows |
| TabList | `TabList` | `Tabs` |
| Tab | `Tab` + `position` | each tab label |
| Tree | `Tree` | `Tree` |
| TreeItem | `TreeItem` + `expanded`, `level` | `TreeNode`, `Collapsible` |
| Image | `Image` | `Image` (label is the alt text) |
| Link | `Link` | OSC 8 style links in rich text, later |

Not in the set: `Table`/`Row`/`Cell` (`Table` synthesises cells, which are
not regions; it exposes as `Text`), `Terminal` (the grid's), menus,
sliders. Adding a role is additive under ADR 0011; removing one is not.

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
  the tree builder can find while they are on screen.
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
   idle-wakeup numbers before and after.
4. **Grid as Terminal**: tabs, panes, focused grid rows and caret, coalesced
   rebuilds. Check: builder tests; manual steps 1 to 3 recorded.
5. **Native scenes**: regions to nodes, derived and explicit roles, focus.
   Check: builder tests; manual step 6.
6. **Live regions**: queue, flush rules, cap, pruning, `busy`. Check: builder
   tests with a scripted clock; manual step 5, the B4 done criterion.
7. **ulttui**: `View::semantics`, widget defaults, builders, `Semantic`,
   backend diffing, done-bar line. Check: widget snapshots gain semantics;
   `regions --json` on `chat` shows a `Log` with `Polite`.
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
  or only the first after activation? Slice 4 reads the adapter source and
  measures; incremental updates wait on the answer.
- Whether the unused `tokio` dependency of `crates/ultty` should go, now
  that a second executor arrives.
