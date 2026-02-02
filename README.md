# Ultty

A GPU-accelerated terminal emulator with a native TUI protocol, designed to make traditional ANSI-based architectures obsolete.

## Vision

Modern TUI applications are bottlenecked by the 1970s ANSI protocol: apps compute layout, serialize to escape sequences, terminals re-parse and re-render. Ultty eliminates this by:

- **Terminal-side layout**: Apps declare constraints, terminal computes positions
- **Retained scene graph**: Terminal owns the tree, apps send diffs
- **Binary protocol**: No parsing overhead, direct structured messages
- **Atomic frames**: `BeginFrame`/`EndFrame` prevents tearing

## Architecture

```
ultty/
├── crates/
│   ├── ultty/            # Terminal emulator (GPU-rendered via wgpu)
│   ├── ulttui/           # TUI framework library (apps link this)
│   └── ultty-protocol/   # Shared wire protocol definitions
├── examples/             # Reference implementations
└── docs/                 # Specs and architecture docs
```

## Quick Start

```bash
# Run the terminal emulator
cargo run -p ultty

# Run example apps (inside ultty terminal)
cargo run -p hello-native   # Raw protocol demo
cargo run -p counter-app    # Framework counter
cargo run -p todo-app       # Framework todo list
```

## Current Status

- [x] Protocol specification and implementation
- [x] GPU renderer with wgpu
- [x] VT100/ANSI parser (legacy compatibility)
- [x] Native protocol handler with scene graph
- [x] Framework with component system
- [x] ANSI fallback mode (run in any terminal)
- [x] Protocol hardening (bounds checks, size limits)
- [x] Performance benchmarks

## Documentation

- [Getting Started](docs/GETTING_STARTED.md) - Installation and first app
- [Architecture](docs/ARCHITECTURE.md) - System design and components
- [Protocol Specification](docs/PROTOCOL_SPEC.md) - Native protocol details
- [Testing Strategy](docs/TESTING_STRATEGY.md) - How to test

## Development

```bash
cargo test --workspace      # Run all tests
cargo check --workspace     # Type check
cargo clippy --workspace    # Lint
cargo fmt --all             # Format
```

## Performance Targets

| Metric | Target | Measured | Margin |
|--------|--------|----------|--------|
| Layout recompute | < 1ms | 27µs | 37x |
| Frame render (typical) | < 5ms | 290µs | 17x |
| Frame render (empty) | < 5ms | 102µs | 49x |
| Input latency (internal) | < 10ms | ~0.3ms | 33x |
| Memory (terminal) | < 50MB | ~18MB | 2.8x |

All targets met with significant margins. Run benchmarks: `cargo bench -p ultty`

## License

MIT
