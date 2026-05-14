# Ultty

A GPU-accelerated terminal that replaces ANSI escape sequences with a native binary protocol for TUI applications.

## The Problem

Every terminal application today renders through the same 1970s pipeline:

```
App computes layout → Serializes to ANSI escape sequences → Terminal parses escapes → Terminal re-renders
```

This means layout is computed twice (app and terminal), every frame is serialized and re-parsed as text, there are no layout primitives (apps manually position every character), and partial updates cause tearing because there's no frame boundary.

## The Solution

Ultty introduces a native binary protocol that lets applications declare *what* to render, while the terminal handles *where* and *how*:

```
App declares constraints ──→ Binary protocol ──→ Terminal computes layout ──→ GPU renders
                                                       ▲
                                                 Retained scene graph
                                                 (apps send diffs, not full frames)
```

**Terminal-side layout** — Apps send flex/grid constraints. The terminal runs Taffy layout and positions regions.

**Retained scene graph** — The terminal owns a tree of regions. Apps create, update, and delete regions. Only diffs cross the wire.

**Binary protocol** — Structured messages, no parsing overhead. `CreateRegion`, `SetContent`, `SetConstraints` instead of escape sequences.

**Atomic frames** — `BeginFrame`/`EndFrame` batching prevents tearing. The terminal renders nothing until the frame is complete.

## Quick Start

### Run the terminal

```bash
cargo run -p ultty
```

This opens Ultty as a GPU-accelerated terminal emulator. It runs any existing terminal application (vim, htop, tmux, etc.) through full VT100/xterm ANSI compatibility.

Platform status: Ultty is Unix-first today. Linux and macOS are the supported runtime targets; Windows support is future work that requires a native transport and PTY platform boundary.

### Run native apps inside Ultty

From within an Ultty terminal:

```bash
cargo run -p hello-native   # Raw protocol demo
cargo run -p counter-app    # Elm-architecture counter
cargo run -p todo-app       # Todo list with vim navigation
cargo run -p dashboard      # Multi-panel dashboard with live updates
```

Native apps auto-detect whether they're running inside Ultty. Inside Ultty, they use the native protocol for zero-overhead rendering. Outside Ultty, they fall back to ANSI rendering via ratatui — the same binary works in any terminal.

## ANSI Compatibility

Ultty is a fully functional terminal emulator. ANSI support is not an afterthought — it's the baseline:

- VT100/VT220/xterm escape sequences
- 256-color and 24-bit true color
- Mouse tracking (SGR, X10, normal)
- Bracketed paste, focus reporting
- Sixel graphics
- Alternate screen buffer
- Synchronized update (mode 2026)
- Tabs, splits, scrollback, clipboard (OSC 52)

## Framework: ulttui

`ulttui` is an Elm-architecture framework for building native TUI apps:

```rust
use ulttui::prelude::*;

struct App { count: i32 }

enum Msg { Inc, Dec }

impl Application for App {
    type Message = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Inc => self.count += 1,
            Msg::Dec => self.count -= 1,
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Key(k) => match k.key {
                Key::Char('+') => Some(Msg::Inc),
                Key::Char('-') => Some(Msg::Dec),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .child(Text::new(format!("Count: {}", self.count)))
            .child(Row::new()
                .child(Button::new("-"))
                .child(Button::new("+")))
    }
}

fn main() -> anyhow::Result<()> {
    App { count: 0 }.run()
}
```

Widgets: `Text`, `Button`, `List`, `TextInput`, `ScrollView`, `Column`, `Row`, `Stack`

All widgets implement both native protocol rendering and ANSI fallback.

## Performance

| Metric | Target | Measured | Margin |
|--------|--------|----------|--------|
| Layout recompute | < 1ms | 27µs | 37x |
| Frame render (typical) | < 5ms | 290µs | 17x |
| Frame render (empty) | < 5ms | 102µs | 49x |
| Input latency (internal) | < 10ms | ~0.3ms | 33x |
| Memory (terminal) | < 50MB | ~18MB | 2.8x |

Run benchmarks: `cargo bench -p ultty`

## Architecture

```
crates/
├── ultty/            Terminal emulator — GPU renderer (wgpu), ANSI parser,
│                     native protocol handler, scene graph, tabs/splits
├── ulttui/           TUI framework — Application trait, widgets, Elm architecture,
│                     dual-mode rendering (native + ANSI fallback)
└── ultty-protocol/   Wire protocol — Message definitions, layout types,
                      content types, shared between terminal and apps
examples/
├── hello-native/     Raw protocol usage (no framework)
├── counter-app/      Framework counter demo
├── todo-app/         Todo list with vim navigation
└── dashboard/        Multi-panel dashboard with live updates
```

## Status & Roadmap

**Working:**
- Native binary protocol with retained scene graph
- GPU-accelerated rendering via wgpu
- Full VT100/xterm ANSI compatibility
- Framework with Elm architecture and widget system
- Dual-mode apps (native inside Ultty, ANSI fallback elsewhere)
- Flexbox and grid layout (via Taffy)
- Protocol hardening (bounds checks, size limits)
- Tabs, splits, vim-style copy mode

**Next:**
- Mouse event forwarding to native apps
- Animation primitives
- Image widget (native protocol)
- Scrollback search in native regions
- Package for distribution

## Documentation

- [Getting Started](docs/GETTING_STARTED.md) — Installation and first app
- [Architecture](docs/ARCHITECTURE.md) — System design and components
- [Architecture Vocabulary](CONTEXT.md) — Stable terms for core seams
- [Protocol Specification](docs/PROTOCOL_SPEC.md) — Native protocol details
- [Testing Strategy](docs/TESTING_STRATEGY.md) — How to test
- [30/60/90 Execution Plan](docs/ROADMAP_30_60_90.md) — Milestones, ticket map, and acceptance criteria
- [30/60/90 Issue Backlog](docs/ISSUE_BACKLOG_30_60_90.md) — Copy-paste issue templates with dependencies

## Development

```bash
cargo test --workspace      # Run all tests
cargo check --workspace     # Type check
cargo clippy --workspace    # Lint
cargo fmt --all             # Format
```

## License

MIT
