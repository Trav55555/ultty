# Ultty + Ulttui Issue Backlog (30/60/90)

**Date:** 2026-02-17
**Source plan:** `docs/ROADMAP_30_60_90.md`
**Use:** Copy each issue body into GitHub Issues, then assign owner, estimate, and sprint.

---

## Label and Milestone Conventions

- Milestones: `M0-Streaming`, `M1-Workflow-Parity`, `M2-Context-Hardening`
- Priority labels: `priority/p0`, `priority/p1`, `priority/p2`
- Area labels: `area/protocol`, `area/framework`, `area/app`, `area/widgets`, `area/perf`, `area/docs`
- Type labels: `type/feature`, `type/refactor`, `type/perf`, `type/docs`

---

## Sequencing

1. `M0-1 -> M0-2 -> M0-3 -> M0-4 -> M0-5`
2. `M1-1`, `M1-2`, `M1-4` can run in parallel after M0, then `M1-3`, then `M1-5`
3. `M2-4` can start after M0; `M2-1`, `M2-2`, `M2-3`, `M2-5` start after M1

---

## M0: Streaming Foundation (Days 0-30)

### Issue M0-1

**Title:** `[M0-1] Add streaming protocol messages (AppendContent, StreamStart, StreamEnd)`
**Milestone:** `M0-Streaming`
**Labels:** `priority/p0`, `area/protocol`, `type/feature`
**Depends on:** none
**Estimate:** 3-5 days

```markdown
## Problem
The protocol cannot represent incremental token streaming, so apps must replace full regions.

## Scope
- Add protocol message types: `AppendContent`, `StreamStart`, `StreamEnd`.
- Add version-safe encode/decode paths in `ultty-protocol`.
- Add terminal-side handling stubs behind feature gates if needed.
- Add protocol tests for roundtrip encode/decode and unknown message handling.

## Out of Scope
- No websocket client implementation.
- No UI changes in `porygon-tui`.

## Acceptance Criteria
- Messages serialize/deserialize successfully.
- Backward compatibility behavior is documented and tested.
- Unknown/future message handling does not crash decoder.

## Validation
- `cargo test -p ultty-protocol`
- `cargo clippy --workspace --all-targets -- -D warnings`
```

### Issue M0-2

**Title:** `[M0-2] Add stream subscription API to ulttui (Cmd::websocket / Sub::stream)`
**Milestone:** `M0-Streaming`
**Labels:** `priority/p0`, `area/framework`, `type/feature`
**Depends on:** `M0-1`
**Estimate:** 4-6 days

```markdown
## Problem
`ulttui` lacks a native abstraction for external streaming sources (websocket/event streams).

## Scope
- Add `Cmd::websocket(...)` and/or `Sub::stream(...)` API.
- Wire stream messages into app message loop safely.
- Add reconnect/error signaling contract.
- Add example app proving stream subscription flow.

## Out of Scope
- Full porygon endpoint integration.

## Acceptance Criteria
- Framework can subscribe to a stream and emit messages into `update()`.
- Error and disconnect events are observable by the app.
- API is documented with minimal examples.

## Validation
- `cargo test -p ulttui --lib`
- `cargo clippy --workspace --all-targets -- -D warnings`
```

### Issue M0-3

**Title:** `[M0-3] Add incremental append APIs for streaming text and lists`
**Milestone:** `M0-Streaming`
**Labels:** `priority/p0`, `area/widgets`, `type/feature`
**Depends on:** `M0-2`
**Estimate:** 3-5 days

```markdown
## Problem
Widgets require full content replacement, which causes unnecessary rerenders for token streams.

## Scope
- Add append-oriented APIs for streaming text widgets.
- Add append-oriented APIs for list/chat timeline widgets.
- Ensure append paths map to protocol incremental updates where available.
- Add tests for long transcript append behavior.

## Out of Scope
- Virtual scrolling.

## Acceptance Criteria
- Token append does not require full region replacement.
- Long append sessions remain stable and render correctly.
- ANSI fallback still behaves correctly.

## Validation
- `cargo test -p ulttui --lib`
- `cargo test -p ulttui --test snapshots`
```

### Issue M0-4

**Title:** `[M0-4] Switch porygon-tui chat path to /agent/stream websocket`
**Milestone:** `M0-Streaming`
**Labels:** `priority/p0`, `area/app`, `type/feature`
**Depends on:** `M0-2`, `M0-3`
**Estimate:** 3-5 days

```markdown
## Problem
Current chat flow is blocking and returns full response only after completion.

## Scope
- Replace blocking `/api/ask` path with `/agent/stream` websocket path.
- Handle stream start, token events, end, and error events.
- Keep compatibility fallback if stream endpoint unavailable.

## Out of Scope
- Session and memory features.

## Acceptance Criteria
- Responses stream token-by-token in chat view.
- Fallback path is explicit and does not silently fail.
- No regressions in approval flow.

## Validation
- `cargo test --workspace`
- Manual smoke test in native and ANSI mode
```

### Issue M0-5

**Title:** `[M0-5] Ship streaming UX controls (cancel, typing indicator, autoscroll, token counter)`
**Milestone:** `M0-Streaming`
**Labels:** `priority/p0`, `area/app`, `area/perf`, `type/feature`
**Depends on:** `M0-4`
**Estimate:** 2-4 days

```markdown
## Problem
Streaming feels incomplete without cancellation, status cues, and stable scrolling behavior.

## Scope
- Add cancel/abort action while streaming.
- Add typing indicator and stream state transitions.
- Add sticky autoscroll behavior with user override.
- Add live token counter display.
- Add metrics instrumentation for TTFT and append latency.

## Out of Scope
- Animation polish beyond basic indicators.

## Acceptance Criteria
- TTFT < 200ms p95 (local), append latency < 50ms p95.
- Cancel to visual stop < 150ms p95.
- User can scroll up without forced autoscroll jump.

## Validation
- `cargo test --workspace`
- Perf capture script or log output attached in issue comment
```

---

## M1: Workflow Parity (Days 31-60)

### Issue M1-1

**Title:** `[M1-1] Implement session workflows (list, save, resume, fork, export)`
**Milestone:** `M1-Workflow-Parity`
**Labels:** `priority/p0`, `area/app`, `type/feature`
**Depends on:** `M0`
**Estimate:** 4-6 days

```markdown
## Problem
Users cannot persist and resume work reliably inside the TUI.

## Scope
- Integrate session endpoints for list/save/resume/fork/export.
- Add session browser UI and keyboard navigation.
- Add explicit status/error states for failed session operations.

## Out of Scope
- Memory search and slash commands.

## Acceptance Criteria
- Session save/load operations complete under 500ms p95.
- Users can list and resume sessions without leaving TUI.

## Validation
- `cargo test --workspace`
- Manual workflow test across at least 3 sessions
```

### Issue M1-2

**Title:** `[M1-2] Implement memory workflows (search, remember, recall, browse)`
**Milestone:** `M1-Workflow-Parity`
**Labels:** `priority/p0`, `area/app`, `type/feature`
**Depends on:** `M0`
**Estimate:** 3-5 days

```markdown
## Problem
The agent cannot leverage memory features from the TUI, reducing continuity and usefulness.

## Scope
- Integrate memory endpoints for search/remember/recall.
- Add memory panel or modal with relevance metadata.
- Add explicit empty/loading/error states.

## Out of Scope
- Planner/Kanban views.

## Acceptance Criteria
- Users can store and retrieve memory from TUI.
- Recall results show relevance/context metadata.

## Validation
- `cargo test --workspace`
- Manual memory roundtrip test (remember -> recall)
```

### Issue M1-3

**Title:** `[M1-3] Add slash command registry and autocomplete`
**Milestone:** `M1-Workflow-Parity`
**Labels:** `priority/p0`, `area/app`, `type/feature`
**Depends on:** `M1-1`, `M1-2`
**Estimate:** 3-4 days

```markdown
## Problem
Power users lack fast command-driven access to core actions.

## Scope
- Build slash command registry and parser.
- Add autocomplete UI for command names and basic args.
- Ship baseline commands: `/save`, `/resume`, `/sessions`, `/remember`, `/recall`, `/help`.

## Out of Scope
- Command palette global search.

## Acceptance Criteria
- Commands execute reliably from input box.
- Autocomplete is keyboard-first and non-blocking.
- Help text is discoverable in-app.

## Validation
- `cargo test --workspace`
- Command smoke matrix in issue comment
```

### Issue M1-4

**Title:** `[M1-4] Add live tool execution timeline with status transitions`
**Milestone:** `M1-Workflow-Parity`
**Labels:** `priority/p1`, `area/app`, `type/feature`
**Depends on:** `M0`
**Estimate:** 3-5 days

```markdown
## Problem
Users cannot see what tools are doing in real time, reducing trust and debuggability.

## Scope
- Add tool timeline panel (pending/running/success/fail).
- Show tool name, summarized args, and result snippet.
- Support expand/collapse for details.

## Out of Scope
- Full diff visualization.

## Acceptance Criteria
- Tool events appear in order with clear status changes.
- Failed tools are visually distinct and inspectable.

## Validation
- `cargo test --workspace`
- Manual test with multi-tool run
```

### Issue M1-5

**Title:** `[M1-5] Upgrade approval UX with rich body + keyboard shortcuts`
**Milestone:** `M1-Workflow-Parity`
**Labels:** `priority/p1`, `area/widgets`, `area/app`, `type/feature`
**Depends on:** `M1-4`
**Estimate:** 3-5 days

```markdown
## Problem
Approval modals are text-limited and do not present structured details effectively.

## Scope
- Support rich modal body content (structured sections, collapsibles).
- Add keyboard shortcuts for approve/reject/navigation.
- Ensure high-risk details are prominently visible.

## Out of Scope
- Side-by-side diff widget.

## Acceptance Criteria
- Approval modal supports structured data rendering.
- Full flow is keyboard-operable.
- Irreversible actions require explicit confirmation.

## Validation
- `cargo test --workspace`
- Snapshot tests for modal rendering
```

---

## M2: Project Context + Hardening (Days 61-90)

### Issue M2-1

**Title:** `[M2-1] Build Tree widget and project file explorer with lazy expansion`
**Milestone:** `M2-Context-Hardening`
**Labels:** `priority/p1`, `area/widgets`, `area/app`, `type/feature`
**Depends on:** `M1`
**Estimate:** 5-7 days

```markdown
## Problem
Users lack in-TUI project navigation and file hierarchy visibility.

## Scope
- Implement `Tree` widget with expand/collapse and keyboard navigation.
- Add project file explorer panel backed by project tree endpoint.
- Add lazy loading for large directory trees.

## Out of Scope
- Inline editor integration.

## Acceptance Criteria
- Initial tree load under 1s for representative repos.
- Navigation is stable with keyboard only.

## Validation
- `cargo test --workspace`
- Manual test on small and large repos
```

### Issue M2-2

**Title:** `[M2-2] Implement Diff widget (unified mode first)`
**Milestone:** `M2-Context-Hardening`
**Labels:** `priority/p1`, `area/widgets`, `type/feature`, `area/perf`
**Depends on:** `M1`
**Estimate:** 5-7 days

```markdown
## Problem
Users cannot inspect file changes in a first-class in-TUI diff view.

## Scope
- Implement unified diff widget with line numbers and +/- styling.
- Add syntax highlight hooks where available.
- Integrate diff viewer into approval and tool result flows.

## Out of Scope
- Side-by-side mode (optional follow-up).

## Acceptance Criteria
- 1000-line diff renders under 100ms in native mode.
- Diff is readable and keyboard-scrollable.

## Validation
- `cargo test --workspace`
- `cargo bench -p ultty`
```

### Issue M2-3

**Title:** `[M2-3] Add virtual scrolling for long conversations and large lists`
**Milestone:** `M2-Context-Hardening`
**Labels:** `priority/p1`, `area/widgets`, `area/perf`, `type/perf`
**Depends on:** `M1`
**Estimate:** 4-6 days

```markdown
## Problem
Long transcripts and big lists degrade rendering and interaction performance.

## Scope
- Add virtualized rendering path for long list/chat components.
- Preserve scroll position and selection behavior.
- Add benchmarks for 1k/5k/10k item scenarios.

## Out of Scope
- Visual animation effects.

## Acceptance Criteria
- Long lists remain responsive under expected workload.
- No visual gaps or item duplication while scrolling.

## Validation
- `cargo test --workspace`
- Perf benchmark output attached to issue
```

### Issue M2-4

**Title:** `[M2-4] Add protocol capability negotiation v1 (version + feature flags)`
**Milestone:** `M2-Context-Hardening`
**Labels:** `priority/p1`, `area/protocol`, `type/feature`
**Depends on:** `M0`
**Estimate:** 3-5 days

```markdown
## Problem
Current backend detection is coarse and does not negotiate protocol feature support explicitly.

## Scope
- Add handshake capability map and protocol version declaration.
- Gate optional features on negotiated capabilities.
- Add compatibility matrix tests across versions.

## Out of Scope
- Major protocol redesign.

## Acceptance Criteria
- Native sessions negotiate supported features before use.
- Unsupported features degrade gracefully without panic.
- Compatibility tests pass for old/new combinations.

## Validation
- `cargo test -p ultty-protocol`
- `cargo test --workspace`
```

### Issue M2-5

**Title:** `[M2-5] Add MCP/model operational status panels`
**Milestone:** `M2-Context-Hardening`
**Labels:** `priority/p2`, `area/app`, `type/feature`
**Depends on:** `M1`
**Estimate:** 3-4 days

```markdown
## Problem
Operators cannot quickly inspect model and MCP runtime health in one place.

## Scope
- Add panel(s) for model defaults/availability and MCP server status.
- Include health state, last error, and reconnect actions where supported.
- Keep panel updates incremental and lightweight.

## Out of Scope
- Full MCP management console.

## Acceptance Criteria
- Runtime status is visible without leaving chat flow.
- Common failure states are clear and actionable.

## Validation
- `cargo test --workspace`
- Manual disconnect/reconnect smoke test
```

---

## Backlog Health Checklist

- Every issue has: owner, estimate, dependency, milestone.
- Every issue includes measurable acceptance criteria.
- Every merged issue updates docs where behavior changes.
- No issue closes without validation command output attached.
