# ultty

**Purpose**: GPU-accelerated terminal emulator with native TUI protocol
support. Also produces the `ulttyctl` CLI.

Repository-wide conventions live in [CLAUDE.md](../../CLAUDE.md) and
[AGENTS.md](../../AGENTS.md). This file covers only what is specific to this
crate.

## Structure

```
src/
├── main.rs             # Entry point
├── lib.rs              # Library surface used by tests and ulttyctl
├── bin/                # ulttyctl CLI
├── app/                # winit event loop, input, render driving, control plane
├── control/            # Control socket server and request handling
├── renderer/           # GPU rendering (wgpu, cosmic-text)
│   ├── pipeline.rs     # wgpu render pipeline
│   ├── atlas.rs        # Glyph atlas
│   ├── text.rs         # Text shaping and layout
│   ├── native_render/  # Native protocol region rendering
│   ├── panes.rs        # Split pane rendering
│   ├── tabbar.rs       # Tab chrome
│   ├── overlays.rs     # Shortcut and protocol status overlays
│   └── screenshot.rs   # Headless capture for smokes
├── protocol/           # Protocol handling
│   ├── ansi/           # VT100/xterm parser
│   ├── server.rs       # Native protocol Unix socket server
│   └── session/        # Per-client native session state
├── grid/               # Terminal cell grid and scrollback
├── pty/                # PTY spawn and IO
├── scene/              # Scene graph
│   ├── region.rs       # Region node
│   ├── tree.rs         # Tree operations
│   └── layout/         # Taffy integration
├── vim/                # Vim-style copy mode
├── testing/            # TestTerminal helper
└── platform/           # Platform-specific code
```

Run `ls crates/ultty/src` when this tree looks stale; the filesystem wins.

## Key Responsibilities

1. **Window management**: Create window via winit
2. **GPU rendering**: Render scene graph via wgpu
3. **Protocol handling**: Parse both native protocol AND legacy ANSI
4. **Layout computation**: Run Taffy on constraint changes
5. **Input dispatch**: Route events to focused regions
6. **PTY management**: Fork shell processes (for legacy apps)

## Rendering Pipeline

```
Scene Graph Changes
       │
       ▼
┌──────────────┐
│   Dirty?     │──No──► Skip
└──────┬───────┘
       │Yes
       ▼
┌──────────────┐
│ Layout Pass  │  ◄── Taffy computes rects
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ Render Pass  │  ◄── Generate GPU commands
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ GPU Submit   │  ◄── wgpu frame submission
└──────────────┘
```

## Testing Strategy

- Unit tests for protocol and ANSI parsing, inline in the source files
- Integration tests in `crates/ultty/tests/`, driven by `ultty::testing::TestTerminal`
- Benchmarks under `crates/ultty/benches/` (`cargo bench -p ultty`)
- Visual regression through the screenshot smoke scripts; see
  [docs/visual-regression.md](../../docs/visual-regression.md)

## Output Rules

Diagnostics go through `tracing`. The `ulttyctl` binary is the one place that
writes to stdout with `println!`, because its stdout is the CLI's result.
