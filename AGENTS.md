# Ultty — Agent Guide

GPU-accelerated terminal emulator + TUI framework. Rust workspace, edition 2021, MSRV 1.75.

## Repository Layout

```
crates/
  ultty/            # Terminal emulator binary (GPU renderer, ANSI parser, native protocol)
  ulttui/           # TUI framework library (Elm architecture, 22+ widgets, dual-mode)
  ultty-protocol/   # Wire protocol codec (binary encode/decode, layout types)
  ultty-client/     # Low-level client SDK (raw protocol access)
examples/           # 8 example apps (counter, todo, dashboard, chat, porygon-tui, etc.)
tests/              # Workspace-level integration tests
```

## Build / Test / Lint Commands

```bash
# Full workspace
cargo build --workspace
cargo test --workspace                # ~1820 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo fmt --all -- --check            # CI format check

# Single crate
cargo test -p ultty --lib             # Terminal tests (~1005)
cargo test -p ulttui --lib            # Framework tests (~537)
cargo test -p ultty-protocol          # Protocol tests
cargo test -p ulttui --test snapshots # Snapshot tests (25, uses insta)

# Single test by name
cargo test -p ultty -- test_name
cargo test -p ulttui -- test_name

# Snapshot review (after changing widget rendering)
cargo insta review

# Benchmarks
cargo bench -p ultty

# Run
cargo run -p ultty                    # Terminal emulator
cargo run -p counter-app              # Example app
```

**CI** (`.github/workflows/ci.yml`): check, test, clippy (`-D warnings`), fmt (`--check`), security audit, coverage (tarpaulin), benchmarks. Tests on Linux, macOS, Windows. **RUSTFLAGS=-Dwarnings** — all warnings are errors.

## Architecture

Apps built with `ulttui` auto-detect environment:
- **Inside Ultty**: Native binary protocol over Unix socket → terminal-side layout (Taffy) → GPU rendering (wgpu)
- **Any other terminal**: ANSI fallback via ratatui → character-cell rendering

**Elm architecture**: `Application` trait with `update()` → `view()` → `on_event()`. Side effects via `Cmd::spawn()` (async) and `Sub` (subscriptions).

**Retained scene graph**: Terminal owns a region tree. Apps send diffs (`CreateRegion`, `SetContent`, `DeleteRegion`). Atomic `BeginFrame`/`EndFrame` prevents tearing.

## Code Style

### Imports

Group in order, separated by blank lines: (1) std, (2) external crates, (3) internal crates, (4) crate-local.

```rust
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::View;
use crate::theme::Theme;
use ultty_protocol::vim::{VimAction, VimMode};
```

Prefer explicit imports over globs. Preludes exist in `ulttui::prelude` for convenience re-exports.

### Error Handling

- **Library crates** (`ultty-protocol`, `ulttui`): `thiserror` with `#[derive(Debug, Error)]` enums. Define crate-local `Result<T>` alias.
- **Binaries/examples**: `anyhow::Result`, `?` operator throughout.
- Never `unwrap()` in library code (OK in tests). Never suppress errors with empty catch blocks.

```rust
// Library pattern (crates/ultty-protocol/src/error.rs)
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid magic bytes: expected 0x5554, got {0:#06x}")]
    InvalidMagic(u16),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, ProtocolError>;
```

### Naming

- **Modules**: `snake_case` (`event_loop`, `glyph_cache`)
- **Types/Traits**: `PascalCase` (`ProtocolError`, `TextContent`)
- **Functions**: `snake_case` (`render_ansi`, `encode_to_bytes`)
- **Constants**: `SCREAMING_SNAKE_CASE` (`TRANSITION_ALL`, `EVENT_KEY`)
- **Builders**: Fluent methods returning `Self` (`Text::new("hi").id("label")`)

### Documentation

- All public items: `///` doc comments. Module-level: `//!` at top of file.
- Error enum variants: doc comment explaining when each occurs.
- Key APIs: include `## Example` section.

### Logging & Safety

- `tracing` crate only. Never `println!` or `log` in non-test code.
- No `unsafe`. If unavoidable, add `// SAFETY:` comment.

### Formatting

Standard `rustfmt` defaults (no config file). `rust-analyzer.toml` sets clippy as check command.

### Commit Discipline

- Commit frequently — make small, atomic commits after each logical unit of work

## Testing

- **Unit**: Inline `#[cfg(test)] mod tests { ... }` in same file
- **Integration**: `tests/` directory at crate or workspace level
- **Snapshot**: `insta` for widget rendering golden files → `cargo insta review`
- **Property**: `proptest` for codec roundtrips
- **Names**: `test_<thing>_<scenario>` (e.g., `test_parse_empty_input`)
- **Helpers**: `ultty::testing::TestTerminal` (headless terminal), `MockBackend` (framework, `test-utils` feature)

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `wgpu` / `winit` | GPU rendering, window management |
| `taffy` | Flexbox/grid layout engine |
| `ratatui` / `crossterm` | ANSI fallback rendering |
| `tokio` | Async runtime |
| `thiserror` / `anyhow` | Error handling (library / binary) |
| `tracing` | Structured logging |
| `insta` / `proptest` | Snapshot / property testing |

**Policy**: Minimize deps. Prefer `no_std`-compatible. Pin versions in workspace `Cargo.toml`.

## Layout Pitfalls

- `AlignItems::Start` in Row = children keep natural height. Use `Stretch` for fill.
- Content area = region size minus padding AND border.
- ScrollView defaults: `flex_grow: 1.0`, `overflow_y: Scroll`, `flex_direction: Column`.
- Unicode block chars (▁▂▃▄▅▆▇█) render poorly in GPU — use ASCII alternatives.

## Widgets

All implement `View` with `content()` (native) and `render_ansi()` (fallback). Interactive widgets support `.id()` for focus. `Tab`/`Shift+Tab` cycles focus.

**Display**: Text, RichText, Image, ProgressBar, Spinner, Divider, Badge
**Interactive**: Button, TextInput, Checkbox, Toggle, Select
**Data**: List, Table, Tabs
**Containers**: Column, Row, Stack, ScrollView, Modal
