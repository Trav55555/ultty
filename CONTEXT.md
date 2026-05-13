# Ultty Architecture Vocabulary

This file names the core seams in the Ultty workspace. Keep it factual and close to the code; detailed design belongs in `docs/ARCHITECTURE.md` and protocol behavior belongs in `docs/PROTOCOL_SPEC.md`.

## Terms

**Ultty terminal**
The GPU-accelerated terminal emulator in `crates/ultty`. It runs ordinary ANSI terminal applications and hosts native protocol sessions from Ulttui apps.

**Ulttui framework**
The application framework in `crates/ulttui`. It owns the `Application` trait, Elm-style update loop, widgets, focus, themes, commands, subscriptions, and dual native/ANSI rendering.

**Native protocol**
The binary app-to-terminal wire protocol defined in `crates/ultty-protocol`. Apps send structured messages such as `CreateRegion`, `SetContent`, `SetConstraints`, `BeginFrame`, and `EndFrame`.

**Native protocol session**
The terminal-side state for one native app connection. It owns protocol negotiation, socket buffers, the retained scene graph, layout engine state, subscriptions, focus, scroll state, dirty flags, streams, clipboard requests, and keepalive state.

**Retained scene graph**
The terminal-owned tree of regions in `crates/ultty/src/scene`. Apps send diffs; the terminal stores the current region tree, content, style, constraints, metadata, and computed layout.

**Region**
One node in the retained scene graph. A region has an ID, optional parent, children, constraints, computed layout, content, style, metadata, and dirty state.

**Frame batching**
The `BeginFrame`/`EndFrame` protocol behavior that queues native app operations and applies them atomically before layout recomputation.

**Layout translation**
The conversion from protocol `Constraints` to Taffy style values. Unsupported or approximate protocol dimensions are handled inside the layout module so callers do not duplicate those rules.

**Backend adapter**
An Ulttui rendering adapter selected at runtime. The native adapter sends protocol messages to Ultty; the ANSI adapter renders through ratatui/crossterm; the mock adapter drives tests.

**Native frame diff**
The Ulttui native backend behavior that turns a rendered view tree into optimized protocol messages by comparing the current frame against previous region snapshots.

**Runtime tick**
One pass through the Ulttui runner: poll backend events, route screen/app events, drain messages, process subscriptions and spawned tasks, expire toasts, render, and decide whether to continue.

**Command scheduling**
The Ulttui runtime behavior that interprets `Cmd` values from `Application::update`, including immediate messages, batches, spawned futures, delayed messages, subscriptions, toasts, screen stack changes, and quit requests.
