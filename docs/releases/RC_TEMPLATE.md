# Ultty Release Candidate Checklist

Copy this file to `docs/releases/<version-or-date>.md` for each public alpha or
general-use release candidate.

## Candidate

| Field | Value |
|-------|-------|
| Candidate name | TBD |
| Commit SHA | TBD |
| Branch or tag | TBD |
| Date | TBD |
| Release owner | TBD |

## Required Evidence

| Gate | Evidence |
|------|----------|
| Hosted CI | Link to successful `ci.yml` run |
| Security audit | Link to successful audit job or local `scripts/security-audit.sh` output |
| Release artifacts | Link to `release.yml` run and artifact smoke output |
| Live smoke | Link to `live-smoke.yml` run and uploaded artifact |
| Performance evidence | Link to dated `docs/perf/` artifact for this candidate or explicit reuse rationale |
| Linux install check | Package path, install command, uninstall command, and result |
| macOS install check | Homebrew formula result, or explicit "not advertised for this candidate" |
| Terminal workflow smoke | Artifact path from `scripts/terminal-workflow-smoke.sh`, or explicit deferral |
| Known limitations | Link to release notes section |

## Manual Checks

- `ultty --version` prints the candidate version.
- `ulttyctl --version` prints the candidate version.
- `ulttyctl ping` works against a running instance.
- `ulttyctl --json info` returns valid JSON.
- Screenshot capture produces a non-empty artifact.
- README, Getting Started, and release notes state Unix-first scope.

## Decision

| Question | Answer |
|----------|--------|
| Is this suitable for maintainer dogfood? | TBD |
| Is this suitable for public alpha? | TBD |
| Is this suitable for general use? | TBD |

## Notes

Record release-specific caveats, hardware/runner notes, failed attempts, and
follow-up issues here.
