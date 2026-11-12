# Rex and Tailscale implementation progress

Worktree: `.worktrees/rex-features`, branch `feat/rex-session-workflows`.
Starting commit: `ffcf318`. The user authorized the full roadmap in stages on 2026-11-12.

The [implementation plan](rex-tailscale-implementation-plan.md) defines the scope and acceptance gates. This record distinguishes completed stages from work in progress. Main remains unchanged.

| Stage | Status | Ownership |
| --- | --- | --- |
| Control input acceptance, strict discovery, argv MCP launch | Implemented; verification below | Sol control |
| Pane-owned in-band transport, inline splits, bounded native queues | Implemented; verification below | Terra |
| OSC 7501 store, lifecycle, queries and application reporting | Implemented; verification below | Sol status; root/control integration |
| Profiles, pane focus/zoom/swap, workspace recipes | Implemented; verification below | Root |
| Completion identity, versioned observation and capability discovery | Implemented; verification below | Sol control |
| Semantic actions and accessibility dispatch | Pending | Sol after status |
| Independently driven VT/native session runtime | Pending | Terra after pane qualification |
| Local persistent host and bounded viewer attachment | Pending | Runtime integration |
| SSH attachment and separate remote capabilities/diagnostics | Pending | Remote integration |

Verification receipts and commit boundaries will be added as each stage passes. No implementation or performance claim follows from this table alone. External Tailscale enrollment, cloud provisioning, public listeners, merge and push are outside this implementation run.

## Initial verification

- Status VT core passed `RUSTFLAGS=-Dwarnings cargo test -p ultty-vt --lib` in the independent `target/rex-vt-check` build directory; the final boundary-vector additions passed a subsequent focused `program_status` run.
- The first integrated build caught a new `Cmd::ReportStatus` match while the emitter was still being wired. The next caught the new status event before the control mapping was added. Both source omissions have been addressed; the integrated gates remain pending.
- All subsequent Cargo gates use `RUSTFLAGS=-Dwarnings` and `CARGO_TARGET_DIR=/home/travers/projects/tui-terminal/target/rex-features` to share one consistent build cache. An independent VT-only target avoids blocking core tests behind graphics compilation.
- Source review caught GUI keyboard broadcasting across two in-band panes. Focused-pane routing now covers keys, releases, IME and paste; the native split smoke includes an actual GUI key event to detect regression. Runtime results are pending.
- Explicit process launches now use a bounded startup receipt, report normal exec/directory failures and preserve argv. A closed receipt is not proof of application readiness; exceptional death before exec can also close it. The receipt uses portable Unix sockets and keeps the child path allocation-free.

## Foundation checkpoint

Implemented C1–C4, N1–N2 and W1: bounded/error-reporting input, checked argv launches, retained job results and fenced OSC 133 completion, owner epochs, versioned snapshot/replay subscriptions, pane-owned native and inline transport, OSC 7501 lifecycle/reporting, profiles and workspace recipes, focus/zoom/swap. A neutral geometry helper removes the manager's renderer dependency. The unregistered runtime draft is excluded from this checkpoint.

Verified on Linux with local PTYs and Unix sockets:

- `cargo test --locked --offline --workspace` passed (`/tmp/ultty-rex-workspace-test.log`); `cargo fmt --all -- --check` passed. The subsequent completion-drain correction passed the focused terminal-manager suite (`/tmp/ultty-rex-manager-final.log`).
- Native protocol suite passed, as did binary/control tests and focused status/EventLoop tests. The ANSI PTY integration exercised working/blocked/done/clear through the actual application writer. Updated CLI tests passed after owner-schema and closed-job corrections.
- Native split GUI smoke passed at `/tmp/rex-native-split.2QT18l`, including two in-band panes and actual GUI-key isolation. Inline GUI smoke passed at `/tmp/rex-inline-split.0LKmOc`, including independent same-ID surfaces in split panes and sibling closure. Each directory records tested binary hashes; these were distinct builds.
- Full-App control gate 01 (`/tmp/ultty-rex-control-20261112-01`) caught premature pane cleanup before final output/completion drainage. Cleanup now waits for the EventLoop's exit-drained boundary; its regression follows the real process/cleanup loop. The fresh full-App rerun and current-source binary rebuild are pending at this checkpoint.
- Workspace clippy caught one collapsible conditional in pane focus; corrected. Final clippy rerun is pending.

The local full-App smoke's failed receipt is retained. Sandbox-only Unix socket `EPERM` failures were rerun with local socket permission; those failures do not establish a source defect. No performance, macOS, real SSH host, screen-reader or remote deployment qualification has been claimed. Inline split rendering currently rebuilds native vertices when inline surfaces are present; optimize only after correctness and measurement.
