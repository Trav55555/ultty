# Live Smoke Automation

Ultty live renderer smokes run on a dedicated self-hosted Linux runner, not on
hosted CI. The runner must provide a GUI compositor, a working `wgpu` backend,
a PTY-capable shell, and a stable session where `ulttyctl` can reach the live
control socket.

The workflow is [Live Smoke](../.github/workflows/live-smoke.yml). It runs by
manual dispatch so maintainers can execute it for release candidates or
renderer-sensitive changes without blocking every hosted CI run on GUI runner
availability.

## Runner Contract

Register the runner with these labels:

```text
self-hosted
linux
ultty-gui
```

The runner should have:

- Rust stable toolchain support through `dtolnay/rust-toolchain`.
- Linux packages required by the normal build, including Wayland and
  `libxkbcommon` development files.
- A compositor or desktop session available to the runner service.
- A GPU or software `wgpu` backend capable of opening an Ultty window.
- Permission to create PTYs, Unix sockets, screenshots, and temporary files
  under `${RUNNER_TEMP}`.

## Workflow Behavior

The workflow:

1. Builds `ultty`, `ulttyctl`, and the `hello-native` smoke binaries.
2. Starts `cargo run -p ultty --bin ultty` and preserves its log.
3. Waits for `target/debug/ulttyctl ping` before running the gate.
4. Runs `scripts/live-smoke-gate.sh`.
5. Uploads the full `${RUNNER_TEMP}/ultty-live-smoke` directory on success or
   failure.

The gate currently runs chrome screenshot, terminal surface, native rich text,
and AppendContent smokes against one live Ultty instance. It captures
`ulttyctl ping`, `tabs`, `info`, and screenshots before and after each smoke.

## Artifacts

Uploaded artifacts include:

- `ultty.log`
- `startup-ping.txt` and `startup-ping.err`
- `gate/summary.txt`
- per-smoke logs
- per-smoke screenshots and analyzer output
- health-check screenshots and control-plane output

Use these artifacts for renderer and native-runtime failure diagnosis. Do not
use them as performance evidence.

## Performance Boundary

This workflow intentionally has no performance thresholds. Runner compositor,
GPU, scheduler, and window-focus variance have not been characterized. Keep
performance and live-readiness measurements in the performance artifact process
until runner variance is documented.
