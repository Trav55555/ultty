# Ultty Architecture

## Overview

For the canonical project vocabulary used in architecture work, see
[`CONTEXT.md`](../CONTEXT.md). For accepted design decisions, see
[`docs/adr/`](adr/). For the immediate-output / retained-internals
layout contract, see [`docs/LAYOUT_MODEL.md`](LAYOUT_MODEL.md).

Ultty is composed of these main crates:

| Crate | Purpose | Users |
|-------|---------|-------|
| `ultty-vt` | Terminal emulation core (VT parser, grid, scrollback); no I/O | `ultty`, fuzzers, embedders |
| `ultty` | Terminal emulator binary | End users |
| `ulttui` | TUI framework library | App developers |
| `ultty-protocol` | Wire protocol definitions | Both above |

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        User System                               │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                  Ultty Terminal (ultty)                   │   │
│  ├──────────────────────────────────────────────────────────┤   │
│  │                                                          │   │
│  │  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │   │
│  │  │  EventLoop  │───►│  Terminal   │───►│  Renderer   │  │   │
│  │  │             │    │  (Grid)     │    │  (wgpu)     │  │   │
│  │  └──────┬──────┘    └─────────────┘    └─────────────┘  │   │
│  │         │                                                │   │
│  │         │ PTY I/O                                        │   │
│  │         ▼                                                │   │
│  │  ┌─────────────┐    ┌─────────────────────────────────┐ │   │
│  │  │  PTY/Shell  │    │  Protocol Server                │ │   │
│  │  │  (bash,zsh) │    │  (one Unix socket per tab)      │ │   │
│  │  └─────────────┘    └──────────────┬──────────────────┘ │   │
│  │                                    │                     │   │
│  └────────────────────────────────────┼─────────────────────┘   │
│                                       │                          │
│                                       │ Native Protocol          │
│                                       ▼                          │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                  TUI Application (ulttui)                 │   │
│  ├──────────────────────────────────────────────────────────┤   │
│  │  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │   │
│  │  │  App State  │───►│   Views     │───►│  Backend    │  │   │
│  │  │  + Logic    │    │  (widgets)  │    │  (native/   │  │   │
│  │  └─────────────┘    └─────────────┘    │   ansi)     │  │   │
│  │                                         └─────────────┘  │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Terminal Emulator (ultty)

### Event Loop

The event loop (`event_loop.rs`) is the heart of the terminal:

```
                    ┌─────────────────────┐
                    │     Event Loop      │
                    └─────────┬───────────┘
                              │
         ┌────────────────────┼────────────────────┐
         ▼                    ▼                    ▼
   ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
   │  PTY Read   │    │ Socket Read │    │   Timers    │
   │  (shell)    │    │ (native)    │    │  (cursor)   │
   └─────────────┘    └─────────────┘    └─────────────┘
```

### Grid System

The terminal grid manages cells, scrollback, and selection:

```
Terminal
├── main_buffer: Buffer (primary screen)
│   ├── grid: Grid (visible cells)
│   └── history: Vec<Row> (scrollback)
├── alternate_buffer: Buffer (for vim, etc.)
├── selection: Option<Selection>
└── images: ImageStore (sixel images)
```

### Rendering Pipeline

```
Grid Changes ──► Layout Pass ──► Render Pass ──► GPU Submit
                (Taffy)         (vertices)      (wgpu)
```

## TUI Framework (ulttui)

### Application Model (Elm Architecture)

```
     ┌──────────────────────────────────────────┐
     │                                          │
     ▼                                          │
┌─────────┐      ┌─────────┐      ┌─────────┐  │
│  View   │─────►│ Message │─────►│ Update  │──┘
│  f(s)   │      │         │      │  f(s,m) │
└─────────┘      └─────────┘      └─────────┘
```

### Backend Selection

```rust
// Automatic detection
if env::var("ULTTY_SOCKET").is_ok() {
    NativeBackend::connect()  // Binary protocol
} else {
    AnsiBackend::new()        // ratatui fallback
}
```

## Protocol (ultty-protocol)

### Message Flow

```
App                                   Terminal
 │                                        │
 │──── ClientHello { version } ──────────►│
 │◄─── ServerHello { version, caps } ─────│
 │◄─── SessionReady { id, size } ─────────│
 │                                        │
 │──── BeginFrame { id } ────────────────►│
 │──── CreateRegion { ... } ─────────────►│
 │──── SetContent { ... } ───────────────►│
 │──── EndFrame { id } ──────────────────►│
 │                                        │
 │◄─── LayoutComputed { ... } ────────────│
 │◄─── InputEvent { ... } ────────────────│
 │                                        │
```

### Native Session Split

The terminal-side native protocol session keeps socket I/O, keepalive, and
session orchestration in `crates/ultty/src/protocol/session.rs`. Message-specific
state changes live in `protocol::session::commands`, frame batching lives in
`protocol::session::frame`, and scene-to-layout output lives behind
`scene::layout::LayoutEngine::compute_scene_layout`.

See [ADR 0001](adr/0001-native-session-decomposition.md) for the native session
decomposition decision and testing strategy.

### Wire Format

```
┌────────────┬──────────┬────────────┬──────────────────┐
│  Magic (2) │ Type (2) │ Length (4) │  Payload (var)   │
│   "UT"     │  u16 LE  │  u32 LE    │  Binary codec    │
└────────────┴──────────┴────────────┴──────────────────┘
```

## Performance Characteristics

Ultty performance is judged first by idle efficiency and user-visible interaction responsiveness. The current reproducible workstation baseline is [docs/perf/2026-05-14-baseline.md](perf/2026-05-14-baseline.md); release claims should link to a dated artifact like that baseline rather than uncited local measurements.

| Operation | Target |
|-----------|--------|
| Idle CPU/GPU work | near-zero when terminal and native app state are unchanged |
| Layout compute | < 1ms |
| Frame render | < 5ms |
| Input latency | < 10ms |
| Memory (terminal) | < 50MB |

## Key Design Decisions

1. **Immediate output, retained internals**: `ulttui` apps rebuild views from state each render while backends retain protocol/layout caches
2. **Terminal-side layout**: Apps declare constraints, terminal computes positions
3. **Retained mode scene graph**: Terminal owns the tree, apps send diffs
4. **Atomic frames**: BeginFrame/EndFrame prevents tearing
5. **Dual-mode framework**: ANSI mode and Ultty-native mode are both product commitments; native mode adds capability uplift
6. **Native session decomposition**: command application, frame batching, and layout output have named seams behind `NativeSession`

## File Organization

```
ultty/
├── crates/
│   ├── ultty/              # Terminal emulator
│   │   ├── src/
│   │   │   ├── grid/       # Grid, cells, ANSI parsing
│   │   │   ├── renderer/   # GPU rendering
│   │   │   ├── protocol/   # Native protocol server
│   │   │   └── ...
│   │   └── benches/        # Performance benchmarks
│   ├── ulttui/             # TUI framework
│   │   ├── src/
│   │   │   ├── widgets/    # Built-in widgets
│   │   │   ├── backend/    # Native and ANSI backends
│   │   │   └── ...
│   │   └── tests/          # Snapshot tests
│   └── ultty-protocol/     # Wire protocol
├── examples/               # Example applications
└── docs/                   # Documentation
```
