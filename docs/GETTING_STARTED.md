# Getting Started with Ultty

This guide will help you get started with the Ultty terminal emulator and TUI framework.

## Installation

### From Source

```bash
git clone https://github.com/yourusername/ultty.git
cd ultty
cargo build --release

# Install the terminal
cargo install --path crates/ultty
```

### Running the Terminal

```bash
# Run ultty
ultty

# Or run from cargo
cargo run -p ultty
```

## Building TUI Applications

Ultty includes a framework (`ulttui`) for building TUI applications that work in any terminal but get native acceleration in Ultty.

### Hello World

```rust
use ulttui::prelude::*;

struct HelloApp;

impl Application for HelloApp {
    type Message = ();
    
    fn view(&self) -> impl View {
        Text::new("Hello, World!")
    }
}

fn main() {
    HelloApp.run().unwrap();
}
```

### Counter Example

```rust
use ulttui::prelude::*;

struct Counter {
    count: i32,
}

enum Msg {
    Increment,
    Decrement,
}

impl Application for Counter {
    type Message = Msg;
    
    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Increment => self.count += 1,
            Msg::Decrement => self.count -= 1,
        }
    }
    
    fn view(&self) -> impl View {
        Column::new()
            .child(Text::new(format!("Count: {}", self.count)))
            .child(
                Row::new()
                    .child(Button::new("-").on_click(|| Msg::Decrement))
                    .child(Button::new("+").on_click(|| Msg::Increment))
            )
    }
}

fn main() {
    Counter { count: 0 }.run().unwrap();
}
```

## Running Examples

```bash
# Raw protocol demo (run inside ultty)
cargo run -p hello-native

# Counter app
cargo run -p counter-app

# Todo list app
cargo run -p todo-app
```

## Configuration

Ultty looks for configuration in `~/.config/ultty/ultty.toml`:

```toml
[font]
family = "JetBrains Mono"
size = 14.0

[colors]
foreground = "#c0caf5"
background = "#1a1b26"

[behavior]
bell_visual = true
bell_sound = false
```

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Ultty Terminal                           │
├─────────────────────────────────────────────────────────────┤
│  Scene Graph ◄── Layout Engine ◄── Protocol Parser          │
│       │              (Taffy)            ▲                   │
│       ▼                                 │                   │
│  GPU Renderer ────────────────── Binary Protocol            │
│    (wgpu)                        (Unix socket)              │
└─────────────────────────────────────────────────────────────┘
                                          ▲
                                          │
┌─────────────────────────────────────────┴───────────────────┐
│                      TUI App (ulttui)                       │
├─────────────────────────────────────────────────────────────┤
│  App State ──► Component Tree ──► Protocol Messages         │
│                                                             │
│  In ultty:  Native binary protocol (zero parsing overhead)  │
│  Elsewhere: ANSI fallback (via ratatui)                     │
└─────────────────────────────────────────────────────────────┘
```

## Key Features

- **GPU Rendering**: Hardware-accelerated rendering via wgpu
- **Native Protocol**: Binary protocol for TUI apps (no ANSI parsing)
- **Dual-Mode Framework**: Apps work in any terminal, optimized in ultty
- **Performance**: Sub-millisecond layout, <5ms frame render
- **Modern Features**: Sixel images, hyperlinks, shell integration

## Next Steps

- Read the [Protocol Specification](PROTOCOL_SPEC.md) for native app development
- Check the [Testing Strategy](TESTING_STRATEGY.md) for testing approaches
- Explore the example apps in `examples/`
