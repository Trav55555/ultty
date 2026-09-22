# Native Rendering Validation Report

**Date**: 2026-02-11
**Scope**: Execute the runnable matrix in `docs/archive/NATIVE_RENDERING_EXECUTION_PLAN.md` and record pass/fail evidence.

---

## Environment

- Workspace: `/home/travers/brain-2/01-projects/tui-terminal`
- Artifact folder: `/tmp/ultty-validation-2026-02-11`
- ultty session: `tmux` session `ultty-test-plan`

---

## Gate Results

### A) Build/Test Gates

- PASS: `cargo check --workspace`
- PASS: `cargo test --workspace`
- PASS: `cargo build --release -p ultty -p counter-app -p todo-app -p dashboard -p form-app -p chat -p async-fetch -p porygon-tui`

Notes:
- Initial attempt including `-p ulttyctl` failed because `ulttyctl` is a bin target within package `ultty`, not a standalone package.
- Corrected by building package `ultty` only.

### B) ultty Control Channel

- PASS: started isolated ultty session in tmux
- PASS: `./target/release/ulttyctl ping` returned `PONG`

Launch command used:

```bash
ULTTY_PTY_LOG=/tmp/ultty-validation-2026-02-11/ultty-pty.bin ./target/release/ultty
```

---

## Native Smoke Run Results

### Counter App

- PASS (launch): `./target/release/counter-app`
- Evidence screenshot: `/tmp/ultty-validation-2026-02-11/counter-native.png`

### Example Sweep

- PASS (launch): `./target/release/todo-app`
  - Screenshot: `/tmp/ultty-validation-2026-02-11/todo-app-native.png`
  - PTY text: `/tmp/ultty-validation-2026-02-11/todo-app-text.txt`
- PASS (launch): `./target/release/dashboard`
  - Screenshot: `/tmp/ultty-validation-2026-02-11/dashboard-native.png`
  - PTY text: `/tmp/ultty-validation-2026-02-11/dashboard-text.txt`
- PASS (launch): `./target/release/form-app`
  - Screenshot: `/tmp/ultty-validation-2026-02-11/form-app-native.png`
  - PTY text: `/tmp/ultty-validation-2026-02-11/form-app-text.txt`
- PASS (launch): `./target/release/chat`
  - Screenshot: `/tmp/ultty-validation-2026-02-11/chat-native.png`
  - PTY text: `/tmp/ultty-validation-2026-02-11/chat-text.txt`
- PASS (launch): `./target/release/async-fetch`
  - Screenshot: `/tmp/ultty-validation-2026-02-11/async-fetch-native.png`
  - PTY text: `/tmp/ultty-validation-2026-02-11/async-fetch-text.txt`

### Porygon-tui

- PASS (launch): `./target/release/porygon-tui`
- Screenshot: `/tmp/ultty-validation-2026-02-11/porygon-tui-native.png`
- PTY text: `/tmp/ultty-validation-2026-02-11/porygon-tui-text.txt`

Daemon note:
- No explicit daemon connection error text was observed in PTY captures.
- Native-overlay correctness and interactive behavior still require live visual/manual confirmation.

---

## Limitations Observed During Validation

1. `ulttyctl text` reports PTY content only; it does not capture native overlay widgets.
2. Automated screenshot interpretation was unavailable in this environment (image-analysis tool did not return results).
3. Physical keyboard validation cannot be completed via `ulttyctl`; it requires interactive user confirmation on the ultty window.

---

## Pending Manual Checks (Blocking Full Sign-off)

Run these in a real ultty window:

1. `counter-app`: press `+`, `-`, `Tab`, `Shift+Tab` and verify value/focus updates.
2. Confirm key routing does not cause unintended PTY shell side effects while native app is focused.
3. Confirm divider parity visually in native mode (full-width divider behavior).
4. `porygon-tui`: verify input and status updates while daemon is running.

---

## Overall Status

- **Automated matrix status**: PASS (build/test/launch/screenshot capture)
- **Manual interactivity status**: PENDING
- **Release readiness for native UX sign-off**: BLOCKED on manual checks above
