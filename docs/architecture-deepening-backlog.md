# Architecture Deepening Backlog

This backlog tracks implementation-depth gaps that should be fixed as small
vertical slices. The public Interfaces should stay stable unless a slice
explicitly changes the release contract.

## Current Opportunities

| Area | Files | Problem | Target Slice | Validation |
|------|-------|---------|--------------|------------|
| Control socket structure | `crates/ultty/src/bin/ulttyctl.rs`, `crates/ultty/src/control/server.rs`, `crates/ultty-protocol/src/control.rs` | Automation should not depend on shell-like text parsing or lose argument boundaries. | Send typed JSON envelopes from `ulttyctl`; keep text commands as the human/debug Adapter. | `cargo test -p ultty --bin ulttyctl`; `cargo test -p ultty --lib control::server` |
| Protocol codec locality | `crates/ultty-protocol/src/codec.rs`, `crates/ultty-protocol/tests/roundtrip.rs` | Layout, content, input, frame, and terminal message encoding all live in one large Implementation. | Split codec helpers by message family while keeping `AppMessage` and `TerminalMessage` encode/decode as the public Interface. | `cargo test -p ultty-protocol` |
| Terminal grid locality | `crates/ultty/src/grid/mod.rs`, `crates/ultty/src/grid/terminal.rs`, `crates/ultty/src/grid/term_handler.rs` | Cursor movement, write paths, scroll regions, reflow, dirty tracking, and tests are concentrated in large modules. | Extract dirty tracking or reflow planning first; keep `Grid` behavior unchanged. | `cargo test -p ultty --lib grid` |
| Renderer planning | `crates/ultty/src/renderer/mod.rs`, `crates/ultty/src/renderer/text.rs`, `crates/ultty/src/renderer/panes.rs` | Terminal, native regions, backgrounds, overlays, scrollbars, animations, and cache handling are coupled in the renderer entrypoint. | Extract a render-plan builder for dirty terminal/native surfaces. | `cargo test -p ultty --lib renderer` plus live smoke |
| Framework runtime | `crates/ulttui/src/app.rs` | Command scheduling, subscriptions, screen stack, backend polling, rendering, and cleanup share one broad runtime module. | Extract subscription reconciliation and command scheduling behind internal runtime modules. | `cargo test -p ulttui --lib app` |
| Widget readiness | `crates/ulttui/src/widgets/*`, `docs/WIDGET_REFERENCE.md` | The widget catalog is broad, but readiness is not tracked per widget across native mode, ANSI mode, focus/input, resize, and snapshots. | Maintain [Widget Readiness](widget-readiness.md) and file focused issues from gaps. | `cargo test -p ulttui --test snapshots` |

## Slice Rules

- One responsibility per commit.
- Prefer moving code before changing behavior.
- Keep adapters narrow and named after the boundary they protect.
- Add focused tests only where the extraction exposes an untested behavior.
- Do not mix release-readiness docs with broad implementation refactors unless
  the release gate depends on that exact change.
