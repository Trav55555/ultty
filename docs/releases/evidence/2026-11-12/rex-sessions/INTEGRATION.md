# Post-merge integration evidence

This ledger records completed checks and remaining gates separately. The
historical runtime evidence in [README.md](README.md) verifies earlier
binaries and does not establish a runtime pass for the merged source.

Merged Rust source boundary: `b61448a`

Upstream integration base: `ba43d2e`

Documentation, harness and evidence follow-up changes are outside that Rust
source boundary. The source was frozen for the integration checks. Harness
follow-up commit `65ff448` contains no Rust source changes.

The focused PID/MCP gate used the frozen headless host/CLI binaries in
[pid-binaries.json](integration/pid-binaries.json). The rebuilt GUI/runtime
set is recorded in [binary-manifest.json](integration/binary-manifest.json),
including the inline example. Its GUI SHA-256 is
`ed2fe21a2250861b8d0a7246fd69423bdb64757f5fec4e65033c9f1725e1686f`.
[toolchains.json](integration/toolchains.json)
records the observed compiler versions, including the Homebrew Rust 1.98.1
build and Rust 1.90.0 MSRV compiler.

| Gate | Result | Command and retained receipt |
|---|---|---|
| Formatting and whitespace | Passed, no log retained | `cargo fmt --all -- --check` and `git diff --check`, executed before the merge commit. This row is an operator receipt rather than a retained command log. |
| Workspace Clippy | Passed | [Clippy log](integration/pr-integration-clippy-02.log); command below. |
| Workspace tests, attempt 01 | Failed; preserved | [Failed test log](integration/pr-integration-workspace-test-01.log); serial test command below. |
| Workspace tests, attempt 02 | Passed, exit 0 | [Rerun log](integration/pr-integration-workspace-test-02.log), same command/environment after builds finished. |
| Headless runtime check | Passed, exit 0 | [Headless check log](integration/integration-headless-check-01.log), pinned Rust 1.98.1, 24.11 seconds including lock waits. |
| Headless host/CLI build | Passed, exit 0 | [Build log](integration/integration-headless-build-01.log), 1 minute 37 seconds. |
| MSRV check | Passed, exit 0 | [MSRV check log](integration/integration-msrv-check-01.log), Rust 1.90, 43.77 seconds including lock waits. |
| Excluded Porygon | Passed | [Porygon check log](integration/integration-porygon-check-01.log); command below. |
| Focused PID selection and MCP pinning | Passed | [Result](integration/control-pid-result.json), [input hashes](integration/control-pid-inputs.json), 1.35 seconds. Two private owners verified strict environment selection, explicit PID precedence, MCP owner/token/capability pinning, refusal of a foreign implicit pane and refusal to fall back from an unavailable selected owner. Owned hosts and MCP processes stopped. This is not the full App control workflow gate. |
| Local persistent viewer | Passed, exit 0 | [Receipt](integration/viewer/receipt.json), [driver](integration/viewer/driver.log), [initial image](integration/viewer/first.png), [reattached image](integration/viewer/reattached.png). Key plus 12-key burst reaches 13; viewer close/kill preserves host, child and native identity; observer input refused; explicit owner cleanup completed. Graceful closes took 90 ms and 18 ms. Reattached count 13 visually inspected. |
| Existing App native split | Passed, exit 0 | [Log](integration/native-split/driver.log), [screenshot](integration/native-split/split.png). Native layout/input/resize and independent in-band panes passed. |
| Existing App inline surface | Passed, exit 0 | [Log](integration/inline/driver.log), [placed](integration/inline/placed.png), [shifted](integration/inline/shifted.png), [siblings](integration/inline/split.png). Placement, offscreen restore, linewise scrolling, copy text and sibling survival passed. [GUI log](integration/inline/ultty.log) preserves the X11 connection/clipboard errors from simultaneous Xvfb/GUI cleanup; functional assertions passed, graceful teardown is covered by the viewer gate. |
| Existing App control | Passed, exit 0 | [Result](integration/app-control/result.json), [semantic result](integration/app-control/semantic-result.json), [driver](integration/app-control/driver.log), 5.98 seconds. Full App control fences, status, jobs, replay, queue pressure, Rust/Luau semantic actions, no-handler refusal and ANSI parity passed. A stopped app timed out with unknown outcome, then applied the queued action after resume without an automatic retry. |
| Persistent host control | Passed, exit 0 | [Result](integration/session-control/result.json), [driver](integration/session-control/driver.log), [detached actions](integration/session-control/detached-actions.json), 2.02 seconds. Literal argv, retained jobs, fenced Bash, status, Rust/Luau actions, zero-reader detached actions, V2 replay and MCP passed. |
| Actual Tailscale path | Pending, `NeedsLogin` | The remote daemon requires login. No actual tailnet runtime pass is claimed; ordinary DNS/public SSH evidence remains distinct. |
| Remote archive build | Passed; source delta retained | [Build log](integration/remote-build/build.log), [source provenance](integration/remote-build/source-provenance.json), [exact delta](integration/remote-build/source-delta.diff). Archive SHA-256 `7d10119564c26ecb0432f1902993af47d9f2a401c4b2ffe32f8924d8f5165b1e` built in 7 minutes 15 seconds. This archive differs from `b61448a` in the test-helper reexport gating in `app/control_wait.rs`; it is not an equal-byte source receipt. |
| Deterministic memory budget | Passed, exit 0 | [Budget log](integration/pr-integration-memory-01.log). The existing workload, including the simulated 10-hour agent session, remained unchanged. This is a budget check, not a comparative performance result or a 10-hour wall-clock soak. |

All Cargo commands ran from `.worktrees/rex-features` with
`TMPDIR=/home/travers/projects/tui-terminal/target/rex-tmp`,
`CARGO_INCREMENTAL=0` and `RUSTFLAGS=-Dwarnings`. The workspace Clippy,
workspace test, excluded Porygon and memory-budget commands used:

```sh
CARGO_TARGET_DIR=/home/travers/projects/tui-terminal/target/rex-features
```

```sh
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
RUST_TEST_THREADS=1 cargo test --workspace --locked --offline
cargo check --manifest-path examples/porygon-tui/Cargo.toml --locked --offline
cargo test -p ultty --bench memory --locked --offline
```

The headless check used the pinned Rust 1.98.1 toolchain:

```sh
CARGO_TARGET_DIR=/home/travers/projects/tui-terminal/target/rex-vt-check \
  cargo check -p ultty --no-default-features --lib --bin ultty-session --bin ulttyctl --locked --offline
CARGO_TARGET_DIR=/home/travers/projects/tui-terminal/target/rex-vt-check \
  cargo build -p ultty --no-default-features --bin ultty-session --bin ulttyctl --locked --offline
```

The MSRV check used its separate cache:

```sh
CARGO_TARGET_DIR=/home/travers/projects/tui-terminal/target/rex-msrv \
  rustup run 1.90 cargo check --workspace --all-targets --locked --offline
```

The first workspace test attempt failed at
`overrides::script::tests::two_hundred_widgets_call_luau_once_each_then_not_at_all`:
the cold frame produced 19 yellow widgets where the assertion expects 20.
The existing Luau per-frame budget remains 8 ms. The failure occurred while
other compilation was active; scheduling sensitivity is the current
explanation. The failure log is retained and neither the budget nor the
expected assertion was changed. The complete serial rerun after compilation
finished passed. This does not establish that the timing-sensitive assertion
will always pass under competing build load.

The runtime gates used the frozen binaries named above and private disposable
local fixtures. The persistent-host gate ran with task-local `TMPDIR`,
`CARGO_TARGET_DIR` set to `target/rex-features`, and `ULTTY_SESSION`,
`ULTTYCTL`, `COUNTER_APP` and `ULTTUI_LUA` set to the corresponding frozen
files in `logs/pr-integration-bin`:

```sh
python3 scripts/e2e/session-control.py /home/travers/projects/tui-terminal/target/rex-features/logs/pr-integration-session-control-01
```

The existing App control gate used the same task-local `TMPDIR` and
`CARGO_TARGET_DIR`, with these frozen binary arguments:

```sh
python3 scripts/e2e/control-workflows.py \
  --ultty /home/travers/projects/tui-terminal/target/rex-features/logs/pr-integration-bin/ultty \
  --ulttyctl /home/travers/projects/tui-terminal/target/rex-features/logs/pr-integration-bin/ulttyctl \
  --counter /home/travers/projects/tui-terminal/target/rex-features/logs/pr-integration-bin/counter-app \
  --lua /home/travers/projects/tui-terminal/target/rex-features/logs/pr-integration-bin/ulttui-lua \
  --output /home/travers/projects/tui-terminal/target/rex-features/logs/pr-integration-app-control-01
```

The actual tailnet test remains pending because the remote Tailscale daemon
reports `NeedsLogin`. The remote build is preparation evidence only and does
not establish a tailnet connection or a Tailscale runtime pass.

The selected receipts omit credentials, token files, full configurations
and large raw snapshots. Historical and integration runtime records retain
their separate binary hashes. CI publication remains pending; this package
contains local gate evidence rather than a completed CI run.
