# Remote workflow and workspace change map

Review date: 2026-11-12. Source revision: `c718910fad12749ec53379ab09256ae5a52a826b`. This is a source review and proposed implementation sequence. No networking setup, runtime qualification, code changes, or new protocol authority is included.

## Recommendation

Start with selectable launch profiles that run an external `ssh` process, then add factual per-pane connection diagnostics. The terminal already launches arbitrary argv, owns a split tree, publishes stable pane IDs, and carries native UI over an SSH PTY. The first missing pieces are profile selection, configured-pane construction, and retained launch provenance.

Rex-like workspace control also needs explicit focus-by-ID, zoom, pane rearrangement, and named workspace definitions. Nested splits, divider dragging, directional keyboard focus, tab reordering and saved layout trees already exist. Durable remote process sessions and remote native UI in split tabs require larger, separate changes.

The narrow networking boundary is an installed Tailscale client plus the user's SSH configuration. Enrollment, auth keys, routing, exit-node selection, ACL changes and VPN embedding remain outside the terminal. Optional remote helpers should begin as separately authorized, read-only processes over SSH, with bounded requests and explicit instance selection.

## Current launch path

| Entry or owner | Current behavior | Implication |
|---|---|---|
| [main.rs](../../crates/ultty/src/main.rs), `invocation`, line 72 | Only help/version and `-e` or `--` execution. Everything after the execution delimiter belongs to the child | `ultty -e ssh -tt devbox` is already expressible. A native `--profile NAME` selector does not exist |
| [app/mod.rs](../../crates/ultty/src/app/mod.rs), `create_terminal_manager`, line 547 | Explicit startup command wins; otherwise restore last session when enabled, then configured default profile, then shell | A profile selector needs an explicit precedence rule. It must not accidentally restore another workspace first |
| [config.rs](../../crates/ultty/src/config.rs), `ProfilesConfig`, `LaunchProfile`, `TabConfig`, lines 990-1026 | Named profiles contain tabs with program, argument vector, cwd, title and a `panes` vector | Existing configuration can describe SSH launches without new networking types |
| [app/mod.rs](../../crates/ultty/src/app/mod.rs), `create_manager_from_profile`, line 614 | Creates each tab's command, args, cwd and title. Does not read `TabConfig.panes` | Configured split panes are currently ignored. Fix this existing path before adding a second workspace launcher |
| [terminal_manager.rs](../../crates/ultty/src/terminal_manager.rs), `create_tab_with_command`, line 1059, and `split_pane_with_command`, line 901 | Spawn a PTY child through an argument vector | Reuse these operations for SSH connections |
| [pty/child.rs](../../crates/ultty/src/pty/child.rs), `Child::spawn`, line 64 | Prepares argv/env/cwd and forks/executes the child | Local process creation is not evidence that SSH authenticated or the remote application started |
| [bin/ulttyctl.rs](../../crates/ultty/src/bin/ulttyctl.rs), `new-tab`, line 294, and `parse_split`, line 439 | Preserve command/argument boundaries into typed control requests | Existing external automation can already launch SSH in a new tab or split |
| [app/control.rs](../../crates/ultty/src/app/control.rs), `RunCommand` branch, line 908 | Joins arguments into shell text and writes a newline into the existing PTY | Use the spawn operations for connection profiles. A `run` acknowledgment is not exec success or remote authorization |

A configured tab can already contain `command = "ssh"` and `args = ["-tt", "devbox"]`, where `devbox` is a user-managed SSH alias, including an alias resolving to a tailnet host. This does not need a protocol change. The current GUI uses only the configured default profile; selecting another requires editing the default or a future selector.

`TerminalManager::inherited_directory` consults host-qualified OSC 7 and process cwd. It rejects using a remote path as a local directory through `inherit_dir::pick`. Keep local cwd and remote cwd distinct in any profile editor. A remote directory is not a suitable `working_dir` for spawning local `ssh`. [terminal_manager.rs](../../crates/ultty/src/terminal_manager.rs), line 354; [inherit_dir.rs](../../crates/ultty/src/inherit_dir.rs).

## Small external SSH/Tailscale integration

### 1. Make launch profiles usable

Extend `Invocation::Run` into an explicit startup choice, such as shell/default, named profile, or argv command. Pass that choice through `StartupCommand`'s current entry points in `app/mod.rs`. An explicit profile should override automatic last-session restore and fail clearly if unknown. Preserve the existing `-e` behavior that neither restores nor overwrites the saved last session.

Implement `TabConfig.panes` construction using `split_pane_with_command`, with a documented target order and directory inheritance. The current flat pane configuration has no recursive target tree or ratios. Do not pretend it can describe an arbitrary mixed layout. Define a small sequential meaning for this unused field, or revise it deliberately with a migration and validation rule.

Keep SSH aliases and authentication in OpenSSH initially. Preserve argv boundaries and let the visible PTY handle host-key and login prompts. Do not parse stderr text into an authoritative diagnosis of authorization failure. A later reusable connection section only earns its place when several profiles need the same alias and diagnostics metadata.

Retain per-pane launch provenance in `ManagedTerminal`, distinct from its mutable title. A small `LaunchOrigin` value can record local shell, explicit argv, or configured profile/tab/pane key, plus the original executable and redacted display label. This enables diagnostics and relaunch intent without inferring the connection from an OSC title or foreground process name. Avoid persisting arbitrary environment variables or credentials.

### 2. Report evidence at the owning layer

Add a small optional connection module rather than teaching the VT parser about Tailscale. Its state belongs with the pane/host owner, not `ultty-vt` or the wire codec.

| Observation | Existing source | Proposed state | Must not imply |
|---|---|---|---|
| Local child forked/running/exited | `Child::try_wait`, `TerminalEvent::ChildExited` and `ManagedTerminal` | Starting, running, exited with status | SSH authentication succeeded |
| Tailscale executable/daemon/query outcome | Optional bounded child query outside render/update | Unavailable, query failed, last observation timestamp, observed peer/path data | Policy permits SSH or the remote service is healthy |
| Native protocol lifecycle | `ProtocolServer`, `NativeSession` and pane binding | Probe not observed, connected socket/in-band, disconnected with known reason | A running SSH process guarantees native mode |
| Remote helper response | Future explicit SSH helper protocol | Unavailable, timed out, response at time with helper version and target instance | Full remote shell authority or a durable PTY |

Keep these as separate observations. One `Connected` boolean would collapse process, network, SSH and native UI states that fail independently. Use `Unknown` where the chosen external transport supplies no authoritative SSH-stage signal.

The useful seams are [terminal_manager.rs](../../crates/ultty/src/terminal_manager.rs), `ManagedTerminal` and `process_all`; [app/control_events.rs](../../crates/ultty/src/app/control_events.rs), `control_event`; and [app/mod.rs](../../crates/ultty/src/app/mod.rs), `AppEvent`, line 106. A bounded worker can return an owned snapshot through the existing winit event proxy pattern. Do not run `tailscale status`, DNS lookup, or remote SSH preflight synchronously in rendering or control dispatch.

Existing `PaneStatus` is application-reported status, with explicit control status taking precedence over OSC progress in `ManagedTerminal::shown_status`. Transport health should not replace an application's task label. A later pane-info/control-schema extension can carry a separate health field once the schema owner defines it.

The local CLI/control path already supports process-exited events, pane IDs and command marks. Prefer a diagnostic view assembling those facts before a new connection manager. Limit sampling to an explicit request or bounded cadence, expire stale results, cap subprocess output, and cancel work when the pane or window closes. Record client version when interpreting Tailscale JSON; the prior research documents its format-change caveat.

### 3. Treat relaunch as a new action

The first launcher should not retry SSH automatically, replay shell commands, or enroll missing devices. Store a retryable launch intent separately from a live process. A user-triggered relaunch receives a new runtime pane/process identity while retaining an optional workspace key. Reusing an old pane ID after close would undermine existing stale-target errors.

If a profile is meant to keep a failed connection visible, expose a per-launch hold-on-exit choice rather than relying solely on global `behavior.close_on_exit`. Current `TerminalManager::process_all` annotates retained exited panes and `cleanup_exited` removes them when the global flag permits it, lines 782-846. This is a useful existing behavior to extend, not a new networking concern.

## Native timeout and EOF behavior

The existing behavior is more specific than automatic fallback or reconnect:

- [ultty-client/src/in_band.rs](../../crates/ultty-client/src/in_band.rs), `InBandTty::read`, line 137, returns `Open` when its poll timeout expires. It returns `Closed` when `read` returns zero. Silence and EOF are different outcomes.
- The same file defines a two-second heartbeat interval. Both the low-level client and framework send unsolicited `Pong` messages while idle. The terminal does not send in-band `Ping`, because it could become shell input after the application dies.
- [protocol/session.rs](../../crates/ultty/src/protocol/session.rs), `check_keepalive`, line 590, expires an unconfirmed in-band handshake after two seconds and an established in-band session after ten seconds without app traffic. The socket path separately has a 30-second ping interval, ten-second pong timeout and 120-second hard idle limit.
- `NativeSession::in_band_app_alive`, line 991, can inspect only local foreground groups/processes. Its own comment says a remote application over SSH relies on `Disconnect` and idle expiry.
- [ulttui/src/backend/native/transport.rs](../../crates/ulttui/src/backend/native/transport.rs), `read_available`, line 60, preserves EOF as `Closed`. [backend/native.rs](../../crates/ulttui/src/backend/native.rs), `process_pending_messages`, line 490, drains buffered messages, clears the connection and queues `Event::Quit` on closure. It does not switch an established native application to ANSI.
- [ultty-client/src/lib.rs](../../crates/ultty-client/src/lib.rs), `read_available`, line 672, marks closure and returns `ConnectionClosed` to low-level callers.
- [app/update.rs](../../crates/ultty/src/app/update.rs), `pump_in_band`, line 53, allows only single-pane tabs. Splitting a tab with a live in-band session explicitly ends that native session. New probes in split tabs receive no native answer and can choose ANSI during startup.

A network pause can therefore expire the terminal's in-band session while the SSH PTY remains open. That does not necessarily produce EOF in the remote app. The current client has no established-session recovery state machine that detects this condition and reconstructs the scene. Re-handshake, fallback, and reconnection are proposed behavior requiring separate protocol/runtime acceptance criteria.

Before advertising remote native multi-pane workflows, change in-band ownership from tab to the PTY/pane that carries the frame. The current `ProtocolServer::in_band_input` signature and session lookup use `TabId`; the application intentionally gates split input. Merely using the existing native socket pane binding cannot fix this remote path. Socket sessions already have pane-based rendering/viewports through [app/native_panes.rs](../../crates/ultty/src/app/native_panes.rs), and ADR 0025 explicitly leaves lifting the in-band restriction to another slice.

## Workspace and pane parity

| Outcome | Current implementation | Concrete remaining change |
|---|---|---|
| Nested splits and resize | `PaneLayout` tree, `split`, `remove`, weighted rectangles, divider drag and `TerminalManager::layout_panes` | Keep and reuse. A new workspace model must drive the same geometry for PTYs, rendering, input and native viewports |
| Directional focus | `focus_direction`, next/previous pane and configured arrow bindings in `app/input.rs` | Already implemented. Add explicit control focus-by-ID, including switching the owning tab when needed; `FocusWindow` currently focuses only the window |
| Pane zoom | No zoomed pane state or layout projection. Existing zoom references concern font size/window maximize | Add optional zoomed `TerminalId` on `Tab`, keep the stored split tree, and compute one common visible rectangle projection. Update `is_on_screen`, hit testing, PTY sizes, native viewport sync, screenshots and accessibility consistently |
| Pane reorder/swap | `PaneLayout` supports split/remove; tab reordering exists, pane movement does not | Start with swap inside one tab, preserving PTY and terminal IDs. Cross-tab moves need a separate native-session/socket ownership design |
| Named workspace | Named startup profiles exist; `SessionData` can save arbitrary paths, but application auto-save targets `last.json` | Add list/save/load/select UX for workspace snapshots with validated names/paths and explicit replace/new-window behavior |
| Restore commands/remote targets | `TerminalData` saves title, cwd and focus. `from_session` recreates shells, not original argv or SSH connections | Store a deliberate launch reference if relaunch is wanted. Call it relaunch/restore layout, not durable session reattachment |
| Reconcile a live workspace | Saved tree restoration validates pane coverage and falls back to flat layout. No desired-state workspace keys or apply operation | Separate a stable workspace pane key from runtime `TerminalId`, validate complete desired layout first, then propose/create missing panes. Preserve live panes and require explicit destructive removal |
| Native UI in multiple panes | Local socket sessions bind to panes; remote in-band remains single-pane | Separate transport/session change described above. Do not equate local multi-pane support with remote parity |

Source owners are [pane_layout.rs](../../crates/ultty/src/pane_layout.rs), lines 21-206; [terminal_manager.rs](../../crates/ultty/src/terminal_manager.rs), `Tab`, lines 170-225, focus methods, lines 950-1005 and 1141-1160; [session.rs](../../crates/ultty/src/session.rs), `TerminalData`, line 59; and `TerminalManager::to_session_data`/`from_session`, lines 1204-1380. The control request enum in [ultty-protocol/src/control.rs](../../crates/ultty-protocol/src/control.rs) currently has no `FocusPane`, `ZoomPane`, `MovePane`, workspace apply, or named-session operations.

For zoom, a renderer-only special case would leave hidden panes classified on-screen and sized/input-routed as before. Put the visible projection beside `PaneLayout`/`TerminalManager`, then consume it consistently. The existing `layout_generation` and native viewport sync provide a useful invalidation seam.

For cross-tab movement, `ManagedTerminal.tab_id`, `Tab.shell_env`, per-tab protocol sockets, and `NativeSession.tab_id` already carry ownership. A running process keeps its original environment. Moving a rectangle and editing a vector cannot safely redefine which tab socket that process inherited. Keep the first operation within one tab or specify how old/new native sessions and IPC endpoints migrate.

Reconciliation should validate unique pane keys, exactly-once layout coverage, recognized directions, finite positive weights, and depth/resource limits before creating anything. Reusing restore's fallback silently is acceptable for recovering an old snapshot, but a requested live apply needs a visible validation error or explicit degraded plan. Snapshot restore and live desired-state application are different operations.

## Local authentication and optional remote helpers

Two existing socket boundaries must stay distinct:

- Native sockets are per-tab, mode 0600, and the server checks the Unix peer's UID. Peer PID/process group contributes to pane binding and key-input eligibility. [protocol/server.rs](../../crates/ultty/src/protocol/server.rs), `peer_is_authorized`, line 1194, and [protocol/session.rs](../../crates/ultty/src/protocol/session.rs), `NativeSession::new`, line 273.
- The control socket is per Ultty process, mode 0600, with a random 32-byte token written as a mode-0600 file. Every content read and terminal action requires token authentication; only liveness/version and envelope/auth handling are exempt. Batch children are checked individually. [control/server.rs](../../crates/ultty/src/control/server.rs), `new_at`, line 123; [app/control.rs](../../crates/ultty/src/app/control.rs), lines 1502-1603.

The control accept path does not perform the native server's UID check. Its filesystem permissions and token are the boundary. `private_socket_dir` accepts a nonempty `XDG_RUNTIME_DIR` as provided; it verifies ownership/mode for its fallback directory. Do not describe every runtime-directory path as independently verified by Ultty. [platform/socket.rs](../../crates/ultty/src/platform/socket.rs), lines 33-97.

`ulttyctl` automatically reads the instance token and wraps queries as authenticated requests. `find_instance` prefers `ULTTY_PID` while listening and otherwise falls back to the newest instance. That is useful locally, but a remote helper should require an explicit instance and fail when it disappears. Silently switching to another user's workspace window would be the wrong remote behavior. [bin/ulttyctl.rs](../../crates/ultty/src/bin/ulttyctl.rs), lines 174-217; [control/paths.rs](../../crates/ultty/src/control/paths.rs), lines 46-80.

A first remote helper should accept only a versioned query enum, invoke a deliberately narrow local query set, and return bounded JSON over a dedicated noninteractive SSH process. Select instance and allowed pane IDs explicitly. Local token reading stays on the host; never return the token, forward the Unix socket, or proxy arbitrary `ControlRequest`/`Batch` data. Fix the allowed operation set before exposing any helper. Cwd, titles and terminal contents are sensitive reads even when they cause no mutation.

The helper's authorized SSH account can already run other commands outside this application boundary unless host-side SSH policy constrains it. The helper can limit its own API; it cannot create an OS sandbox by convention. Conversely, Tailscale node identity alone cannot authorize every local pane. Tailnet reachability, SSH account access, Ultty instance authentication and application query permission need separate evidence.

A helper querying a remote running Ultty can inspect that instance. It cannot inspect arbitrary remote shell jobs as Ultty panes where no host/session owner exists. Durable remote PTYs need the separately proposed host extraction; this document does not assume it is already implemented.

## Suggested delivery sequence and gates

1. Implement named profile selection and honor configured panes. Prove exact argv, missing-profile errors, local/remote cwd separation, split construction and unchanged `-e` save behavior. This delivers a useful SSH launcher without network dependencies.
2. Add launch provenance and read-only diagnostics. Prove bounded sampling, missing-client/daemon behavior, child exit, stale result expiry, cancellation and no renderer-thread subprocess blocking. Keep application task status separate.
3. Add explicit focus-by-ID and within-tab zoom/swap. Prove PTY identity/process continuity, hidden-pane input exclusion, common screenshot/live geometry and native viewport updates. Expose control requests only after their contract is approved.
4. Add named workspace snapshots and deliberate relaunch references. Prove malformed layouts fail before spawning, partial failure cleanup, no duplicate panes on repeated application and preservation of unrelated live work. Distinguish snapshot restore from durable PTY reattachment.
5. Qualify single-pane SSH/APC, then separately design remote native pane routing and post-timeout recovery. Include pauses exceeding ten seconds, remote app exit, SSH process exit, split-under-live-app and tmux/ANSI fallback. Stop on misdirected input or silent dead native state.
6. Add an optional read-only remote helper only when an actual remote host/session owner and explicit instance-selection contract exist. Prove denied mutation/batch requests, bounded response/timeout handling and cancellation. Escalating it to input or command execution is a separate authority decision.

This review used current source, nearby ADRs and existing test code. No tests or GUI/network workflows were run for this document. A proposed change is not evidence of runtime parity; its acceptance gate is listed above.
