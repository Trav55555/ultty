# Ultty Readiness Roadmap

**Updated:** 2026-05-29
**Scope:** `ultty`, `ulttui`, `ultty-protocol`, `ultty-client`, examples, release tooling
**Readiness source of truth:** [Public Alpha Readiness](alpha-readiness.md)
**Issue backlog:** [Readiness Issue Backlog](ISSUE_BACKLOG_30_60_90.md)

This roadmap supersedes the older February M0/M1/M2 streaming plan. The
streaming/native foundation has moved from speculative roadmap work into the
implemented baseline. Current planning is organized around readiness levels:
dogfood, public alpha, and general use.

## Current State

| Area | Status | Evidence |
|------|--------|----------|
| Native protocol and retained scene graph | Implemented | [Architecture](ARCHITECTURE.md), [Protocol Specification](PROTOCOL_SPEC.md) |
| Append-oriented native updates | Implemented | `AppendContent` smoke in [Visual Regression](visual-regression.md) |
| Dual native/ANSI app model | Implemented | `ulttui` examples and workspace tests |
| Renderer screenshot smokes | Implemented locally; promoted to dedicated-runner workflow | [Live Smoke Automation](live-smoke-automation.md) |
| Version flags | Implemented | `ultty --version`, `ulttyctl --version` |
| Desktop icon and package icon assets | Implemented | [Brand Assets](brand.md), [Release Artifact Smokes](release-artifacts.md) |
| License and release metadata hygiene | Implemented | [Release Metadata](release-metadata.md) |
| Security audit gate | Implemented | [Security Audit](security-audit.md) |
| Current live performance evidence | Not clean yet | [2026-05-29 readiness artifact](perf/2026-05-29-current-readiness.md), issue #103 |

## Readiness Board

| Level | Goal | Current Assessment |
|-------|------|--------------------|
| Dogfood | Maintainers can use Ultty from source with known limitations. | Mostly satisfied when local GUI smoke passes. The sequential live smoke passed on 2026-05-29. |
| Public alpha | External technical users can install prerelease builds with explicit caveats. | Blocked on clean live pipeline/soft-wrap evidence and process-level idle evidence (#103). |
| General use | Broad recommendation as a dependable terminal emulator. | Blocked on public-alpha evidence plus runtime maintainability work (#102) and repeated external dogfood feedback. |

## Completed Historical Plan

The original M0 streaming foundation is now considered done or superseded:

| Historical item | Disposition |
|-----------------|-------------|
| Protocol append messages | Done through append-oriented native protocol support. |
| Incremental content updates | Done for native append smoke coverage; structural markdown still uses normal content updates where needed. |
| Native rich text and widget rendering proof | Done through `native-richtext` live smoke. |
| Streaming readiness docs | Superseded by [Public Alpha Readiness](alpha-readiness.md) and [Performance Reports](perf/README.md). |

The old M1/M2 agent-workflow parity items are no longer the primary release
gate for Ultty itself. They should become app-specific issues only when a
concrete product surface depends on them.

## Next Executable Slices

| Priority | Issue | Why it matters | Exit condition |
|----------|-------|----------------|----------------|
| P1 | #103 Refresh current performance and live-readiness evidence | Public-alpha claims need fresh live evidence, not only microbenchmarks. | Live pipeline and soft-wrap probes complete on current `main`; process-level idle evidence is captured. |
| P2 | #102 Split remaining large app runtime modules | General-use work needs faster, lower-risk fixes in runtime/chrome/control paths. | Runtime modules are split enough that targeted fixes and instrumentation do not require broad edits. |

## Release Gate Cadence

Before any public alpha release candidate:

1. Run hosted CI: format, check, test, clippy, security audit.
2. Run release artifact smokes for the produced tarball/package/formula.
3. Run the dedicated live smoke workflow or an equivalent local GUI gate.
4. Refresh or cite a current performance artifact.
5. Confirm [Public Alpha Readiness](alpha-readiness.md) has no open alpha
   blocker.

Before claiming general use:

1. Repeat the public-alpha gate across at least two release candidates.
2. Promote or document stable live renderer automation artifacts.
3. Close known P0/P1 terminal correctness and live-readiness issues.
4. Complete the runtime maintainability slice tracked in #102 or replace it
   with more precise follow-up issues.
5. Publish install, upgrade, uninstall, diagnostics, and known-limitations docs
   for every advertised distribution channel.
