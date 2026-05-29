# Ultty Readiness Issue Backlog

**Updated:** 2026-05-29
**Source plan:** [Ultty Readiness Roadmap](ROADMAP_30_60_90.md)
**Readiness gates:** [Public Alpha Readiness](alpha-readiness.md)

The GitHub issue tracker is the canonical queue. At the time this backlog was
refreshed there were no open readiness issues, so this file records the next
candidate slices to file when work resumes.

## Active Issues

None.

## Recently Completed Readiness Work

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
