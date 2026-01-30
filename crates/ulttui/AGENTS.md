# ultimate-tui

**Purpose**: TUI framework library that apps link against. Dual-mode: native protocol when in Ultimate Terminal, ANSI fallback otherwise.

## Structure

```
src/
├── lib.rs              # Public API, re-exports
├── app.rs              # App trait and runner
├── widgets/            # Built-in widgets
│   ├── mod.rs
│   ├── text.rs         # Text, RichText
│   ├── input.rs        # TextInput
│   ├── container.rs    # Column, Row, Stack
│   ├── scroll.rs       # ScrollView
│   └── button.rs       # Button
├── layout/             # Layout helpers
│   ├── mod.rs
│   └── flex.rs         # Flexbox shortcuts
├── state/              # State management
│   ├── mod.rs
│   └── signal.rs       # Reactive signals (optional)
├── backend/            # Rendering backends
│   ├── mod.rs
│   ├── native.rs       # Native protocol (Ultimate Terminal)
│   ├── ansi.rs         # ANSI fallback (via Ratatui)
│   └── detect.rs       # Auto-detect which backend to use
└── event.rs            # Event types
```

## DX Goals

1. **Declarative**: UI = f(State)
2. **Elm-like**: Message → Update → View
3. **Auto-fallback**: Works in any terminal
4. **Zero-flicker**: Native mode guaranteed, ANSI mode best-effort

## Usage Pattern

```rust
use ultimate_tui::prelude::*;

struct MyApp {
    count: i32,
}

enum Message {
    Increment,
    Decrement,
}

impl App for MyApp {
    type Message = Message;
    
    fn update(&mut self, msg: Message) {
        match msg {
            Message::Increment => self.count += 1,
            Message::Decrement => self.count -= 1,
        }
    }
    
    fn view(&self) -> impl View {
        Column::new()
            .child(Text::new(format!("Count: {}", self.count)))
            .child(
                Row::new()
                    .child(Button::new("-").on_click(|| Message::Decrement))
                    .child(Button::new("+").on_click(|| Message::Increment))
            )
    }
}

fn main() {
    MyApp { count: 0 }.run();
}
```

## Backend Detection

```
Is ULTIMATE_TERMINAL env var set?
       │
       ├──Yes──► Connect to Unix socket
       │         Use native protocol
       │
       └──No───► Initialize Ratatui
                 Use ANSI rendering
```
