# Public Alpha Readiness

This document defines the release-readiness language for Ultty. It is the
source of truth for deciding whether the project is suitable for maintainer
dogfood, a public alpha, or general use.

Ultty is Unix-first today. The platform contract and future exclusions are
captured in [ADR 0003](adr/0003-unix-first-runtime.md) and
[ADR 0014](adr/0014-next-phase-exclusions.md). Runtime trust and control-plane
semantics are captured in [ADR 0015](adr/0015-trustworthy-terminal-inspectable-runtime.md)
and [ADR 0017](adr/0017-runtime-socket-and-control-plane-semantics.md).

## Readiness Levels

| Level | Intended users | Release claim |
|-------|----------------|---------------|
| Dogfood | Maintainers and close contributors running from source. | Usable for daily project work when a local GUI session is available and known limitations are acceptable. |
| Public alpha | Technical external users willing to install prerelease builds and report issues. | Safe to publish as a preview with explicit limitations, reproducible smoke evidence, package metadata, and current performance notes. |
| General use | Users expecting a dependable terminal emulator for normal work. | Suitable to recommend broadly after repeated release evidence, automated live gates, polished docs, and no known P0/P1 correctness blockers. |

## Gate Types

| Type | Meaning |
|------|---------|
| Automated CI | Runs without local human judgment in CI or release workflows. |
| Local-only | Requires a developer workstation, GUI session, hardware, or long-running local setup. |
| HITL/manual | Requires human inspection, install verification, release judgment, or interactive dogfood. |

## Dogfood Gates

Dogfood is the lowest acceptable level for maintainers to use Ultty during
active development. It does not imply a stable external release.

| Area | Gate | Type | Evidence |
|------|------|------|----------|
| Build and tests | `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings` pass on the target branch. | Automated CI | `.github/workflows/ci.yml` |
| Terminal correctness | Parser, grid, protocol, widget, snapshot, and property tests pass. Regressions in VT behavior, tabs, splits, resize, copy mode, and native region diffing block dogfood. | Automated CI | [Testing Strategy](TESTING_STRATEGY.md), [Protocol Specification](PROTOCOL_SPEC.md) |
| Native runtime observability | A running instance responds to `ulttyctl ping`, `ulttyctl info`, `ulttyctl tabs`, and `ulttyctl screenshot`. The diagnostics overlay remains available through the documented shortcut. | Local-only | [ADR 0015](adr/0015-trustworthy-terminal-inspectable-runtime.md), [ADR 0017](adr/0017-runtime-socket-and-control-plane-semantics.md) |
| Live smoke | `scripts/live-smoke-gate.sh /tmp/ultty-live-smoke-gate` passes against a real Ultty window. | Local-only | [Visual Regression](visual-regression.md) |
| Performance sanity | Current local behavior is consistent with the dated baseline and live trace expectations: near-zero idle work, responsive input, and no obvious live-pipeline regression. | Local-only | [Performance Reports](perf/README.md), [2026-05-14 baseline](perf/2026-05-14-baseline.md), [live pipeline trace](perf/2026-05-14-live-pipeline-trace.md) |
| Docs | README run commands and version flags work from source. Known Unix-first scope is visible. | HITL/manual | [README](../README.md), [Getting Started](GETTING_STARTED.md) |

## Public Alpha Gates

Public alpha requires every dogfood gate plus the following release-facing
checks. Alpha is still a preview; unsupported platforms and rough edges must be
stated plainly in the release notes.

| Area | Gate | Type | Evidence |
|------|------|------|----------|
| Release artifacts | Tarball, `.deb`, `.rpm`, and Homebrew formula structure pass release smoke checks. Binaries expose `--version`. Desktop files and icon assets are packaged. | Automated CI | [Release Artifact Smokes](release-artifacts.md), `.github/workflows/release.yml` |
| Security audit | `scripts/security-audit.sh` passes. Any ignored advisory is warning-only, documented, and not directly reachable through Ultty code. | Automated CI | [Security Audit](security-audit.md) |
| License and metadata | Repository license files, package metadata, release metadata, desktop metadata, and crate metadata are complete and consistent. | Automated CI plus HITL/manual | [Release Metadata](release-metadata.md) |
| Fresh performance evidence | A dated performance and live-readiness artifact from the current release candidate exists before publishing alpha claims. | Local-only plus HITL/manual | [2026-05-29 readiness artifact](perf/2026-05-29-current-readiness.md); refresh per release candidate. |
| Native runtime evidence | A local live smoke artifact is attached or linked from the release checklist. Screenshots should show real rendered output rather than only process success. | Local-only plus HITL/manual | [Visual Regression](visual-regression.md) |
| Packaging install check | At least one Linux install path is manually verified from the produced artifact. macOS/Homebrew should be verified before claiming macOS install support. | HITL/manual | [Release Artifact Smokes](release-artifacts.md) |
| Public docs | README, Getting Started, architecture overview, protocol spec, performance docs, release artifact docs, security audit docs, and known limitations are reachable from the documentation index. | HITL/manual | [README documentation index](../README.md#documentation) |
| Known limitations | Release notes call out Unix-first scope, Windows exclusion, alpha status, GUI/live-smoke requirements, and any current terminal correctness or packaging limitations. | HITL/manual | [ADR 0003](adr/0003-unix-first-runtime.md), [ADR 0014](adr/0014-next-phase-exclusions.md) |

## General Use Gates

General use means Ultty is ready to recommend as a normal terminal emulator for
users outside the project. It requires every public alpha gate plus repeated
evidence that the release process and live renderer remain stable.

| Area | Gate | Type | Evidence |
|------|------|------|----------|
| Automated live renderer gate | At least one renderer screenshot or equivalent live smoke runs in automation on a suitable runner, or the release process has a documented dedicated-runner substitute. | Automated CI or dedicated runner | [Live Smoke Automation](live-smoke-automation.md) |
| Repeated performance evidence | Current performance docs include repeated runs, variance notes, and hardware/window conditions for any public performance claim. | Local-only plus HITL/manual | [Performance Artifact Conventions](perf/README.md) |
| Runtime module maintainability | Remaining large app runtime modules are split enough that issue triage, instrumentation, and targeted fixes are practical. | HITL/manual | [Architecture Deepening Backlog](architecture-deepening-backlog.md) |
| Roadmap and backlog | Roadmap and backlog match the readiness language in this document and do not imply unsupported release maturity. | HITL/manual | [Readiness Issue Backlog](ISSUE_BACKLOG_30_60_90.md) |
| Diagnostics and recovery | Control-plane diagnostics, screenshot capture, panic/crash reporting, and failure logs are documented well enough for external bug reports. | HITL/manual | [ADR 0015](adr/0015-trustworthy-terminal-inspectable-runtime.md), [ADR 0017](adr/0017-runtime-socket-and-control-plane-semantics.md) |
| Terminal correctness confidence | No known P0/P1 correctness bugs remain in common interactive workloads such as shells, editors, pagers, resize-heavy TUIs, tabs, splits, and copy mode. | Automated CI plus HITL/manual | [Testing Strategy](TESTING_STRATEGY.md), live dogfood notes |
| Install and upgrade path | Install, uninstall, upgrade, and version reporting are documented and verified for each advertised distribution channel. | Automated CI plus HITL/manual | [Release Artifact Smokes](release-artifacts.md), [Getting Started](GETTING_STARTED.md) |

## Current Status

As of this checklist, Ultty is suitable for maintainer dogfood when the dogfood
commands above pass locally. It is not ready to describe as general-use
software.

The shortest path to a credible public alpha is:

1. Create a release-candidate checklist from
   [RC_TEMPLATE.md](releases/RC_TEMPLATE.md).
2. Run hosted CI, security audit, release artifact smoke, and the dedicated
   live smoke workflow for the candidate commit.
3. Verify at least one Linux install path from the produced artifact.
4. Publish alpha notes that explicitly state Unix-first scope and known
   limitations.

The shortest path from public alpha to general use is:

1. Repeat the public-alpha gate across at least two release candidates.
2. Preserve live renderer workflow URLs and uploaded artifacts for those
   candidates.
3. Split the remaining large runtime modules that slow targeted fixes according
   to [Architecture Deepening Backlog](architecture-deepening-backlog.md).
4. Collect repeated external dogfood feedback and close any P0/P1 correctness
   blockers before broad recommendations.
