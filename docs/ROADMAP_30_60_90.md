# Ultty + Ulttui 30/60/90 Execution Plan

**Date:** 2026-02-17
**Scope:** `ultty`, `ulttui`, `ultty-protocol`, `porygon-tui`
**Goal:** Reach competitive feature parity for AI-agent TUI workflows while preserving Ultty's native-protocol performance advantage.

**Issue backlog:** `docs/ISSUE_BACKLOG_30_60_90.md`

---

## Milestone Board

| Milestone | Window | Theme | Exit Criteria |
|-----------|--------|-------|---------------|
| **M0** | Days 0-30 | Streaming foundation | Token streaming and cancellation are production-ready in native mode and ANSI fallback |
| **M1** | Days 31-60 | Workflow parity | Sessions, memory, and tool visibility are complete and keyboard-first |
| **M2** | Days 61-90 | Project context + hardening | Tree/diff flows, virtual scroll, and protocol capability negotiation are stable |

---

## Dependency Graph

```text
M0-1 Protocol streaming messages
  -> M0-2 Framework stream subscriptions
    -> M0-3 Incremental widget updates
      -> M0-4 porygon-tui websocket integration
        -> M0-5 Streaming UX stabilization

M0 complete
  -> M1-1 Session workflows
  -> M1-2 Memory workflows
  -> M1-3 Slash command system
  -> M1-4 Tool timeline + approvals UX

M1 complete
  -> M2-1 Tree widget + file explorer
  -> M2-2 Diff widget (unified first)
  -> M2-3 Virtual scrolling
  -> M2-4 Capability negotiation v1
  -> M2-5 MCP/model operational panels
```

---

## M0 (Days 0-30): Streaming Foundation

### Must Ship

| Ticket | Deliverable | Depends On |
|--------|-------------|------------|
| **M0-1** | Add protocol messages: `AppendContent`, `StreamStart`, `StreamEnd` | - |
| **M0-2** | Add `Cmd::websocket(...)` and/or `Sub::stream(...)` in `ulttui` | M0-1 |
| **M0-3** | Add incremental append APIs for content/list widgets | M0-2 |
| **M0-4** | Switch `porygon-tui` from blocking `/api/ask` to `/agent/stream` | M0-2, M0-3 |
| **M0-5** | Add cancel/abort, typing indicator, auto-scroll, token counter | M0-4 |

### Acceptance Criteria

- `TTFT` (time to first token) `< 200ms` p95 in local environment.
- Token append latency `< 50ms` p95.
- Cancel request to visual stop `< 150ms` p95.
- No full-region replacement for token streaming paths (validated in frame/protocol logs).
- Feature works in both native mode and ANSI fallback mode.

### Validation Commands

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

---

## M1 (Days 31-60): Workflow Parity

### Must Ship

| Ticket | Deliverable | Depends On |
|--------|-------------|------------|
| **M1-1** | Session flows: list/save/resume/fork/export | M0 |
| **M1-2** | Memory flows: search/remember/recall + browser | M0 |
| **M1-3** | Slash command registry + autocomplete (`/save`, `/resume`, `/sessions`, `/remember`, `/recall`, `/help`) | M1-1, M1-2 |
| **M1-4** | Tool execution timeline (pending/running/success/fail) | M0 |
| **M1-5** | Rich approval UX: structured details + collapsible sections + keyboard shortcuts | M1-4 |

### Acceptance Criteria

- Session save/load `< 500ms` p95.
- Memory recall/search visible in UI with relevance metadata.
- All M1 actions accessible without mouse.
- Endpoint utilization increases from `4` to at least `25` meaningful endpoints.
- Approval flow preserves explicit human decision before irreversible operations.

### Validation Commands

```bash
cargo test --workspace
cargo test -p ulttui --test snapshots
cargo clippy --workspace --all-targets -- -D warnings
```

---

## M2 (Days 61-90): Project Context + Hardening

### Must Ship

| Ticket | Deliverable | Depends On |
|--------|-------------|------------|
| **M2-1** | Tree widget + project file tree panel with lazy expand | M1 |
| **M2-2** | Diff widget (unified mode first, side-by-side optional) | M1 |
| **M2-3** | Virtual scrolling for long conversations and long lists | M1 |
| **M2-4** | Protocol capability negotiation v1 (version + feature flags) | M0 |
| **M2-5** | MCP/model status panels for runtime observability | M1 |

### Acceptance Criteria

- File tree initial load `< 1s` for representative repos.
- 1000-line diff render `< 100ms` in native mode.
- Idle CPU near 0% in native mode with no updates.
- Endpoint utilization reaches `40+` used in production paths.
- Backward compatibility validated for protocol negotiation changes.

### Validation Commands

```bash
cargo test --workspace
cargo bench -p ultty
cargo clippy --workspace --all-targets -- -D warnings
```

---

## Ticket Templates (Definition of Done)

Use this for each ticket (`M0-*`, `M1-*`, `M2-*`).

```text
Title:
Problem:
Scope:
Out of scope:
Implementation notes:
Tests:
Perf checks:
Docs updated:
Acceptance criteria met:
```

---

## Risk Register

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Streaming implementation complexity | Medium | High | Ship minimal vertical slice first, then optimize |
| Protocol changes break compatibility | Low | High | Feature-flag handshake + versioned message guards |
| Long-chat rendering regressions | Medium | Medium | Add virtual scroll and append-only benchmark suite |
| Scope creep in UX polish | High | Medium | Enforce milestone gates before cosmetic work |

---

## Weekly Operating Cadence

1. Monday: lock top-priority tickets and success metrics.
2. Daily: merge small vertical slices only (no long-lived mega branches).
3. Thursday: run performance regression checks and compare against baseline.
4. Friday: milestone review against acceptance criteria and adjust next week.

---

## North Star at Day 90

- Parity on core agent workflows (streaming, sessions, memory, approvals, project context).
- Preserved Ultty advantages (native protocol, retained scene graph, atomic frames, GPU path).
- Reliable ANSI fallback so one binary works inside and outside Ultty.

This plan is intentionally sequenced: no advanced polish before streaming and workflow parity are stable.
