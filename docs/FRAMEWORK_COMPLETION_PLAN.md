# ulttui Framework Completion Plan

**Goal**: Framework-complete, ship fast. Make ulttui competitive as a TUI framework library.
**Estimated effort**: ~5 focused days across 3 phases.
**Date**: 2026-02-05

---

## Current State

ulttui is a Rust TUI framework with Elm architecture. It has 14 widgets, dual-mode rendering (native protocol + ANSI fallback), vim motion support, and ~9K LOC with 239 unit tests. The terminal emulator (ultty) is production-quality with GPU rendering, terminal-side layout, and a binary protocol.

The framework itself has critical DX gaps that prevent real-world adoption.

---

## Critical Gaps

| # | Gap | Impact |
|---|-----|--------|
| 1 | `Button::on_click()` is a no-op | #1 DX trap. Closures accepted but discarded. |
| 2 | No Cmd/Sub pattern | Can't spawn async tasks, timers, or side effects from `update()`. |
| 3 | No widget event routing | Only way to handle events is monolithic `on_event()`. No widget IDs. |
| 4 | No theming system | Styles hardcoded per-widget. No global theme. |
| 5 | Focus management is primitive | Incrementing counter. Not wired to actual widgets. |
| 6 | No async support | Synchronous 16ms polling despite tokio dependency. |
| 7 | View reconciliation is full-recreate | O(n) messages/frame. Acceptable for now. |

---

## Architectural Decisions

### 1. Cmd/Sub Pattern: Return `Cmd<M>` from update()

```rust
fn update(&mut self, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::FetchData => Cmd::spawn(async {
            let data = fetch().await;
            Msg::DataLoaded(data)
        }),
        Msg::StartTimer => Cmd::after(Duration::from_secs(1), || Msg::Tick),
        Msg::Inc => { self.count += 1; Cmd::none() }
    }
}
```

**Cmd enum**: `None`, `Message(M)`, `Batch(Vec<Cmd<M>>)`, `Spawn(BoxFuture<M>)`, `After(Duration, Fn() -> M)`.

**Subscriptions** via separate method:
```rust
fn subscriptions(&self) -> Sub<Msg> {
    Sub::every(Duration::from_millis(100), || Msg::Tick)
}
```

**Rationale**: Rust's `&mut self` means we don't return Model like Bubbletea. `Cmd::none()` is ergonomic. Default impl returns `Cmd::none()` so existing code doesn't break unless `update()` is overridden.

### 2. Events: Widget IDs, not closures

Closures on widgets are a Rust antipattern (generics pollution, non-object-safe View, lifetime hell).

```rust
// Widget side
Button::new("+").id("increment")

// App side
fn on_event(&mut self, event: &Event) -> Option<Msg> {
    match event {
        Event::Click { id } if id == "increment" => Some(Msg::Inc),
        _ => None
    }
}
```

New event variants: `Click { id }`, `Change { id, value }`, `Submit { id }`.

**Kill `Button::on_click()`** -- it's a no-op trap.

### 3. View Diffing: Skip (defer to Phase 4)

Full-recreate works. Terminal parses 100 regions in <100us. Frame batching prevents tearing. Revisit when >100 regions cause >5ms frame times.

### 4. Theming: Thread-local Theme context

```rust
pub struct Theme {
    pub primary: Color,
    pub background: Color,
    pub foreground: Color,
    pub button: ButtonTheme,
    pub input: InputTheme,
    // per-widget theme slots
}

// Presets
Theme::dark()
Theme::light()

// Widget access (thread-local, no generics pollution)
let theme = Theme::current();
```

### 5. Focus: ID-based focus tree

Widgets with `.id()` and `is_focusable() == true` get registered in tree order during render. Tab cycles through them. Focused widget receives focus state through render context.

---

## Phase 1: Core DX (2 days)

*Makes the framework actually usable for real apps.*

| # | Task | Effort | Details |
|---|------|--------|---------|
| 1.1 | Define `Cmd<M>` enum | 2h | None, Message, Batch, Spawn, After |
| 1.2 | Define `Sub<M>` type | 1h | None, Every(Duration, Fn), Batch |
| 1.3 | Change `update()` return type | 2h | Returns `Cmd<M>`, default `Cmd::none()` |
| 1.4 | Add `subscriptions()` to Application | 1h | Default returns `Sub::none()` |
| 1.5 | Wire Cmd execution in App::run | 4h | Process Cmds, tokio runtime, Sub polling |
| 1.6 | Add `.id()` to interactive widgets | 2h | Button, TextInput, Checkbox, Toggle, Select |
| 1.7 | Add Event::Click/Change/Submit | 2h | Emitted by widgets when activated |
| 1.8 | Remove Button::on_click() no-op | 30m | Delete method, update examples |
| 1.9 | Update all examples | 2h | Use Cmd pattern + widget IDs |

**Exit criteria**: counter-app uses `Cmd::none()`, button clicks route through `Event::Click { id }`, `cargo test --workspace` passes, `cargo clippy` clean.

## Phase 2: Async + Subscriptions (1 day)

*Enables apps that do real work.*

| # | Task | Effort | Details |
|---|------|--------|---------|
| 2.1 | `Cmd::spawn()` with tokio runtime | 2h | Async task -> message. `M: Send + 'static` |
| 2.2 | Tokio integration in App::run | 2h | `tokio::select!` for event + cmd polling |
| 2.3 | Sub polling loop | 2h | Check subscriptions, emit messages on schedule |
| 2.4 | Dashboard example with timer | 2h | Prove async + Sub::every() works |

**Exit criteria**: dashboard-app uses `Sub::every()` for live updates, can spawn async tasks.

## Phase 3: Polish (2 days)

*Makes it feel like a real framework.*

| # | Task | Effort | Details |
|---|------|--------|---------|
| 3.1 | Theme struct + dark/light presets | 3h | Color palette, per-widget theme slots |
| 3.2 | Thread-local theme access | 2h | `Theme::current()`, `run_with_theme()` |
| 3.3 | ID-based focus management | 3h | Collect focusable IDs, Tab cycles |
| 3.4 | Focus styling in all widgets | 2h | Visible focus ring/highlight |
| 3.5 | Getting started guide | 3h | From `cargo new` to working app |
| 3.6 | Widget reference | 2h | Each widget's API, one example each |

**Exit criteria**: `Theme::dark()` works, Tab highlights focused widget, docs/ has tutorial.

## Phase 4: Deferred (when needed)

| Task | Trigger |
|------|---------|
| View diffing/reconciliation | >100 regions cause >5ms frames |
| CSS-like style selectors | Theme struct proves insufficient |
| `SimpleWidget` trait | Users struggle with View trait |
| Animation primitives | Users request transitions |
| Form validation system | Form-heavy apps emerge |
| Widget gallery site | Community grows |

---

## Breaking Change

`update()` returning `Cmd<M>` is the one breaking change. Default impl returns `Cmd::none()`, so apps that don't override `update()` are unaffected. Apps that do override need one change:

```rust
// Before
fn update(&mut self, msg: Msg) {
    match msg { Msg::Inc => self.count += 1 }
}

// After -- add Cmd::none() return
fn update(&mut self, msg: Msg) -> Cmd<Msg> {
    match msg { Msg::Inc => { self.count += 1; Cmd::none() } }
}
```

---

## Target API

```rust
use ulttui::prelude::*;

struct App { count: i32 }

#[derive(Clone)]
enum Msg { Inc, Dec, Tick }

impl Application for App {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Inc => { self.count += 1; Cmd::none() }
            Msg::Dec => { self.count -= 1; Cmd::none() }
            Msg::Tick => Cmd::none(),
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Click { id } if id == "inc" => Some(Msg::Inc),
            Event::Click { id } if id == "dec" => Some(Msg::Dec),
            Event::Key(k) if k.key == Key::Char('+') => Some(Msg::Inc),
            _ => None
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        Sub::none()
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
    App { count: 0 }.run_with_theme(Theme::dark())
}
```
