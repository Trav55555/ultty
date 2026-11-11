# Widget Reference

All built-in widgets in the `ulttui` framework. Import via `use ulttui::prelude::*`.

## Display Widgets

### Text

Plain text display.

```rust
Text::new("Hello, World!")
Text::new(format!("Count: {}", count))
```

| Method | Description |
|--------|-------------|
| `new(content)` | Create with string content |
| `content()` | Get the text content |

Not focusable. No keyboard handling.

### RichText

Styled text with multiple spans.

```rust
RichText::new()
    .text("Hello ")
    .span("world", Style::new().bold())
    .span("!", Style::new().fg(Color::Rgb(255, 0, 0)))
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty rich text |
| `text(s)` | Append unstyled text |
| `span(s, style)` | Append styled text |
| `spans()` | Get all spans |

Not focusable. No keyboard handling.

### Image

Inline image from RGBA pixel data. GPU-rendered in Ultty, placeholder in ANSI mode.

```rust
Image::new(rgba_data, width, height)
    .scale(ImageScale::Fit)
```

| Method | Description |
|--------|-------------|
| `new(data, w, h)` | Create from RGBA bytes |
| `scale(mode)` | Set scaling: `Fit`, `Fill`, `None` |

Not focusable.

---

## Interactive Widgets

All interactive widgets support `.id()` for targeted event handling.

### Button

Clickable button. Renders as `[ label ]`.

```rust
Button::new("Submit").id("submit-btn")
```

| Method | Description |
|--------|-------------|
| `new(label)` | Create with label text |
| `id(id)` | Set widget ID for event targeting |
| `label()` | Get the label text |
| `widget_id()` | Get the widget ID |

**Keyboard**: Enter → `FocusAction::Consumed`, Tab → next, Shift+Tab → previous.

### TextInput

Text input with vim motion support.

```rust
TextInput::new()
    .id("username")
    .value("initial text")
    .placeholder("Enter username...")
    .multiline()
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty input |
| `id(id)` | Set widget ID |
| `value(s)` | Set initial value (cursor moves to end) |
| `cursor(n)` | Put the cursor after the first `n` grapheme clusters |
| `placeholder(s)` | Set placeholder text |
| `multiline()` | Enable multiline mode |
| `current_value()` | Get current text |
| `cursor_position()` | Get cursor position |
| `selection()` | Get selection range `Option<(start, end)>` |
| `widget_id()` | Get the widget ID |

**Keyboard**: Starts in vim Normal mode. `i` enters Insert mode, `Escape` returns to Normal. Full vim motions: `h/l` (left/right), `w/b/e` (word), `0/$` (line), `gg/G` (document), `d/c/y` (operators), `v/V` (visual).

In Insert mode, typing edits at the cursor by grapheme cluster: Left/Right, Home/End, Ctrl+A/E, Ctrl/Alt+arrows (words), Backspace, Delete, Ctrl+W (word back), Ctrl+U/K (to start/end). The same operations are available as pure functions in `ulttui::edit`. A focused input draws its cursor: the reversed cell in ANSI mode, a `|` in native mode.

### Checkbox

Toggleable checkbox with label.

```rust
Checkbox::new("Accept terms")
    .id("terms")
    .checked(true)
```

| Method | Description |
|--------|-------------|
| `new(label)` | Create unchecked checkbox |
| `id(id)` | Set widget ID |
| `checked(bool)` | Set initial state |
| `is_checked()` | Get current state |
| `toggle()` | Flip the state |
| `widget_id()` | Get the widget ID |

**Keyboard**: Space/Enter → toggle, Tab → next.

### Toggle

On/off switch with label.

```rust
Toggle::new("Dark mode")
    .id("dark-mode")
    .on(true)
```

| Method | Description |
|--------|-------------|
| `new(label)` | Create in off state |
| `id(id)` | Set widget ID |
| `on(bool)` | Set initial state |
| `is_on()` | Get current state |
| `toggle()` | Flip the state |
| `widget_id()` | Get the widget ID |

**Keyboard**: Space/Enter → toggle, Tab → next.

### Select

Dropdown select with expandable options list.

```rust
Select::new(vec!["Red".into(), "Green".into(), "Blue".into()])
    .id("color-picker")
    .selected(1)  // Green
```

| Method | Description |
|--------|-------------|
| `new(options)` | Create with option list |
| `id(id)` | Set widget ID |
| `selected(index)` | Set initial selection |
| `selected_index()` | Get selected index |
| `selected_value()` | Get selected text |
| `is_expanded()` | Check if dropdown is open |
| `widget_id()` | Get the widget ID |

**Keyboard (collapsed)**: Enter/Space → expand, Tab → next. **Expanded**: Up/Down → navigate, Enter/Space → select, Escape → collapse.

---

## Container Widgets

### Column

Vertical flex container.

```rust
Column::new()
    .padding(1.0)
    .gap(1.0)
    .child(Text::new("First"))
    .child(Text::new("Second"))
    .child(Text::new("Third"))
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty column |
| `child(view)` | Add a child widget |
| `padding(px)` | Set padding on all sides |
| `gap(px)` | Set gap between children |

### Row

Horizontal flex container.

```rust
Row::new()
    .gap(2.0)
    .child(Text::new("Left"))
    .child(Text::new("Right"))
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty row |
| `child(view)` | Add a child widget |
| `padding(px)` | Set padding on all sides |
| `gap(px)` | Set gap between children |

### Stack

Overlay container — all children rendered at the same position.

```rust
Stack::new()
    .child(Text::new("Background"))
    .child(Text::new("Foreground"))
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty stack |
| `child(view)` | Add a child widget |

---

## Data Widgets

### List

Selectable list with vim navigation and filtering.

```rust
List::new()
    .items(vec!["Item 1".into(), "Item 2".into(), "Item 3".into()])
    .selected(0)
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty list |
| `items(vec)` | Set list items |
| `selected(index)` | Set initial selection |
| `selected_index()` | Get selected index |
| `filter(query)` | Filter items by substring |

**Keyboard**: j/Down → next, k/Up → previous, g → first, G → last, `/` → search. Full vim motions supported.

### Table

Table with column headers and selectable rows.

```rust
Table::new()
    .headers(vec!["Name", "Age", "City"])
    .rows(vec![
        vec!["Alice", "30", "NYC"],
        vec!["Bob", "25", "LA"],
    ])
    .column_widths(vec![40.0, 20.0, 40.0])
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty table |
| `headers(vec)` | Set column headers |
| `rows(vec)` | Set row data |
| `column_widths(vec)` | Set column width percentages |
| `selected_index()` | Get selected row index |
| `selected_row()` | Get selected row data |

**Keyboard**: j/Down → next row, k/Up → previous row, g → first, G → last, Tab → next widget.

### Tabs

Tab bar for switching between views.

```rust
Tabs::new()
    .labels(vec!["General", "Advanced", "About"])
    .active(0)
```

| Method | Description |
|--------|-------------|
| `new()` | Create empty tabs |
| `labels(vec)` | Set tab labels |
| `active(index)` | Set active tab |
| `active_index()` | Get active tab index |
| `active_label()` | Get active tab label |
| `tab_count()` | Get number of tabs |

**Keyboard**: h/Left → previous tab, l/Right → next tab, 1-9 → jump to tab.

---

## Overlay Widgets

### Modal

Dialog overlay with title, body, and buttons.

```rust
Modal::new()
    .title("Confirm")
    .body("Are you sure?")
    .buttons(vec!["Cancel", "OK"])
    .visible(true)
```

| Method | Description |
|--------|-------------|
| `new()` | Create modal (visible by default) |
| `title(s)` | Set title text |
| `body(s)` | Set body text |
| `buttons(vec)` | Set button labels |
| `visible(bool)` | Show/hide |
| `selected_label()` | Get selected button label |

**Keyboard**: h/Left → previous button, l/Right/Tab → next button, Enter → confirm, Escape → hide.

### ScrollView

Scrollable container with vim navigation and search.

```rust
ScrollView::new()
    .child(long_content_view)
    .horizontal()
```

| Method | Description |
|--------|-------------|
| `new()` | Create scroll view |
| `child(view)` | Set scrollable content |
| `horizontal()` | Enable horizontal scrolling |
| `scroll_to(y)` | Scroll to position |
| `scroll_to_top()` | Scroll to top |
| `scroll_to_bottom()` | Scroll to bottom |
| `position()` | Get `(x, y)` scroll position |

**Keyboard**: j/Down → scroll down, k/Up → scroll up, Ctrl+D → half page down, Ctrl+U → half page up, g → top, G → bottom, `/` → search.

---

## Common Patterns

### Layout Composition

```rust
fn view(&self) -> impl View {
    Column::new()
        .child(self.header())
        .child(
            Row::new()
                .child(self.sidebar())
                .child(self.content()),
        )
        .child(self.footer())
}
```

### Conditional Rendering

```rust
fn view(&self) -> impl View {
    let mut col = Column::new()
        .child(Text::new("Always visible"));

    if self.show_details {
        col = col.child(Text::new("Details here"));
    }

    col
}
```

### Dynamic Lists

```rust
fn view(&self) -> impl View {
    let items: Vec<String> = self.todos
        .iter()
        .map(|t| format!("[{}] {}", if t.done { "x" } else { " " }, t.text))
        .collect();

    List::new().items(items)
}
```
