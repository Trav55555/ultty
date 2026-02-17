# ulttui

**Purpose**: TUI framework library that apps link against. Dual-mode: native protocol when in Ultty, ANSI fallback otherwise.

## Structure

```
src/
├── lib.rs              # Public API, re-exports, prelude
├── app.rs              # Application trait and App runner
├── cmd.rs              # Cmd<M> (side effects) and Sub<M> (subscriptions)
├── event.rs            # Event types (Key, Mouse, Click, Change, Submit)
├── focus.rs            # FocusManager (ID-based focus tracking)
├── theme.rs            # Theme system (dark/light presets, thread-local)
├── vim/                # Vim keybinding support
│   └── mod.rs
├── widgets/            # Built-in widgets
│   ├── mod.rs          # View trait definition
│   ├── text.rs         # Text, RichText, TextInput
│   ├── button.rs       # Button
│   ├── checkbox.rs     # Checkbox
│   ├── toggle.rs       # Toggle
│   ├── select.rs       # Select (dropdown)
│   ├── container.rs    # Column, Row, Stack
│   ├── scroll.rs       # ScrollView
│   ├── list.rs         # List (vim-navigable)
│   ├── table.rs        # Table (headers + rows)
│   ├── tabs.rs         # Tabs
│   ├── modal.rs        # Modal dialog
│   ├── image.rs        # Inline image
│   ├── progress.rs     # ProgressBar
│   ├── spinner.rs      # Spinner (loading indicator)
│   ├── divider.rs      # Divider (horizontal/vertical)
│   └── badge.rs        # Badge (status indicator)
├── layout/             # Layout containers (Column, Row, Stack)
│   └── mod.rs
└── backend/            # Rendering backends
    ├── mod.rs
    ├── native.rs       # Native protocol (Ultty)
    ├── ansi.rs         # ANSI fallback (via ratatui)
    └── detect.rs       # Auto-detect which backend to use
```

## Architecture

```
State ──► view() ──► Render
  ▲                    │
  │                    ▼
update() ◄── on_event() ◄── Input
  │
  ▼
Cmd (side effects)
```

## Application Trait

```rust
use ulttui::prelude::*;
use ulttui::event::{Event, Key};

struct MyApp { count: i32 }

enum Msg { Increment, Decrement }

impl Application for MyApp {
    type Message = Msg;  // Must be Send + 'static

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Increment => { self.count += 1; Cmd::none() }
            Msg::Decrement => { self.count -= 1; Cmd::none() }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Key(ke) => match ke.key {
                Key::Char('+') => Some(Msg::Increment),
                Key::Char('-') => Some(Msg::Decrement),
                _ => None,
            },
            _ => None,
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        Sub::none()  // Or Sub::every(Duration, || Msg::Tick)
    }

    fn view(&self) -> impl View {
        Column::new()
            .child(Text::new(format!("Count: {}", self.count)))
            .child(Row::new()
                .child(Button::new("-").id("dec"))
                .child(Button::new("+").id("inc")))
    }
}

fn main() -> anyhow::Result<()> {
    MyApp { count: 0 }.run()
}
```

## Commands (Side Effects)

`update()` returns `Cmd<M>`:

| Variant | Usage |
|---------|-------|
| `Cmd::none()` | No side effect |
| `Cmd::message(msg)` | Dispatch another message immediately |
| `Cmd::batch(vec![...])` | Execute multiple commands |
| `Cmd::spawn(async { ... })` | Spawn async task (tokio) |
| `Cmd::after(duration, \|\| msg)` | Delayed message |

## Subscriptions

`subscriptions()` returns `Sub<M>`:

| Variant | Usage |
|---------|-------|
| `Sub::none()` | No subscription |
| `Sub::every(duration, \|\| msg)` | Tick at interval |
| `Sub::batch(vec![...])` | Multiple subscriptions |

## Widgets

### Display
- `Text` — Plain text
- `RichText` — Styled text spans
- `Image` — Inline RGBA image
- `ProgressBar` — Horizontal progress bar (0-100%)
- `Spinner` — Loading indicator (app controls frame via Sub)
- `Divider` — Horizontal/vertical separator
- `Badge` — Status indicator (Primary/Success/Error/Warning)

### Interactive (support `.id()` for focus + events)
- `Button` — Clickable button
- `TextInput` — Text input with vim motions
- `Checkbox` — Toggleable checkbox
- `Toggle` — On/off switch
- `Select` — Dropdown select

### Data
- `List` — Selectable list with vim navigation
- `Table` — Headers + selectable rows
- `Tabs` — Tab bar

### Containers
- `Column` — Vertical flex
- `Row` — Horizontal flex
- `Stack` — Overlay
- `ScrollView` — Scrollable container with vim navigation
- `Modal` — Dialog overlay

## Theme System

```rust
// Default dark theme
MyApp::new().run()?;

// Light theme
MyApp::new().run_with_theme(Theme::light())?;

// Widgets read Theme::current() during render
```

## Focus Management

- Interactive widgets with `.id()` self-register during render
- `Tab`/`Shift+Tab` cycles through focusable widgets
- Focused widgets render with distinct theme colors
- `FocusManager` tracks focus state via thread-local

## Backend Detection

```
Is ULTTY_TERMINAL env var set?
       │
       ├──Yes──► Connect to Unix socket, use native protocol
       │
       └──No───► Initialize ratatui, use ANSI rendering
```

## Conventions

- Commit frequently — make small, atomic commits after each logical unit of work

## Testing

```bash
cargo test -p ulttui              # All tests (unit + integration + snapshot)
cargo test -p ulttui --lib        # Unit tests only
cargo clippy -p ulttui            # Lint
```

Integration tests use `MockBackend` (behind `test-utils` feature) for headless event loop testing.
