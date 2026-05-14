# Ultty Architecture Vocabulary

This file names the core seams in the Ultty workspace. Keep it factual and close to the code; detailed design belongs in `docs/ARCHITECTURE.md` and protocol behavior belongs in `docs/PROTOCOL_SPEC.md`.

## Terms

**Ultty terminal**
The primary product: a GPU-accelerated terminal emulator that runs ordinary ANSI terminal applications and hosts native protocol sessions.

**Ulttui framework**
The companion application framework that proves and exercises Ultty's native app runtime.

**Native protocol**
The semantic app-to-terminal boundary for retained regions, terminal-side layout, structured content, focus, events, and incremental updates.

**Semantic UI primitive**
A terminal-understandable UI concept that preserves intent across the native app boundary.

**Native app runtime**
The terminal-hosted runtime that lets structured applications communicate with Ultty through the native protocol instead of ANSI escape sequences.

**Native app**
A terminal-hosted application that communicates through semantic UI primitives while remaining keyboard-first, inspectable, resize-resilient, and usable without pointer precision.

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

## Relationships

- The **Ultty terminal** is the product center of the workspace.
- The **Native app runtime** belongs to the **Ultty terminal**.
- The **Native protocol** is the boundary between the **Native app runtime** and native applications.
- The **Native protocol** carries **Semantic UI primitives**, not arbitrary remote drawing commands.
- A **Native app** runs inside the **Native app runtime**.
- The **Ulttui framework** is a companion framework for building applications that exercise the **Native app runtime**.

## Flagged ambiguities

- "Native protocol" does not mean a generic remote GUI or drawing protocol; it means semantic terminal UI communication.
- "Native app" does not mean an embedded desktop GUI; it means a terminal-hosted application with semantic structure.
