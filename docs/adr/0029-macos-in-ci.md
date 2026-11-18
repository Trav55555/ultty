# ADR 0029: macOS in CI

## Status

**Proposed** 2026-11-17. Not scheduled. Code and workflow references are
to `main` at 252c082. Prices were read from public pricing pages on
2026-11-17 and change; each is marked with how sure I am. The open
questions were settled on 2026-11-17; see "Decisions (2026-11-17)".

## Context

### The gap

`docs/feature-parity.md` gap 10: macOS code exists but is not built or
tested by CI. ADR 0003 names macOS a supported runtime target, and the
README says "Building from source on macOS is expected to work, but there
are no macOS release builds and CI does not test macOS."

### What runs today

- **Gitea CI** (`.gitea/workflows/ci.yml`) runs every job on the `ultty`
  pool, "x86_64 Fargate Spot tasks", on pushes to `main` and on pull
  requests. Its header says: "There is no macOS runner: macOS builds only
  run in the GitHub release workflow."
- **GitHub CI** (`.github/workflows/ci.yml`) mirrors the Gitea jobs on
  `ubuntu-latest`, on pushes to `main` and pull requests on GitHub. The
  GitHub repository receives `main` and tags; pull requests live on Gitea.
  It ran 96 times in the 30 days to 2026-11-17 (`gh run list`). Its
  longest job, Test, took 13 minutes in the latest run.
- **GitHub release** (`.github/workflows/release.yml`) builds
  `x86_64-apple-darwin` and `aarch64-apple-darwin` on `macos-latest`, but
  only on `workflow_dispatch`, and both are `optional: true`. Releases are
  published by `.gitea/workflows/release.yml`, which builds Linux x86_64
  only. So no macOS build runs in the normal flow.
- The GitHub repository `Trav55555/ultty` is public (`gh repo view`).

### What is broken on macOS right now

A cross-compile check from Linux (below) found one compile error that CI
would have caught:

- `crates/ulttui/src/backend/ansi.rs`, in `#[cfg(test)] mod tests`, calls
  `libc::openpty(&mut master, &mut slave, null_mut(), null(), &size)`.
  On Apple targets `libc` declares the last two parameters `*mut`, so
  `cargo test` does not compile on macOS (E0308, "types differ in
  mutability").

Not macOS-specific, found on the way: `cargo check -p ultty
--no-default-features` fails on Linux too, because `scene/selection.rs`
calls `crate::url`, which `lib.rs` compiles only with the `gui` feature.
`docs/SESSION_ATTACHMENT.md` documents that build for `ultty-session`, and
CI does not run it.

### What a cross-compile check catches

Run on the dev machine with the pinned 1.98.1 toolchain and the
`aarch64-apple-darwin` standard library, no macOS SDK:

| Command | Result |
|---|---|
| `cargo check --workspace --all-targets --target aarch64-apple-darwin` | Fails in build scripts: `mlua-sys` (Luau, C++, via `mlua` in `ulttui`) and `mac-notification-sys` (Objective-C, via `notify-rust` in `ultty`) call the host `cc` with `-arch arm64`. |
| Same, with `clang` as the target C and C++ compiler | Fails: no C++ standard headers (`climits`) and no libc headers for Darwin. |
| `cargo zigbuild check --workspace --all-targets` (zig supplies Darwin libc and libc++ headers) | Luau builds. Fails at `mac-notification-sys`: `Cocoa/Cocoa.h` not found. That needs the SDK. |
| `cargo zigbuild check --workspace --exclude ultty --all-targets` | Checks everything else and finds the `openpty` error above. |
| `cargo check -p ultty-vt -p ultty-protocol -p ultty-client --all-targets` (plain) | Passes. |

So without the SDK a cross check type-checks `ultty-vt`, `ultty-protocol`,
`ultty-client`, `ulttui` and the examples for macOS, but not the `ultty`
crate, which holds `platform/macos.rs`, the winit and wgpu code and every
other `cfg(target_os = "macos")` path that matters most. It never links,
never runs a test, and never opens a window. Getting the SDK onto Linux
(osxcross) means downloading Xcode under Apple's licence terms, which I
would not put into a CI image without checking them.

### Pricing (public pages, 2026-11-17)

- **GitHub-hosted runners.** GitHub's runner reference says "Use of the
  standard GitHub-hosted runners is free and unlimited on public
  repositories." Standard macOS labels include `macos-latest`,
  `macos-15` and `macos-26` (arm64, 3-core M1, 7 GB) and `macos-15-intel`
  (4-core, 14 GB). For private repositories the billing page lists
  $0.062 per minute for 3- or 4-core macOS runners. Larger runners are
  always billed. GitHub cut hosted prices on 2026-01-01 and announced, then
  postponed, a per-minute charge for self-hosted runners in private
  repositories (press reports; current status unconfirmed). There is a
  limit on concurrent macOS jobs on free plans; I did not confirm the
  current number.
- **AWS EC2 Mac.** Dedicated hosts, billed per second with a 24-hour
  minimum allocation (AWS's Mac instance page). Third-party price
  trackers list `mac-m4.metal` at $1.23 per hour on demand in us-east-1,
  so $29.52 is the least one allocation can cost and about $900 a month
  kept running. `mac2.metal` (M1) was listed around $0.65 per hour (about
  $475 a month); unconfirmed. New accounts may start with no Mac quota.
- **Mac mini.** The M4 Mac mini launched at $599 (16 GB, 256 GB) in
  October 2024; I did not check Apple's current price. Over three years
  that is about $17 a month, plus power and a network it can reach Gitea
  from.
- **Gitea runner on macOS.** The Gitea runner ships darwin arm64 and
  amd64 builds. On macOS jobs run with the `host` label, directly on the
  machine without isolation.

### Getting Gitea events to GitHub, and results back

- Pull requests exist only on Gitea, so a GitHub workflow on `push` only
  sees `main` and tags.
- A Gitea job can start a GitHub workflow and wait for it:
  `gh workflow run macos.yml --ref <branch>` then `gh run watch
  --exit-status`. The ref must exist on GitHub, so the job pushes the PR
  head to a `ci/pr-<n>` branch first. The Gitea job's own pass or fail is
  the PR's check; no status API is needed. It needs a GitHub token with
  contents write and actions write on `Trav55555/ultty`, stored as a
  Gitea secret, like the existing `GH_RELEASE_TOKEN` in
  `.gitea/workflows/release.yml`. The Gitea job holds a Fargate Spot task
  while it waits, about 15 to 25 minutes.
- The alternative is a GitHub workflow that posts a commit status to
  Gitea's API (`POST /api/v1/repos/{owner}/{repo}/statuses/{sha}`, path
  from memory, not checked against this Gitea's version). That needs a
  Gitea token in GitHub's secrets and Gitea reachable from GitHub. More
  moving parts for the same result.

## Options

| Option | Monthly cost | Catches | Costs besides money |
|---|---|---|---|
| A. GitHub-hosted `macos-latest`, started from Gitea | $0 while the repository is public; at private rates about $90 for 96 runs of 15 min, more with PR runs | Build, clippy, tests, link errors; probably a GUI smoke (see below) | PR branches pushed to the public mirror; GitHub availability; queue time |
| B. Self-hosted Mac mini in the Gitea pool | about $17 amortised, plus power; one-time $599 | Everything A does, plus a real GPU for GUI smoke | Hardware to look after, OS updates, no isolation between jobs (`host` label), one job at a time |
| C. AWS EC2 Mac in the Gitea pool | about $475 to $900 kept running; at least $29.52 per allocation | Same as B | 24-hour minimum makes scale-to-zero pointless; quota requests |
| D. Cross-compile check on Linux | $0 (runs on the existing pool; zig in the image) | Type errors in every crate except `ultty` | Misses the code that matters most; no tests |
| E. Nothing | $0 | Nothing | macOS regressions found by users, if any |

## Decision (proposed)

**Option A.** A GitHub Actions workflow on `macos-latest` (arm64), free
for this public repository, started by a Gitea job on every push to `main`
and every pull request from the owner's own branches, whose result is the
Gitea check.

What it gates:

1. **Build and clippy**, required from the start:
   `cargo clippy --workspace --all-targets -- -D warnings` and
   `cargo build --locked -p ultty`.
2. **Tests**, required once they pass: `cargo test --workspace`. They do
   not compile today (the `openpty` error). Fix that first, then expect
   some Linux assumptions in tests to surface; mark each one
   `#[cfg(target_os = "linux")]` only where the behaviour really is
   Linux-only, with a reason.
3. **GUI smoke**, advisory at first: start `ultty`, wait for its control
   socket, `ulttyctl send-text`, `wait-for`, `screenshot`. GitHub's macOS
   runners run a logged-in desktop session, so a window should open; I
   have not confirmed that Metal works there well enough for wgpu. Make
   it required only after it has passed reliably for two weeks.

Not gated: the snapshot, fuzz, VT integrity, icount and memory-budget
jobs, which test platform-independent code that Linux already covers;
`porygon-tui`; release packaging. The release workflow's macOS targets stay
optional until the macOS tests are required and green, then become
required.

Add option D as well, as long as zig is cheap to add to the runner image: it
gives a macOS type check in a couple of minutes on the existing pool, and
it would have caught the `openpty` error. It does not replace A.

Revisit with option B if the repository goes private (A then costs real
money per run), if GitHub's macOS queue makes PRs wait, or if the GUI
smoke needs a real GPU.

### Decisions (2026-11-17)

1. **PR branches on GitHub.** Pushing PR heads to the public mirror is
   accepted, so macOS runs on pull requests as well as `main`. Only PRs
   from the owner's own branches of `trav/ultty` are pushed (as
   `ci/pr-<n>`, deleted after the run); PRs from forks or other authors
   skip the macOS job and are covered on `main` after merge. Nothing is
   pushed until the owner adds the GitHub token as a Gitea secret and
   enables the `macos` job.
2. **Intel Macs.** arm64 only (`macos-latest`) in CI; the release workflow
   keeps building `x86_64-apple-darwin`. Decided by default; revisit if
   needed.
3. **macOS releases.** Yes: once the macOS build, clippy and test checks
   are required and green, the release workflow's macOS targets drop
   `optional: true`. Signing and notarisation are separate work. Decided
   by default; revisit if needed.
4. **Zig in the runner image.** Yes, as slice 4, if adding zig and
   `cargo-zigbuild` stays a small image change; drop it otherwise. It
   complements option A and does not replace it.

## Slices and rough effort

| Slice | Contents | Effort | Check |
|---|---|---|---|
| 1 | `.github/workflows/macos.yml` (`workflow_dispatch` with a ref, and `push` to `main`): clippy, build, test; fix the `openpty` error and whatever the first run finds | 1–3 days, depending on what the tests find | A green run on `main` |
| 2 | Gitea job `macos` in `.gitea/workflows/ci.yml`: for PRs from the owner's own branches, push the head to `ci/pr-<n>` (pushes to `main` use `main`; other PRs skip the job), `gh workflow run`, `gh run watch --exit-status`; delete the branch after; token as a Gitea secret | 1 day | A PR with a deliberate macOS-only error fails the Gitea check |
| 3 | GUI smoke on macOS, advisory | 2–3 days, uncertain | Screenshot artifact uploaded from a passing run |
| 4 | zig cross check on the Linux pool, if the image change stays small | 0.5 day plus image change | Catches a planted macOS-only type error |

## Consequences

- macOS becomes a platform CI keeps working, which ADR 0028's macOS
  slice and any macOS release need.
- PRs wait on GitHub's macOS queue as well as the Gitea pool.
- The owner's PR branches appear on the public GitHub mirror under `ci/`,
  and are deleted after the run.
- The setup depends on the repository staying public; going private moves
  the cost from $0 to tens of dollars a month or to option B.
- A GitHub token with write access lives in Gitea's secrets.
