# Layout Model

Ultty uses an immediate-output / retained-internals split.

## Core Contract

- `ulttui::Application::view()` rebuilds a fresh view tree from application state on each render pass.
- Application state, focus IDs, selection, scroll positions, and async results live in the application or app-owned widget state.
- Native backends may retain protocol/layout caches between frames, but app code must not rely on Rust object identity of returned `View` values.

## Native Pipeline

1. `ulttui` turns the current view tree into a native protocol frame.
2. The terminal retains a scene graph of regions.
3. `ultty` computes terminal-side layout with Taffy.
4. Rendering and input routing use the last committed computed layout.

This keeps the app API immediate-mode while allowing the terminal to cache retained scene, layout, and GPU resources.

## Identity

Today, native `RegionId`s are assigned by traversal order within a frame. That is simple and deterministic, but sibling insertion or reordering changes semantic identity. Widget `widget_id()` / `native_key()` values should be stable because they are the future path for preserving identity across reorder-heavy native frames.

## Hit Testing

Mouse routing hit-tests against the last committed computed layout. During input handling this is effectively previous-frame hit testing: events target what the user could actually see, not a speculative layout that depends on handling the event first.

If a future widget needs frame-accurate hover for rapidly moving layout, prefer a narrow special case or an explicit second layout pass over making the whole pipeline stateful.

## Layout and Testing Rules

- Use Taffy for general flex/grid layout; do not replace it with a bespoke engine without measurements.
- Add intrinsic text/image measurement only for concrete auto-sizing bugs.
- Protect layout behavior with `ulttui` ANSI snapshots and focused native protocol/scene tests.
- Prefer dated performance evidence before layout-cache rewrites.
