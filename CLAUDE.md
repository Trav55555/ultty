# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build
cargo build --workspace              # Debug build (opt-level=1, deps at opt-level=3)
cargo build --workspace --release    # Release build (LTO, codegen-units=1, panic=abort)

# Test
cargo test --workspace               # All tests
cargo test -p ultty-vt               # VT parser + grid core (no I/O, fast)
cargo test -p ultty --lib            # Terminal emulator tests only
cargo test -p ulttui --lib           # Framework tests only
cargo test -p ulttui --test snapshots # Snapshot tests (uses insta)
cargo test -p ultty -- test_name     # Run a single test by name
cargo insta review                   # Review snapshot diffs interactively

# Lint & Format
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo fmt --all -- --check           # CI format check

# Benchmarks
cargo bench -p ultty                 # Parser, grid, layout, render benchmarks

# Run
cargo run -p ultty                   # Run the terminal emulator
cargo run -p counter-app             # Run an example app

# porygon-tui is excluded from the workspace; build it from its own manifest
cargo build --manifest-path examples/porygon-tui/Cargo.toml
```

The toolchain is pinned by `rust-toolchain.toml`. CI runs the same version plus
an MSRV job; see [CONTRIBUTING.md](CONTRIBUTING.md).

## Architecture

**Ultty** is a GPU-accelerated terminal emulator with a native binary protocol. Instead of ANSI escape sequences, apps declare layout constraints and the terminal computes layout and renders via GPU.

### Crate Dependency Graph

```
ultty (terminal binary)  ←─depends─  ultty-protocol (wire format)
ultty (terminal binary)  ←─depends─  ultty-vt (VT parser + grid, no I/O)
ulttui (TUI framework)   ←─depends─  ultty-protocol
ultty-client (low-level)  ←─depends─  ultty-protocol
examples/*                ←─depends─  ulttui
```

### Core Crates

| Crate | Role |
|-------|------|
| `ultty-vt` | Terminal emulation core: ANSI/VT parser (`ansi`), cells, rows with grapheme clusters, scrollback, reflow, modes (`grid`). No I/O, GPU, or async deps. `ultty` re-exports it as `ultty::grid` and `ultty::protocol::ansi`. |
| `ultty` | Terminal emulator binary. GPU renderer (wgpu), PTY, native protocol server, scene graph, tabs/splits. Also produces `ulttyctl` CLI tool. |
| `ulttui` | TUI application framework. Elm architecture (Application trait with update/view/on_event), 28 widget modules, dual-mode rendering. |
| `ultty-protocol` | Wire protocol codec. Binary message encode/decode, layout constraint types (wraps Taffy), content types, event types. |
| `ultty-client` | Low-level client SDK for apps that want raw protocol access without the framework. |

### Dual-Mode Rendering

Apps built with `ulttui` auto-detect their environment:
- **Inside Ultty**: Native binary protocol over Unix socket → terminal-side layout → GPU rendering
- **Any supported Unix terminal**: ANSI mode via ratatui → traditional character-cell rendering

Same binary works in both modes. Widgets implement both `render_child_native()` and `render_ansi()`.

### Key Patterns

**Elm Architecture**: Apps implement `Application` trait with `update()`, `view()`, `on_event()`. Side effects via `Cmd::spawn()` (async tasks) and `Sub` (subscriptions like timers).

**Retained Scene Graph**: Terminal owns a region tree. Apps send diffs (`CreateRegion`, `SetContent`, `SetConstraints`, `DeleteRegion`). Atomic `BeginFrame`/`EndFrame` prevents tearing.

**Terminal-Side Layout**: Apps send Taffy flex/grid constraints. Terminal runs layout engine and sends back `LayoutComputed`. Protocol defaults: `FlexDirection::Column`, `JustifyContent::Start`, `AlignItems::Start`.

**Focus Management**: ID-based (`widget_id: Option<&str>`). Tab/Shift+Tab navigation. `KeyEvent` includes `focused_id` for semantic dispatch.

### Testing Layers

- **Unit tests**: Inline `#[cfg(test)]` in source files
- **Property tests**: proptest for protocol codec roundtrips
- **Snapshot tests**: insta golden files for widget rendering (both native and ANSI)
- **Integration tests**: `TestTerminal` helper in ultty, `MockBackend` in ulttui
- **E2E automation**: `ulttyctl` CLI for sending input, capturing screenshots, waiting for patterns

## Coding Conventions

- **Error handling**: `thiserror` in library crates, `anyhow` in binaries/examples
- **Logging**: `tracing` crate only, never `println!`. Exception: CLI binaries write their own output with `println!` (`ulttyctl` command results, `--version`).
- **Unsafe**: Avoid; if required, add `// SAFETY:` comment
- **Rust edition**: 2021, MSRV 1.85 (`rust-version` in the root `Cargo.toml`)
- **Commit frequently**: Make small, atomic commits after each logical unit of work

## Layout Pitfalls

- `AlignItems::Start` in a Row means children keep natural height (won't stretch to fill). Use `AlignItems::Stretch` when children should fill the cross axis.
- Content area = region size minus padding AND border.
- ScrollView defaults: `flex_grow: 1.0`, `overflow_y: Scroll`, `flex_direction: Column`.
- Unicode block characters (▁▂▃▄▅▆▇█) render poorly in the GPU renderer — use ASCII alternatives.
