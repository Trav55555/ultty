# ultimate-protocol

**Purpose**: Defines the wire protocol between terminal and TUI apps.

## Structure

```
src/
├── lib.rs          # Public API, re-exports
├── messages.rs     # Message types (CreateRegion, SetContent, etc.)
├── layout.rs       # Layout constraint types (mirrors Taffy)
├── style.rs        # Style types (colors, borders, etc.)
├── content.rs      # Content types (text, widgets)
├── events.rs       # Event types (input, resize, etc.)
├── codec.rs        # Serialization/deserialization
└── error.rs        # Protocol errors
```

## Design Principles

1. **Zero-copy where possible**: Use `zerocopy` for fixed-size types
2. **Minimal allocations**: Prefer borrowed data in parsing
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

- `docs/spec/PROTOCOL_SPEC.md` - Full protocol specification
