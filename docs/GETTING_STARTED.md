# Getting Started with ulttui

Build TUI applications that work in any terminal, with native GPU acceleration in Ultty.

## Installation

Add `ulttui` to your `Cargo.toml`:

```toml
[dependencies]
ulttui = { path = "../crates/ulttui" }
anyhow = "1"
```

## The Elm Architecture

ulttui follows the Elm architecture: **State → View → Event → Message → Update → State**.

```
┌──────────────────────────────────────────┐
│                                          │
│   State ──► view() ──► Render            │
│     ▲                    │               │
│     │                    ▼               │
│   update() ◄── on_event() ◄── Input     │
│     │                                    │
│     ▼                                    │
│   Cmd (side effects)                     │
│                                          │
└──────────────────────────────────────────┘
```

Every app implements the `Application` trait:

```rust
use ulttui::prelude::*;
use ulttui::event::{Event, Key};

struct MyApp { /* state */ }

enum Msg { /* messages */ }

impl Application for MyApp {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> { /* handle message, return command */ }
    fn on_event(&mut self, event: &Event) -> Option<Msg> { /* map input to message */ }
    fn view(&self) -> impl View { /* build UI from state */ }
}

fn main() -> anyhow::Result<()> {
    MyApp { /* initial state */ }.run()
}
```

## Hello World

The simplest app — just renders text:

```rust
use ulttui::prelude::*;

struct HelloApp;

impl Application for HelloApp {
    type Message = ();

    fn view(&self) -> impl View {
        Text::new("Hello, World!")
    }
}

fn main() -> anyhow::Result<()> {
    HelloApp.run()
}
```

## Counter Example

A complete app with state, messages, and commands:

```rust
use ulttui::prelude::*;
use ulttui::event::{Event, Key};

struct Counter { count: i32 }

enum Msg {
    Increment,
    Decrement,
    Reset,
}

impl Application for Counter {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Increment => self.count += 1,
            Msg::Decrement => self.count -= 1,
            Msg::Reset => self.count = 0,
        }
        Cmd::none()
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Key(ke) => match ke.key {
                Key::Char('+') => Some(Msg::Increment),
                Key::Char('-') => Some(Msg::Decrement),
                Key::Char('r') => Some(Msg::Reset),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .child(Text::new(format!("Count: {}", self.count)))
            .child(
                Row::new()
                    .child(Button::new("-"))
                    .child(Button::new("+"))
                    .child(Button::new("Reset")),
            )
            .child(Text::new("+/-: change | r: reset | Ctrl+C: quit"))
    }
}

fn main() -> anyhow::Result<()> {
    Counter { count: 0 }.run()
}
```

## Commands (Side Effects)

`update()` returns a `Cmd<M>` to trigger side effects:

```rust
fn update(&mut self, msg: Msg) -> Cmd<Msg> {
    match msg {
        // No side effect
        Msg::Increment => { self.count += 1; Cmd::none() }

        // Dispatch another message immediately
        Msg::DoubleIncrement => Cmd::batch(vec![
            Cmd::message(Msg::Increment),
            Cmd::message(Msg::Increment),
        ]),

        // Spawn an async task
        Msg::FetchData => Cmd::spawn(async {
            let data = reqwest::get("https://api.example.com/data")
                .await.unwrap().text().await.unwrap();
            Msg::DataLoaded(data)
        }),

        // Delayed message
        Msg::StartTimer => Cmd::after(
            Duration::from_secs(5),
            || Msg::TimerDone,
        ),

        Msg::DataLoaded(data) => { self.data = data; Cmd::none() }
        Msg::TimerDone => { self.done = true; Cmd::none() }
    }
}
```

## Subscriptions (Recurring Events)

Override `subscriptions()` for time-based events:

```rust
fn subscriptions(&self) -> Sub<Msg> {
    // Tick every second
    Sub::every(Duration::from_secs(1), || Msg::Tick)
}

// Combine multiple subscriptions
fn subscriptions(&self) -> Sub<Msg> {
    Sub::batch(vec![
        Sub::every(Duration::from_secs(1), || Msg::Tick),
        Sub::every(Duration::from_millis(100), || Msg::FastTick),
    ])
}
```

## Streaming Content

For assistant-style partial responses, render the active message with
`MarkdownView::streaming(true)` and turn it off when the response completes:

```rust
fn view(&self) -> impl View {
    MarkdownView::new(self.response.clone()).streaming(self.waiting_for_response)
}
```

In Ultty-native mode, append-shaped text growth can travel over the
`AppendContent` protocol path. Keep full `SetContent` updates for structural
changes such as replacing a whole markdown block or reparsing completed
markdown.

## Widget IDs and Events

Interactive widgets can have IDs for targeted event handling:

```rust
fn view(&self) -> impl View {
    Column::new()
        .child(TextInput::new().id("username").placeholder("Username"))
        .child(TextInput::new().id("password").placeholder("Password"))
        .child(Button::new("Submit").id("submit"))
}

fn on_event(&mut self, event: &Event) -> Option<Msg> {
    match event {
        Event::Click { id } if id == "submit" => Some(Msg::Submit),
        Event::Change { id, value } if id == "username" => Some(Msg::SetUsername(value.clone())),
        Event::Submit { id } if id == "username" => Some(Msg::FocusPassword),
        _ => None,
    }
}
```

## Themes

Use built-in dark/light themes or customize:

```rust
// Default (dark theme)
MyApp::new().run()?;

// Light theme
MyApp::new().run_with_theme(Theme::light())?;

// Access current theme in widgets
let theme = Theme::current();
```

## Layouts

Build layouts with `Column`, `Row`, and `Stack`:

```rust
fn view(&self) -> impl View {
    Column::new()
        .padding(1.0)
        .gap(1.0)
        .child(Text::new("Header"))
        .child(
            Row::new()
                .gap(2.0)
                .child(Text::new("Left"))
                .child(Text::new("Right")),
        )
        .child(Text::new("Footer"))
}
```

## Running Examples

```bash
cargo run -p counter-app    # Counter with +/- buttons
cargo run -p todo-app       # Todo list with add/remove
cargo run -p dashboard      # Dashboard with live timer (Sub::every)
cargo run -p form-app       # Form with text inputs and checkboxes
cargo run -p chat           # Chat interface with message history
cargo run -p hello-native   # Raw protocol demo (run inside ultty)
cargo run -p hello-native --bin append-content-smoke  # AppendContent smoke app
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Ultty Terminal                            │
├─────────────────────────────────────────────────────────────┤
│  Scene Graph ◄── Layout Engine ◄── Protocol Parser          │
│       │              (Taffy)            ▲                    │
│       ▼                                 │                    │
│  GPU Renderer ────────────────── Binary Protocol             │
│    (wgpu)                        (Unix socket)               │
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

## Next Steps

- [Widget Reference](WIDGET_REFERENCE.md) — All built-in widgets and their APIs
- [Protocol Specification](PROTOCOL_SPEC.md) — Native protocol for advanced use
- Explore `examples/` for real-world patterns
