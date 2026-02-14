# Tech Debt Reduction Plan

**Source**: Code review of 50 commits (+10,500 LOC) from late Jan–mid Feb 2026.

**Principle**: Each chunk is ≤2 hours, independently mergeable, and leaves tests green. No feature work — pure cleanup.

---

## Phase 1 — Red Tests & Clippy (Day 1)

### 1.1 Fix `test_client_hello_handshake` Flake

**File**: `crates/ultty/tests/native_protocol.rs:44-70`

**Problem**: Test panics with `AddrInUse`. The `ProtocolServer::new()` likely binds a fixed or colliding address while another test holds the same socket.

**Work**:
1. Identify the port/address collision (check if `ProtocolServer::new` binds an internal TCP listener beyond the Unix socket).
2. Ensure each test gets an isolated address. If the server binds a secondary port, randomize it or make it configurable for tests.
3. Verify by running `cargo test -p ultty --test native_protocol -- --test-threads=1` (sequential) and `--test-threads=4` (parallel).

**Done when**: `cargo test --workspace` passes with zero failures.

---

### 1.2 Fix Clippy Warnings in `porygon-tui`

**File**: `examples/porygon-tui/src/main.rs`

**Problem**: 4 warnings — `manual_div_ceil` (line 902), `needless_range_loop` (lines 910, 933), `useless_format` (line 1178).

**Work**:
1. Replace `(n + 23) / 24` with `n.div_ceil(24)`.
2. Replace `for row in 0..chart_rows` with iterator on `grid.iter_mut().take(chart_rows)`.
3. Replace `for col in 0..chart_width` with `grid.iter_mut().enumerate().take(chart_width)`.
4. Replace `format!("{}", moon.full_moon_date)` with `moon.full_moon_date.to_string()`.

**Done when**: `cargo clippy --workspace --all-targets` reports zero warnings.

---

## Phase 2 — Split `native_render.rs` (Day 1–2)

The file is 3,139 lines with 8 `#[allow(clippy::too_many_arguments)]` suppressions. Split into a module directory.

### 2.1 Create `RenderContext` Struct

**File**: `crates/ultty/src/renderer/native_render.rs` (new: `native_render/context.rs`)

**Problem**: Most functions pass `(bg_vertices, bg_indices, text_vertices, text_indices, deferred_images)` as separate args — that's 5 params that always travel together.

**Work**:
1. Define `NativeRenderContext` holding mutable refs to all five buffers.
2. Update all internal functions to accept `&mut NativeRenderContext` instead of 5 separate params.
3. Remove `#[allow(clippy::too_many_arguments)]` from every function that no longer needs it.

**Done when**: At least 5 of the 8 `too_many_arguments` suppressions are removed. Tests pass.

---

### 2.2 Extract Syntax Tokenizer → `native_render/syntax.rs`

**Functions to move** (currently in `native_render.rs`):
- `tokenize_code_line` (~130 lines)
- `code_keywords` (~50 lines)
- `is_line_comment` (~30 lines)
- `TokenKind` enum + `Token` struct
- `is_type_like` helper
- All `#[cfg(test)]` tokenizer tests (~350 lines)

**Work**:
1. Create `crates/ultty/src/renderer/native_render/syntax.rs`.
2. Move all tokenizer types and functions.
3. Make `tokenize_code_line`, `code_keywords`, `is_line_comment` `pub(super)`.
4. Update imports in the parent module.

**Done when**: `native_render.rs` is ~350 lines shorter. All tokenizer tests pass from the new location.

---

### 2.3 Extract Markdown GPU Renderer → `native_render/markdown.rs`

**Functions to move**:
- `render_markdown_content` (~180 lines)
- `render_markdown_inline` (~200 lines)
- `render_markdown_table` (~90 lines)
- `emit_md_segment` (~20 lines)

**Work**:
1. Create `crates/ultty/src/renderer/native_render/markdown.rs`.
2. Move functions as methods on a new `MarkdownRenderer` struct (or keep as `impl Renderer` methods via trait extension).
3. These functions need access to `self.text_renderer`, `self.cell_width`, `self.cell_height`, `self.color_scheme` — pass via `&Renderer` ref or the `RenderContext`.

**Done when**: `native_render.rs` is ~500 lines shorter. Markdown snapshot tests pass.

---

### 2.4 Extract Code Renderer → `native_render/code.rs`

**Functions to move**:
- `render_code_content` (~50 lines)
- `render_code_line` (~80 lines)

**Work**:
1. Create `crates/ultty/src/renderer/native_render/code.rs`.
2. Move functions. They depend on the syntax tokenizer from 2.2 — import from `super::syntax`.

**Done when**: `native_render.rs` is ~130 lines shorter. Code widget tests pass.

---

### 2.5 Extract Border Generation → `native_render/borders.rs`

**Functions to move**:
- `generate_border_quads` (~120 lines)

**Work**:
1. Create `crates/ultty/src/renderer/native_render/borders.rs`.
2. Move the function. It only needs `self.color_scheme` from `Renderer`.

**Done when**: File moved cleanly. No behavior change.

---

### 2.6 Extract Widget Content Dispatch → `native_render/widgets.rs`

**Functions to move**:
- `render_widget_content` (~200 lines — the big `match` on `WidgetContent`)
- `upload_native_image` (if it exists in this file)

**Work**:
1. Create `crates/ultty/src/renderer/native_render/widgets.rs`.
2. Move the widget dispatch. It calls into markdown/code renderers — use cross-module calls.

**Done when**: `native_render.rs` main file is under ~1,000 lines (scene traversal + buffer submission + text/richtext rendering). All widget snapshot tests pass.

---

## Phase 3 — Deduplicate `render_view_to_native` (Day 2)

### 3.1 Extract Shared `render_view_to_native` Utility

**Duplicate locations**:
- `crates/ulttui/src/app.rs:28-58`
- `crates/ulttui/src/widgets/dock.rs:68-113`
- `crates/ulttui/src/widgets/primitives.rs:15-45`

**Work**:
1. Add a `render_to_native` default method on the `View` trait in `widgets/mod.rs`, or a free function in that module.
2. Support the `constraint_overrides: Option<Constraints>` parameter that `dock.rs` needs (the others pass `None`).
3. Replace all three call sites with the shared function.
4. Remove the now-dead local copies.

**Done when**: `cargo grep 'fn render_view_to_native\|fn render_child_to_native'` finds exactly one definition. Tests pass.

---

## Phase 4 — Render Path Allocation Reduction (Day 2–3)

### 4.1 Cache `scroll_adjustments` and `clip_rects`

**File**: `crates/ultty/src/renderer/native_render.rs` (after split: `native_render/mod.rs`), lines 241-319

**Problem**: Two `HashMap` allocations + two full scene-tree traversals per frame, even when layout hasn't changed.

**Work**:
1. Add `cached_scroll_adj: HashMap<RegionId, (f32, f32)>` and `cached_clip_rects: HashMap<RegionId, (f32, f32, f32, f32)>` fields to the render state (or `NativeSession`).
2. Invalidate on layout recomputation (set a `layout_dirty` flag in `NativeSession`).
3. Only recompute when dirty. Clear the flag after recomputation.

**Done when**: Adding regions doesn't increase per-frame allocation count. Existing tests pass.

---

### 4.2 Eliminate `Vec<char>` in Text Rendering

**File**: `native_render.rs`, lines 431, 532

**Problem**: `let chars: Vec<char> = line.chars().collect()` allocates per text line per frame.

**Work**:
1. For the single-char fill check (line 433): use `line.chars().count() == 1` and `line.chars().next()` — no allocation needed.
2. For `wrap_line`: rewrite to work with `&str` slices and byte offsets instead of `Vec<char>`. Use `str::char_indices()` for space-finding.
3. For RichText span splitting (line 532): use `segment.char_indices()` instead of collecting.

**Done when**: `Vec<char>` does not appear in non-test code in the renderer. Wrap tests still pass.

---

### 4.3 Pre-allocate Render Buffers

**File**: `native_render.rs`, lines 227-231

**Problem**: `Vec::with_capacity(est * 4)` uses a rough estimate. The estimate could be carried forward.

**Work**:
1. Store previous frame's vertex/index counts on the `Renderer`.
2. Use `Vec::with_capacity(prev_count + prev_count / 4)` (25% headroom) instead of region-count estimate.
3. After render, store actual counts for next frame.

**Done when**: Buffer reallocation count is measurably lower for stable scenes. Can verify with a simple counter or benchmark.

---

## Phase 5 — Fix Production `unwrap()` Calls (Day 3)

### 5.1 App Runner RAII Guard

**File**: `crates/ulttui/src/app.rs`, lines 342-343

**Problem**: `guard.old_focus.take().unwrap()` / `guard.old_theme.take().unwrap()` — panics if guard state is corrupted.

**Work**:
1. Replace with `unwrap_or_else(|| FocusManager::new())` / `unwrap_or_else(Theme::dark)` — graceful fallback instead of crash.
2. Add `tracing::warn!` if the fallback path is hit (indicates a bug, but doesn't kill the app).

**Done when**: No `.unwrap()` in non-test code in `app.rs`. Tests pass.

---

### 5.2 Primitives Section Title

**File**: `crates/ulttui/src/widgets/primitives.rs`, line 187

**Problem**: `self.title.as_ref().unwrap().clone()` — guarded by `is_some()` but still uses unwrap.

**Work**:
1. Replace the `if self.title.is_some()` + `.unwrap()` block with `if let Some(title_text) = &self.title { ... }`.

**Done when**: No `.unwrap()` in non-test code in `primitives.rs`.

---

### 5.3 Markdown Child Render

**File**: `crates/ulttui/src/widgets/markdown.rs`, line 694

**Problem**: `.expect("render child should succeed")` in production render path.

**Work**:
1. Propagate the error with `?` instead of panicking.

**Done when**: No `.expect()` in non-test code in `markdown.rs`.

---

## Phase 6 — Dual Markdown Renderer Convergence (Day 3–4)

### 6.1 Audit Divergence

**Problem**: Two independent markdown renderers exist:
- **Framework side**: `ulttui/src/widgets/markdown.rs` — uses `pulldown_cmark`, parses into `Block` types.
- **GPU side**: `native_render/markdown.rs` — hand-rolled line-by-line string matching.

The GPU renderer doesn't handle: nested emphasis, reference links, HTML entities, footnotes, or any edge case `pulldown_cmark` handles correctly.

**Work**:
1. Enumerate the markdown features each renderer supports. Document the gaps.
2. Decide on the convergence strategy: either (a) send pre-parsed blocks via `WidgetContent::Markdown` so the GPU side receives structured data, or (b) switch the GPU side to also use `pulldown_cmark`.
3. Write the decision as a short design note (≤1 page).

**Done when**: A design decision document exists. No code changes yet — this is planning only.

---

### 6.2 Implement Convergence (Based on 6.1 Decision)

**Scope depends on 6.1 output.** Estimated 2–4 hours.

If option (a) — structured data over the wire:
1. Extend `WidgetContent::Markdown` to carry parsed `Vec<MarkdownBlock>` instead of raw `source: String`.
2. Update the framework's `MarkdownView` to serialize blocks.
3. Update the GPU renderer to consume blocks instead of re-parsing.

If option (b) — `pulldown_cmark` on GPU side:
1. Add `pulldown_cmark` dependency to `ultty` crate.
2. Replace line-by-line parsing with `pulldown_cmark` event stream.
3. Remove ~400 lines of hand-rolled parsing.

**Done when**: One markdown parser, not two. Rendering output matches for a corpus of test markdown documents.

---

## Phase 7 — `porygon-tui` Modularization (Day 4)

### 7.1 Split Into Modules

**File**: `examples/porygon-tui/src/main.rs` — 1,817 lines

**Work**:
1. Create `examples/porygon-tui/src/theme.rs` — move `porygon_theme()` + color constants.
2. Create `examples/porygon-tui/src/splash.rs` — move `SplashScreen` + boot log.
3. Create `examples/porygon-tui/src/approval.rs` — move `ApprovalRequest` + approval modal.
4. Create `examples/porygon-tui/src/weather.rs` — move `WeatherData` + chart rendering.
5. Create `examples/porygon-tui/src/status.rs` — move `AgentStatus` + status bar.
6. Keep `main.rs` as the `Application` impl + message routing (~300 lines).

**Done when**: `main.rs` is under 400 lines. `cargo run -p porygon-tui` still compiles and runs.

---

## Phase 8 — Documentation Pass (Day 4–5)

### 8.1 Doc Comments on Public Widget API

**Files**: `ulttui/src/widgets/{container,text,scroll,list,table,modal}.rs`

**Problem**: ~30 public items (structs, builder methods) lack doc comments. These are the user-facing framework API.

**Work**:
1. Add `///` doc comments to every `pub struct` and `pub fn` in the widget files.
2. Focus on: what the widget does, one example usage, any gotchas.
3. Run `cargo doc -p ulttui --no-deps --open` to verify rendered output.

**Done when**: `cargo doc -p ulttui --no-deps 2>&1 | grep "missing documentation"` produces no output (if the lint is enabled), or manual review confirms all public items are documented.

---

### 8.2 Doc Comments on Framework Core

**Files**: `ulttui/src/{app,focus,theme,cmd,screen}.rs`

**Problem**: `App`, `App::new()`, `App::new_with_theme()`, `FocusManager` methods missing docs.

**Work**:
1. Add doc comments to all public items.
2. Include a module-level `//!` doc for `app.rs` explaining the Elm architecture loop.

**Done when**: Same verification as 8.1.

---

## Phase 9 — Unicode Width Correctness (Day 5)

### 9.1 Fix `wrap_line` for CJK Characters

**File**: `native_render.rs`, `wrap_line()` function (lines 10-54)

**Problem**: Uses `chars.len()` to count columns. CJK characters are 2 columns wide. Emoji are 2 columns wide. Current code will wrap mid-character or overflow.

**Work**:
1. Add `unicode-width` dependency to `ultty` crate (already widely used in the Rust ecosystem).
2. Replace `chars.len()` with `UnicodeWidthStr::width(line)`.
3. Update the word-wrap loop to account for variable character width.
4. Add test cases with CJK text (`"你好世界"`) and emoji (`"👋🌍"`).

**Done when**: `wrap_line("你好世界", 5)` returns `["你好", "世界"]` (each char is 2 cols). Existing wrap tests still pass.

---

## Summary

| Phase | Chunks | Est. Hours | Risk if Skipped |
|-------|--------|-----------|-----------------|
| 1. Red Tests & Clippy | 2 | 1 | CI breaks, bad examples |
| 2. Split native_render | 6 | 4 | 3K file becomes unmaintainable |
| 3. Deduplicate utility | 1 | 0.5 | Bug fixes won't propagate |
| 4. Render allocations | 3 | 2 | Perf degrades with complexity |
| 5. Fix unwraps | 3 | 1 | App crashes on edge cases |
| 6. Markdown convergence | 2 | 3 | Renderers diverge silently |
| 7. porygon-tui split | 1 | 1 | Bad example for users |
| 8. Doc comments | 2 | 2 | Framework is hard to adopt |
| 9. Unicode width | 1 | 1 | CJK/emoji text wraps wrong |
| **Total** | **21** | **~15.5** | |
