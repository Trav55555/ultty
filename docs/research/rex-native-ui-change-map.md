# Rex native UI change map

Source baseline: `c718910fad12749ec53379ab09256ae5a52a826b`, inspected 2026-11-12. The change from the previous research baseline contains documentation only. This is a source review and implementation map. No implementation, build, experiment, GUI run, or qualification was performed.

This document owns pane-native transport, inline surfaces, program status, semantic actions, Ulttui backend behavior, and native reconnect implications. The automation/control roadmap and persistent-runtime extraction are separate workstreams. File and line references describe the baseline, not a future patch. Proposed names and interfaces below are design choices, not existing APIs.

## Decisions to make before implementation

1. Keep PTY ownership explicit in native transport and inline pairing. A tab remains a presentation/container relationship. It must not identify the return path for bytes from two different panes.
2. Retain OSC 7501 records independently of the tab's short status indicator. Preserve the existing `PaneStatus` presentation API as a compatibility projection. Store lifecycle state with the terminal, where prompt/reset processing is ordered with output.
3. Keep application state in `Application`. Add targeted actions delivered to that owner. Do not mutate freshly rendered `View` values or silently turn a role into permission to execute an action.
4. Distinguish rendering identity, application widget identity, accessibility node identity, and attachment generation. Existing keys do not supply all four contracts.
5. Preserve full-scene EOF-to-quit and inline fallback until a persistent host makes viewer loss invisible to the application's native connection. Do not add automatic application reconnect as a shortcut for viewer reconnect.

The smallest useful order is pane-owned in-band transport, pane-owned inline rendering, bounded status ingestion/lifecycle, targeted activation, then native detach qualification against the persistent-owner design. Status ingestion is independent of the daemon. Semantic action delivery requires an agreed identity contract before an external action endpoint is added.

## Verified current behavior

| User path | Current source path | Boundary still missing |
|---|---|---|
| Socket-native Ulttui app in a split | `app/native_panes.rs::bind_native_sessions` finds a pane by process group, then synchronizes the pane viewport; `renderer/panes.rs::encode_panes` draws full scenes inside pane bounds | This already exists. Preserve it as the control. |
| Ulttui app over SSH in a single pane | `ultty-client/detect.rs` selects the APC probe; `EventLoop::take_in_band` transfers decoded frames; `App::pump_in_band` feeds the protocol server and returns replies to that PTY | Server identity and output accessors remain tab-scoped. A split disables this path. |
| Inline native card in a split | Anchor events remain per terminal, but pairing rejects new anchors when `single_pane` is false | Removing this check alone leaves GPU, cache, and accessibility omissions. |
| Program reports progress | OSC 9;4 becomes `TerminalEvent::Progress`, then `ManagedTerminal.progress`, then tab/title presentation | No OSC 7501 parser or hierarchical record store; status changes are not subscription events. |
| Named native region inspection | Stable native keys become `id` metadata; `regions` exposes content/layout/focus/semantics | Inspection identity is not connected to application action dispatch. |
| Human presses Enter on counter button | Local `FocusManager` annotates the key; `counter-app::on_event` matches Enter plus focused widget ID | This works by explicit application handling, not automatic Button activation. |
| Pointer input in a native app | Wire input becomes a raw `Event::Mouse` | Region identity is discarded. Raw mouse delivery exists; general named activation is absent. |
| Screen reader inspects a native app | AccessKit tree contains roles, names, values, selection, focus, and live announcements | `ActionRequested` is logged and ignored. Actual Orca/VoiceOver qualification remains separate. |
| Native connection is lost | Full-scene backend queues `Event::Quit`; inline console freezes existing cards and continues in fallback | Neither path resumes a previous native session. |

Paths in this table are expanded with exact source references below.

## N1. Give each in-band stream its originating pane

### Runtime trace

The parser/PTY state is already per terminal. `State.in_band`, `in_band_broken`, `in_band_open`, `held_responses`, and `outbound` are in `crates/ultty/src/event_loop.rs:155-166`. `take_in_band` and `finish_in_band` preserve query ordering under one lock at `event_loop.rs:508-545`. This ordering must survive the change. `finish_in_band` must run even when a probe is unsupported, malformed, or rejected, so held DA replies are released.

`App::pump_in_band` already iterates `(TabId, Vec<TerminalId>)`, then reads the specific terminal. It computes `single = terminals.len() == 1`, accepts frames only under `Some(server) if single`, and later ends a session when the tab no longer has exactly one terminal at `crates/ultty/src/app/update.rs:46-118`.

`ProtocolServer` still indexes opening fragments and session lookup by tab. `in_band_input` creates `NativeSession::new_in_band` with tab, foreground group and captured local PIDs at `crates/ultty/src/protocol/server.rs:538-596`. The related `end_in_band_if_app_gone`, `take_tab_in_band_output`, `end_in_band_session`, `take_in_band_output`, `in_band_tabs`, and private `in_band_session_key` use tab identity at `server.rs:617-678`.

Socket sessions bind through `ProtocolServer::bind_unbound_sessions` at `server.rs:302-325` and `App::bind_native_sessions` at `crates/ultty/src/app/native_panes.rs:10-25`. That heuristic is appropriate for the socket peer. An in-band hello arrives on a known PTY and should bind directly there. Inferring its pane from the SSH process's current group or the focused pane is unnecessary and can be wrong when tabs have multiple panes.

### Concrete change map

| File / symbol | Required change |
|---|---|
| `app/update.rs::pump_in_band` | Pass both tab and originating pane on every feed. Remove the single-pane admission/end checks only after the server uses pane identity. Return frames only to the originating `event_loop`. |
| `protocol/server.rs::ProtocolServer.in_band_openers` | Key partial hellos by `TerminalId`, or a pane identity type shared with the runtime owner. Preserve tab as ownership metadata, not return routing. |
| `protocol/server.rs::in_band_input` | Accept `pane`; create and bind the new native session before it can be drawn or sent input. Set its viewport from `pane_grid_size`, not the whole-window default. |
| `protocol/server.rs` in-band lookup/output/end accessors | Replace tab-scoped methods and their callers together with pane-scoped equivalents. `take_in_band_output` returns pane IDs. Permit one confirmed in-band session per pane, including two in the same tab. |
| `protocol/session.rs::new_in_band`, `bind_pane`, keepalive/liveness helpers | Retain local process-group/death checks and unconfirmed-session restrictions. Associate them with that pane's SSH/sudo/recorder process. These checks do not prove remote child lifetime after SSH loss. |
| `app/native_panes.rs`, `protocol/server.rs::retain_pane_sessions` | Keep socket binding unchanged. Pane close removes that pane's opener and native session; sibling close/split/resize does not end it. Keep whole-tab close as a sweep across its panes. |
| `docs/PROTOCOL_SPEC.md` sections 1.2/1.3 and ADR 0025 | Update the one-session-per-tab and one-pane restrictions. Record that this is terminal routing behavior, without adding a client-supplied pane ID to the wire. |

Suggested runtime seam, with names illustrative: `feed_in_band(tab, pane, foreground, local_pids, input)`, `take_pane_in_band_output(pane)`, and `end_pane_in_band(pane)`. Migrate old accessors in the same slice rather than retaining two subtly different routing APIs.

### Verification and compatibility

Extend `protocol/server.rs` tests `in_band_hello_split_across_calls_opens_a_session`, `in_band_sessions_are_one_per_tab_and_stay_on_their_tab`, `in_band_session_ends_on_disconnect_broken_stream_or_new_foreground`, both confirmation tests, and `in_band_sessions_receive_keys_whatever_the_foreground`. The one-per-tab test becomes one-per-pane, with two panes of one tab and matching foreground groups to ensure pane identity actually decides.

Add cases for a fragmented hello in each pane concurrently, late replies after a sibling closes, pane resize before `SessionReady`, active-tab changes, and malformed input in one pane without ending its sibling. Preserve `in_band_sessions_may_not_read_the_clipboard` and held-query ordering tests in `event_loop.rs`.

Extend `scripts/e2e/native-split.sh` and its checker to exercise one socket app and one genuine in-band app, then two in-band apps. A local PTY-based in-band fixture proves terminal routing. A real SSH fixture is additionally required for the SSH-native user claim; it should use an explicitly supplied host, not create infrastructure. Confirm native mode with `regions` and transport flags, rather than treating a visually similar ANSI fallback as success.

No app wire change is necessary for pane attribution. Keep the current probe failover and security behavior in `crates/ultty-client/src/detect.rs:36-60`, `protocol/server.rs:11-24`, and `protocol/session.rs:83-95`. Tailscale does not alter any of these parser/PTY rules.

## N2. Complete inline surfaces across pairing, drawing, inspection, and accessibility

### Runtime trace and identity

`ProtocolServer.pairings` already keys `AnchorPairing` by terminal. However, `needs_pairing` considers any surface in the tab, and `pair_inline_surfaces` passes all the tab's sessions to the pairing at `crates/ultty/src/protocol/server.rs:680-717`. `App::pair_inline_surfaces` supplies `single_pane` and `single_pane && active_tab` at `crates/ultty/src/app/update.rs:129-156`.

`AnchorPairing::update` releases waiting/new anchors when they are not claimable at `crates/ultty/src/protocol/pairing.rs:265-296`. Pairing otherwise matches anchor session ID, surface ID and row count at `pairing.rs:365-397`. Tag reuse is protected by placement serials, and freeze/copy/search behavior is part of the implementation at `pairing.rs:309-353` and `pairing.rs:401-428`.

Therefore removing `single_pane` without filtering sessions by owning pane weakens an existing ownership check. The same `(session, surface)` arriving through another pane must never claim that pane's anchor. Keep tag serial reconciliation on overflow and anchor loss.

### Concrete change map

| File / symbol | Required change |
|---|---|
| `protocol/server.rs::needs_pairing`, `pair_inline_surfaces` | Filter by `session.pane() == Some(terminal_id)`; use pane ownership as admission. Do not let a temporarily unbound socket session claim an arbitrary sibling's anchor. Decide whether an anchor can establish an otherwise unbound inline-only session's pane after existing trust checks, or whether it waits for normal binding. |
| `protocol/pairing.rs::AnchorPairing::update` | Replace the tab-alone meaning of `claimable` with explicit owner validation or remove the flag only once caller filtering is mandatory. Keep timeout, rows agreement, duplicate anchor, serial, final-frame and overflow reconciliation behavior. |
| `app/update.rs::pair_inline_surfaces` | Pair each pane independently. Visibility depends on that pane's active tab and actual screen intersection; splitting does not release its anchors. Expire unplaced surfaces after all relevant pairing passes. |
| `renderer/panes.rs::encode_panes` | Add inline surface collection and drawing for panes without a covering full scene. Current code gathers only full scenes at `:181-196` and `:254-298`. Each surface needs pane-local horizontal origin, anchor-relative vertical origin, pane scissor and surface clipping. Preserve borders above content. |
| `renderer/native_render/mod.rs::visible_surfaces`, `SurfaceOrigin` and native cache | Current surface placement uses a top offset at `:118-139`; extend placement/cache identity with pane origin and clipping. `encode_panes` currently passes no surfaces to `native_cache_valid` and clears `cached_native_surfaces` at `renderer/panes.rs:195-196` and `:304-307`. Include moved/evicted surfaces and pane geometry in invalidation. |
| `app/accessibility.rs::accessibility_snapshot` | Replace `surfaces_shown = rects.len() == 1` at `:165`. Gather each pane's visible surfaces at its bounds, retain native focus rules and the current scrolled-back limitation explicitly. Current collection runs only inside the focused grid branch at `:191-223`; supporting exposed surfaces in unfocused panes is a separate deliberate tree policy. |
| `app/control.rs` screenshot path | Keep sharing `encode_panes` between the window and screenshot. Assert semantic tree state and pixels; a screenshot file merely existing does not prove surfaces were included. |
| `protocol/session/surfaces.rs`, `ultty-client`, `ulttui::inline` | Preserve fixed surface height, fallback text and lifecycle events. Pane-width resize must regenerate native layout/rasterized copy text. Do not make inline cards interactive as a side effect. |

`NativeSession::key_target` excludes inline-surface regions at `crates/ultty/src/protocol/session.rs:1082-1094`, and subscribing to inline keys is refused at `protocol/session/commands.rs:193-203`. Keep this contract. An inline command transcript and a full-scene form have different interaction ownership.

### Verification and compatibility

Change the intentional restriction tests `anchors_of_a_pane_that_is_not_alone_are_released` and `a_split_releases_waiting_anchors` in `protocol/pairing.rs`. Replace them with owner-specific rejection and split-survival cases. Preserve `an_anchor_pairs_only_with_its_own_session`, both arrival-order tests, rows disagreement, lost-anchor eviction, overflow reconciliation, tag-reuse, final-frame-before-close, ended-session freeze, selection fidelity, resize and alternate-screen tests in the same module.

Add two panes using equal local surface IDs and reused tag values under different sessions, with one surface waiting while a sibling already placed. Test drag/resize/cache invalidation, tab switches, scrolling each pane independently, full-scene coverage of inline content, closing one pane, and a sibling session disconnect. Native region IDs and anchor tags are not globally unique without session/pane context.

Extend `scripts/e2e/inline-surface.sh`, `agent-transcript.sh`, `native-split.sh`, `accessibility-atspi.sh`, and `crates/ultty/src/a11y/scene.rs` tests. Check copy/search text, screenshot pixels, placed/hidden/evicted events, and AT-SPI placement. Inline-only sessions should continue reading ordinary keys from their tty.

This requires no new surface wire format. It changes eligibility and UI coverage, so update ADR 0020/0025, protocol section 10.6, and user docs together. Do not advertise supported native inline splits until rendering and copy/accessibility gates pass, even if anchors successfully pair.

## S1. Add program-status parsing and retained state

### External contract checkpoint

The vendor's [OSC 7501 revision 0.4](https://www.superlogical.com/rex/docs/build/program-status), checked 2026-11-12, defines colon-separated key/value reports and an optional query. Records replace by hierarchical ID; clear removes a subtree. States are idle, working, done, blocked and error. Prompt-start/process-exit cleanup removes working/blocked; done/error survive. RIS clears records; DECSTR and alternate-screen changes preserve them. Reports require bounded validation before mutation, including strict text decoding. Malformed pairs may be skipped; unknown keys are ignored; duplicate keys use the last value. Caps include 4096 wire bytes, 2048 decoded message bytes, 192 decoded title bytes, 128 ID bytes, eight ID levels, and 256 records with least-recently-updated eviction. If OSC 9;4 is mapped into this model, the specification recommends stopping that mapping after a valid OSC 7501 report until RIS. This is a vendor proposal, not evidence of broad adoption.

### Current trace and traps

`AnsiParser::dispatch_osc` decodes the whole OSC buffer lossily into UTF-8 at `crates/ultty-vt/src/ansi/osc.rs:65-75`, handles OSC 9;4/133 at `:125-142`, and logs unknown sequences at `:216-219`. The generic string cap is 65,536 bytes at `osc.rs:9-10`; overflow resets the parser at `:32-37`. There is no status report action or store. OSC 7501 needs its own precise wire-length/error accounting, not just the larger generic OSC cap.

The existing `base64_decode` is intentionally lenient. It strips padding/newlines and silently filters unknown characters, then always returns `Ok`, at `osc.rs:499-526`. Reusing it would accept invalid status text. Use a strict bounded decoder for the new protocol, with explicit invalid UTF-8 rejection. Leave OSC 52's established behavior alone unless separately reviewed.

`EventLoop::parse_into` converts selected `Action`s into `TerminalEvent`s, then applies `TerminalHandler` at `crates/ultty/src/event_loop.rs:1021-1090`. Events beyond `MAX_PENDING_EVENTS` can be lost. Consecutive progress events coalesce. Keeping the only copy of status in this lossy event queue would make the UI and automation permanently wrong after a flood.

OSC 133 prompt start is applied in `TerminalHandler::handle` at `crates/ultty-vt/src/grid/term_handler.rs:379-389`, but is not a runtime event in `parse_into`. Current progress presentation is cleared on `CommandFinished`, not prompt-start, at `crates/ultty/src/app/update.rs:388-395`. The comment calls that "the prompt"; the actual branch is the command-finish event. Explicit control status remains separate in `ManagedTerminal.status`, ahead of `progress`, at `crates/ultty/src/terminal_manager.rs:102-112`.

### Concrete change map

| File / symbol | Required change |
|---|---|
| New VT-core status module, re-exported by `ultty-vt` | Define validated report, record ID/path, state/kind, record store and typed mutation result. Keep it independent of `ultty-protocol::control::PaneStatus`; the VT core currently has no protocol dependency. Implement replacement/clear/eviction as pure bounded operations. |
| `ansi/mod.rs::Action`, `ansi/osc.rs::dispatch_osc` | Add report/query actions. Parse raw OSC bytes with the protocol-specific budget before lossy conversion. Require strict text validation. Ensure oversized/malformed data cannot emit a partial mutation or expose control bytes as rendered text on recovery. |
| `grid/terminal.rs::Terminal`, `reset_auxiliary_state`, `mark_prompt_start` | Own records and an update generation with the terminal. Apply prompt/RIS lifecycle at the exact point those actions are consumed. Preserve store across primary/alternate-screen switching. Define an explicit `process_exited` transition callable by the runtime. |
| `grid/term_handler.rs::TerminalHandler::handle` | Apply reports/query actions to the store. Queue only the static capability reply via the ordinary terminal response mechanism. Do not echo report IDs or text. |
| `event_loop.rs::parse_into`, `process` | Expose generation/delta notification after authoritative store mutation. Reconcile from a snapshot if events overflow. At `process`'s final-output-before-child-exit boundary (`:616-629`), apply process-exit lifecycle after last output and before any pane removal. |
| `terminal_manager.rs::ManagedTerminal::shown_status`, `progress_status` | Compute a short display projection from records. Keep legacy explicit status, OSC progress and program records distinguishable. Pick precedence explicitly; do not overwrite the hierarchy with the existing 64-character label. |
| `app/update.rs`, `app/mod.rs::update_tab_statuses` | Refresh status after report/reset/prompt/exit mutations, including background panes. Draw the existing short marker/label/progress projection. Preserve source pane attribution and keep status text separate from permission/action authority. |
| `ultty-protocol/src/control.rs`, `app/control_events.rs` | Add full-record query data and status-change events with generation/reason. Extend `ControlEvent::pane` and structured formatting. Add fields/variants deliberately; root automation work owns external command naming/discovery. |
| `control/server.rs` / subscription consumer contract | Keep bounded queues, but a disconnected consumer must resnapshot. Prefer an event carrying new generation or a bounded current snapshot over a claim that every transient status was delivered. A full owner-wide replay stream is later runtime work. |

Recommended state seam: `ProgramStatusStore::apply(report) -> Change`, `on_prompt()`, `on_process_exit()`, `on_ris()`, and `snapshot()` under terminal ownership. `Change` identifies upserts/removals or a new generation, so all mutation paths can drive presentation and external events consistently. Do not make the GUI poll `/proc` for status liveness or require report heartbeats.

### Compatibility decisions and blockers

- Existing `SetPaneStatus` is an explicit local override. Preserve its shape and document whether it outranks program reports; current behavior gives it precedence over OSC progress. Add the full record query separately. A flat field addition cannot express hierarchy/clear/inheritance faithfully.
- Map blocked records to waiting presentation, but preserve blocked kind, message, title and hierarchy for inspection. A permission request is untrusted program text, not execution approval.
- Do not clear all records on command finish. The new store needs its exact prompt/reset/exit lifecycle while legacy progress can retain its existing contract until deliberately changed.
- Default `close_on_exit` removes the pane at `crates/ultty/src/terminal_manager.rs:812-837`. A record cannot remain queryable in a pane that no longer exists. Honor terminal-record lifetime while the pane exists; keeping completion visible after pane closure requires an explicit bounded activity-history product, outside basic protocol support.
- Queue status effects after validation. Validate all pairs, not merely the retained duplicate winners. A later duplicate cannot hide an oversized/invalid earlier field.
- A zero-output/background status transition should update status without forcing a full foreground terminal repaint. Coalesce visual redraws and external attention effects; do not rate-limit authoritative state application by dropping arbitrary reports.
- Unknown OSC handling currently logs full content. Avoid dumping status payloads into diagnostics at normal levels; applications may put sensitive text there. Redacted malformed-report counters are enough.

### Tests and qualification

Add parser tests beside OSC tests for BEL/ST, every byte split through both feed paths, query followed immediately by DA, invalid pairs/UTF-8/base64/control text, duplicate/unknown keys, all size boundaries, invalid IDs and recovery after oversize. Preserve OSC 9;4 and OSC 52 tests unchanged as controls. Exercise the VT fuzz target, and add valid/invalid status seeds to `fuzz/fuzz_targets/vt_terminal.rs`'s corpus workflow.

Test the store directly for replacement, subtree clear, missing parents, inheritance, capacity eviction, update-order accounting, prompt/RIS/DECSTR/alternate screen behavior, process exit and idempotent clears. Add `EventLoop` tests proving final done/error output is applied before exit, working/blocked cleanup survives event overflow, and an unrelated pane cannot mutate the store.

Extend control roundtrip tests and `app/control_events.rs` tests for source pane, generation, clear reason, explicit override and reset. Add a real PTY-to-tab/status/event test with shell integration, then an SSH test for remote reporting. A parser-only pass does not qualify the visible tab marker or event stream.

## S2. Give Ulttui one reporting API without an extra input reader

`Cmd<M>` currently has messages, tasks, subscriptions, toasts, screens and quit, but no status effect at `crates/ulttui/src/cmd.rs:12-55`. The runtime executes effects in `crates/ulttui/src/app/runtime.rs::process_cmd`. The native backend owns its socket/in-band transport; the ANSI backend owns its ratatui terminal/output path at `crates/ulttui/src/backend/ansi.rs:47-80`.

Start with a proposed `Cmd::report_status(report)`/`Cmd::clear_status(id)` effect processed on the app loop. Put validated public report types/encoder in a reusable client-facing module, with a small compatibility mapping if the VT store uses a separate type. Do not require `ulttyctl`, local socket credentials, or a running Ultty daemon in application code.

The least invasive first transport is the same OSC 7501 encoding for ANSI, socket-native, and in-band-native applications. Socket-native apps still own a PTY. Serialize status output through each backend's existing writer. `UlttyClient::write_output` already coordinates ordinary bytes with frames and output at `crates/ultty-client/src/lib.rs:413-435`; `InBandTty::write_raw` is available. Ulttui's full-scene backend has its own transport and needs an equivalent owned-writer method, not a concurrent global stdout write from `Cmd::spawn`.

A native binary status message is an optional later optimization. It needs capability negotiation, a bound-session-to-pane source rule, and the exact same authoritative store mutation/lifecycle path. It must not create a second status store or override the OSC record lifetime accidentally. Do not spend protocol authority on two encodings before real reporting justifies it.

Do not introduce a second stdin reader for feature detection. Native transport currently ignores non-protocol bytes at `crates/ulttui/src/backend/native/transport.rs:60-70`, and ANSI input has its own decoder. A query response consumed by the wrong reader is a race. Reporting can work without querying. If callers later request detection, integrate it into the existing backend input parser with a finite startup timeout and preserve unrelated user input.

ANSI support means sending a harmless optional terminal report while keeping the app's ordinary UI usable; it does not give an arbitrary ANSI terminal Ultty's status UI. Provide a normal in-app text/status view for the essential meaning. Avoid silently polling an environment-provided socket to simulate native capability.

Tests: encoder boundary vectors; `MockBackend` capture of effects and failures; PTY byte verification in `crates/ulttui/tests/ansi_pty.rs`; native socket/in-band serialization and unchanged scene diff behavior; reporting beside large frames without interleaving. Add one example that reports working, blocked, done, and clear through the same application source in both modes. Keep output/report failure distinct from task completion.

## A1. Preserve identity from native input to application-owned actions

### Current dispatch, verified end to end

1. Native wire `InputEvent` includes `region_id`, event type, modifiers and coordinates relative to the receiving region at `crates/ultty-protocol/src/events.rs:16-26`.
2. `NativeSession::hit_test` hit-tests committed computed region rectangles and returns region-local coordinates at `crates/ultty/src/protocol/session.rs:1043-1071`. `key_target` chooses focused subscribed region or ROOT at `:1082-1094`. Ordinary foreground permissions remain distinct from region targeting at `:1097-1113`.
3. `NativeBackend::convert_input_event` matches `input.event_type`, drops `region_id`, casts pointer coordinates to unsigned cells, emits raw `Event::Mouse`, and gives keys `focused_id: None` at `crates/ulttui/src/backend/native.rs:551-603`. Text input is split into character keys. The conversion does not look up `RetainedTree` keys.
4. `App::prepare_event` assigns the current local `FocusManager` ID to each key at `crates/ulttui/src/app.rs:591-607`. `Application::on_event` receives events at `app.rs:505-518`. `View::on_key/on_event` explicitly are helpers for app-owned widget values, not automatic dispatch through the render tree, at `crates/ulttui/src/widgets/mod.rs:333-347`.
5. `Event::Click`, `Change`, and `Submit` exist at `crates/ulttui/src/event.rs:12-17`, but the inspected native and ANSI converters do not synthesize these variants. ANSI mouse decoding emits raw pointer input at `crates/ulttui/src/backend/ansi/input.rs:307` onward. Applications may handle raw pointer input themselves. This finding does not establish that every pointer-driven application is broken.
6. `counter-app::on_event` handles Enter plus `focused_id`, and character shortcuts, at `examples/counter-app/src/main.rs:43-56`. It ignores other event variants. `Button::on_key` merely returns `FocusAction::Consumed` on Enter at `crates/ulttui/src/widgets/button.rs:163-171`, and the runner does not call that helper on rendered views.
7. Luau listens for `Event::Click` at `crates/ulttui-lua/src/runner.rs:338-347`, but `examples/lua/hello.luau:16-33` currently implements button behavior with Enter plus focused ID and ignores non-key messages. Neither fact proves live mouse activation. Both example paths need an intentional migration when semantic activation is added.

### Identity is currently four different things

| Identity | Current semantics | Required action decision |
|---|---|---|
| `View::widget_id` | Focus/overrides, intended stable across frames; focus manager warns and ignores duplicate focusable IDs (`focus.rs:37-43`) | An addressable action target must reject ambiguity and be validated against the currently presented app state. |
| `View::native_key` | Defaults to widget ID, but may differ; keyed reconciliation is sibling-scoped (`widgets/mod.rs:301-318`, `backend/native_diff.rs:1-12`) | Keep scoped rendering keys. Do not assume global uniqueness or equality to widget ID. |
| Native `RegionId` | Retained ID within one native session | Use `(owner epoch, native session, region, expected presentation generation)` for external stale-target checks. Do not address by a bare number across reconnect. |
| AccessKit `NodeId` | Hash of stable session/region names, e.g. `region_node_id` at `a11y/tree.rs:595-596` | Keep a reverse routing table when publishing the tree. Hashes are not decodable routing data. |

`RetainedTree::region_with_key` chooses the oldest region with a matching key globally at `crates/ulttui/src/backend/native_diff.rs:84-91`. `NativeBackend::report_focus` looks up the `widget_id` through that method at `crates/ulttui/src/backend/native.rs:269-285`. Thus a custom native key that differs from widget ID, repeated keys under different parents, or an id-bearing wrapper around another id-bearing region needs explicit handling before named actions use this lookup. Existing duplicate-key behavior treats a later duplicate sibling as unkeyed at `native_diff.rs:559`; it is a rendering fallback, not safe action resolution.

`name_new_regions` publishes native keys as metadata named `id` at `crates/ulttui/src/backend/native.rs:35-48`. Preserve this existing inspection meaning. Add explicit widget-target metadata/retained mapping if necessary rather than silently changing every existing `id` to another namespace.

### Recommended app-owned design

Introduce a typed, capability-gated targeted action carrying target widget ID, action kind and optional bounded value. Begin with focus and activate. Set-value, toggle, expand, select and scroll need their own semantics, validation and result contracts. Deriving permission to invoke them from accessibility role alone is insufficient.

The app runner owns a registry from the last committed rendered view: target widget ID, native region where applicable, supported actions, visibility/enabled state and presentation generation. This is an immutable description; application state still changes only through `Application::on_event`/`update`. A handler declaration or an explicit application capability list makes actions opt-in. An app that never handles activation must not advertise an actionable button merely because it rendered `Role::Button`.

Convert a targeted native action to an application event before generic focus annotation/remapping. Keep its target even if local focus changed while it was in flight. Ordinary typing can retain the current local-focus contract initially. Preserve region identity in pointer events as optional target metadata and preserve coordinate meaning separately; do not attach region-local coordinates to an API documented or assumed as full-screen coordinates without a transform.

Do not translate every semantic action into a simulated Enter or mouse press. Enter on an input can submit rather than activate; apps may intercept it; disabled controls and hidden overlays need rejection. Shared app-owned helpers can convert keyboard activation, semantic activation and hit-tested pointer activation into one app message once their equivalence is explicit. Start by migrating `counter-app` and the Luau hello example to the same handler. Preserve existing keyboard shortcuts and screen interception order.

### Concrete change map

| File / symbol | Required change |
|---|---|
| `ulttui/src/widgets/mod.rs::View`, `focus.rs` | Add declarative action support/target identity to view descriptions or a parallel app-owned registry. Keep `for_each_child` and override hiding in the registry walk. Decide duplicate target behavior; use an error or non-actionable target, never first-match mutation. |
| `ulttui/src/backend/native_diff.rs::RegionSnapshot`, reconciliation notes | Retain semantic target separately from scoped native key. Expose region-to-widget lookup for input conversion. Preserve keyed prepend/reorder identity and deletion semantics. |
| `ulttui/src/backend/native.rs::convert_input_event`, `report_focus`, `name_new_regions` | Keep target identity in application-facing native events, correctly distinguish native key from widget ID, and reject unknown/stale targets. Avoid focus feedback loops when the terminal asks to focus a region. |
| `ulttui/src/event.rs`, `app.rs::prepare_event` and event loop | Add targeted action/result path without overwriting its target from current focus. Deliver through active screen/modal interception and the owning Application. Define whether events are accepted/rejected/completed and how an app reports its result. |
| `ultty-protocol/src/{events,messages,codec,lib}.rs` | Add a capability-gated semantic-action message, bounded arguments, request/result correlation if external callers await an answer, and optional action declarations. Update normative codec, not just Rust enums. Root owns the control request endpoint. |
| `ultty/src/protocol/session.rs`, session command/frame handling | Validate action capability, target existence, committed scene/generation, hidden/disabled state and session authority. Queue the event to the right app; report rejection without pretending the state changed. Do not reuse pane input permission as arbitrary spawn permission. |
| `ulttui-lua/src/{runner,script,node}.rs` | Map actions into documented script messages; preserve app-owned input editing and grants. Validate unique actionable widget targets, not just current duplicate button/input IDs. Lua reload must invalidate stale action generations while keeping agreed stable targets. |
| `examples/counter-app`, `examples/lua/hello.luau` | Explicitly handle the new activation route and keyboard route through the same state mutation. An event type existing in an enum is not a live feature. |

### ANSI compatibility

`FocusManager::register_view_tree` is already backend-independent at `crates/ulttui/src/focus.rs:46-62`; preserve it. The ANSI backend draws the view through ratatui and does not retain a region target/geometry map at `crates/ulttui/src/backend/ansi.rs:74-82`. Native region actions can work with capability negotiation while ANSI keyboard use continues, but generic named external control of arbitrary ANSI apps is not available.

If pointer-to-semantic activation is required in both modes, record actual rendered hit boxes through the ANSI widget render helpers and include scroll, clip, z-order, border and override-hidden behavior. Recomputing a second guessed layout in the action handler risks disagreeing with the screen. A fully general ANSI geometry registry is a separate slice; the first keyboard activation parity gate can avoid that expansion.

Current `parity.rs` tests compare rendering and focus registration, not complete event dispatch. Extend `focus_registration_and_focused_button_style_match_in_both_modes`, `a_focused_input_sends_its_typed_text_and_a_name_natively`, `widget_ids.rs`, and App event-order tests. Add wire event tests where target A remains A after local focus moves to B; deleted/disabled/hidden A is rejected; two nested rendering keys coincide but actionable IDs remain unambiguous; an override hides a target; a modal intercepts actions; no Enter is delivered twice.

Minimal live gate: `counter-app` and Luau hello increment exactly once on a targeted activation, preserve Tab/Shift+Tab and Enter, and keep the same result through sibling prepend, resize, and split focus changes. Prove the native wire path with the app's resulting count and event acknowledgment. Prove ANSI keyboard parity on a real PTY. Claim pointer parity only after clicking the actual rendered control in both backends, including clipped/reordered layouts. Do not use a mocked `Event::Click` alone as that proof.

## A2. Route accessibility actions through the same contract

The AccessKit adapter handles `ActionRequested` only by logging at `crates/ultty/src/a11y/adapter.rs:90-92`. `AppHandler::user_event` forwards accessibility events to the adapter but does not drive runtime update/dispatch afterward at `crates/ultty/src/app/mod.rs:1693-1697`. Native nodes receive roles/state/bounds in `crates/ultty/src/a11y/scene.rs:639-705`; no `add_action`/action advertisement was found in the a11y module. This is a read/announcement implementation, not a completed action implementation.

Changes:

- Make `Adapter::handle` return or enqueue a typed action intent. Keep AccessKit types in the adapter layer. Schedule runtime work after a valid intent.
- Build an immutable reverse map with each published tree from `NodeId` to tab/pane/native-session/region/generation and supported actions. Use only nodes actually included in that published tree; native tree collisions/truncation/hiding can omit nodes.
- Validate tab/pane focus operations locally, and route native app actions through A1. A stale accessibility node or deactivated tree must not target a newer app occupying the pane.
- Advertise only supported actions. Start with focus and button activation for participating native apps. Set value, text selection, table/select operations and terminal scrollback need separate app/adapter contracts. Never synthesize a password value query or expose secret text through action errors.
- Preserve the existing no-active-reader fast path in `app/accessibility.rs`. The action map should be created with tree publication, not rebuilt or hashed on every frame while inactive.

Tests belong in `a11y/adapter.rs` for intent decoding, `a11y/scene.rs`/`tree.rs` for advertised actions and reverse-map consistency, and app tests for routing. Extend `scripts/e2e/accessibility-atspi.sh` with an actual AT-SPI action invocation and observable counter result. Keep existing roles/names/focus/selection and live-announcement tests. Record manual Orca and VoiceOver results in `docs/accessibility-test.md`; its results table currently has no manual runs. AT-SPI inspection and a generated tree do not prove usable screen-reader interaction.

## R1. Preserve native app connections across viewer detach

Two existing disconnect paths must remain distinct:

- Full scene: `NativeBackend::process_pending_messages` clears transport and enqueues Quit on EOF at `crates/ulttui/src/backend/native.rs:541-545`; `backend/native/transport.rs::disconnect_becomes_quit_event_instead_of_busy_loop` tests this.
- Inline console: `Session::pump` checks `UlttyClient::is_closed` at `crates/ulttui/src/inline.rs:171-198`; `check_session`/`lose_session` changes to fallback, drains pending events/input, and makes live cards final at `inline.rs:607-638`. `a_lost_session_falls_back_and_cards_become_final` tests this. It does not recreate an old native session.

The persistent runtime should retain app-side native sockets, protocol session IDs, committed scenes, surface placement, focus/scroll/streams and query handling while GUI attachments come and go. A new GUI attaches to that owner; Ulttui applications should see neither EOF nor a new `SessionReady` merely because the old GUI disappeared. NativeSession is currently driven by `App::update`, including polling, pairing, viewport synchronization and keepalives. Terra's owner extraction must move those mechanics, not only retain a PTY fd.

`NativeBackend::render` advances `self.retained` before sending BeginFrame/ops/EndFrame at `native.rs:239-264`. An automatic reconnect to a fresh server would otherwise send only later diffs against a tree that server never received. Full republish, subscription re-registration, focus restoration and invalidation of region-layout caches would be necessary. Inline surfaces are more difficult because their old session IDs are encoded in anchors and their fallback rows already occupy scrollback. Do not silently replay old anchors as fresh text.

Therefore do not change client EOF behavior during pane/status/action work. If application-to-owner reconnect is later required for owner upgrades, design a distinct negotiated resume protocol with epoch, committed-frame cursor, snapshot/full-redraw behavior and rejection. It cannot preserve arbitrary process memory after the owner itself killed its child processes. An in-band idle timeout also remains an app-session timeout, not a viewer detach detector.

Native detach gate for runtime work: keep a shell, a full-scene Ulttui app and an inline console running; kill only the GUI; verify identical PIDs, native session IDs and progressing state; attach another viewer during an incomplete frame. Present only the previous complete scene until commit, preserve ordered input and surface copy text, and reject stale action generations. Include slow viewer backpressure and controller-size changes. Add session-owner tests and real GUI smoke; the existing EOF/fallback tests should still pass when the actual owner closes the app link.

## Suggested review units and stop rules

| Unit | Smallest coherent files | Required decisive result | Stop / compatibility boundary |
|---|---|---|---|
| N1 pane in-band | App pump, server in-band APIs/state, pane binding/tests/spec | Two confirmed in-band apps in one tab never cross replies/input/liveness | Keep current fallback until true owner identity is wired throughout. No wire change. |
| N2 inline splits | Server pairing, pairing rules, pane renderer/cache, a11y snapshot, scripts/spec | Two panes' cards render, copy, scroll, resize and freeze independently | Stop if only pairing works. Preserve noninteractive inline contract. |
| S1 status core | VT parser/store/handler, event-loop snapshot/generation, lifecycle tests | Invalid report never partly mutates; lifecycle survives lost notifications | Do not reuse lenient decoder or GUI-only store. No daemon needed. |
| S1 status presentation/events | Manager projection, app refresh, typed query/events, scripts | One pane's real report reaches query, indicator and subscriber consistently | Preserve legacy override semantics; no implicit activity inbox. |
| S2 reporting helper | Cmd/runtime, backend owned writers, reusable encoder, example/tests | Same source reports through native socket, in-band and ANSI without interleaving | Avoid new stdin readers or required local control credentials. |
| A1 identity/activation | View descriptions, retained mapping, app event route, protocol capability, two examples | Named activation remains on A while focus moves to B; exact one update | Reject stale/ambiguous/unsupported actions; no role-implies-handler shortcut. |
| A2 assistive actions | Adapter intents/reverse map, tree action support, shared A1 dispatch | Real AT-SPI focus/activate changes the intended app once | Advertise only implemented actions; manual screen-reader claim remains gated. |
| R1 native detach | Persistent-owner extraction plus viewer snapshot, no app reconnect shortcut | GUI death leaves app-native link and committed state intact | Owner loss remains different; no implicit checkpoint/restart claim. |

Native message additions require a capability/version decision under `docs/adr/0011-pre-1-0-protocol-evolution.md:3`, corresponding codec tests and `CAP_ALL_KNOWN`/client-default updates in `crates/ultty-protocol/src/lib.rs:105-148`. Update compatibility, malformed, roundtrip and fuzz targets alongside the actual codec. Do not enable a default capability bit in clients before terminal and client routing are complete. Adding Rust enum variants also affects downstream exhaustive matches; document the pre-1.0 API change.

No Tailscale integration is needed for N1, N2, S1, S2, A1 or A2. They improve local and ordinary SSH paths. Validate Tailscale as an optional host reachability layer after those user paths are correct. Transport identity cannot recover a discarded native region target, authorize an app mutation, or turn a lost session into a resumed one.
