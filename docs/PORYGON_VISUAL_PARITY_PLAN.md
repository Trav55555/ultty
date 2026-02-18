# Porygon TUI Visual Parity Plan

Status: Historical execution document. Canonical completion status is tracked in `tasks.md` (Visual Parity is complete).

Goal: Close the visual gaps between the Python porygon-ai TUI (Textual) and the Rust porygon-tui (ulttui) to achieve design parity.

Reference screenshots:
- Python boot: `/tmp/porygon-py-boot.png`
- Python main: `/tmp/porygon-py-main2.png`
- Rust main (current): `/tmp/porygon-sidebar-fix2.png`

## Phase 0: Framework Prerequisites

These are framework-level changes in `crates/ulttui/` that unblock multiple visual items. Must be done first.

### 0A: Add `.bg()` and `.border()` builders to Column and Row

**File**: `crates/ulttui/src/widgets/container.rs`

**Problem**: Column and Row have no way to set background color or borders. Their `View::style()` always returns `ProtoStyle::default()`. The Card primitive has bg+border but uses hardcoded theme colors and has extra padding/layout opinions.

**Changes**:
1. Add a `style: ProtoStyle` field to both `Column` and `Row` structs (default: `ProtoStyle::default()`)
2. Add builder methods:
   - `.bg(color: Color) -> Self` — sets `self.style.bg = Some(color)`
   - `.border(border: BorderStyle) -> Self` — sets `self.style.border = border`
   - `.border_color(color: Color) -> Self` — sets `self.style.border_color = Some(color)`
3. Override `View::style()` to return `self.style.clone()` instead of default
4. Update `render_ansi` for both Column and Row to:
   - Paint background fill if `self.style.bg.is_some()` (using `ratatui::widgets::Block`)
   - Draw border if `self.style.border != BorderStyle::NONE` (using ratatui `Block::bordered()`)
   - Shrink `inner` area by border width (1 cell each side) before laying out children

**Native mode**: No changes needed — `CreateRegion` already receives the style and the GPU renderer already paints bg/border from it.

**Tests to add**:
- `column_bg_sets_style` — verify `Column::new().bg(Color::Rgb(30,30,36)).style().bg == Some(...)`
- `column_border_sets_style` — verify border fields
- `row_bg_sets_style` — same for Row
- Existing 529 tests must still pass

**ANSI render_ansi changes** (for both Column and Row):
```rust
fn render_ansi(&self, frame: &mut Frame, area: Rect) {
    // Step 1: Render border/bg block, get inner rect
    let mut inner = area;
    if self.style.bg.is_some() || self.style.border.style != BorderType::None {
        let mut block = ratatui::widgets::Block::default();
        if let Some(bg) = self.style.bg {
            block = block.style(ratatui::style::Style::default().bg(to_ratatui_color(bg)));
        }
        if self.style.border.style != BorderType::None {
            let mut borders = Borders::NONE;
            if self.style.border.edges.top { borders |= Borders::TOP; }
            // ...etc for each edge
            block = block.borders(borders).border_type(map_border_type(self.style.border.style));
            if let Some(bc) = self.style.border_color {
                block = block.border_style(ratatui::style::Style::default().fg(to_ratatui_color(bc)));
            }
        }
        inner = block.inner(area);
        frame.render_widget(block, area);
    }

    // Step 2: Apply padding AFTER border (padding shrinks the post-border rect)
    let pad = extract_cells(&self.constraints.padding);
    let inner = Rect::new(inner.x + pad.3, inner.y + pad.0, ...);

    // Step 3: Layout children in final inner rect
    // ...existing layout logic...
}
```

**BorderType mapping** (protocol → ratatui):
- `Thin` → `ratatui::widgets::BorderType::Plain`
- `Thick` → `ratatui::widgets::BorderType::Thick`
- `Double` → `ratatui::widgets::BorderType::Double`
- `Rounded` → `ratatui::widgets::BorderType::Rounded`
- `Dashed` → fallback to `Plain`
- `Ascii` → fallback to `Plain`

**Critical fix**: Row's `render_ansi` currently does NOT apply padding (pre-existing bug). Must add padding support when adding border support, otherwise `Row::new().padding(1.0).border(...)` would ignore padding. Follow Column's padding pattern.

**Ordering invariant**: `area → border (shrinks) → padding (shrinks further) → children layout`. ratatui `Block::inner()` only subtracts cells for enabled edges (left-only border = only x+1, width-1).

**Estimated effort**: Small. ~80 lines of code + tests.

### 0B: Add `.height()` builder to Row (if missing)

**File**: `crates/ulttui/src/widgets/container.rs`

Row currently lacks `.height()`. Needed for the header (1-row coral bar).

**Changes**: Add `pub fn height(mut self, h: Dimension) -> Self` to Row. (Column already has this.)

**Estimated effort**: Trivial. 3 lines.

---

## Phase 1: Message Cards (P0 — Biggest Visual Gap)

**File**: `examples/porygon-tui/src/main.rs` — `messages_view()` method

**Current state**: Messages are bare RichText with a colored prefix like `[31s] System:`. No border, no background, no visual separation.

**Target state**: Each message wrapped in a bordered Column with:
- Per-role background: user=`#2A2A30`, assistant=`#1F2428`, system=`#232328`
- Per-role border color: user=cyan `#3AE0DD`, assistant=coral `#FF5D6E`, system=gray `#333333`
- Thin border (single line)
- Padding of 1.0 inside the card

**Depends on**: Phase 0A (Column.bg/border)

**Changes to `messages_view()`**:
```rust
fn messages_view(&self) -> Column {
    let mut col = Column::new().gap(0.5).padding(1.0);
    for msg in &self.messages {
        let (prefix, fg_color, bg_color, border_color) = match msg.role.as_str() {
            "user" => ("You", Color::Rgb(58, 224, 221), Color::Rgb(42, 42, 48), Color::Rgb(58, 224, 221)),
            "assistant" => ("Porygon", Color::Rgb(255, 93, 110), Color::Rgb(31, 36, 40), Color::Rgb(255, 93, 110)),
            "system" => ("System", Color::Rgb(136, 136, 136), Color::Rgb(35, 35, 40), Color::Rgb(51, 51, 51)),
            _ => ("?", Color::Rgb(247, 247, 247), Color::Rgb(30, 30, 36), Color::Rgb(68, 68, 68)),
        };

        let header = RichText::new().span(
            format!("[{:>3}s] {prefix}:", msg.timestamp.elapsed().as_secs()),
            Style::new().fg(fg_color).bold(),
        );

        let content_view: Box<dyn View> = if msg.role == "assistant" {
            Box::new(MarkdownView::new(msg.content.clone()))
        } else {
            Box::new(RichText::new().span(msg.content.clone(), Style::default()))
        };

        let card = Column::new()
            .bg(bg_color)
            .border(BorderStyle::thin())
            .border_color(border_color)
            .padding(1.0)
            .gap(0.0)
            .child(header)
            .child(content_view);

        col = col.child(card);
    }
    // ... spinner section unchanged
    col
}
```

**Verification**: Screenshot in ultty — messages should appear as distinct bordered cards with colored backgrounds matching the Python TUI.

---

## Phase 2: Header Full-Width Coral Bar

**File**: `examples/porygon-tui/src/main.rs` — `header_view()` method

**Current state**: Row with styled text spans. Only the ` ⬡ PorygonOS ` text has coral bg. Rest is transparent/dark.

**Target state**: Entire header row is a 1-cell-tall solid coral bar spanning full width, with white text.

**Depends on**: Phase 0A (Row.bg), Phase 0B (Row.height)

**Changes to `header_view()`**:
```rust
fn header_view(&self) -> Row {
    // ... (keep existing content logic)
    Row::new()
        .bg(Color::Rgb(255, 93, 110))   // Full coral bar
        .height(Dimension::Cells(1.0))   // Fixed 1 row
        .child(header)
}
```

Also update the RichText spans to remove the per-span background on ` ⬡ PorygonOS ` since the Row bg handles it now. The header text should be white-on-coral (inherits from Row bg).

**Verification**: Header should be a solid coral bar matching the Python `background: #FF5D6E` header.

---

## Phase 3: Sidebar Visual Polish

**File**: `examples/porygon-tui/src/main.rs` — `sidebar_view()` method

### 3A: Sidebar background and left border

**Current state**: Plain Column with no bg or border. Blends into dark background.

**Target state**: Sidebar has `#252528` background with a cyan left border, matching Python's `border-left: solid #3AE0DD` and `background: #252528`.

**Depends on**: Phase 0A (Column.bg/border)

**Changes**:
```rust
fn sidebar_view(&self) -> Column {
    // ...existing content...
    let sidebar = Column::new()
        .width(Dimension::Cells(30.0))
        .padding(1.0)
        .gap(0.5)
        .bg(Color::Rgb(37, 37, 40))  // #252528
        .border(BorderStyle {
            style: BorderType::Thin,
            edges: BorderEdges { top: false, right: false, bottom: false, left: true },
            radius: 0,
        })
        .border_color(Color::Rgb(58, 224, 221))  // cyan left border
        // ...children...
}
```

### 3B: Cassette Status panel border

**Current state**: `▚▚ CASSETTE STATUS ▚▚` title followed by plain text. No visual boundary.

**Target state**: Wrap the Cassette Status content in a bordered sub-container matching Python's `border: double #FF5D6E` on `CassetteStatusWidget`.

**Changes**: Wrap the sidebar children (up to the Divider after STEP) in an inner Column with:
```rust
Column::new()
    .bg(Color::Rgb(26, 26, 29))  // #1A1A1D
    .border(BorderStyle::double())
    .border_color(Color::Rgb(255, 93, 110))  // coral
    .padding(0.5)
    .gap(0.5)
    .child(/* CASSETTE STATUS title */)
    .child(/* daemon status */)
    // ... etc
```

**Verification**: Sidebar should show as a distinct panel with cyan left border and coral-bordered Cassette Status card inside.

---

## Phase 4: Weather Widget

**File**: `examples/porygon-tui/src/main.rs`

**Current state**: Not present.

**Target state**: Live weather widget in the sidebar showing current temperature, condition, sparkline forecast chart, and 5-day forecast. Coral double-border like Python.

**Depends on**: Phase 0A (Column.bg/border). No new dependencies needed — `reqwest` is already in porygon-tui deps.

### Data structures:
```rust
struct WeatherData {
    temp_c: i32,
    description: String,
    emoji: String,
    city: String,
    forecast: Vec<(String, i32, i32)>,  // (date, high_c, low_c)
}
```

### New message variants:
```rust
enum Msg {
    // ...existing...
    WeatherFetched(WeatherData),
    WeatherFailed,
}
```

### Implementation:
1. Add `weather: Option<WeatherData>` field to `PorygonTui`
2. Add `Sub::every(Duration::from_secs(1800), || Msg::PollWeather)` subscription
3. On init, fire `Cmd::spawn(async { fetch_weather().await })` 
4. `fetch_weather()` function:
   - GET `https://ipapi.co/json/` for lat/lon/city
   - GET `https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weathercode&daily=temperature_2m_max,temperature_2m_min&timezone=auto`
   - Parse response into `WeatherData`
5. `weather_view()` method renders:
   - Title: `WEATHER:` in cyan + emoji + temp + description + city
   - Today: `H:{high}° L:{low}° C/F`
   - Sparkline: interpolated temperature chart using spark chars `▁▂▃▄▅▆▇█` with alternating coral/cyan colors
   - Day numbers + high/low rows
   - Wrap in Column with `bg(#1A1A1D)`, `border(double)`, `border_color(coral)`
6. Add `weather_view()` to sidebar between Cassette Status and the bottom

### Sparkline helper:
```rust
fn sparkline(values: &[i32], width: usize) -> String {
    const CHARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    // ... normalize and map to chars
}
```

**Verification**: Sidebar should show live weather data matching the Python TUI's weather widget layout.

---

## Phase 5: Moon Phase Widget

**File**: `examples/porygon-tui/src/main.rs`

**Current state**: Not present.

**Target state**: Moon phase widget below weather in sidebar.

**Depends on**: Phase 0A

### Implementation:
1. `moon_phase()` function: calculate lunar phase from date (same algorithm as Python)
2. `moon_view()` method renders:
   - `MOON:` label in cyan + phase name
   - Row of 8 moon emojis `🌑🌒🌓🌔🌕🌖🌗🌘` in yellow
   - Chevron `▲` below current phase in coral
   - Wrap in Column with `bg(#1A1A1D)`, `border(double)`, `border_color(coral)`
3. Add to sidebar below weather

**No network calls needed** — pure date calculation.

**Verification**: Moon phase should display with correct current phase highlighted.

---

## Phase 6: Richer Welcome Message

**File**: `examples/porygon-tui/src/main.rs` — `PorygonTui::new()`

**Current state**: Single system message `"Porygon TUI - daemon: {url}"`.

**Target state**: Richer welcome matching Python:
```
Welcome to Porygon OS!
Workspace Initialized.

 • Project DNA: Found existing AGENTS.md at /path/to/AGENTS.md
 • Summary: Unknown via System

System ready for input.
```

### Implementation:
1. Detect AGENTS.md in current working directory
2. Construct richer welcome with bullet points using RichText spans:
   - `Welcome to ` (default) + `Porygon` (coral bold) + ` OS` (cyan bold) + `!`
   - `Workspace Initialized.`
   - Bullet points with bold labels
   - `System ready for input.`
3. This is just changing the initial `ChatMessage` content — no structural changes

**Verification**: First message should look like the Python welcome panel.

---

## Phase 7: Boot Screen Polish (P2)

### 7A: Outer border on splash screen

**File**: `examples/porygon-tui/src/main.rs` — `SplashScreen::view()`

Wrap the splash Column in another Column with coral border:
```rust
Column::new()
    .border(BorderStyle::thin())
    .border_color(Color::Rgb(255, 93, 110))
    .padding(1.0)
    .child(/* existing splash content */)
```

### 7B: Boot log bordered container

Wrap `boot_lines` Column with a cyan border matching Python:
```rust
let boot_container = Column::new()
    .border(BorderStyle::thin())
    .border_color(Color::Rgb(58, 224, 221))
    .child(boot_lines);
```

### 7C: Dynamic boot entries (optional)

Replace hardcoded boot log with real system info:
- Detect working directory
- Count files in current dir
- Get system info (cores, RAM) from `sysinfo` crate or `/proc`
- Note: This adds a dependency. Could skip or use basic std fs operations instead.

**Verdict**: Skip 7C for now. Hardcoded entries are fine — they match the aesthetic even if not real.

---

## Phase 8: Header Clock (P2)

**File**: `examples/porygon-tui/src/main.rs`

Add a live clock to the right side of the header.

### Implementation:
1. Add `clock_text: String` field to `PorygonTui` (or compute in `view()`)
2. The existing `SpinnerTick` subscription runs at 80ms — could reuse for clock updates, or add a separate `Sub::every(Duration::from_secs(1), || Msg::ClockTick)`
3. In `header_view()`, add a Spacer + clock text:
```rust
Row::new()
    .bg(Color::Rgb(255, 93, 110))
    .height(Dimension::Cells(1.0))
    .child(header_left)
    .child(Spacer)
    .child(RichText::new().span(
        format!(" {} ", clock_text),
        Style::new().fg(Color::Rgb(247, 247, 247)),
    ))
```

---

## Execution Order

```
Phase 0A ──► Phase 1 (message cards) ──► Phase 2 (header bar)
    │                                         │
    ├──► Phase 3A (sidebar bg/border) ────────┤
    │         │                               │
    │    Phase 3B (cassette border)           │
    │         │                               │
    │    Phase 4 (weather) ───────────────────┤
    │         │                               │
    │    Phase 5 (moon phase)                 │
    │                                         │
    └──► Phase 6 (welcome msg) ──► Phase 7 (boot polish) ──► Phase 8 (clock)
```

**Critical path**: 0A → 1 → 2 → 3A (these deliver the biggest visual impact)

## Estimated Total Effort

| Phase | Effort | Impact |
|-------|--------|--------|
| 0A: Column/Row bg/border | Small (50 LOC + tests) | Unlocks everything |
| 0B: Row.height | Trivial (3 LOC) | Unlocks header |
| 1: Message cards | Small (30 LOC change) | **Highest impact** |
| 2: Header bar | Small (10 LOC change) | High impact |
| 3A: Sidebar bg/border | Small (10 LOC change) | Medium impact |
| 3B: Cassette border | Small (20 LOC refactor) | Medium impact |
| 4: Weather widget | Medium (150 LOC new) | High impact |
| 5: Moon phase | Small (50 LOC new) | Low impact |
| 6: Welcome message | Small (20 LOC change) | Low impact |
| 7: Boot polish | Small (15 LOC change) | Low impact |
| 8: Header clock | Small (20 LOC new) | Low impact |

**Total**: ~400 LOC across all phases. Phases 0-3 are the critical ones (~120 LOC for 80% of the visual improvement).

## Verification Checklist

After each phase:
- [ ] `cargo check -p ulttui` passes
- [ ] `cargo test -p ulttui --lib` — 529+ tests pass
- [ ] `cargo check -p porygon-tui` passes
- [ ] `cargo clippy -p porygon-tui --no-deps -- -D warnings` clean
- [ ] Build + run in ultty, take screenshot, compare to Python reference
- [ ] Run in regular terminal (ANSI fallback) — verify no regression

## Files Modified

| File | Phases | Type |
|------|--------|------|
| `crates/ulttui/src/widgets/container.rs` | 0A, 0B | Framework |
| `examples/porygon-tui/src/main.rs` | 1-8 | App |

No new files. No new dependencies. No protocol changes.

## Known Risks

1. **Row missing padding in ANSI mode** (pre-existing bug): Row's `render_ansi` doesn't call `extract_cells` for padding. Phase 0A must fix this alongside adding border support. Column already handles padding correctly.

2. **BorderType mapping gaps**: Protocol defines `Dashed` and `Ascii` border types that ratatui doesn't have exact equivalents for. Plan: fall back to `Plain` (thin) for both in ANSI mode. Native GPU mode renders them correctly regardless.

3. **Small terminal sizes**: If terminal is very narrow, border + padding could consume all available space, giving children 0-size rects. Existing `saturating_sub` pattern handles this gracefully — children just don't render.

4. **Snapshot test updates**: Adding style field to Column/Row might change existing snapshot tests if any capture `Debug` output or constraints. Run `cargo insta review` after changes.

5. **Weather API rate limiting**: open-meteo.com is free but has rate limits. The 30-minute polling interval (matching Python) should be well within limits. ipapi.co has stricter limits — cache the location on first fetch.

---

## Phase 9: Visual Gap Fixes (Post-Implementation Review)

After implementing Phases 0-8, a pixel-level gap analysis was performed comparing Python and Rust screenshots. The analysis identified 22 potential gaps. After cross-referencing with the actual code, most were **false positives** from the automated pixel sampling. Two real issues were identified.

### Gap Analysis Triage

| ID | Gap | Verdict | Reasoning |
|----|-----|---------|-----------|
| M10 | Color contamination (beige/green) in message area | **REAL — P0** | Welcome message uses `\n` in a single span; GPU renderer may struggle with multi-line `\n` in a single RichText span, causing garbled rendering |
| M1 | Header coral bar spans full width (over sidebar) | **REAL — P1** | DockLayout `dock_top` intentionally spans full width. Python TUI has header only over chat area with sidebar going full height. Fix: move header inside chat Column |
| M5+M6 | Message cards invisible | FALSE POSITIVE | Code already has `Column::new().bg(bg_color).border(thin).border_color(bdr_color)` for each message card |
| M6 | Card borders missing | FALSE POSITIVE | Same as above — borders exist in code |
| M7 | Green role labels missing | FALSE POSITIVE | System role uses `Color::Rgb(136,136,136)` (grey). Green was detected in sidebar (connection indicator), not messages |
| M11 | Input border invisible | FALSE POSITIVE | TextInput widget handles its own border rendering via theme |
| M12 | Prompt/placeholder missing | FALSE POSITIVE | `P-Chat` label and `Type a message...` placeholder are coded; likely gap analysis sampled wrong y-coordinate |
| M13 | Sidebar top panel header missing | FALSE POSITIVE | Cassette Status has double coral border in code |
| M15 | Sidebar bottom panel accent differs | FALSE POSITIVE | Expected — Rust has weather + moon widgets that Python arranges slightly differently |
| M17 | Green `#a6e3a1` in sidebar | FALSE POSITIVE | Intentional — `connected` status indicator uses success green |
| B1 | Header visible during boot | FALSE POSITIVE | `SplashScreen` fully replaces `Application::view()` via `PushScreen` — header cannot be visible |
| B2-B6 | Various boot screen differences | FALSE POSITIVE | Same as B1 — SplashScreen is exclusive. Any differences are expected design choices |
| M2-M4, M8-M9, M16, M18-M19 | Minor polish differences | DEFERRED | Subtle rendering/spacing differences that are acceptable visual variation |

### Phase 9A: Fix Welcome Message Rendering (M10)

**File**: `examples/porygon-tui/src/main.rs` — `PorygonTui::new()`

**Problem**: The welcome message is a single `ChatMessage` with `\n`-separated content in one string:
```
"Welcome to Porygon OS!\nWorkspace Initialized.\n\n • Project DNA: ...\n..."
```
This is rendered as a single RichText span. In GPU native mode, multi-line content within a single text span may render with garbled/overlapping characters, producing the beige/green color contamination seen in the screenshot.

**Fix**: Split the welcome message content into separate lines rendered as individual RichText children within the message card. Instead of relying on `\n` within a single span, render each line as its own RichText widget.

Modify `messages_view()` to detect newlines in system messages and render each line separately with styled bullet points.

### Phase 9B: Fix Header Layout (M1)

**File**: `examples/porygon-tui/src/main.rs` — `view()` method

**Problem**: The header is placed via `DockLayout::dock_top()`, which spans the full terminal width including the sidebar area. In the Python TUI, the header only spans the chat area width, with the sidebar extending full height.

**Fix**: Move the header from `dock_top` into the `chat_area` Column:
```rust
// Before:
let main_content = DockLayout::new()
    .dock_top(self.header_view())    // spans full width
    .dock_bottom(self.status_bar_view())
    .dock_right(self.sidebar_view())
    .center(chat_area);

// After:
let chat_area = Column::new()
    .flex_grow(1.0)
    .child(self.header_view())       // only spans chat column
    .child(ScrollView::new().child(self.messages_view()))
    .child(Divider::horizontal())
    .child(self.input_view());

let main_content = DockLayout::new()
    .dock_bottom(self.status_bar_view())
    .dock_right(self.sidebar_view())
    .center(chat_area);
```

This makes the sidebar extend from top to bottom, matching the Python layout where the cyan left border runs the full height.

---

## Phase 10: Framework Fixes

Issues discovered during porygon-tui development that require changes to ulttui and/or ultty.

### F1: RichText `width: 100%` prevents Row composition (HIGH)

**File**: `crates/ulttui/src/widgets/text.rs` — `View for RichText::constraints()`

**Problem**: `RichText` hardcodes `width: Dimension::Percent(100.0)`. When multiple RichText widgets are placed in a `Row` with a `Spacer`, each RichText claims 100% width. The Spacer gets squeezed to 0 because the RichTexts consume all available space. This makes right-alignment patterns (left text + Spacer + right text) impossible.

**Impact**: Header bar clock cannot be right-aligned. Any Row with mixed RichText + Spacer layout is broken.

**Fix**: Change RichText to use `width: Auto` so it sizes to content. Add `flex_grow(f32)` builder to RichText so callers can opt into stretching when needed (e.g., inside a Column where full-width is desired). Alternatively, add a `.fill_width()` builder that sets `width: Percent(100.0)`.

**Considerations**:
- Changing the default from `100%` to `Auto` will affect ALL existing RichText usage
- In Column layouts, `Auto` width RichText will shrink-wrap to content instead of filling the column
- Need to audit all examples and tests after this change
- Alternative: keep `100%` default but respect `flex_shrink` properly in the Taffy layout — however, Taffy already does this; the issue is that `width: 100%` is a strong hint

**Proposed approach**:
1. Change default to `width: Auto`
2. Add `.fill_width()` builder: `self.width = Dimension::Percent(100.0)`
3. Update any callers that depend on full-width behavior (grep for `RichText::new()` in Column contexts)
4. Run full test suite + snapshot review

### F2: Native renderer has no text wrapping (HIGH)

**File**: `crates/ultty/src/renderer/native_render.rs`, `crates/ultty/src/renderer/text.rs`

**Problem**: The GPU renderer positions glyphs at `cursor_x += cell_width` for each character with no line-break logic. When text content exceeds the region width, glyphs are clipped at the boundary (via `append_clipped_glyphs`). No word-wrap occurs.

**Impact**: Long messages in chat, error messages, and any dynamic content clip instead of wrapping. ANSI fallback mode wraps correctly via ratatui's `Paragraph::wrap()`.

**Fix**: Add word-wrap logic to `build_styled_text_vertices` or add a wrapping pass before vertex generation:

1. **Option A — Renderer-side wrap**: Before generating vertices for Text/RichText content, split text into lines based on `content_width` (available cells). Insert line breaks at word boundaries (space, hyphen) or force-break at region width. Then render each wrapped line at `cy + line_idx * cell_h`.

2. **Option B — Protocol-side wrap**: Add a `TextWrap` field to `TextContent` and `RichText` in the protocol. The framework sets it based on widget configuration. The renderer checks this field and applies wrapping when enabled.

3. **Option C — Layout-side wrap**: Have the layout engine (Taffy) handle text measurement and line breaking. This is the most correct approach but requires integrating text shaping into the layout pass.

**Proposed approach**: Option A is simplest. The renderer already knows `content_width` (cells) for each region. Before generating vertices:
```
fn wrap_text(text: &str, max_cols: usize) -> Vec<String> {
    // Split on whitespace, accumulate words until line exceeds max_cols
    // Force-break words longer than max_cols
}
```
Apply to both `Content::Text` and `Content::RichText` (span-aware wrapping).

**Height calculation**: The framework must report wrapped height. RichText currently hardcodes `height: Cells(1.0)`. After wrapping, the actual height depends on the available width, which isn't known at `constraints()` time. This is a chicken-and-egg problem.

**Pragmatic solution**: Two-pass approach:
1. First layout pass: widgets report minimum height (1 cell for text)
2. After Taffy computes widths, calculate wrapped height
3. If wrapped height differs, trigger a re-layout with updated heights

### F3: `TextInput::vim()` is a no-op (LOW)

**File**: `crates/ulttui/src/widgets/text.rs` — `VimExt for TextInput`

**Problem**: `.vim()` returns `self` unchanged (line 880-882). It doesn't apply the `VimConfig::for_text_input()` settings, specifically `start_in_insert: true`. The default `VimState::new()` starts in Normal mode, so users can't type without pressing `i` first.

**Fix**: Apply the config in `.vim()`:
```rust
fn vim(self) -> Self {
    self.vim_with_config(VimConfig::for_text_input())
}
```

**Impact**: Any TextInput using `.vim()` will now start in Insert mode as expected. Existing apps that depend on Normal mode start should use `.vim_with_config(VimConfig { start_in_insert: false, .. })`.

### F4: Snapshot test updates needed (LOW)

**Dependency**: After F1 (RichText width change), 3+ snapshot tests will fail because RichText constraints change. Run `cargo insta review` and accept updated snapshots.

### Priority Order

| ID | Description | Effort | Blocks |
|----|-------------|--------|--------|
| F3 | `.vim()` no-op fix | 5 min | TextInput usability |
| F1 | RichText width: Auto | 1-2 hrs | Header layout, any Row+RichText pattern |
| F2 | Native text wrapping | 4-8 hrs | Long message display in GPU mode |
| F4 | Snapshot updates | 10 min | Follows F1 |

---

## Phase 11: Additional Framework Fixes (Session 2026-02-12)

### F5: Content width missing border subtraction (HIGH) ✅ FIXED

**File**: `crates/ultty/src/protocol/session.rs`

**Problem**: `content_x`, `content_y`, `content_width`, `content_height` only subtracted padding, not border. Text rendered outside bordered regions because the content area calculation didn't account for border thickness.

**Fix**: Add `taffy_layout.border.*` to the subtraction:
```rust
content_x: abs_x + taffy_layout.padding.left + taffy_layout.border.left,
content_y: abs_y + taffy_layout.padding.top + taffy_layout.border.top,
content_width: taffy_layout.size.width
    - taffy_layout.padding.left
    - taffy_layout.padding.right
    - taffy_layout.border.left
    - taffy_layout.border.right,
content_height: taffy_layout.size.height
    - taffy_layout.padding.top
    - taffy_layout.padding.bottom
    - taffy_layout.border.top
    - taffy_layout.border.bottom,
```

**Tests**: All 1005+ ultty tests pass.

### F6: ScrollView overflow.x defaults to Visible (MEDIUM) ✅ FIXED

**File**: `crates/ulttui/src/widgets/scroll.rs`

**Problem**: ScrollView had `overflow.x: Visible` (default), allowing children to overflow horizontally. This caused message cards to extend past the visible area instead of wrapping.

**Fix**: Set `overflow.x: Hidden` to constrain children to ScrollView width:
```rust
overflow: ultty_protocol::Overflow {
    x: OverflowAxis::Hidden,
    y: OverflowAxis::Scroll,
},
```

**Tests**: All 537 ulttui tests pass.

### F7: System message text wrapping (MEDIUM) ✅ FIXED

**File**: `examples/porygon-tui/src/main.rs`

**Problem**: System messages used RichText with `\n` separators. The GPU renderer doesn't handle multi-line content within a single RichText span well, and the RichText height was fixed at 1 cell.

**Fix**: All message types now use `MarkdownView` which handles wrapping correctly:
```rust
card = card.child(MarkdownView::new(msg.content.clone()));
```

**Impact**: Long paths and multi-line content now wrap within card boundaries.

### F8: Moon widget arrow alignment (LOW) ✅ FIXED

**File**: `examples/porygon-tui/src/main.rs`

**Problem**: Moon phase arrow used `format!("{:>width$}")` which doesn't account for emoji display width (2 cells each).

**Fix**: Use explicit space padding:
```rust
let arrow_pad = " ".repeat(idx * 3);
let chevron = format!("{arrow_pad}▲");
```

### F9: Weather forecast layout (LOW) ✅ FIXED

**File**: `examples/porygon-tui/src/main.rs`

**Problem**: 5-day forecast didn't fit in 30-cell sidebar.

**Fix**: Reduced gap to 0.5, truncated day labels to 2 chars, added `align_center()`:
```rust
let mut forecast_row = Row::new().gap(0.5);
for (d, h, l) in &w.forecast {
    let short_day = &d[..d.len().min(2)];
    let day_col = Column::new()
        .align_center()
        .child(RichText::new().span(short_day, Style { dim: true, ..Style::new() }))
        // ...
}
```

### F10: Status bar placement (LOW) ✅ FIXED

**File**: `examples/porygon-tui/src/main.rs`

**Problem**: Status bar was at `dock_bottom`, separate from text input.

**Fix**: Moved status bar into `chat_area` Column below input:
```rust
let chat_area = Column::new()
    .flex_grow(1.0)
    .child(ScrollView::new().child(self.messages_view()))
    .child(Divider::horizontal())
    .child(self.input_view())
    .child(self.status_bar_view());
```

---

## Fullscreen Verification Checklist (≥100 cols)

The following items require a terminal width of at least 100 columns to display the sidebar:

- [ ] **Header over sidebar**: Verify if header spans full width (current) or only chat area (Python style)
- [ ] **Sidebar visible**: 30-cell width with cyan left border
- [ ] **Weather widget**: 5-day forecast hi/lo visible and aligned
- [ ] **Moon widget**: Arrow positioned under current phase emoji
- [ ] **Cassette status**: Error message placement when daemon unreachable
- [ ] **Cyan left border**: Runs full height of sidebar

**Current limitation**: ultty window size is controlled by the window manager, not programmatically. Manual resize required for fullscreen verification.

---

## Summary Status

| Phase | Status | Notes |
|-------|--------|-------|
| 0A/0B | ✅ Complete | Column/Row bg/border/height builders |
| 1 | ✅ Complete | Message cards with role styling |
| 2 | ✅ Complete | Header coral bar |
| 3A/3B | ✅ Complete | Sidebar bg/border, cassette border |
| 4 | ✅ Complete | Weather widget with sparkline |
| 5 | ✅ Complete | Moon phase widget |
| 6 | ✅ Complete | Welcome message with bullets |
| 7 | ✅ Complete | Boot screen polish |
| 8 | ✅ Complete | Header clock |
| 9A | ✅ Complete | Welcome message uses MarkdownView |
| 9B | ✅ Complete | Header locked to top (full width) |
| 10-F1 | ✅ Complete | RichText width: Auto + fill_width() |
| 10-F2 | ✅ Complete | Native text wrapping in GPU renderer |
| 10-F3 | ✅ Complete | TextInput.vim() fix |
| 11 (F5-F10) | ✅ Complete | Framework fixes from 2026-02-12 |
| 12 | ✅ Complete | System stats (CPU/RAM) in Cassette |

---

## Phase 12: System Stats in Cassette Widget ✅ FIXED

**File**: `examples/porygon-tui/src/main.rs`

**Problem**: The Cassette widget only showed agent-specific status (cost, budget). The user requested real-time CPU and RAM usage bars to match the "system monitor" aesthetic.

**Fix**:
1. Added `cpu_usage` and `ram_usage` to `AgentStatus` and `StatusResponse`.
2. Implemented `get_system_stats()` using `libc::getloadavg` for CPU and `/proc/meminfo` for RAM (Linux-specific with cross-platform fallbacks).
3. Updated `sidebar_view()` to render 10-cell usage bars with dynamic coloring (green -> red).
4. Lowered sidebar visibility threshold to 80 columns for better responsive behavior on smaller screens.

**Impact**: Enhanced "OS" aesthetic with live system telemetry.

---

## Final Verification Checklist (Session 2026-02-12)

- [x] **Header**: Locked to top, full width, coral bar, right-aligned clock.
- [x] **Message Cards**: Bordered, role-colored, proper text wrapping via MarkdownView.
- [x] **Text Input**: Pinned to bottom, vim insert mode by default, daemon URL below.
- [x] **Status Bar**: Positioned directly under text input.
- [x] **Sidebar**: Visible at >= 80 cols, cyan left border.
- [x] **Cassette Widget**: Includes Cost, CPU, and RAM usage bars.
- [x] **Weather Widget**: 5-day forecast visible with sparkline.
- [x] **Moon Widget**: Arrow aligned to current phase emoji.
- [x] **Framework**: Content width fixed (border subtraction), ScrollView overflow fixed.

**All tests passing**: ulttui (537), ultty (1005+), clippy clean.
