# Tasks

Consolidated tracker for all ultty/ulttui/porygon-tui work.

**Source plans**:
- `docs/TECH_DEBT_PLAN.md` — 21 chunks, ~15.5 hrs
- `docs/PORYGON_VISUAL_PARITY_PLAN.md` — 16 phases (all ✅)
- `docs/FEATURE_PARITY_EXECUTION.md` — 38 chunks, ~93-125 hrs

---

## Visual Parity (COMPLETE)

- [x] 0A: Column/Row bg/border builders
- [x] 0B: Row.height builder
- [x] 1: Message cards with role styling
- [x] 2: Header coral bar
- [x] 3A: Sidebar bg/border
- [x] 3B: Cassette border
- [x] 4: Weather widget with sparkline
- [x] 5: Moon phase widget
- [x] 6: Welcome message with bullets
- [x] 7: Boot screen polish
- [x] 8: Header clock
- [x] 9A: Welcome message uses MarkdownView
- [x] 9B: Header locked to top
- [x] F1: RichText width: Auto + fill_width()
- [x] F2: Native text wrapping in GPU renderer
- [x] F3: TextInput.vim() fix
- [x] F5-F10: Framework fixes (border subtraction, ScrollView overflow, etc.)
- [x] 12: System stats (CPU/RAM) in Cassette

---

## Tech Debt

### TD Phase 1 — Red Tests & Clippy

- [x] 1.1: Fix `test_client_hello_handshake` flake — `crates/ultty/tests/native_protocol.rs`
- [x] 1.2: Fix clippy warnings in porygon-tui — `examples/porygon-tui/src/main.rs`

### TD Phase 2 — Split `native_render.rs`

- [ ] 2.1: Create `RenderContext` struct — `crates/ultty/src/renderer/native_render.rs`
- [ ] 2.2: Extract syntax tokenizer → `native_render/syntax.rs`
- [ ] 2.3: Extract markdown GPU renderer → `native_render/markdown.rs`
- [ ] 2.4: Extract code renderer → `native_render/code.rs`
- [ ] 2.5: Extract border generation → `native_render/borders.rs`
- [ ] 2.6: Extract widget content dispatch → `native_render/widgets.rs`

### TD Phase 3 — Deduplicate

- [ ] 3.1: Extract shared `render_view_to_native` utility

### TD Phase 4 — Render Allocations

- [ ] 4.1: Cache `scroll_adjustments` and `clip_rects`
- [ ] 4.2: Eliminate `Vec<char>` in text rendering
- [ ] 4.3: Pre-allocate render buffers

### TD Phase 5 — Fix Production `unwrap()`

- [ ] 5.1: App runner RAII guard — `crates/ulttui/src/app.rs`
- [ ] 5.2: Primitives section title — `crates/ulttui/src/widgets/primitives.rs`
- [ ] 5.3: Markdown child render — `crates/ulttui/src/widgets/markdown.rs`

### TD Phase 6 — Dual Markdown Convergence

- [ ] 6.1: Audit divergence (planning only)
- [ ] 6.2: Implement convergence

### TD Phase 7 — porygon-tui Modularization

- [ ] 7.1: Split main.rs into modules (theme, splash, approval, weather, status)

### TD Phase 8 — Documentation

- [ ] 8.1: Doc comments on public widget API
- [ ] 8.2: Doc comments on framework core

### TD Phase 9 — Unicode

- [ ] 9.1: Fix `wrap_line` for CJK characters

---

## Feature Parity

### FP Phase 1 — Streaming Foundation (P0, ~30-40 hrs)

- [x] 1.1: Protocol: Add `AppendContent` message — `ultty-protocol/src/messages.rs`, `codec.rs`
- [x] 1.2: Protocol: Add `StreamStart`/`StreamEnd` — `ultty-protocol/src/messages.rs`, `codec.rs`
- [x] 1.3: Terminal: Handle `AppendContent` in NativeSession — `ultty/src/protocol/session.rs`
- [x] 1.4: Terminal: Handle `StreamStart`/`StreamEnd` state — `ultty/src/protocol/session.rs`
- [x] 1.5: Framework: Add `Cmd::Subscribe`/`Unsubscribe` (generic `Stream<Item=M>`, not WS-specific) — `ulttui/src/cmd.rs`, `app.rs`
- [x] 1.6: Skipped — `Cmd::Subscribe` covers the `Sub::Stream` use case; declarative stream subs deferred
- [x] 1.7: Skipped — full-frame render handles streaming; AppendContent optimization deferred until profiling shows need
- [x] 1.8: Framework: Add `MarkdownView::streaming()` — `ulttui/src/widgets/markdown.rs`
- [x] 1.9: App: WebSocket client via `Cmd::subscribe` with HTTP fallback — `porygon-tui/src/main.rs`
- [x] 1.10: App: Streaming UX (cursor, auto-scroll, Escape abort, token counter) — `porygon-tui/src/main.rs`

### FP Phase 2 — Session & Memory (P0, ~10-15 hrs)

- [ ] 2.1: App: Slash command parser — new `porygon-tui/src/commands.rs`
- [ ] 2.2: App: Session list and resume (`/sessions`, `/resume`)
- [ ] 2.3: App: Memory commands (`/remember`, `/recall`)
- [ ] 2.4: App: Auto-save session on exit
- [ ] 2.5: App: Input autocomplete for slash commands

### FP Phase 3 — Tool Visibility (P1, ~12-15 hrs)

- [ ] 3.1: Framework: Rich modal body (View instead of String) — `ulttui/src/widgets/modal.rs`
- [ ] 3.2: Framework: Collapsible widget — new `ulttui/src/widgets/collapsible.rs`
- [ ] 3.3: App: Tool execution display in chat
- [ ] 3.4: App: Enhanced approval modal with rich content
- [ ] 3.5: App: Tool registry browser (`/tools`)
- [ ] 3.6: App: MCP server status panel (`/mcp`)

### FP Phase 4 — Project Context (P1, ~15-20 hrs)

- [ ] 4.1: Framework: Tree widget — new `ulttui/src/widgets/tree.rs`
- [ ] 4.2: Framework: Diff widget — new `ulttui/src/widgets/diff.rs`
- [ ] 4.3: App: File tree panel (`/tree`, `/scan`)
- [ ] 4.4: App: Project info display (`/project`, `/switch`)
- [ ] 4.5: App: File diff viewer for tool outputs

### FP Phase 5 — Planning & Tasks (P2, ~8-10 hrs)

- [ ] 5.1: App: Plan creation and tracking (`/plan`, `/next`, `/current`)
- [ ] 5.2: App: Kanban board (`/kanban`, `/add`, `/done`)
- [ ] 5.3: App: Async task queue display (`/tasks`, `/cancel-task`)
- [ ] 5.4: App: Cron job management (`/cron`, `/cron-run`)

### FP Phase 6 — Advanced UX (P2, ~10-15 hrs)

- [ ] 6.1: App: Model selection (`/models`, `/model`)
- [ ] 6.2: App: Web search integration (`/search`, `/ai-search`)
- [ ] 6.3: App: Trainer profile and event stream (`/profile`, `/party`)
- [ ] 6.4: App: Validation pipeline (`/validate`, `/gym`)
- [ ] 6.5: Framework: Virtual scrolling for long conversations — `ulttui/src/widgets/scroll.rs`

### FP Phase 7 — Competitive Advantages (P3, ~8-10 hrs)

- [ ] 7.1: Terminal: Streaming cursor animation — `ultty/src/renderer/native_render.rs`
- [ ] 7.2: Terminal: Native image widget rendering — `ultty/src/renderer/native_render.rs`
- [ ] 7.3: App: porygon-tui module split (commands, daemon, sidebar, views, types)

---

## Summary

| Track | Total | Done | Remaining | Est. Hours |
|-------|-------|------|-----------|------------|
| Visual Parity | 18 | 18 | 0 | 0 |
| Tech Debt | 21 | 2 | 19 | ~14 |
| Feature Parity | 38 | 10 | 28 | ~73-103 |
| **Total** | **77** | **30** | **47** | **~87-117** |

## Recommended Order

1. ~~**Tech Debt 1.1-1.2** — Fix red tests + clippy~~ ✅
2. ~~**Feature Parity 1.1-1.10** — Streaming foundation (protocol → terminal → framework → app)~~ ✅
3. **Feature Parity 2.1** — Slash command system (unblocks all Phase 2-6 app work) ← **NEXT**
4. **Feature Parity 2.2-2.5** — Session + memory
5. **Tech Debt 2.1-2.6** — Split native_render.rs (before adding more renderer code)
6. **Feature Parity 3.1-3.6** — Tool visibility
7. **Feature Parity 4.1-4.5** — Project context
8. Remaining tech debt + FP phases 5-7 interleaved
