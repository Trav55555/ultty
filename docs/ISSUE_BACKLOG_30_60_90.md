# Ultty Readiness Issue Backlog

**Updated:** 2026-05-29
**Source plan:** [Ultty Readiness Roadmap](ROADMAP_30_60_90.md)
**Readiness gates:** [Public Alpha Readiness](alpha-readiness.md)

This backlog replaces the older copy-paste M0/M1/M2 issue templates. The
current issue tracker is the canonical queue; this file explains how the open
work maps to readiness levels.

## Active Issues

| Issue | Readiness Level | Priority | Area | Status |
|-------|-----------------|----------|------|--------|
| #103 Refresh current performance and live-readiness evidence | Public alpha | P1 | Performance/live runtime | Open; partially refreshed in `2026-05-29-current-readiness.md`, blocked by live pipeline timeout and missing process-level idle evidence. |
| #102 Split remaining large app runtime modules | General use | P2 | Runtime maintainability | Open; next refactor slice after live-readiness evidence is clean. |

## Recently Completed Readiness Work

| Issue | Result | Evidence |
|-------|--------|----------|
| #101 Create public alpha readiness checklist | Done | [Public Alpha Readiness](alpha-readiness.md) |
| #97 Add license files and release metadata hygiene | Done | [Release Metadata](release-metadata.md), top-level license files |
| #104 Promote a live renderer smoke into automation | Done | [Live Smoke Automation](live-smoke-automation.md) |
| #100 Add sequential live smoke gate | Done | `scripts/live-smoke-gate.sh`, [Visual Regression](visual-regression.md) |
| #99 Add release artifact smoke tests | Done | [Release Artifact Smokes](release-artifacts.md) |
| #96 Add dependency security audit gate | Done | [Security Audit](security-audit.md) |
| #95 Add local version flags | Done | `ultty --version`, `ulttyctl --version` |
| #98 Fix native rich text smoke launch | Done | `scripts/native-richtext-smoke.sh` |
| #106 Create logo/icon assets for installable app metadata | Done | [Brand Assets](brand.md), `dist/icons/hicolor` |

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

## Next Issue Templates

### Public Alpha: Live Evidence Follow-Up

```markdown
## Problem
Public-alpha readiness is blocked because the latest live pipeline refresh did
not complete workload 6 and did not capture process-level idle evidence.

## Scope
- Diagnose whether the workload 6 timeout is a runtime regression, compositor
  issue, PTY backpressure issue, or stress harness problem.
- Preserve failure logs when `stress-repeat` times out.
- Re-run `scripts/live-pipeline-trace.sh`.
- Re-run `scripts/softwrap-pty-probe.sh`.
- Capture process-level idle CPU evidence and document methodology.

## Out of scope
- Do not add performance thresholds to the dedicated live smoke workflow until
  runner variance is measured.

## Validation
- `cargo fmt --all -- --check`
- `scripts/live-pipeline-trace.sh /tmp/ultty-live-pipeline-trace`
- `scripts/softwrap-pty-probe.sh /tmp/ultty-softwrap-pty-probe`
- Dated artifact under `docs/perf/`
```

### General Use: Runtime Module Split

```markdown
## Problem
Large runtime modules slow targeted fixes, instrumentation, and review.

## Scope
- Identify the largest app/runtime/chrome/control modules still carrying mixed
  responsibilities.
- Extract one cohesive responsibility per commit.
- Preserve existing tests and public behavior.
- Add focused tests only where the split exposes untested behavior.

## Out of scope
- No visual redesign.
- No protocol changes unless required by the extracted boundary.

## Validation
- `cargo fmt --all -- --check`
- `cargo check --workspace`
- Relevant crate tests for touched modules
```
