# Ultty - Project DNA

**Mission**: Build the terminal emulator + TUI framework that makes Claude Code's architecture obsolete.

## Quick Context

```
ultty/
├── crates/
│   ├── ultty/            # The terminal emulator (GPU-rendered)
│   ├── ulttui/           # The framework (apps link this)
│   └── ultty-protocol/   # Shared wire protocol
├── examples/             # Reference implementations
├── docs/                 # Specs and guides
└── tools/                # Dev utilities
```

## The Two Products

| Crate | What It Is | Who Uses It |
|-------|-----------|-------------|
| `ultty` | Terminal emulator binary | End users install this |
| `ulttui` | TUI framework library | Developers build apps with this |
| `ultty-protocol` | Wire protocol definitions | Both above depend on this |

## Architecture (30-Second Version)

```
┌─────────────────────────────────────────────────────────────┐
│                    User's Terminal                          │
│                       (ultty)                               │
├─────────────────────────────────────────────────────────────┤
│  Scene Graph ◄── Layout Engine (Taffy) ◄── Protocol Parser │
│       │                                          ▲          │
│       ▼                                          │          │
│  GPU Renderer ──────────────────────── Binary Protocol      │
│    (wgpu)                              (Unix socket)        │
└─────────────────────────────────────────────────────────────┘
                                               ▲
                                               │
┌──────────────────────────────────────────────┴─────────────┐
│                      TUI App                                │
│                  (built with ulttui)                        │
├─────────────────────────────────────────────────────────────┤
│  App State ──► Component Tree ──► Protocol Messages         │
│                                                             │
│  When in ultty:          Native binary protocol             │
│  When in other terminal: ANSI fallback (via ratatui)        │
└─────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

1. **Terminal-side layout**: Apps declare constraints, terminal computes positions
2. **Retained mode scene graph**: Terminal owns the tree, apps send diffs
3. **Atomic frames**: `BeginFrame`/`EndFrame` prevents tearing
4. **Dual-mode framework**: Native protocol when possible, ANSI fallback otherwise

## File Navigation

| Looking for... | Go to... |
|----------------|----------|
| Protocol wire format | `docs/spec/PROTOCOL_SPEC.md` |
| Architecture overview | `docs/architecture/OVERVIEW.md` |
| Terminal emulator code | `crates/ultty/` |
| Framework code | `crates/ulttui/` |
| Example apps | `examples/` |

## Development Commands

```bash
# Run terminal emulator
cargo run -p ultty

# Run example chat app
cargo run -p example-chat

# Run tests
cargo test --workspace

# Check all targets
cargo check --workspace --all-targets

# Format
cargo fmt --all

# Lint
cargo clippy --workspace --all-targets
```

## Coding Standards

- **Error handling**: Use `thiserror` for library errors, `anyhow` in binaries/examples
- **Logging**: Use `tracing` crate, not `println!` or `log`
- **Unsafe**: Avoid; if required, document why and add `// SAFETY:` comment
- **Tests**: Unit tests in same file, integration tests in `tests/` directory
- **Docs**: All public items must have doc comments

## Current Status

- [x] Protocol spec drafted
- [ ] Protocol implementation
- [ ] Terminal: VT100 parser
- [ ] Terminal: GPU renderer
- [ ] Terminal: Native protocol handler
- [ ] Framework: Component system
- [ ] Framework: ANSI fallback
- [ ] Example: Chat app

## Dependencies Policy

- Minimize dependencies
- Prefer `no_std` compatible crates where possible
- Audit new dependencies for security
- Pin versions in workspace

## Performance Targets

| Metric | Target |
|--------|--------|
| Layout recompute | < 1ms |
| Frame render | < 5ms |
| Input latency | < 10ms |
| Memory (terminal) | < 50MB |
| Memory (framework) | < 10MB |
