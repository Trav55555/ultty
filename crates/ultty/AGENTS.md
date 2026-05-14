# ultimate-terminal

**Purpose**: GPU-accelerated terminal emulator with native TUI protocol support.

## Structure

```
src/
├── main.rs             # Entry point, window setup
├── app.rs              # Application state machine
├── renderer/           # GPU rendering
│   ├── mod.rs
│   ├── pipeline.rs     # wgpu render pipeline
│   ├── glyph_cache.rs  # Font glyph caching
│   └── texture.rs      # Texture management
├── input/              # Input handling
│   ├── mod.rs
│   ├── keyboard.rs     # Key mapping
│   └── mouse.rs        # Mouse events
├── protocol/           # Protocol handling
│   ├── mod.rs
│   ├── native.rs       # Binary protocol parser
│   ├── ansi.rs         # VT100/ANSI parser
│   └── connection.rs   # Unix socket / IPC
├── scene/              # Scene graph
│   ├── mod.rs
│   ├── region.rs       # Region node
│   ├── tree.rs         # Tree operations
│   └── layout.rs       # Taffy integration
└── platform/           # Platform-specific code
    ├── mod.rs
    ├── linux.rs        # Linux-specific (pty, etc)
    └── macos.rs        # macOS-specific
```

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

## Conventions

- Commit frequently — make small, atomic commits after each logical unit of work

## Testing Strategy

- Unit tests for protocol parsing
- Integration tests with mock TUI client
- Benchmarks for render performance
- Visual regression tests (screenshot comparison)
