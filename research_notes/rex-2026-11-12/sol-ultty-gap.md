# Ultty and Ulttui against Rex's target workflows

Date: 2026-11-12. Source commit inspected: `b75af7ac35423e7103a1a9065ce64f118ae0d428`.

This memo covers the current repository and a proposed direction. Rex's externally verified behavior belongs to the accompanying source-research memo. Here, persistent server-owned sessions, multi-client attach, native splits, program activity over OSC 7501, and scripting are comparison targets supplied by the research task. They are not independently verified Rex claims in this memo.

No code was implemented, built, benchmarked, or driven through the GUI. Source and existing dated measurement notes support the findings below. Proposed gates are unrun. `CLAUDE.md`, `AGENTS.md`, `docs/NORTH_STAR.md`, and the current agent-TUI plan were read. Existing untracked `.pi/`, `reports/`, and other research material were left alone.

## Recommendation

Keep Ultty's current rendering, VT core, and semantic TUI model. First finish the pane boundaries and automation semantics already in place. Then, if persistence is a product priority, move live terminal ownership into an optional local session process. Treat remote attach as a separate decision after local detach and reattach works for both ANSI and native applications.

Ultty already has much of the visible machinery one might otherwise propose after seeing Rex: nested splits, divider dragging, directional focus, pane-targeted automation, status indicators, event subscriptions, a packaged MCP server, Luau tools, inline native blocks, keyed scene reconciliation, and accessibility semantics. The largest missing capability is process lifetime independent of a GUI window. The next largest is a viewer protocol with replay, input authority, and explicit sizing rules. The existing app-to-terminal native protocol is not that viewer protocol.

The strongest way to improve on the target experience is to preserve structured regions, status, command boundaries, and readable accessibility information across attach, rather than reducing every application to an opaque terminal image. This is a proposed advantage. No comparative user study or performance result proves it yet.

## Current ownership and lifetime

The GUI `App` owns `TerminalManager` next to its `Window`, GPU `Surface`, and `Renderer` in `crates/ultty/src/app/mod.rs:130-141`. The manager owns `ManagedTerminal` values, and each owns a `TerminalEventLoop`, pane title, explicit status, and OSC progress status in `crates/ultty/src/terminal_manager.rs:92-105`. Its constructor spawns a shell and passes the `Child` into the event loop at `terminal_manager.rs:117-134`.

Each `EventLoop` owns one PTY child. It creates a dedicated reader thread with a shared terminal, mutex, condition variable, wake state, and ordered output queue at `crates/ultty/src/event_loop.rs:347-399`. The parser and terminal replies run in that reader thread, independently of GPU frames, as described and implemented at `event_loop.rs:1-23` and `event_loop.rs:701-725`.

Closing a window calls `save_session` and exits at `crates/ultty/src/app/input.rs:25-36`. Dropping an event loop sets its stop flag, wakes its poller, and joins the reader at `event_loop.rs:691-698`. Dropping a still-live `Child` sends TERM, waits briefly, and then KILLs it if necessary at `crates/ultty/src/pty/child.rs:251-274`. This proves that current ownership has no detach lifetime. Keeping a window hidden is a different operation from closing its process.

Current persistence saves layout metadata. `SessionData`, `TabData`, and `TerminalData` hold tab order, title, cwd, focused pane, and the complete weighted split tree at `crates/ultty/src/session.rs:10-62`. They contain no live process handle, VT grid, alternate screen, native region tree, native stream state, or reconnect cursor. Restoration calls constructors that spawn new shells at `crates/ultty/src/terminal_manager.rs:1293-1304`, then reconstructs the split tree and active tab at `terminal_manager.rs:1330-1357`. `restore_session` is disabled by default at `crates/ultty/src/config.rs:884-898`.

Consequences:

- Say "restore layout and directories" for the shipped behavior. "Persistent sessions" would imply a stronger guarantee than this code provides.
- Save/restore must remain useful as layout recovery even if live detach is added. A surviving session process, a restarted session process, and a restarted machine have different guarantees.
- Native app lifetime must move with PTY lifetime. Otherwise a shell might survive a window while its Ulttui child receives native EOF and quits.

## Capability map

| Target workflow | Current Ultty state | Remaining work |
|---|---|---|
| Work survives closing the UI | Window owns PTYs; child drop terminates processes | Separate session owner, explicit detach, attach snapshot |
| Open the same work from another client | Multiple app connections and control subscribers exist; GUI viewers do not | Viewer identity, read/write authority, snapshots/deltas, resize policy |
| Remote attach with recovery | Native apps can reach a local Ultty over an existing SSH PTY using APC framing | Server-side process lifetime, authenticated attach path, disconnect/replay contract |
| Native splits | Nested terminal split tree and socket-native apps in panes already work | In-band sessions and inline surfaces in split panes remain gated off |
| Program activity/status | `PaneStatus`, OSC 9;4, OSC 133, tab urgency, CLI/MCP already exist | OSC 7501 compatibility if useful, report provenance, lifecycle and subscriptions |
| Scripting and tools | CLI, typed JSON, batch, event stream, MCP, hot-reloaded Luau child tools | Honest command execution semantics, idempotent launch operations, event continuity |
| Semantic inspection | Native regions include content, layout, focus, semantics; password values omitted | Semantic actions, session-event coverage, stable attach semantics |
| Accessibility | Protocol semantics and AccessKit tree are implemented, enabled by default | Assistive action handling and recorded Orca/VoiceOver behavior remain incomplete |

## Splits are substantially present

`Tab` owns an ordered set of pane IDs and a `PaneLayout` at `crates/ultty/src/terminal_manager.rs:169-184`. `PaneLayout` supports split, remove/collapse, rectangles, divider hit testing, divider dragging with a minimum ratio, and geometric directional navigation at `crates/ultty/src/pane_layout.rs:119-225`. This is a native tree with mixed directions, not only one flat horizontal or vertical split.

The window has split-right/down chrome buttons at `crates/ultty/src/tabs.rs:173-186`, corresponding main-menu entries at `tabs.rs:68-100`, and configurable split and directional-focus bindings at `crates/ultty/src/config.rs:604-614` and `config.rs:665-670`.

Socket-native scenes are now bound to the pane whose foreground process group matches the connecting app. The binding and viewport sync are implemented at `crates/ultty/src/app/native_panes.rs:10-49`. `MouseCapture` keeps a drag's move/release events with the pane that took the press, then clears it if the pane disappears or the window loses focus at `native_panes.rs:53-102`. `ulttyctl split` can launch a program into a targeted split through `crates/ultty/src/app/control.rs:812-865`.

Do not inherit the older claim that native apps disappear in split tabs. ADR 0025 slices 1-5 are marked done, including pane rendering, input, accessibility, inspection, screenshots, and `lua open --split`, at `docs/adr/0025-native-sessions-in-panes.md:179-205`. Current code matches the new ownership model.

Two boundaries remain real:

1. `pump_in_band` accepts native data only when a tab has one pane and ends a running in-band session if its tab is split, at `crates/ultty/src/app/update.rs:46-118`. Therefore an Ulttui program over SSH in a split currently falls back to ANSI. Local socket-native support does not prove remote-native split support.
2. Inline pairing still receives a `single_pane` flag and considers a surface shown only for a single-pane active tab at `app/update.rs:129-156` and `crates/ultty/src/protocol/server.rs:695-713`. ADR 0025 explicitly defers split-pane in-band, split-pane inline surfaces, native lifecycle events, and per-pane screenshot crops at `docs/adr/0025-native-sessions-in-panes.md:206-208`.

Improve this by carrying the already known originating `TerminalId` through in-band and inline pairing, with all state keyed by the owning pane. The terminal receives the in-band hello on a specific PTY; it should not guess ownership from currently focused UI state. Keep foreground-group checks, pane-local coordinates, and viewport generation rules.

Drag-to-reparent panes, pane zoom, stable named workspaces, and layout transactions were not found in the inspected manager/control interfaces. They are possible later UX additions, not evidence that the existing split implementation needs replacement.

## The native protocol is an app protocol

The terminal retains a semantic region tree. Apps send constraints, style, content, appends, stream lifecycle, metadata, semantics, parent changes, frame boundaries, focus, and subscriptions in `crates/ultty-protocol/src/messages.rs:20-147`. Terminal replies include session readiness, computed layout, resize, and input at `messages.rs:150-184`.

`NativeSession` owns its scene, Taffy layout, link, negotiated version/capabilities, focus, scroll state, active streams, dirty flags, and viewport at `crates/ultty/src/protocol/session.rs:101-175`. Per-tab sockets and same-user peer checks are part of the existing trust boundary at `crates/ultty/src/protocol/server.rs:165-185`; pane/foreground input rules are documented at `server.rs:1-24` and implemented in the pane-binding path.

This gives an unusually good starting point for reconnecting a viewer because the terminal has structured UI state. It still does not supply a viewer attachment protocol. In particular:

- `ClientHello` has version and requested capability bits, with no resume token, viewer identity, or state cursor, at `messages.rs:23-26`.
- `SessionReady` creates a session/root/viewport context at `messages.rs:157-163`. That session is an application connection, not a persistent workspace or attached viewer.
- EOF causes the Ulttui native backend to clear the transport and enqueue `Event::Quit` at `crates/ulttui/src/backend/native.rs:541-545`. Initialization can fall back to ANSI at `crates/ulttui/src/backend/mod.rs:69-95`; that is not mid-session reconnect.
- A protocol server's `active_tab` currently represents one visible tab with input/clipboard privileges, at `protocol/server.rs:82-86`. Two viewers require per-viewer visibility and input state rather than another write to this singleton.

Use a distinct attach protocol if persistence proceeds. Keep app-to-session semantics authoritative; expose bounded versioned terminal/scene snapshots plus ordered updates to viewers. A detached viewer should not disconnect the application from the session process.

For a first design, let the persistent owner keep a canonical grid and native viewport chosen by one controlling attachment. Other attachments render that size, with letterboxing or scrolling where necessary. Supporting independently laid-out native scenes at several sizes means duplicating layout and reconciling app feedback, hit testing, focus, and dimensions. That is materially more work and should be a separately demonstrated need.

## Remote transport and recovery

Current detection uses the tab Unix socket only when the app's controlling terminal matches `ULTTY_TTY`. This prevents an inherited socket from bypassing tmux, a nested terminal, or a recorder, at `crates/ultty-client/src/detect.rs:1-15` and `detect.rs:63-83`. Without a usable socket, a native probe can traverse SSH/sudo/containers through in-band APC frames. Detection skips tmux/screen and unsafe TERM values at `detect.rs:36-60`.

In-band sessions confirm hello, expire if unconfirmed after 2 seconds, expire after 10 seconds idle, and restrict clipboard access because any PTY writer may be remote, at `crates/ultty/src/protocol/server.rs:11-24` and `crates/ultty/src/protocol/session.rs:83-95`.

This solves native UI transport through a live remote shell. It does not preserve the remote program when SSH loses its controlling session, restore a dropped TCP stream, create another client for the same session, or recover terminal state after loss. None of those guarantees follows from APC framing or heartbeat alone.

Prefer a separately invoked remote helper over an SSH channel before adding a network listener to the GUI. This can reuse SSH authentication and keep Ultty's own transport local/stdio, though it still adds a remote-support product and must be described honestly. A remote attach must authenticate the session endpoint and define which capabilities it receives. Do not expose today's all-powerful local control token as a general shared network credential.

The project direction needs an explicit decision here. README's current promise is no account, no telemetry, no network calls and no networking code at `README.md:129-131`. The agent plan repeats the local promise at `docs/agent-tui-plan.md:21`. `docs/NORTH_STAR.md:25-35` keeps the native protocol semantic and excludes generic desktop remoting. An optional local session process fits those constraints more readily. A native TCP/TLS service is a scope change. Merely calling it opt-in does not erase that change.

## Activity reporting should converge on one model

`PaneStatus` already contains state, short label, progress, and attention at `crates/ultty-protocol/src/control.rs:287-312`. Sanitization bounds labels to 64 characters, strips controls, rejects non-finite progress, and clamps the remainder at `control.rs:314-333`. Its urgency order puts errors and waiting panes ahead of working/done/idle at `control.rs:336-345`.

OSC 9;4 maps into this status model at `crates/ultty/src/terminal_manager.rs:71-89`. Explicit control status takes precedence over OSC progress at `terminal_manager.rs:108-112`. A status request updates the pane, can mark an unfocused window urgent, and refreshes the tab bar at `crates/ultty/src/app/control.rs:968-986`. Each tab shows its most urgent pane's status while the title band shows the focused pane's at `crates/ultty/src/app/mod.rs:938-952`.

The parser handles OSC 9;4 and OSC 133 at `crates/ultty-vt/src/ansi/osc.rs:125-142`. No OSC 7501 handler was found in the parser source. Current control events include pane lifecycle, command start/finish, bell, notification, title, and process exit at `crates/ultty-protocol/src/control.rs:433-457`. They do not include status changes or native-session lifecycle.

An OSC 7501 adapter, if Rex's exact vocabulary and interoperability justify it, should normalize into the same status type rather than build another tab indicator. Keep distinctions that matter:

- A program-reported "waiting" state is evidence from that program. A quiet PTY is only absence of output. Do not infer a request for user input from quietness.
- A shell command exit comes from OSC 133 and a child exit comes from process observation. They are different events.
- Status should have an owning source, generation and clear/lifetime rule. Otherwise an old "working" report can linger after a child exits or an unrelated foreground command starts.
- Native widget `busy`/`live` semantics describe parts of an application. Pane status summarizes a process or job. An automatic conversion needs an explicit precedence policy to avoid taking the most recently rendered spinner as the entire pane's job state.

The smallest valuable improvement is an in-band, bounded status report with a compatibility adapter and `status_changed` subscription event. It would work from remote programs without access to `ulttyctl`'s local socket and give a Luau/Ulttui workspace overview direct event data.

## Automation already has a useful foundation, with concrete gaps

The control envelope carries typed request fields and machine-readable responses at `crates/ultty-protocol/src/control.rs:31-43`. Pane-targeted text, key, raw input, launch, split, text queries, regions, command output, search, waits, and subscriptions are defined at `control.rs:48-206`. The CLI supports JSON output and wraps authenticated requests at `crates/ultty/src/bin/ulttyctl.rs:53-105`. `ulttyctl mcp` is a Rust stdio MCP server, not the old Python shell wrapper, at `ulttyctl.rs:28-35` and `crates/ultty/src/control/mcp.rs:31-65`.

`WaitFor` supports text, command finish, bell, and quiet duration at `control.rs:266-279`. The app fixes a wait's target when the request arrives at `crates/ultty/src/app/control.rs:455-458` and observes events at `control.rs:369-399`. This provides a better automation base than repeated screenshot or title scraping.

Current inspection can expose a native region's content, layout, focus and semantics. Password input values are omitted at `crates/ultty/src/protocol/session.rs:43-57`. NativeScene's current schema includes session, in-band transport, focused region, tree, and surfaces at `crates/ultty-protocol/src/control.rs:527-543`.

There are several source-proven limitations worth fixing before wider automation:

1. `RunCommand` joins `args` with spaces and writes a line into the existing PTY at `crates/ultty/src/app/control.rs:908-924`. The typed JSON envelope preserves argument boundaries over transport, but this command path does not preserve them as process argv. It is shell input. It also assumes the foreground program accepts shell commands. Define separate operations for typing shell text and spawning an argv-preserving process, reusing the current new-tab/split spawn paths for the latter.
2. Input handlers ignore `event_loop.send_input` errors and still reply `Ok` at `control.rs:717-719`, `control.rs:755-757`, and `control.rs:923-924`. The PTY output queue is bounded and can fail. A success receipt should mean acceptance into that queue. It should not imply a command finished.
3. `Batch` executes every member in order and collects results at `control.rs:1178-1183`. It has no transaction or rollback guarantee. Keep that contract explicit; add declarative workspace reconciliation later only if repeated scripted setup needs it.
4. Events have no sequence/cursor in `ControlEvent` at `control.rs:433-457`. Slow subscribers are disconnected once backlog exceeds 1 MiB; there are at most 64 subscribers at `crates/ultty/src/control/server.rs:31-36` and `server.rs:432`. These are good bounds, but reconnecting tools need a resnapshot or a bounded replay cursor to recover missed events.
5. Pane IDs are stable during manager lifetime, but `TerminalId`/`TabId` are process-local counters at `terminal_manager.rs:55-68` and `terminal_manager.rs:153-166`; restoration assigns new ones at `terminal_manager.rs:1282-1295`. Persistent session addressing needs an owner epoch plus identity so a stale script cannot accidentally address a newly created pane after restart.

For scripting, `ulttyctl lua open` already checks a script and command grants before opening it at `crates/ultty/src/bin/ulttyctl.rs:301-307` and `ulttyctl.rs:509-519`. `docs/adr/0024-lua-in-ultty.md` chooses a separate `ulttui-lua` process, fixed grants, hot reload, native drawing, and ANSI fallback. Retain that isolation. An embedded scripting VM in the terminal or session process adds failure coupling without making these workflows clearer.

## Ulttui can provide the structured workspace UI

The application owns interactive state. `Application::update`, `on_event`, and `view` are explicit at `crates/ulttui/src/app.rs:113-151`; the view tree is rebuilt as output each pass. The native backend retains regions and reconciles keyed siblings rather than relying on traversal IDs at `crates/ulttui/src/backend/native_diff.rs:1-12`. `View::native_key` defaults to stable `widget_id` at `crates/ulttui/src/widgets/mod.rs:301-318`. A proposal to add keyed diffing would duplicate existing work.

The widget set already includes status badges, progress, spinners, trees, tables, tabs, inputs, dialogs, and scroll views. `DockLayout` can arrange top/bottom/left/right/center regions at `crates/ulttui/src/widgets/dock.rs:10-20` and `dock.rs:48-70`. A workspace overview can be an ordinary Ulttui/Luau program consuming pane/status/command events. It need not become a permanently coupled sidebar in Ultty's GPU chrome.

That UI should use stable session/pane keys, keyboard commands, readable labels, and explicit host identity. Show "local / attached / detached / reconnecting / unavailable" as attachment state, separately from "working / waiting / done / error" as job state. A remote host name plus cwd prevents an identical directory path from obscuring where a command will execute. Keep status changes sparse and announce only consequential waiting/error transitions through live semantics.

Accessibility is already deeper than a role-string placeholder. The protocol carries semantics; the bridge maps scenes into the tree, and widget semantics are sent when they add information at `crates/ulttui/src/widgets/semantic.rs:16-30` and `widgets/mod.rs:266-273`. The adapter is on by default at `crates/ultty/src/config.rs:878-898`. It does no tree gathering while inactive and publishes native live appends only for shown bound sessions at `crates/ultty/src/app/accessibility.rs:23-74`.

Limits remain visible. ANSI mode ignores the protocol semantics at `widgets/mod.rs:266-273`. AccessKit `ActionRequested` is logged as unhandled at `crates/ultty/src/a11y/adapter.rs:90-92`. Recorded Orca and VoiceOver work is still incomplete in `docs/adr/0021-accessibility-semantics.md:386-388`, `:440-441`, `:657-659`. A semantic tree and AT-SPI smoke are useful evidence; they do not prove a person can navigate the entire workflow with a screen reader. A "better than Rex" accessibility claim requires that user-path gate and comparative evidence.

## Resource and performance implications

The core data layout is already improved. Screen cells occupy a shared allocation and scrolling rotates 16-bit row slots at `crates/ultty-vt/src/grid/screen.rs:1-14`. Eight-byte packed cells separate common inline styles from interned styles at `crates/ultty-vt/src/grid/packed.rs:1-20`. History uses 256-row pages with ASCII, codepoint and packed-cell forms, trims blank tails, reuses evicted pages, and reads rows in place at `crates/ultty-vt/src/grid/history.rs:1-36`. Do not propose another row-object-to-page migration from an old report.

PTY reads hold the parser lock in 64 KiB chunks; plain output wakeups are throttled to 4 ms; pending events, in-band data and outbound bytes are bounded at `crates/ultty/src/event_loop.rs:41-64`. An occluded window stops drawing/requesting frames while parsing continues at `crates/ultty/src/app/pacing.rs:19-20` and `pacing.rs:31-65`. Output from hidden tabs does not request a redraw at `crates/ultty/src/app/update.rs:295-301`. Native buffers are cached at `crates/ultty/src/renderer/native_render/cache.rs:8-41` and sessions track content/layout/scroll dirty state at `crates/ultty/src/protocol/session.rs:135-154`.

Persistent sessions add retained terminal/history/scene memory even when no window is open. Multi-client attach adds per-viewer queues, viewport caches, and rendering resources. Keep one canonical parsed VT state per pane. Avoid parsing the same byte stream independently for each viewer as the default architecture, because query replies, resize, inline anchors, and command marks then have multiple inconsistent owners.

The present one-reader-thread-per-PTY design is easy to isolate. Keep it for the first persistence experiment; measure its thread count and memory at 1, 8, 32 and 128 detached panes before replacing it with a shared reactor. The reactor is a separate optimization with its own fairness and wakeup risks.

Bound total history/scene memory per session owner as well as per pane. Existing native limits are 4 MiB text per region and 64 MiB text per session at `crates/ultty/src/protocol/session/content_limits.rs:14-22`, 10,000 queued frame operations and 32 MiB queued frame bytes at `protocol/session/frame.rs:11-15`, plus region/depth bounds at `crates/ultty/src/scene/tree.rs:9-16`. Multiplying these valid per-object maxima across many unattended panes still needs an owner-level budget.

A daemon that links the current `ultty` crate directly brings its unconditional `wgpu`, `winit`, text, clipboard and AccessKit dependencies at `crates/ultty/Cargo.toml:22-92`. `ultty-vt` already has a small I/O-free dependency set at `crates/ultty-vt/Cargo.toml:1-17`. Separate the process/runtime owner from renderer/platform services only when needed for the daemon's shipped dependency closure. Validate a real headless build and runtime; counting lockfile packages does not establish the daemon's cost.

Existing dated performance notes are baselines to reuse, not fresh results from this review. `docs/perf/2026-11-11-t4-three-way.md:15-21` fixes the compared commit and T4/headless X11/Wayland setup. It reports X11 idle key-to-pixel 3.80 ms median/4.07 ms p99, ASCII 137 MiB/s and 149 MiB with 32 tabs at `:48-61`. It also reports 114 idle switches/s in that NVIDIA setup, so a blanket zero-wake claim is false there. The page-storage note explicitly corrects frame-quantized `pty_flood` results at `docs/perf/2026-11-11-page-storage.md:106-117`; use the corrected harness before making latency/throughput comparisons. None of these notes compares Rex or measures detached session cost.

## Options

| Option | Benefit | Cost and limitation | Recommendation |
|---|---|---|---|
| Finish pane/status/automation boundaries within current process | Immediate gain to local and SSH workflows, limited scope | Window close still ends processes | Start here |
| Keep shells in tmux and improve Ultty integration | Mature persistence and remote recovery with little new process ownership | Current detection deliberately falls back to ANSI through tmux; richer native UI needs separate work | Valid incumbent/control and user option |
| Optional local session process, GUI as an attachment | True survival across GUI loss; one authoritative terminal and native scene | Requires extraction, snapshot/delta protocol, authority and sizing rules | Preferred if native persistence is a chosen product goal |
| Full network/collaboration server now | Addresses broad multi-host/multi-client ambition directly | Largest authority and recovery change; conflicts with local promise; delays proving local correctness | Defer |

Tmux is an incumbent control, not an Ultty defect. If the main job is durable remote shells rather than preserved semantic native apps, it may already be the best answer. The extra session process pays for itself only if users need the native scene, inspection, inline surfaces, and automation to survive attachment changes.

## Proposed stages and acceptance gates

These are proposed bounded units, not approval to implement. Each unit should freeze its baseline ref and inputs, retain commands/logs, and stop on the first decisive invariant failure. Normal repo fmt, clippy, workspace tests, affected end-to-end scripts, and required GUI smoke remain necessary when code changes begin.

### Stage 0: make existing claims and receipts precise

Expected payoff: prevent a large rewrite based on stale gaps and give automation an honest success contract.

Scope: refresh the capability matrix; distinguish layout restore from live detach; separate shell text from argv launch in the API; propagate input queue failures. Choose stable names for workspace/session/pane/attachment before adding more state.

Cheapest falsifier: an argument containing spaces or shell syntax through `RunCommand`, compared to the same argv launched in a new pane, and a bounded-queue refusal that currently receives `Ok`. Source inspection already identifies these paths; reproduce them before changing their contract.

Budget and stop rule: one focused regression reproduction per behavior and one contract design, no transport rewrite. Stop if the proposed operation cannot say whether it typed text, accepted input, spawned a process, or observed completion.

Acceptance: exact argv preserved for the spawn operation; text typing remains explicit; failed queue insertion returns error; old CLI behaviors and shell integration still have documented outcomes. Rollback: revert the isolated command changes; no persistent state migration.

### Stage 1: close native pane and activity gaps

Expected payoff: remote native applications and inline blocks behave consistently in the split layouts users already have; tools can see reported job state without polling.

Scope: pane-owned in-band sessions, pane-owned inline pairing, native connected/dropped events, status-changed events, one common status normalization path. Add OSC 7501 only after freezing the exact external grammar and intended mapping. Complete these as separate slices.

Cheapest falsifier: two native apps in different split PTYs, one local socket and one real SSH in-band app, then resize and close one pane. Any input, status, clip, inline anchor, or session event attributed to the other pane fails the design.

Budget and stop rule: one two-pane fixture and the existing single-pane/native-split controls first. Stop on cross-pane routing, an abandoned in-band probe leaving bytes in shell input, or a regression in ordinary ANSI editors/pagers. Do not add a daemon to solve a pane identity bug.

Acceptance: targeted input and status stay in the intended pane; split/resize/close preserve ownership; inline blocks copy and scroll correctly; no clipboard-read authority is added to in-band apps; idle work remains event-driven. Rollback: revert each pane-specific slice; retain working socket-native splits and ANSI fallback.

### Stage 2: prove local detach with an optional session owner

Expected payoff: shell and Ulttui jobs survive GUI crash/close without abandoning native semantics. This is the decisive experiment for whether native persistence is worth its complexity.

Scope: factor a runtime owner with no window/GPU requirement; it owns PTYs, parser/grid, command marks, status, native application connections and retained scenes. The GUI owns fonts, GPU, OS clipboard, accessibility adapter and local presentation. Add a minimal local attach protocol with owner epoch, pane IDs, snapshot generation, bounded delta queues and explicit detach/terminate commands. Start with one controlling GUI attachment.

Keep Taffy/layout ownership explicit. For the first version, use the controlling attachment's cell viewport as canonical. The owner must still process native application messages while detached, retain the last complete frame, and answer protocol requests without a GUI update loop. GPU glyph caches and platform clipboard access stay in the viewer.

Cheapest falsifier: run one shell and one streaming native app, destroy the GUI process, verify identical child PIDs and progressing job state, then attach a fresh GUI and compare text, native region identities, scroll state, command marks and pending status. Native app quit on detach, duplicate terminal replies, or missing scene state kills this design before remote work.

Budget and stop rule: one process boundary, one controller, local Unix transport, two apps, and one reconnect. No history-on-disk or machine-reboot guarantee. Stop if the owner still depends on a display server, GUI frame pumping, or native client restart to recover.

Acceptance: same child processes survive abrupt viewer loss; complete snapshot plus subsequent updates yields current state; unfinished native frame is never presented as complete; explicit terminate reaps processes; local control waits/subscriptions still work detached; daemon starts without DISPLAY/Wayland or graphics driver initialization. Measure real closure, RSS, thread count, idle wakes and input latency against the in-process baseline and tmux control. Choose allowed regressions before running. Rollback: keep persistence opt-in and restore the in-process launcher path; old layout JSON remains readable.

### Stage 3: multiple attachments and bounded recovery

Expected payoff: a second window or remote viewer can inspect the same work without stealing input or resizing it accidentally.

Scope: read-only observers first, explicit control transfer second. Keep one controller-selected canonical viewport. Give each attachment an identity and subscription cursor; define snapshot plus delta ordering, dropped-history resync, bounded buffering, and controller-disconnect policy. Viewer loss must not end native app connections.

Cheapest falsifier: two clients with different window sizes, one slow consumer, and a controller dying during a resize. If a read-only viewer writes input, a late update corrupts a newer snapshot, the slow reader blocks the owner, or repeated sizes alternate the application's dimensions, stop.

Budget and stop rule: two clients, one pane tree, one stalled subscriber, bounded disconnect/reconnect schedule. No independent per-viewer app layout, simultaneous shared typing, or collaborative editing.

Acceptance: observers cannot inject input or read/write local clipboard without the correct local authority; input control and resize ownership are inspectable; slow viewers are resynced or disconnected within a fixed memory budget; a second attach reconstructs native semantics and ordinary terminal state. Rollback: disable multiple attachments and retain stage 2 single-controller attach.

### Stage 4: remote helper over SSH

Expected payoff: reconnect to the same remote owner and native work from another machine using an existing secure transport.

Scope: separate helper/stdio attachment over explicit SSH execution. Negotiate protocol versions and optional semantics; show host/session identity; classify reconnecting separately from stopped jobs. Give remote viewers scoped read or control capability rather than forwarding a local control token. This stage requires a direction review against the README/local promise.

Cheapest falsifier: drop the SSH transport while a native app streams, keep the remote owner alive, reconnect from a clean client, and compare with the local attach control. Missing output beyond the advertised replay window must trigger a snapshot, never silent continuity.

Budget and stop rule: one host, SSH authentication, one dropped connection and one version mismatch. No built-in TCP listener, account service, discovery, browser client, or relay. Stop if recovery depends on replaying arbitrary raw bytes through an application as synthetic input or if every reconnect restarts the application.

Acceptance: same remote child PIDs; replay or resnapshot yields current state; no unauthorized input/clipboard operations; bounded bandwidth and memory under a slow link; terminal behavior remains correct at variable latency. Benchmark complete user paths, including authentication/attach latency, scrollback recovery, native inspection and steady input. Rollback: remote helper stays an optional separate component; local sessions work without it.

### Stage 5: improve the user workflow with Ulttui

Expected payoff: one view answers where work runs, which job needs attention, and whether an attachment is live. An automation client and a person can inspect the same state.

Scope: an Ulttui workspace program built from existing widgets and the public control/attach contracts. Use a keyed session/pane tree, status labels, command results, local/remote identity, explicit read-only/control modes and a reconnect indicator. Use hot-reloaded Luau for small user tools, with existing process isolation and grants. Integrate only proven high-frequency controls into terminal chrome later.

Cheapest falsifier: ask a user to identify the waiting job among several sessions, attach to it, and execute one command on the intended host using keyboard-only interaction. Repeat in native and ANSI mode, with a screen reader, then against the chosen Rex workflow when available.

Budget and stop rule: one representative workspace and one task comparison, no expanded widget catalog or dashboard of speculative metrics. Stop if host/attachment/job state is ambiguous or basic actions require reading implementation details.

Acceptance: accurate target selection, stable focus after status/reorder updates, correct event-driven state, usable ANSI layout, readable screen-reader announcements, and actual assistive actions or documented keyboard equivalents. Require recorded Orca/VoiceOver results before claiming accessibility parity. Rollback: keep the overview as an ordinary removable app; preserve direct CLI and terminal use.

## Non-goals and unresolved decisions

- No renderer rewrite, framework replacement, general GUI remoting, arbitrary canvas protocol, or embedded scripting VM.
- No reboot durability, process checkpoint/restore, remote filesystem sync, built-in account system, hosted relay, or shared collaborative typing in the first persistence scope.
- No claim that native status labels prove what a job actually did, or that silence means it needs the user.
- No claim that current performance notes prove Rex superiority, real desktop latency, detached resource cost, or untested accessibility workflows.
- Need a product decision on whether native session persistence outweighs the extra owner/attach contract. Ordinary remote shells already have tmux.
- Need an authority decision on controller transfer, clipboard mediation, daemon lifetime, and whether closing the last window detaches or terminates by default. A visible explicit distinction is safer for users than silently changing close semantics.
- Need a viewport policy before multi-client attach. One controlling viewport is the smallest coherent version. Independent per-viewer native layout can follow only after its input/focus/layout semantics are designed and tested.
- Need to decide which semantic state is owner-authoritative and which is viewer-local. Native focus, selection/scroll position, accessibility focus, and OS clipboard are not interchangeable.

The next concrete decision is stage 1 pane/status cleanup versus stage 2 local persistence proof. Both can build on the shipped Rust design. Neither requires copying Rex's full architecture before proving the user benefit.
