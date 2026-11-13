# Rex and Tailscale implementation progress

Worktree: `.worktrees/rex-features`, branch `feat/rex-session-workflows`.
Starting commit: `ffcf318`. The user authorized the full roadmap in stages on 2026-11-12.

The [implementation plan](rex-tailscale-implementation-plan.md) defines the scope and acceptance gates. The staged implementation is complete in commits `4241115` and `6881bc4`. Local validation and real devbox SSH/GUI gates passed; platform and performance limits are listed below. The feature branch integrates upstream `ba43d2e` in `b61448a` and `f835cc2` in `76b8db8`; the main checkout remains unchanged.

| Stage | Status | Ownership |
| --- | --- | --- |
| Control input acceptance, strict discovery, argv MCP launch | Implemented; verification below | Sol control |
| Pane-owned in-band transport, inline splits, bounded native queues | Implemented; verification below | Terra |
| OSC 7501 store, lifecycle, queries and application reporting | Implemented; verification below | Sol status; root/control integration |
| Profiles, pane focus/zoom/swap, workspace recipes | Implemented; verification below | Root |
| Completion identity, versioned observation and capability discovery | Implemented; verification below | Sol control |
| Semantic actions and accessibility dispatch | Implemented; full-App Rust/Lua and accessibility tree tests passed | Sol + root |
| Independently driven VT/native session runtime | Implemented; final headless lifecycle and detached automation passed | Terra |
| Local persistent host and bounded viewer attachment | Implemented; final GPU lifecycle and authority gates passed | Terra + root + Sol |
| SSH attachment and separate remote capabilities/diagnostics | Implemented; local helper and real devbox SSH/GUI gates passed | Sol control |

The receipts below identify tested artifacts and commit boundaries. The initial gates used the existing devbox over public SSH. The user subsequently authorized a branch push and Gitea PR, plus testing through a temporary Tailscale node; see the integration checkpoint below for that later state.

## Initial verification

- Status VT core passed `RUSTFLAGS=-Dwarnings cargo test -p ultty-vt --lib` in the independent `target/rex-vt-check` build directory; the final boundary-vector additions passed a subsequent focused `program_status` run.
- The first integrated builds caught incomplete `Cmd::ReportStatus` and control event matches while the emitter was being wired. Both omissions were corrected before the foundation gates passed.
- All subsequent Cargo gates use `RUSTFLAGS=-Dwarnings` and `CARGO_TARGET_DIR=/home/travers/projects/tui-terminal/target/rex-features` to share one consistent build cache. An independent VT-only target avoids blocking core tests behind graphics compilation.
- Source review caught GUI keyboard broadcasting across two in-band panes. Focused-pane routing now covers keys, releases, IME and paste; the passing native split smoke includes an actual GUI key event to detect regression.
- Explicit process launches now use a bounded startup receipt, report normal exec/directory failures and preserve argv. A closed receipt is not proof of application readiness; exceptional death before exec can also close it. The receipt uses portable Unix sockets and keeps the child path allocation-free.

## Foundation checkpoint

Implemented C1–C4, N1–N2 and W1: bounded/error-reporting input, checked argv launches, retained job results and fenced OSC 133 completion, owner epochs, versioned snapshot/replay subscriptions, pane-owned native and inline transport, OSC 7501 lifecycle/reporting, profiles and workspace recipes, focus/zoom/swap. A neutral geometry helper removes the manager's renderer dependency. The unregistered runtime draft is excluded from this checkpoint.

Verified on Linux with local PTYs and Unix sockets:

- `cargo test --locked --offline --workspace` passed (`/tmp/ultty-rex-workspace-test.log`); `cargo fmt --all -- --check` passed. The subsequent completion-drain correction passed the focused terminal-manager suite (`/tmp/ultty-rex-manager-final.log`).
- Native protocol suite passed, as did binary/control tests and focused status/EventLoop tests. The ANSI PTY integration exercised working/blocked/done/clear through the actual application writer. Updated CLI tests passed after owner-schema and closed-job corrections.
- Native split GUI smoke passed at `/tmp/rex-native-split.2QT18l`, including two in-band panes and actual GUI-key isolation. Inline GUI smoke passed at `/tmp/rex-inline-split.0LKmOc`, including independent same-ID surfaces in split panes and sibling closure. Each directory records tested binary hashes; these were distinct builds.
- Full-App control gate 01 (`/tmp/ultty-rex-control-20261112-01`) caught premature pane cleanup before final output/completion drainage. Cleanup now waits for the EventLoop's exit-drained boundary; its regression follows the real process/cleanup loop. The fresh full-App rerun passed at `/tmp/ultty-rex-control-20261112-02`, including immediate completion retention, replay/resync, stale epoch refusal and actual queue saturation.
- Workspace clippy caught one collapsible conditional in pane focus; corrected. The exact frozen foundation archive passed workspace clippy (`/tmp/ultty-rex-foundation-clippy.log`) and workspace build (`/tmp/ultty-rex-foundation-build.log`).

The local full-App smoke's failed receipt is retained. Sandbox-only Unix socket `EPERM` failures were rerun with local socket permission; those failures do not establish a source defect. No performance, macOS, real SSH host, screen-reader or remote deployment qualification has been claimed. Inline split rendering currently rebuilds native vertices when inline surfaces are present; optimize only after correctness and measurement.

## Persistent runtime and attachment stage

Foundation commit: `4241115`. Implementation commit `6881bc4` adds N3 and R1-R3/T1-T2 in the same isolated branch. Its results are listed separately from the frozen foundation.

- `SessionRuntime` now drives PTYs, native/in-band transport, pane binding, viewport changes and inline pairing independently of the GUI. The existing in-process application also uses it. The optional headless build excludes wgpu, winit, cosmic-text and AccessKit from its normal dependency closure (`target/rex-features/logs/headless-tree.txt`).
- The host keeps its epoch and processes across viewer disconnects and an empty pane set. Viewers receive explicit validated VT/native snapshots, including prior committed native state while a new frame is pending. The read-only rendering interface is shared with the local GPU path.
- Authority separates read/input/spawn/layout grants from the local administrator credential. Mutations pin owner/grant identity, use bounded deduplication receipts and require the controller lease where applicable. SSH runs a fixed stdio helper; it refuses administrator credentials and never installs or enrolls anything.
- Viewer commands and snapshots are bounded. Closing stops queued input dispatch; sustained input cannot starve publication. Idle snapshots return `Unchanged`. Selection/history position belong to the viewer. Hit testing shares scroll/clip/visibility transforms with live sessions.
- Snapshot review found a compact-row expansion risk across multiple individually valid panes. Validation now limits expanded cells across the whole replacement before any pane hydration. Native declarations are restricted to snapshot data and never dispatched as commands.
- Persistent owners expose the same local control identity to their children before launch. Shared authentication, job/shell completion, search, event and wait helpers serve both window mode and the host. MCP discovery filters tools through the selected owner's supported operations. Local administrator control remains separate from scoped attachment grants.
- Review corrections preserve successful child identities when later layout updates fail, discard pending control work after a full client disconnect while preserving half-close replies, and disconnect viewers on invalid snapshots while retaining their last valid screen.
- Native texture caches include session identity and committed revision. VT textures are uploaded per pane to avoid collisions between pane-local image IDs. A GPU readback gate passed through hydrated native and VT snapshots, including same-size image replacement; the extra VT uploads are a known cost, not a performance improvement.

### Current receipts

All durable relative artifact paths below are under `/home/travers/projects/tui-terminal/`, outside the implementation worktree. They include failed gates as well as passes.

- Workspace build passed: `target/rex-features/logs/current-workspace-build.log`. Later host/cache/CLI/hit-test fixes require their own final rebuild and gates.
- Full-App control and semantic action gate passed: `target/rex-features/gates/control-actions-01/result.json` and request receipts. It exercises Rust and Lua handlers, focus plus keyboard dispatch, stale/wrong target refusal, unknown outcomes after timeout, ANSI keyboard parity, replay and input backpressure. Artifact hashes identify the exact binaries.
- Security audit passed: `target/rex-features/logs/security-audit.log`.
- Final Clippy passed with warnings denied: `target/rex-features/logs/final-clippy-04.log`. Final Rust 1.90 all-target check passed: `target/rex-features/logs/final-msrv-check-02.log`. Final local binaries built successfully: `target/rex-features/logs/final-binaries-build-03.log`.
- Final headless lifecycle gate passed: `target/rex-features/logs/session-host-final-01.log`, host SHA-256 `46b86924a8ce05aa4d5f830c2de3769fb1713c43af13b0819aa07fd488b9d0bc`. It covers retained process/native identities, detached progress, committed frames, idle publication and stale-pointer refusal with continued keyboard delivery. The final dependency closure excludes graphics/window/accessibility crates; receipts are `session-host-final-cargo-tree.txt` and `session-host-final-ldd.txt` in the same log directory.
- The final local fake-SSH gate passed: `target/rex-features/logs/remote-helper-final-01/result.json`. It uses the actual fixed stdio helper and checks lost-reply replay, grants, revocation and transport loss. This receipt alone does not qualify network SSH.
- Final GPU viewer lifecycle passed: `target/rex-features/logs/session-viewer-05/receipt.json`. The real native counter reached 13 after a key burst, retained owner/process/native identities across normal close and SIGKILL, refused observer input, reattached to the same state, then reaped the child on explicit owner shutdown. The final screenshot was visually reviewed. Canonical X11 close requests completed in 82 ms and 27 ms; earlier gates exposed Xvfb setup/focus and `xdotool windowclose` harness errors, preserved separately.
- Final persistent automation passed: `target/rex-features/logs/session-control-02/result.json`. It covers exact argv/cwd, retained nonzero job completion, fenced shell results, replay and owner refusal, Rust/Lua action acknowledgements, filtered MCP discovery and a routed MCP query. Both apps accepted a second semantic action with zero attachment viewers. Gate 01 hit an explicit pre-dispatch stale-frame refusal during Lua startup; the harness now waits/reinspects only that known refusal, never an unknown action outcome.
- GUI gate 05 records a local resource sample, not a comparative benchmark: host RSS about 7.6 MiB with two threads; software-rendered viewers about 114–116 MiB with 31 threads. One-second CPU samples were 0.01–0.02 CPU seconds. See `resource-samples.json` beside its receipt for exact observations and binary hashes.
- The parallel workspace run found one Luau override snapshot failure (`final-workspace-test-02.log`): the existing 8 ms frame budget skipped later hooks. The unchanged failed test binary passed ten isolated runs (`luau-snapshot-direct-00.log` through `09.log`). No golden or execution budget was changed; the final suite runs serially to reduce competing test work.
- Viewer gate 01 rendered the native screen but failed keyboard injection. Evidence identified a missing X11 focus precondition: fresh Xvfb has no window manager, and winit ignores keyboard input without an active window. The corrected gate asserts focus before typing. The failed receipt remains at `target/rex-features/logs/session-viewer-01/`.
- Existing window-mode native split, inline-surface and full control/action regressions passed on copied final binaries: `target/rex-features/logs/final-app-smoke-result.json`. The inline harness kills Xvfb during teardown and logs an X11 panic after successful functional assertions; it is not a graceful-exit receipt. The separate viewer lifecycle gate verifies normal close.
- Full workspace tests passed on the corrected source with `RUST_TEST_THREADS=1`: `target/rex-features/logs/final-workspace-test-06-serial.log`. This includes the real deferred full/half-close regression, atomic invalid-snapshot handling, unchanged-grid PTY pixel metrics, hidden native tab publication and detached inline frame updates. Earlier runs caught an outdated Luau diagnostic expectation and an overlong test socket filename; those fixtures were corrected without changing production behavior.
- Final Clippy and MSRV checks passed after the hidden-tab correction: `final-clippy-05.log` and `final-msrv-check-03.log`. Formatting and whitespace checks passed. Protocol ignored roundtrip, deterministic memory budgets, compatibility scorecard and excluded Porygon checks passed: `final-protocol-roundtrip.log`, `final-memory-budgets.log`, `final-compat-scorecard.log` and `final-porygon-check.log` under the same log directory. The deep VT integrity run and security audit also passed; dependency versions and the lockfile remain unchanged.
- The corrected headless host passed `session-host-final-02.log` and `remote-helper-final-02/result.json`, with SHA-256 `c80619207ef9e1d913f2c345a0fe105650284fcf52f50370616599d55c4005cd`. The focused real-socket/PTY regressions passed separately in `detached-native-regressions.log`. `session-host-final-02-cargo-tree.txt` and `session-host-final-02-ldd.txt` record its dependency closure.
- The first real devbox SSH gate passed: `target/rex-features/logs/devbox-ssh-final-01/result.json`. It built the headless host natively on ARM64 with Rust 1.97.1, exercised the production SSH client/helper and a GPU observer, retained native/process identity across transport loss, verified exact mutation replay, permissions, expiry and revocation, then stopped its test host. Receipt review found hidden native tabs missing from snapshots; snapshot capture, cache invalidation and detached inline pairing were corrected together.
- The corrected implementation passed the expanded real devbox gate: `target/rex-features/logs/devbox-ssh-final-02/result.json`, including `hidden_native_retained: true`. Remote host SHA-256 is `c92b5cef303f86efcd25a519e4c3b133726297b07082231f8ef684cfb17d0683`; the source archive SHA-256 is `a9af2504eceff70b94b2c86365548f9fe0c4bb04571d67be147b2b8ae289cf9b`. `devbox-source-final-02-comparison.json` confirms that its Rust/Cargo sources match the committed worktree. The frozen GUI viewer remains the separately hashed artifact identified in that receipt.
- `target/rex-features/logs/final-local-validation.json` records the local gate log hashes and implementation commit. `devbox-ssh-final-02-cleanup.log` confirms removal of the final devbox test's host, child processes, helper link and staging directory; no local scoped credential files remain in that gate directory. Devbox itself remains reachable and running as requested.

### Deliberate boundaries

The optional host uses bounded whole-state semantic replacements rather than native/VT byte replay or a delta log. A host restart loses its running applications; viewers never silently switch pinned epochs. Inline native surfaces keep the existing noninteractive contract. Full-scene semantic focus/activation is opt-in and uses acknowledged application handlers; unsupported actions remain unavailable.

Public SSH and actual Tailscale SSH/helper/GPU paths were exercised on devbox using the separately identified frozen artifacts. macOS, deployment, a screen-reader session and comparative performance improvements remain unqualified. Read-only diagnostics are distinct from authorization. Keep the existing in-process route as the rollback path; jobs cannot migrate automatically between owners.

The local gates do not constitute a full CI run. The pinned nightly fuzz toolchain and cargo-fuzz were unavailable here; the CI instruction-count comparison was not run. Source and functional validation do not establish a performance improvement over Rex, Tailscale or the previous Ultty build.

## Upstream integration and PR checkpoint

Source commit `b61448a` integrates `origin/main` at `ba43d2e`. Conflict resolution
preserves explicit PID targeting and strict environment discovery, current font
configuration, PTY pixel dimensions and sliced search. Additional integration
fixes cancel copy-mode searches and completed matches when their owning pane
loses focus, expose the pure search configuration module to headless builds,
and prevent explicit PID selection from inheriting a different owner's pane.

The [integration evidence ledger](../releases/evidence/2026-11-12/rex-sessions/INTEGRATION.md)
records exact commands, frozen binary hashes, completed gates and remaining
limits. Earlier evidence in the same package is explicitly historical.
Workspace Clippy, headless checks, Rust 1.90 all-target checks, excluded Porygon,
the full serial workspace suite, live two-host PID/MCP isolation, local GPU
viewer lifecycle, persistent and window-mode control, native split, inline
surfaces and memory budgets passed on the integrated source.
The first workspace attempt retained a failure in the unchanged Luau cache
test during concurrent compilation (19 of 20 hooks applied); the identical
suite passed after builds finished, without changing its 8 ms frame budget or
assertions. Both logs are retained.

Devbox's merged ARM64 host and counter build passed using the repository-pinned
Rust 1.98.1. Its source archive has the same 332 Rust/Cargo paths as `b61448a`;
the sole difference is a GUI test-only re-export added after staging. The
actual tailnet SSH/helper/GPU gate passed in 16.71 seconds using those
separately identified frozen artifacts. The temporary userspace node was
reached at `100.119.165.66`; both Rust client and GUI used that route with
strict verification of the existing devbox SSH identity. Tailscale initially
used DERP and then established a direct path. The gate verified hidden native
retention and exact mutation replay; its receipt records the exact binaries.
The remote setup initially invoked rustup proxies accidentally; repairing the
resulting incomplete stable toolchain updated devbox's `stable` from 1.97.1
to 1.100.0. The product build uses pinned 1.98.1 directly. Devbox remains up.

### Latest image-cache integration

Commit `76b8db8` integrates upstream `f835cc2`, including the byte-bounded VT
image cache and Wayland benchmark corrections. VT textures now use terminal
UID plus image ID; native textures retain their session/region/revision keys.
Snapshot hydration creates a fresh terminal UID. The GPU regression now
checks equal-size changed VT pixels, independent pane images, and redraw of a
previous snapshot while both cache entries remain present. The earlier
per-pane unconditional upload description applies to the prior checkpoints.
See the evidence ledger for validation of this final source boundary.

Final source `76b8db8` passed workspace Clippy, the full serial workspace suite,
headless and Rust 1.90 checks, the expanded software-Vulkan pixel readback test,
and a fresh GPU viewer lifecycle gate. The latest viewer used SHA-256
`bdd84923dfb1bcd28ba71dca0731cc86f4cafe335d16c865cc69bbb4d9a9dba2`;
its host/counter inputs are separately identified in the receipt. Tailnet
cleanup confirmed the temporary node logged out, daemon stopped, all test
processes gone and owned staging/helper/credential files removed. Devbox
remains up. The tailnet gate used the earlier frozen artifacts rather than
being relabeled as a test of the final cache merge.
