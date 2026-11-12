# Rex and Tailscale design evaluation for Ultty and Ulttui

Research date: 2026-11-12. Ultty source baseline: `b75af7ac35423e7103a1a9065ce64f118ae0d428`. This is research and an implementation proposal, not a completed integration or comparative benchmark. Terra and Sol researched Rex, inspected Ultty, and independently assessed Tailscale in parallel.

## Recommendation

Ultty should adopt Rex's separation between running work and the window displaying it, while extending its existing semantic TUI and automation capabilities. Tailscale should initially remain an optional transport supplied by the user. The best opportunity is a terminal where a human and an agent can inspect the same named UI elements, reconnect to running work, and understand why a remote connection is failing.

Ultty already has much of the local automation foundation. Building another pane controller, event stream, MCP wrapper, or native inspection endpoint would duplicate shipped code. The important gaps are live process persistence, reconnectable state, compatible program status, and clear boundaries between transport identity and permission to act inside a session.

There is no evidence from this research that Ultty is faster than Rex or can improve on Tailscale's networking. The proposals below define narrower outcomes we can test. Keep Rust, Taffy, the VT core, and Ulttui's full ANSI fallback. Do not make a terminal project responsible for a new VPN, relay network, identity provider, or agent IDE.

## What the video establishes

The supplied [Why I'm betting on Rex](https://www.youtube.com/watch?v=zZYWQRE-0xs), by reemus_dev, is a product argument and demonstration. The captured English automatic transcript contains 547 segments and 3,762 words, starting at 0.1 seconds and reaching the closing remarks at 1040.302 seconds. The last caption extends slightly past the displayed 17:21 duration. There are no gaps over ten seconds; the opening, middle and ending were reviewed. Captions cover the narrative, but visual details and spelling remain unverified against playback.

The narrator favors a general multiplexer that other tools can automate. Around 4:05 he describes tmux's composed terminal output; around 5:24 he contrasts independently rendered clients. At 6:09 he acknowledges bandwidth costs on slow connections. At 12:19 he introduces restoration scripting; at 12:48 stopping the server requires restarting and resuming the agent. Later comments about future block types and open sourcing are predictions. The video does not prove generic process recovery, production reliability, or a performance lead. Its Tailscale analogy is about composing useful capabilities, not equivalent protocols.

## Rex features and choices

| Capability | Evidence and design choice | Ultty implication |
|---|---|---|
| Persistent work | Sessions and terminal blocks belong to a server; clients can leave independently. Server shutdown ends programs. Saved layout restoration and application-specific resume are different operations. [Server modes](https://www.superlogical.com/rex/docs/linux/server-modes) | Move PTY ownership outside the GUI only if detached workloads are a product requirement. Specify each failure boundary. |
| Native pane workflow | Directional and best-fit splits, zoom, floating panes, movement, session switching and tab tear-off are documented Mac actions. [Actions](https://www.superlogical.com/rex/docs/reference/mac-actions) | Test discoverability and keyboard workflows against existing Ultty splits; more split commands alone do not establish a better experience. |
| Discoverable automation | CLI and Lua use server-provided schemas and examples; discovery reflects the connected version. [Server API](https://www.superlogical.com/rex/docs/automate/server-api) | Extend existing typed control/MCP definitions with versioned capability discovery and consistent errors. Generate help from those definitions. |
| State reconciliation | Attach returns a snapshot plus a sequence boundary; session views have revisions. Slow attach-all consumers are disconnected. Size ownership is distinct from input permission. [API reference](https://www.superlogical.com/rex/docs/reference/api) | Define ordering, overflow, reconnect and resize authority explicitly before supporting multiple viewers. |
| Program status | OSC 7501 supplies bounded program-reported records and lifecycle rules; Rex displays them and emits events. [Status specification](https://www.superlogical.com/rex/docs/build/program-status) | Add interoperability around existing pane status, retaining hierarchical records and untrusted-input limits. |
| Customization | Lua actions, key sequences and modes allow workflows to be assembled from existing actions. Lua scripts are trusted code, not an isolation boundary. [Lua](https://www.superlogical.com/rex/docs/automate/lua-scripts), [key modes](https://www.superlogical.com/rex/docs/customize/key-modes) | Extend existing CLI/events and the separate, grant-controlled `ulttui-lua` process. Keep script failures outside the terminal runtime. |
| Remote operation | Local sockets, SSH forwarding and Tailscale options are documented. Anonymous HTTP grants broad authority; fuller native authentication is planned. [Remote access](https://www.superlogical.com/rex/docs/linux/remote-access) | Keep remote exposure opt-in and enforce application permissions after transport authentication. |
| Agent integration | A bundled skill and environment context help agents choose the right session and pane. App control has a separate opt-in. [Agent guide](https://www.superlogical.com/rex/docs/automate/agent-skill) | Build on Ultty's existing MCP and pane IDs; prevent implicit focused-pane targeting from redirecting unattended work. |

The public API establishes structural control and a separate streaming connection, not a complete terminal replication specification. Raw mirroring must not be simplified to "the server does no parsing." A historical Mitchell Hashimoto thread describes server parsing alongside a tee to clients, but its original endpoint was inaccessible here. That statement remains qualified in the [Terra source memo](../research_notes/rex-2026-11-12/terra-rex-design.md).

Rex's current installation page offers the macOS client and Linux/FreeBSD servers to testers, with restricted Windows-server access. Other client documentation does not establish general availability. No reproducible, matched Rex-versus-Ultty benchmark was obtained. [Installation](https://www.superlogical.com/rex/docs/installation)

## What already exists in Ultty

The [Sol code audit](../research_notes/rex-2026-11-12/sol-ultty-gap.md) contains exact line references. Current code provides pane IDs and targeting, split creation, subscriptions, event-driven waits, OSC 133 command-output extraction, generic pane status, Rust MCP, and native region inspection in `crates/ultty-protocol/src/control.rs` and the Ultty runtime. The older market report's missing-feature list is stale.

Ulttui already combines an Elm application API, stable native keys, retained native diffs, terminal-owned layout, inline surfaces and an ANSI fallback. Hot-reloaded Luau tools run in a separate process. Protocol semantics and AccessKit integration exist and are enabled by default, but accessibility action requests are currently unhandled. That is implementation evidence, not proof of complete screen-reader support or native/ANSI parity in every widget.

Socket-native apps already work in split panes. In-band sessions and inline pairing still have single-pane gates in `crates/ultty/src/app/update.rs`; the former can terminate a native session when its tab is split. Finish attribution to the originating PTY before treating SSH-native split workflows as supported. Current `RunCommand` joins arguments into shell text, and several input paths discard queue errors before returning `Ok`. These are specific correctness gaps, not reasons to replace the control plane. Reproduce them and define acceptance versus execution versus completion before widening automation.

The main structural gap is ownership. `App` owns `TerminalManager`; managed terminals own their event loops and child processes. Dropping the child terminates it. `SessionData` stores workspace presentation and working directories, and restoration creates new shells. It does not preserve live processes. A native protocol `Session` is an application's rendering connection, not a detached multiplexer session.

## How to do the relevant jobs better

**Make automation semantic.** An agent should be able to identify a named input, inspect its role/value/state, and invoke a supported action without guessing coordinates or typing into whichever pane is focused. Region inspection and accessibility semantics provide a starting point. Add a shared action contract only after auditing current accessibility actions. Keep ordinary VT text queries for applications that do not supply structure. Success means fewer wrong-target actions and reliable operation during focus changes and resize, measured on the same task.

**Keep status independent from permission.** Map OSC 7501 `blocked` into the existing waiting presentation, but preserve reason and hierarchy separately. A status saying "permission" is a request for attention, never permission to execute. The emitter can spoof names, so attribution comes from the owning pane. Validation must happen before mutation; repeated reports replace records. Check reset, prompt and child-exit behavior, not only a spinner screenshot. Provide one Ulttui reporting helper with native and ANSI encodings so applications need not invent terminal-specific integrations.

**Separate the work, view and connection lifecycles.** A proposed local session host owns PTYs, terminal state and bounded history. A GUI attachment owns fonts, GPU resources and viewer-local presentation. The session host arbitrates canonical terminal dimensions and native layout/input authority. Connection loss releases the attachment, not the work. Distinguish running-detached, disconnected-unknown, exited and restart-required in the UI. Persisting a command recipe is explicit app restart, not recovered process memory.

**Preserve both VT and native sessions.** Ultty's differentiator makes this harder than moving a PTY into a daemon. Native applications have connection IDs, frame boundaries, region trees, resources, focus and layout feedback. A returning GUI must receive a consistent committed scene, and one authority must decide layout/resize and send application events. Design native ownership before extracting the host so native frame processing and replies do not remain dependent on GUI updates. An initial local prototype can support VT only, but must label native persistence unsupported until those contracts are exercised. Do not retain an unbounded byte log and call it recovery.

**Keep ordinary terminals useful.** Ulttui's ANSI fallback allows adoption without requiring Ultty. It does not automatically provide an ANSI client for a new persistent session service. That would be a separate multiplexer client project. Existing in-band native support over SSH also does not imply tmux passthrough or reconnectable transport.

## Tailscale design and what to borrow

Tailscale separates identity, policy and peer coordination from encrypted data transport. Devices exchange data directly where possible, with DERP and peer-relay alternatives. The operator should not have to understand NAT traversal to reach a machine, but diagnostics must still reveal the path when performance suffers. Cached policy can keep existing connectivity working through a coordination outage; the same outage prevents new revocation information from reaching devices. [Control/data planes](https://tailscale.com/docs/concepts/control-data-planes), [connection types](https://tailscale.com/docs/reference/connection-types), [outages](https://tailscale.com/docs/reference/coordination-server-down)

For Ultty, borrow stable identity independent of address, explicit control/data responsibilities, capability negotiation, useful diagnostics and conservative defaults. Maintain separate permissions for reading a pane, sending input, starting programs and changing a workspace. Network access to the machine does not automatically grant every terminal action. Tailscale SSH has its own policy and lifecycle; ordinary OpenSSH over a tailnet remains a different option. Neither is a persistent terminal service. [SSH](https://tailscale.com/docs/features/tailscale-ssh), [SSH over Tailscale](https://tailscale.com/docs/reference/ssh-over-tailscale)

| Integration | Value | Cost and recommendation |
|---|---|---|
| User-managed Tailscale + ordinary SSH | Existing identity/network tools; Ultty can use its current SSH/in-band path | First evaluation. No VPN dependency or default listener in Ultty. Process persistence remains a separate job. |
| Tailscale SSH | Identity-aware SSH without conventional user-key distribution | Optional, explicitly configured. Validate platform support, policy and daemon restart behavior. |
| Serve in front of a future service | Private HTTP exposure and identity/capability information | Later, only with an actual service. An adapter must authenticate trusted proxy metadata and enforce application permissions. It cannot expose the current control socket unchanged. |
| Go tsnet sidecar | Application-specific tailnet identity without relying on a system daemon | Adds another binary/runtime, enrollment state, updates and failure modes. Price the shipped dependency closure and support cost before adopting. |
| Rust tailscale-rs | Native Rust embedding | Current experimental API; README support policy says Rust 1.97, while the manifest's hard compile floor is 1.92. Both exceed Ultty's 1.90 policy. Keep any evaluation isolated from the workspace. |
| Funnel or a custom VPN | Public exposure or complete network ownership | Outside the proposed terminal scope. Funnel intentionally has a public audience; it is not private Serve with a different URL. |

The [Sol Tailscale memo](../research_notes/rex-2026-11-12/sol-tailscale-design.md) covers grants, tags, DNS, certificates, routing, Tailnet Lock, logging, source boundaries and newer products. [Terra's independent review](../research_notes/rex-2026-11-12/terra-tailscale-review.md) checks failure and embedding assumptions. Primary embedding references are [tsnet](https://tailscale.com/docs/features/tsnet) and [tailscale-rs](https://github.com/tailscale/tailscale-rs).

## Proposed sequence and acceptance gates

These are bounded future work proposals. No experiment or implementation was run in this research. Time budgets are investigation caps, not delivery estimates; a failed gate should produce a recorded no-go or a new scope, not an automatic extension.

| Phase | Budget and expected payoff | Cheapest decisive check and acceptance gate | Rollback or stop |
|---|---|---|---|
| 0. Existing pane and control contracts | Three-day investigation cap; improve current local and SSH use | Reproduce argv flattening and queue-failure success, then fix as separate slices. Exercise a local socket-native app beside an SSH in-band app, followed by split, resize and close. Require correct pane attribution, explicit input acceptance and no new clipboard authority. | Stop on cross-pane delivery. Preserve the current socket-native and ANSI paths; do not introduce a daemon to fix routing. |
| 1. Status interoperability | Two engineering days; benefit existing agent tools without a daemon | Freeze the OSC spec and vectors. Feed real reports into the VT core, observe pane/event/Ulttui behavior, and cover hierarchy, whole-report rejection, prompt/RIS cleanup, clear, overflow and feature detection. Compare existing SetPaneStatus as control. | Stop if faithful semantics require breaking current status consumers; propose a versioned extension. |
| 2. Semantic workflow | Two days; exploit the existing native framework | Use the same small form/task app in native and ANSI modes. Run scripted actions while focus and layout change. Require zero wrong-target actions, accurate state queries and a usable keyboard fallback. Check actual assistive-technology output separately. | Keep read-only inspection if action semantics are not ready. |
| 3. Local persistent session prototype | Five days; preserve expensive running work across GUI exit | One host, one PTY, one detachable viewer. Kill the GUI during output; reconnect and verify the same PID, correct screen/modes and bounded memory. Include resize, alternate-screen, slow consumer and snapshot/event races. | Stop before extraction across the full app if the one-PTY model fails. Existing local path remains default. |
| 4. Native persistence contract | Three-day design/prototype cap after phase 3 | Reattach an Ulttui app during an atomic frame, with focus, scroll and resources intact. One layout/input owner; stale clients cannot mutate a newer generation. | Do not advertise native persistence without this gate. |
| 5. Optional remote workflow | Two days, using already authorized test machines only | Compare ordinary SSH and SSH over Tailscale with the same app. Record direct/DERP/peer relay, RTT, p50/p95/p99 response, bytes, CPU/RSS, reconnect and rejection behavior. Exercise transport loss separately from server loss. | If existing SSH suffices, ship guidance/diagnostics and stop. No embedded node. |
| 6. Multi-client and remote service | Separate scope after the earlier gates | Require explicit read/input/spawn permissions, snapshot sequence rules, bounded queues, cancellation, revocation and size arbitration. Recover state across a deliberate stream gap. | Keep single-client/local deployment if authority or recovery remains ambiguous. |

Measure the entire workload: GUI and daemon RSS, scrollback bytes, per-client queues, native tree retention, serialization copies, GPU resources and network bytes. Shared immutable output chunks, bounded rings and stable arenas are candidates, not automatic wins. Compare with the current local code before adding caches or a second parser. Raw output fan-out may cost more bandwidth than a composed view; evaluate both under the actual workload rather than choosing from an architecture slogan.

The current [North Star](../docs/NORTH_STAR.md) excludes generic desktop remoting and requires explicit permission for protocol/API changes. The [README](../README.md) promises no networking code or phone-home behavior. A remote session service would need an approved scope and accurate revised product wording. This report does not change those contracts. It also does not authorize installations, tailnet changes, cloud machines, publication or implementation.

## Remaining questions

- Does detached local work solve enough observed user pain to justify a daemon now, ahead of the current alpha closeout gates?
- Which native app state belongs to the session host, and which can be reconstructed from the application without losing input or layout decisions?
- Can existing semantic actions support a useful agent workflow before adding another protocol operation?
- What exact Rex build and transport are available for matched tests? Public docs are not a runnable comparison.
- Are remote failures predominantly authentication, path selection, server lifecycle or application state? Measure these separately.
- Would a user-managed SSH/Tailscale path satisfy remote users completely? If yes, embedded networking has no demonstrated payoff.

The next implementation candidates are honest control receipts, complete native pane ownership and OSC 7501 compatibility. The next architectural decision is whether Ultty should own persistent sessions. Remote networking follows that decision; it cannot substitute for it.
