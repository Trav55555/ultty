> Historical document, archived 2026-09-22. Not maintained.
> See [docs/ISSUE_BACKLOG_30_60_90.md](../ISSUE_BACKLOG_30_60_90.md) for current work.

# Native Rendering Execution Plan (Beautiful + Performant)

Status: Historical execution document. Use `tasks.md` as the source of truth for open/closed execution items.

**Goal**: Make ultty native-mode apps visually polished and interaction-correct, while preserving current performance margins.

**Baseline (already true)**:
- `cargo check --workspace` passes
- `cargo test --workspace` passes
- Native bg/text GPU rendering, atomic frame buffering, and screenshot support are implemented

**Non-goals (for now)**:
- Large protocol redesign unrelated to correctness
- Deep renderer rewrites before we have app-level benchmark evidence

---

## Success Criteria

The plan is complete when all are true:

1. Native and ANSI parity checks pass for divider behavior and core widget layouts.
2. Physical keyboard interaction is validated in native apps (no accidental PTY side-effects during native interaction).
3. `porygon-tui` and all shipped examples run in ultty native mode with passing smoke checks.
4. Performance guardrails hold under representative native app scenes.
5. The verification matrix in this document can be run by another engineer and produce the same pass/fail outcomes.

---

## Phase 1 - Correctness Before Optimization

### 1.1 Divider Parity (Native vs ANSI)

**Problem**: Divider currently renders as a single glyph in native mode while ANSI fills width.

**Implementation target**:
- Ensure horizontal divider renders across available width in native mode.

**Done when**:
- Counter app and porygon-tui screens show full-width dividers in native mode.
- ANSI fallback output remains unchanged.

**Files to touch**:
- `crates/ulttui/src/widgets/divider.rs`
- If needed for terminal-side behavior: `crates/ultty/src/renderer/native_render.rs`

---

### 1.2 Keyboard Interaction Validation (Physical)

**Problem**: Logic is present, but physical window interaction is not fully validated end-to-end.

**Implementation target**:
- Verify and harden key routing for native app interaction in real window sessions.

**Done when**:
- Counter app increments/decrements using keyboard in ultty window.
- Form app focus navigation with `Tab`/`Shift+Tab` works in native mode.
- No unexpected shell command input while interacting with focused native app.

**Files to inspect/update if needed**:
- `crates/ultty/src/app/input.rs`
- `crates/ultty/src/protocol/server.rs`

---

## Phase 2 - Native App Quality Sweep

### 2.1 Example App Native Smoke Matrix

Run and verify all native-mode examples:
- `examples/todo-app`
- `examples/dashboard`
- `examples/form-app`
- `examples/chat`
- `examples/async-fetch`
- `examples/counter-app`
- `examples/porygon-tui` (with daemon)

**Done when**:
- Each app launches in ultty.
- Core interaction path works (at least one key action and one focus/navigation action).
- Screenshot artifact captured for each app.

---

### 2.2 Porygon-tui Runtime Validation

**Prereq**: porygon daemon running on `localhost:18789`.

**Done when**:
- Session starts and status UI updates render correctly.
- Keyboard interaction works (input field, modal confirmation where applicable).
- No rendering corruption during updates.

---

## Phase 3 - Testable Performance Guardrails

### 3.1 Native Rendering Guardrails

Define simple guardrails for app-realistic scenes (not only micro benches):

- Frame render target: `< 5ms` steady-state
- Layout recompute target: `< 1ms`
- Input latency target: `< 10ms`

**Done when**:
- Existing benchmark paths still pass.
- New native-app smoke runs do not regress noticeably from baseline.

---

### 3.2 Cache/Dirty Observability

**Implementation target**:
- Add or use existing tracing to confirm when native buffer cache reuse is active vs full rebuild.

**Done when**:
- Static app screens show high cache reuse.
- Dynamic app screens rebuild only when expected.

---

## Test Matrix (Runnable)

Use this section as the executable checklist.

### A. Build + Test Gates

```bash
cargo check --workspace
cargo test --workspace
cargo build --release -p ultty -p counter-app
```

Pass criteria:
- All commands exit 0.

### B. Launch ultty and Verify Control Channel

```bash
tmux new-session -d -s ultty-test
tmux send-keys -t ultty-test "ULTTY_PTY_LOG=/tmp/ultty-pty.bin ./target/release/ultty" Enter
./target/release/ulttyctl ping
```

Pass criteria:
- `ulttyctl ping` returns `PONG`.

### C. Counter App Native Interaction

```bash
./target/release/ulttyctl send-text "cargo run -p counter-app"
./target/release/ulttyctl send-key enter
sleep 3
./target/release/ulttyctl screenshot /tmp/counter-native.png
./target/release/ulttyctl text
```

Manual physical-key check (required):
- Press `+`, `-`, `Tab`, `Shift+Tab` in ultty window.

Pass criteria:
- Counter value changes in UI.
- Focus movement is visible.
- Divider spans expected width.

### D. Example App Sweep

Run each app in ultty and capture screenshot:

```bash
# Repeat pattern per app:
./target/release/ulttyctl send-key c ctrl
./target/release/ulttyctl send-text "cargo run -p todo-app"
./target/release/ulttyctl send-key enter
sleep 3
./target/release/ulttyctl screenshot /tmp/todo-native.png
./target/release/ulttyctl text
```

Repeat for:
- `dashboard`
- `form-app`
- `chat`
- `async-fetch`

Pass criteria:
- App launches without panic.
- UI is legible and aligned.
- One interaction path works.

### E. Porygon-tui Validation

```bash
./target/release/ulttyctl send-key c ctrl
./target/release/ulttyctl send-text "cargo run -p porygon-tui"
./target/release/ulttyctl send-key enter
sleep 5
./target/release/ulttyctl screenshot /tmp/porygon-native.png
./target/release/ulttyctl text
```

Pass criteria:
- Connects to daemon and renders status/chat panes.
- Keyboard interaction works in active pane/input.

### F. Performance Sanity

```bash
cargo check --workspace --all-targets
cargo test --workspace
```

If benches exist in your branch/workstation setup, run them and compare to baseline.

Pass criteria:
- No regressions from established baseline thresholds.

---

## Evidence to Record Per Run

For each phase, store:
- command output (pass/fail)
- screenshot paths
- short note for manual key interaction result
- any regression notes with file references

Suggested artifact folder:
- `/tmp/ultty-validation-<date>/`

---

## Risk Register

1. **Dual-routing input side effects**: key events may reach PTY and native app simultaneously in some flows.
2. **Parity drift**: native and ANSI behavior can diverge in widgets beyond divider.
3. **False confidence from automation**: `ulttyctl` cannot fully replace physical keyboard validation.
4. **Premature optimization**: renderer rewrites before evidence can create churn without user-visible gains.

---

## Shipping Gate

Before calling this complete:

- [ ] Divider parity fixed and visually validated
- [ ] Physical keyboard test completed and documented
- [ ] All example apps smoke-tested in native mode
- [ ] porygon-tui validated with daemon
- [ ] Build/test gates pass
- [ ] Latest commit pushed after validation
