# Ultty Architecture Vocabulary

This file names the core seams in the Ultty workspace. Keep it factual and close to the code; detailed design belongs in `docs/ARCHITECTURE.md` and protocol behavior belongs in `docs/PROTOCOL_SPEC.md`.

## Terms

**Ultty terminal**
The primary product: a GPU-accelerated terminal emulator that runs ordinary ANSI terminal applications and hosts native protocol sessions.

**Ulttui framework**
The first-class dual-mode TUI framework for applications that run in both ordinary terminals and Ultty-native mode.

**Native protocol**
The semantic app-to-terminal boundary for retained regions, terminal-side layout, structured content, focus, events, and incremental updates.

**Semantic UI primitive**
A terminal-understandable UI concept that preserves intent across the native app boundary.

**Native app runtime**
The terminal-hosted runtime that lets structured applications communicate with Ultty through the native protocol instead of ANSI escape sequences.

**Dual-mode app**
An Ulttui application that remains usable in ordinary ANSI terminals and gains semantic native runtime behavior inside Ultty.

**Ultty-native mode**
The Ulttui runtime mode where an application communicates with Ultty through the native protocol.

**ANSI mode**
The Ulttui runtime mode where an application renders through ordinary terminal escape-sequence behavior.

**Native app**
A terminal-hosted application that communicates through semantic UI primitives while remaining keyboard-first, inspectable, resize-resilient, and usable without pointer precision.

**Keyboard-first interaction**
An interaction model where every important native app workflow is usable by keyboard, with mouse input allowed as an accelerator.

**Terminal correctness**
The baseline product obligation that ordinary terminal applications behave reliably before native app features are optimized.

**Native runtime observability**
The ability to inspect native app runtime state from inside Ultty without relying only on external logs.

**Session health**
The observable state of a native protocol session, including connection, focus, dirty, streaming, blocked, and error conditions.

**Render causality**
The observable explanation for why Ultty last redrew or skipped redrawing terminal and native app content.

**Idle efficiency**
The performance expectation that Ultty consumes minimal CPU and GPU when terminal and native app state are unchanged.

**Interaction responsiveness**
The performance expectation that input, PTY output, scrolling, search, copy, and native app updates become visible promptly.

**Append-oriented stream**
Streaming content that grows forward over time, such as agent output, chat messages, logs, and tool output.

**Agent/workshop console**
The flagship native app scenario: a keyboard-first console for agent output, tool activity, status, panes, commands, forms, logs, and long-running work.

**Markdown stream**
An append-oriented stream whose markdown semantics are parsed by Ulttui and carried to Ultty as semantic content updates.

**Block-level diff**
The Ulttui behavior that turns markdown changes into incremental updates for changed blocks or leaves instead of resending the whole document.

**Unix-first runtime**
The current platform contract: Linux and macOS are supported runtime targets, while Windows is a future platform.

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
- A **Native app** uses **Keyboard-first interaction**.
- **Terminal correctness** takes priority over **Native runtime observability**, which takes priority over visual richness.
- **Native runtime observability** starts with **Session health** and **Render causality**.
- Ultty performance prioritizes **Idle efficiency** and **Interaction responsiveness** before microbenchmark speed.
- Streaming content is first optimized for **Append-oriented streams**.
- The **Agent/workshop console** is the flagship scenario for proving native app runtime depth.
- A **Markdown stream** uses **Block-level diff** in the **Ulttui framework**.
- The **Native app runtime** is a **Unix-first runtime**.
- The **Ulttui framework** builds **Dual-mode apps** with **ANSI mode** and **Ultty-native mode** as product commitments.
- **Ultty-native mode** is the premium path, but **ANSI mode** is not an afterthought.

## Flagged ambiguities

- "Native protocol" does not mean a generic remote GUI or drawing protocol; it means semantic terminal UI communication.
- "Native app" does not mean an embedded desktop GUI; it means a terminal-hosted application with semantic structure.
- "Keyboard-first" does not mean keyboard-only; mouse input is valid when it accelerates a complete keyboard workflow.
- "Performance" does not mean microbenchmark speed alone; user-visible responsiveness and idle behavior are primary.
- "Streaming content" does not mean collaborative document editing or arbitrary rich-document mutation in the first architecture.
- "Markdown stream" does not mean the Ultty terminal owns markdown parsing policy; Ulttui owns markdown semantics.
- "ANSI fallback" does not mean best-effort demo support; it means the **ANSI mode** of a **Dual-mode app**.
- "Cross-platform" does not currently include Windows runtime support; Windows requires a future platform boundary.
