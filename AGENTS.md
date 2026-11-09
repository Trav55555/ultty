# Ultty — Agent Guide

GPU-accelerated terminal emulator + TUI framework. Rust workspace, edition 2021,
MSRV 1.89 (`rust-version` in the root `Cargo.toml`). The toolchain is pinned by
`rust-toolchain.toml`.

[CLAUDE.md](CLAUDE.md) is the authoritative guide for build commands,
architecture, coding conventions, and layout pitfalls. Read it first. This file
covers only what CLAUDE.md does not: code style details, testing conventions,
and the dependency policy.

[CONTRIBUTING.md](CONTRIBUTING.md) has the human-facing version of the same
workflow.

## Repository Layout

```
crates/
  ultty-vt/         # Terminal emulation core: VT parser + grid/scrollback, no I/O
  ultty/            # Terminal emulator binary (GPU renderer, PTY, native protocol)
  ulttui/           # TUI framework library (Elm architecture, 28 widget modules, dual-mode)
  ultty-protocol/   # Wire protocol codec (binary encode/decode, layout types)
  ultty-client/     # Low-level client SDK (raw protocol access)
examples/           # 9 workspace example apps: hello-native, counter-app, todo-app,
                    # dashboard, chat, form-app, split-panes, sysmon, async-fetch.
                    # porygon-tui sits under examples/ but is excluded from the
                    # workspace; build it with
                    # `cargo build --manifest-path examples/porygon-tui/Cargo.toml`.
docs/               # Project documentation; docs/archive/ holds historical plans
scripts/            # Smoke gates, screenshot capture, security audit
```

Integration tests live in each crate's own `tests/` directory. There is no
workspace-level `tests/` directory.

## Build / Test / Lint

Commands are listed in [CLAUDE.md](CLAUDE.md). Do not hard-code test counts in
documentation; run `cargo test --workspace` and read the number from there.

**Remotes.** `origin` is Gitea (`git.traversannan.com/trav/ultty`), where PRs
and CI live; GitHub (`github.com/Trav55555/ultty`) is a push mirror. Open PRs
on Gitea (`tea pulls create`), not GitHub.

**CI** (`.gitea/workflows/ci.yml`, Gitea Actions on Linux, `runs-on: ultty`)
runs nine jobs: `fmt`, `security-audit`, `clippy` (`--all-targets -D
warnings`), `test`, `msrv` (`cargo check --workspace --all-targets` on 1.89),
`porygon-tui`, `vt-integrity`, `fuzz`, and `gui-smoke` (Ultty under Xvfb with
software Vulkan). The `ultty` pool's image, defined in the gitea-server
repository, provides every toolchain and package; jobs install nothing, and a
toolchain bump means a new image. There is no coverage or
benchmark job, and no macOS runner: macOS is only built by the release
workflow. Windows requires future named-pipe/ConPTY platform work.
**RUSTFLAGS=-Dwarnings** — all warnings are errors.

`.github/workflows/live-smoke.yml` (manual dispatch, self-hosted GUI runner) and
`.github/workflows/release.yml` (tag push) are the other two workflows; they
run on GitHub, from the mirror.

## Code Style

### Imports

Group in order, separated by blank lines: (1) std, (2) external crates, (3) internal crates, (4) crate-local.

```rust
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::View;
use crate::theme::Theme;
use ultty_protocol::{Key, Modifiers};
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

- `tracing` crate only for diagnostics. Never `log`. Never `println!` in library
  or runtime code. Exception: CLI binaries write their own output with
  `println!` (`ulttyctl` command results, `--version`).
- Avoid `unsafe`. The codebase has a small number of unsafe blocks, mostly in
  the platform and renderer layers. Every unsafe block needs a `// SAFETY:`
  comment explaining the invariant it relies on.

### Formatting

Standard `rustfmt` defaults (no config file). `rust-analyzer.toml` sets clippy as check command.

### Commit Discipline

- Commit frequently — make small, atomic commits after each logical unit of work.
- No AI attribution footers or trailers in commit messages or PR bodies.

## Testing

- **Unit**: Inline `#[cfg(test)] mod tests { ... }` in same file
- **Integration**: `tests/` directory inside the crate under test
- **Snapshot**: `insta` for widget rendering golden files → `cargo insta review`
- **Property**: `proptest` for codec roundtrips
- **Names**: `test_<thing>_<scenario>` (e.g., `test_parse_empty_input`)
- **Helpers**: `ultty::testing::TestTerminal` (headless terminal), `MockBackend` (framework, `test-utils` feature)

See [docs/TESTING_STRATEGY.md](docs/TESTING_STRATEGY.md) for the full picture.

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

**Policy**: Minimize deps. Pin versions in the workspace `Cargo.toml`. Several
of the graphics and TUI dependencies are well behind current releases; upgrading
them is tracked in [docs/ISSUE_BACKLOG_30_60_90.md](docs/ISSUE_BACKLOG_30_60_90.md).

## Widgets

All implement `View` with `content()` (native) and `render_ansi()` (fallback).
Interactive widgets support `.id()` for focus. `Tab`/`Shift+Tab` cycles focus.
[docs/WIDGET_REFERENCE.md](docs/WIDGET_REFERENCE.md) is the full list;
`crates/ulttui/src/widgets/` is the source of truth.
