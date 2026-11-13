# PR 135 repair evidence

This ledger records the repair at source commit
`447032bc1219812519239b9d889174ca3c5faa56`. Local and matching-build remote
attachment gates passed; the subsequent CI run is pending.

| Evidence | Result and scope |
| --- | --- |
| [CI job 6355 excerpt](ci-1719-job-6355-excerpt.txt) | Run 1719 failed the 3% instruction gate on four parse cases. |
| [CI job 6356 excerpt](ci-1719-job-6356-excerpt.txt) | The GUI smoke image assertion failed after Python could not import `PIL`. Commit `e38a17f` changed the assertion to use the bundled PNG decoder. |
| [Original failure reproduced](before-original-ci-table.txt) | Frozen `b2b794c` against original CI base `f835cc2` reproduced the four failures with the same compiler and Valgrind versions. |
| [Variant reorder experiment](after-original-ci-table.txt) | Moving the status actions after the existing actions passed the full comparison against `f835cc2`, worst +0.31%. This is an isolated experiment on `b2b794c`, not the final merged build. |
| [Final instruction gate](after-final-table.txt) | Exact `447032b` against `97f5688`, all parse and VT corpora passed; worst +0.48%, threshold +3%. |
| [Final machine-readable comparison](final-summary.json) | Baseline and final binary hashes, source archive hash, tool versions, input parameters, and every measured case. |
| Ultty VT tests | `cargo test --locked -p ultty-vt` passed on the isolated `b2b794c` reorder experiment; the [full VT suite](pr135-merge-vt-results.txt) also passed at merged commit `a69d6ab` before the reorder. |
| Final clippy and formatting | [Clippy](pr135-final-clippy-01.log), `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`, passed with `RUSTFLAGS=-Dwarnings`; `cargo fmt --all -- --check` and `git diff --check` passed. |
| Final headless and MSRV checks | [Headless](pr135-final-headless-check-01.log) `cargo check -p ultty --no-default-features --lib --bin ultty-session --bin ulttyctl --locked --offline` passed; [MSRV](pr135-final-msrv-01.log) `rustup run 1.90 cargo check --workspace --all-targets --locked --offline` passed. Both used `RUSTFLAGS=-Dwarnings`. |
| Final headless build | The [headless build](pr135-final-headless-build-01.log) passed; [binary manifest](headless-binary-manifest.json) records source and binary hashes. |
| Final workspace tests | `cargo test --workspace --locked --offline -- --test-threads=1` passed at `447032b` with `RUSTFLAGS=-Dwarnings`; [suite results](pr135-final-workspace-results.txt). |
| Final GPU readback | The explicit ignored renderer regression passed using software Vulkan with snapshot schema 2, including alternate-screen and partial-frame cases. [Receipt](pr135-final-gpu-readback-01.json), [log](pr135-final-gpu-readback-01.log). |
| Final inline GUI smoke | Passed with Python site packages disabled, so Pillow was unavailable. [Receipt](pr135-final-inline-01.json), [driver log](pr135-final-inline-01-driver.log), [split screenshot](inline/split-live.png). |
| Final persistent viewer | Passed close, kill, reattachment, input and observer checks. [Receipt](viewer/receipt.json), [driver log](pr135-final-viewer-01-driver.log), [first](viewer/first.png) and [reattached](viewer/reattached.png) screenshots. |
| Final session control | Passed, including detached Rust and Lua semantic actions. [Result](session-control-result.json), [inputs](session-control-inputs.json), [detached actions](detached-actions.json), and [driver log](session-control-driver.log). |
| Final PID and MCP ownership | Passed owner selection, precedence and pinned MCP checks. [Result](control-pid-result.json), [inputs](control-pid-inputs.json), and [driver log](control-pid-driver.log). |
| Final native split | Passed with Python site packages disabled. [Result](native-split-result.json), [driver log](native-split-driver.log), and [screenshot](native-split.png). |
| Final memory budgets | `cargo test -p ultty --bench memory --locked --offline` passed, including the full simulated ten-hour session. [Log](memory-budget.log). |
| Final real tailnet attachment | Exact `447032b` ARM64 host and local GUI passed SSH helper, replay, hidden-native and viewer checks using snapshot schema 2. The route started on DERP iad and became direct; strict SSH host-key checks passed and temporary test resources were removed. [Result](tailnet/result.json), [route and cleanup](tailnet/route-cleanup.json), [source provenance](tailnet/source-provenance.json), and [screenshot](tailnet/remote-counter.png). |

The comparison uses `rustc 1.98.1`, `valgrind-3.27.1`, a 1 MiB corpus,
120×40 geometry, and the repository's unchanged `scripts/bench/icount-gate.sh`.
The checked-in JSON files preserve the raw baseline and measured counts. The
final source archive SHA-256 is
`ba86cc6e032fcf1480dbeb9feb72b8c24cc30d5cc2171809d4f764d0b7e08bc7`;
the final release benchmark binary is
`e7ff12d173cb91ec478d74997263924074fc0bb30f71d2eb711f109a5b72c96a`.

At the time this ledger was updated, a fresh CI result was still pending. The
[final binary manifest](final-binary-manifest.json)
records the frozen local build used by the GUI gates.

`SHA256SUMS` covers the files in this directory except itself.
