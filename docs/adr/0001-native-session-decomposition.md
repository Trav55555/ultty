# ADR 0001: Native Protocol Session Decomposition

## Status

Accepted

## Context

`NativeSession` owns the terminal-side state for one native Ulttui app connection. It has to coordinate protocol decoding, frame batching, command application, retained scene updates, layout computation, subscriptions, focus, scroll, streams, clipboard requests, socket writes, and keepalive.

Keeping every concern directly inside `session.rs` made the module hard to navigate and made small behavior changes risky because protocol control flow, scene mutation, and layout output were interleaved.

## Decision

Keep `NativeSession` as the owning state object, but split behavior into narrow modules with inherent `NativeSession` methods:

- `protocol::session::frame` owns `BeginFrame`/`EndFrame` validation, queued operation replay, and frame-triggered layout recomputation.
- `protocol::session::commands` owns non-frame `AppMessage` application, including negotiation, region mutation, subscriptions, focus, scroll, streams, clipboard, pong, and disconnect handling.
- `scene::layout::LayoutEngine::compute_scene_layout` owns Taffy sync, layout computation, absolute-position accumulation, stale-node cleanup, and scene write-back.

`session.rs` remains the orchestration layer for socket I/O, decode/encode buffers, keepalive, public session accessors, and protocol-level send helpers.

## Consequences

Command application and frame batching can now be tested and changed independently without adding traits or adapter objects. The retained scene graph still has one owner, and all mutation stays behind the existing `NativeSession` state.

Layout output now has one canonical path from scene tree to `ComputedRegionLayout`, which reduces duplicate rules around absolute offsets, content bounds, and dirty-region cleanup.

Future native protocol changes should first decide whether they are frame control, ordinary command application, layout computation, or session I/O before adding code to `session.rs`.
