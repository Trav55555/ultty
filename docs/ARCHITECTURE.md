# Ultty Architecture

## Overview

Ultty is composed of three main crates:

| Crate | Purpose | Users |
|-------|---------|-------|
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
│  │  │  (bash,zsh) │    │  (Unix Socket)                  │ │   │
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

### Wire Format

```
┌────────────┬────────────┬──────────────────┐
│  Magic (2) │ Length (4) │  Payload (var)   │
│   "UT"     │  u32 LE    │  MessagePack     │
└────────────┴────────────┴──────────────────┘
```

## Performance Characteristics

| Operation | Target | Achieved |
|-----------|--------|----------|
| Layout compute | < 1ms | ~27µs |
| Frame render | < 5ms | ~290µs |
| Input latency | < 10ms | ~0.3ms |
| Memory (terminal) | < 50MB | ~18MB |

## Key Design Decisions

1. **Terminal-side layout**: Apps declare constraints, terminal computes positions
2. **Retained mode scene graph**: Terminal owns the tree, apps send diffs
3. **Atomic frames**: BeginFrame/EndFrame prevents tearing
4. **Dual-mode framework**: Native when possible, ANSI fallback otherwise

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
