# ulttui

**Purpose**: TUI framework library that apps link against. Dual-mode: native protocol when in Ultty, ANSI fallback otherwise.

Repository-wide conventions live in [CLAUDE.md](../../CLAUDE.md) and
[AGENTS.md](../../AGENTS.md). This file covers only what is specific to this
crate.

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
├── screen.rs           # Screen abstraction shared by both backends
├── toast.rs            # Transient notifications
├── widgets/            # Built-in widgets, one module each (28 modules)
│   ├── mod.rs          # View trait definition
│   ├── text.rs         # Text, RichText, TextInput
│   ├── container.rs    # Column, Row, Stack
│   ├── markdown.rs     # MarkdownView (streaming-aware)
│   └── ...             # See `ls crates/ulttui/src/widgets`
├── layout/             # Layout containers (Column, Row, Stack)
├── app/                # Runner internals
└── backend/            # Rendering backends
    ├── native.rs       # Native protocol (Ultty)
    ├── ansi.rs         # ANSI fallback (via ratatui)
    └── detect.rs       # Auto-detect which backend to use
```

Run `ls crates/ulttui/src/widgets` when this tree looks stale; the filesystem
wins. [docs/WIDGET_REFERENCE.md](../../docs/WIDGET_REFERENCE.md) is the
user-facing list.

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
- `MarkdownView` — Rendered markdown, with a streaming mode
- `Image` — Inline RGBA image
- `ProgressBar` — Horizontal progress bar (0-100%)
- `Spinner` — Loading indicator (app controls frame via Sub)
- `Divider` — Horizontal/vertical separator
- `Badge` — Status indicator (Primary/Success/Error/Warning)
- `Diff` — Unified diff view

### Interactive (support `.id()` for focus + events)
- `Button` — Clickable button
- `TextInput` — Text input with vim motions and history
- `Checkbox` — Toggleable checkbox
- `Toggle` / `BinaryToggle` — On/off switches
- `Select` — Dropdown select

### Data
- `List` — Selectable list with vim navigation
- `Table` — Headers + selectable rows
- `Tabs` — Tab bar
- `Tree` — Expandable tree view
- `Collapsible` — Expand/collapse section

### Containers
- `Column` — Vertical flex
- `Row` — Horizontal flex
- `Stack` — Overlay
- `ScrollView` — Scrollable container with vim navigation and search
- `Modal` — Dialog overlay
- `Dock` — Edge-anchored container

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

## Testing

```bash
cargo test -p ulttui                  # All tests (unit + integration + snapshot)
cargo test -p ulttui --lib            # Unit tests only
cargo test -p ulttui --test snapshots # Snapshot tests (insta)
cargo insta review                    # Review snapshot diffs
```

Integration tests use `MockBackend` (behind `test-utils` feature) for headless event loop testing.
