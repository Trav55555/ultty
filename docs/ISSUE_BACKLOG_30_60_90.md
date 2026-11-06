# Ultty Readiness Issue Backlog

**Updated:** 2026-11-05
**Source plan:** [Ultty Readiness Roadmap](ROADMAP_30_60_90.md)
**Readiness gates:** [Public Alpha Readiness](alpha-readiness.md)

The GitHub issue tracker is the canonical queue. The Active Issues below are
not filed there yet; file them before starting. Candidate Issues further down
are slices to file when work resumes.

## Active Issues

### 1. Upgrade the graphics, layout, and TUI dependencies

**Problem.** `wgpu 0.19`, `winit 0.29`, `cosmic-text 0.11`, `taffy 0.5`,
`ratatui 0.29`, and `crossterm 0.28` are all two or more years behind current
releases. Nothing in the roadmap covers moving off them. They hold open all
three advisory ignores in [Security Audit](security-audit.md):
`RUSTSEC-2024-0436` (`paste`, via `ratatui` and `wgpu`/`metal`),
`RUSTSEC-2026-0002` (`lru`, via `ratatui`), and `RUSTSEC-2026-0097` (`rand`,
via `tokio-tungstenite`).

**Scope.** Split into one slice per dependency, largest blast radius first.

| Dependency | Files that touch its API |
|------------|--------------------------|
| `wgpu` 0.19 -> current | `crates/ultty/src/app/mod.rs`, `app/render.rs`, `renderer/mod.rs`, `renderer/pipeline.rs`, `renderer/atlas.rs`, `renderer/text.rs`, `renderer/image.rs`, `renderer/overlays.rs`, `renderer/panes.rs`, `renderer/screenshot.rs`, `renderer/native_render/mod.rs`, `renderer/native_render/cache.rs`, `renderer/native_render/images.rs` |
| `winit` 0.29 -> 0.30+ | `crates/ultty/src/app/mod.rs`, `app/input.rs`, `app/control.rs`, `app/wakeup.rs` |
| `cosmic-text` 0.11 -> current | `crates/ultty/src/renderer/text.rs`, `renderer/atlas.rs` |
| `taffy` 0.5 -> current | `crates/ultty/src/scene/layout.rs`, `scene/layout/constraints.rs` |
| `ratatui` 0.29 / `crossterm` 0.28 -> current | `crates/ulttui/src/backend/ansi.rs` plus 30 other files using `ratatui::` types in widget render paths |
| `tokio-tungstenite` 0.24 -> current | `crates/ulttui` dev-dependency only; carries the vulnerable `rand` |

The `winit` 0.30 move is the sharp edge: it replaces the closure-based
`EventLoop::run` with the `ApplicationHandler` trait, so the whole event loop,
wakeup plumbing, and control-socket integration change shape. `wgpu` is the
largest by file count but mostly mechanical API renames.

**Acceptance.**
- Each dependency lands as its own commit with `cargo clippy --workspace
  --all-targets -- -D warnings` and `cargo test --workspace` green.
- The live smoke gate passes afterwards, with evidence recorded under
  `docs/releases/evidence/<date>/` or as a `live-smoke.yml` run URL.
- `scripts/security-audit.sh` runs with zero `--ignore` flags, and the
  Accepted Warning Advisories table in [Security Audit](security-audit.md) is
  empty.

### 2. Sign and notarize macOS release artifacts

**Problem.** The release workflow ships unsigned, un-notarized macOS binaries.
Gatekeeper quarantines them, so every macOS user has to run
`xattr -d com.apple.quarantine` or use the right-click Open workaround. The
README and [Release Artifacts](release-artifacts.md) say so, which is honest
but not a fix. No Apple Developer credentials exist yet.

**Scope.** Obtain an Apple Developer ID Application certificate, store the
certificate, its password, and an App Store Connect API key as repository
secrets, then add codesign and notarytool steps to the `x86_64-apple-darwin`
and `aarch64-apple-darwin` matrix legs of `.github/workflows/release.yml`.

**Acceptance.**
- `codesign --verify --deep --strict` and `spctl --assess --type execute` both
  pass on `ultty` and `ulttyctl` extracted from a published tarball.
- A macOS user can open a downloaded binary with no quarantine workaround.
- The Gatekeeper notes in README.md and `docs/release-artifacts.md` are
  replaced with the signing status.

### 4. Add a `cargo-deny` step to CI

**Problem.** `cargo audit` covers advisories only. Nothing checks licenses,
banned or duplicated crates, or unexpected registry sources. Now that the
workspace has two license files and an excluded standalone package, license
drift is easy to miss.

**Scope.** Add `deny.toml` covering `advisories`, `licenses`, `bans`, and
`sources`, then add a `cargo-deny` job to `.github/workflows/ci.yml` pinned the
same way the other jobs are. Reconcile its advisory ignores with
`scripts/security-audit.sh` so there is one list, not two.

**Not started on purpose.** Tracked here; do not add it until item 1 is far
enough along that the advisory ignore list is stable.

**Acceptance.**
- `cargo deny check` passes locally and in CI.
- The allowed license set is explicit and matches `MIT OR Apache-2.0` plus the
  licenses of the current dependency tree.

### Expand tabs in ulttui ANSI text

**Problem.** ratatui 0.30 drops control characters from text, so a tab in
a `Text` or `RichText` widget now draws as nothing in ANSI mode
(`Tabs:\t\tSpaces` renders as `Tabs:Spaces`). Before 0.30 the raw tab
reached the terminal and moved its cursor, which shifted every cell after
it. Native mode does not expand tabs either.

**Scope.** Expand tabs to the next tab stop (8 columns) in ulttui's text
widgets, before both renderers see the text, and update
`snapshot_special_chars`.

### Find the remaining flaky self-check

**Problem.** A `crates/ultty/tests/self_check.rs` test failed once in a
full parallel `cargo test --workspace` run on 2026-11-06 (7 passed, 1
failed). It did not recur in 36 loaded reruns of the suite or 3 more full
workspace runs, so its name is unknown. `self_check_clear_screen` had the
same shape of bug (a stability wait over an interactive prompt) and was
fixed in #120. The other checks still wait for the screen to go quiet and
match markers that the terminal echoes as they are typed.

**Scope.** Move the remaining self-checks to `TestTerminal::wait_for_text`
with markers that only the commands' output contains, as in
`self_check_clear_screen`.

## Recently Completed Readiness Work

- **GUI smoke in CI (2026-11-06).** The `GUI smoke` job in `ci.yml` runs
  Ultty under Xvfb with Mesa's software Vulkan (lavapipe) on a stock
  `ubuntu-latest` runner. No self-hosted GPU machine is needed. It runs:
  - `scripts/live-smoke-gate.sh`;
  - `scripts/terminal-workflow-smoke.sh`;
  - `scripts/regression-smoke.sh`, which covers:
    - `ulttyctl info` answering;
    - fresh tabs keeping their top lines;
    - `text --scrollback`;
    - counter-app running natively over the socket and in-band.

  Against builds from before the `info` and shrink fixes, it fails on the
  exact bug each time. Run it locally with `scripts/ci-gui-smoke.sh` (needs
  `xvfb` and `mesa-vulkan-drivers`).

- **Run-level text shaping (ligatures) (2026-11-06).** Runs of printable
  ASCII cells that contain punctuation and share a face and foreground
  color are shaped together and cached by text. Each glyph is placed at the
  cell its cluster starts at, and runs end at the block cursor. Checked with
  Miracode: `=>`, `===`, and `<=>` join, and the cursor splits a ligature
  legibly. `render_code/300x100_code` shows no significant change from
  per-cell shaping (440 against 474 µs, p = 0.09).

- **Compact scrollback storage (2026-11-06).** History rows are text plus
  style runs. Memory for 10,000 typical 300-column lines fell from 57 MB to
  2.4 MB, and VT throughput rose 10% at 120x40 and 24% at 300x75. The
  remaining cost is per-cell style comparison; see
  [docs/perf/2026-11-06-scrollback-cost.md](perf/2026-11-06-scrollback-cost.md).

| Issue | Result | Evidence |
|-------|--------|----------|
| #106 Create logo/icon assets for installable app metadata | Done | [Brand Assets](brand.md), `dist/icons/hicolor` |
| #105 Refresh roadmap and backlog around readiness language | Done | [Ultty Readiness Roadmap](ROADMAP_30_60_90.md) |
| #104 Promote a live renderer smoke into automation | Done | [Live Smoke Automation](live-smoke-automation.md) |
| #103 Refresh current performance and live-readiness evidence | Done | [2026-05-29 readiness artifact](perf/2026-05-29-current-readiness.md) |
| #102 Split remaining large app runtime modules | Done for the tracked slice | Control wait handling split; remaining opportunities are tracked in [Architecture Deepening Backlog](architecture-deepening-backlog.md) |
| #101 Create public alpha readiness checklist | Done | [Public Alpha Readiness](alpha-readiness.md) |
| #100 Add sequential live smoke gate | Done | `scripts/live-smoke-gate.sh`, [Visual Regression](visual-regression.md) |
| #99 Add release artifact smoke tests | Done | [Release Artifact Smokes](release-artifacts.md) |
| #97 Add license files and release metadata hygiene | Done | [Release Metadata](release-metadata.md), top-level license files |
| #96 Add dependency security audit gate | Done | [Security Audit](security-audit.md) |
| #95 Add local version flags | Done | `ultty --version`, `ulttyctl --version` |

## Candidate Issues

### Public Alpha: Fill Release Candidate Checklist

## Problem
Public-alpha readiness needs evidence tied to a specific candidate commit.
Standing docs say what to run, but they do not prove a concrete candidate.

## Scope
- Create `docs/releases/<version-or-date>.md` from
  [RC_TEMPLATE.md](releases/RC_TEMPLATE.md).
- Link hosted CI, release artifact smoke, security audit, live smoke workflow
  run, install verification, current performance artifact, and known
  limitations.
- Keep the checklist in the release PR.

## Validation
- Checklist has no empty required evidence fields before alpha publication.

### General Use: Terminal Workflow Smoke Matrix

## Problem
Renderer and protocol smokes do not fully represent common terminal workflows
such as shell editing, job control, pagers, editors, resize-heavy TUIs, tabs,
splits, copy mode, and paste behavior.

## Scope
- Extend `scripts/terminal-workflow-smoke.sh` with one workflow per commit.
- Preserve artifacts for each workflow.
- Start with shell editing, job control, and pager/editor alternate-screen
  behavior.

## Validation
- `scripts/terminal-workflow-smoke.sh /tmp/ultty-terminal-workflow-smoke`
- Relevant `cargo test` targets for touched parser/grid/control code

### General Use: Runtime Module Deepening

## Problem
Large runtime modules still reduce Locality for terminal correctness, renderer,
input, and framework runtime changes.

## Scope
- Pick one item from
  [Architecture Deepening Backlog](architecture-deepening-backlog.md).
- Extract one cohesive responsibility per commit.
- Preserve existing public Interfaces.

## Validation
- `cargo fmt --all -- --check`
- `cargo check --workspace`
- Relevant crate tests for touched modules

## Backlog Policy

Use readiness vocabulary for new issues:

- `dogfood`: maintainer-facing reliability, diagnostics, and local workflow.
- `public alpha`: external preview requirements such as release metadata,
  package checks, live smoke evidence, security audit, and clear limitations.
- `general use`: broad terminal-emulator trust, repeated release evidence,
  runtime maintainability, install/upgrade docs, and no known P0/P1 correctness
  blockers.

New work should be small enough for one focused commit unless it is explicitly
a tracking issue. Prefer vertical slices with concrete validation commands and
artifact expectations.
