# ultimate-protocol

**Purpose**: Defines the wire protocol between terminal and TUI apps.

Repository-wide conventions live in [CLAUDE.md](../../CLAUDE.md) and
[AGENTS.md](../../AGENTS.md). This file covers only what is specific to this
crate.

## Structure

```
src/
├── lib.rs          # Public API, re-exports
├── messages.rs     # Message types (CreateRegion, SetContent, etc.)
├── layout.rs       # Layout constraint types (mirrors Taffy)
├── style.rs        # Style types (colors, borders, etc.)
├── content.rs      # Content types (text, widgets)
├── events.rs       # Event types (input, resize, etc.)
├── control.rs      # Control-socket request/response types
├── vim.rs          # Vim mode and action types shared with the terminal
├── codec.rs        # Serialization/deserialization
├── codec/          # Codec submodules
└── error.rs        # Protocol errors
```

## Design Principles

1. **Compact and self-delimiting**: Length-prefixed binary frames, so the
   terminal never has to re-parse a text stream to recover structure. The
   decoder allocates owned values; it is not zero-copy.
2. **Bounded**: Every length read is checked against a size limit before
   allocating
3. **Version-tagged**: All messages include version for compatibility
4. **Bidirectional**: Both app→terminal and terminal→app messages

## Key Types

```rust
// Region identity
type RegionId = u32;

// Main message enum
enum AppMessage {
    CreateRegion { ... },
    UpdateRegion { ... },
    DeleteRegion { ... },
    SetContent { ... },
    BeginFrame { ... },
    EndFrame { ... },
    // ...
}

enum TerminalMessage {
    LayoutComputed { ... },
    InputEvent { ... },
    ResizeEvent { ... },
    // ...
}
```

## Testing Strategy

- Property-based tests for roundtrip serialization
- Fuzz testing for parser robustness
- Benchmark critical paths (encode/decode)

## Conventions

- Commit frequently — make small, atomic commits after each logical unit of work

## See Also

- [`docs/PROTOCOL_SPEC.md`](../../docs/PROTOCOL_SPEC.md) - Full protocol
  specification. `src/codec.rs` is normative where the two disagree.
