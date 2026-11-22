#!/usr/bin/env bash
# Trigger and watch GitHub Actions macOS CI from Gitea Actions (ADR 0029 Slice 2).
#
# Usage: scripts/ci-macos-dispatch.sh
#
# Environment:
#   GH_TOKEN          Fine-grained GitHub token with Contents + Actions write on
#                     Trav55555/ultty (stored as Gitea secret GH_MACOS_CI_TOKEN).
#                     When unset, exits 0 without pushing anything.
#   GITHUB_EVENT_NAME pull_request or push
#   GITHUB_SHA        Commit SHA to test
#   GITHUB_REPOSITORY Owner/repo on Gitea (e.g. trav/ultty)
#   PR_NUMBER         Pull request number (when pull_request)
#   PR_HEAD_REPO      Head repository full name (when pull_request)
set -euo pipefail

REPO="${GITHUB_MIRROR_REPO:-Trav55555/ultty}"
WORKFLOW="macos.yml"
COMMIT="${GITHUB_SHA:?GITHUB_SHA is required}"

if [ -z "${GH_TOKEN:-}" ]; then
  echo "::notice::GH_MACOS_CI_TOKEN not set; skipping GitHub macOS CI (ADR 0029 Slice 2)"
  exit 0
fi

if [ "${GITHUB_EVENT_NAME:-}" = "pull_request" ]; then
  if [ "${PR_HEAD_REPO:-}" != "${GITHUB_REPOSITORY:-}" ]; then
    echo "::notice::Fork pull request; skipping macOS PR push (covered on main after merge)"
    exit 0
  fi
  : "${PR_NUMBER:?PR_NUMBER is required for pull_request}"
  ref="ci/pr-${PR_NUMBER}"
  askpass="$(mktemp)"
  chmod 700 "$askpass"
  cat >"$askpass" <<'SH'
#!/bin/sh
case "$1" in
  *Username*) printf '%s\n' "x-access-token" ;;
  *) printf '%s\n' "$GH_TOKEN" ;;
esac
SH
  cleanup() {
    GIT_ASKPASS="$askpass" GIT_TERMINAL_PROMPT=0 \
      git push "https://github.com/${REPO}.git" ":refs/heads/${ref}" >/dev/null 2>&1 || true
    rm -f "$askpass"
  }
  trap cleanup EXIT

  GIT_ASKPASS="$askpass" GIT_TERMINAL_PROMPT=0 \
    git push --force "https://github.com/${REPO}.git" "HEAD:refs/heads/${ref}"
else
  ref="main"
  mirrored=""
  for _ in $(seq 1 60); do
    mirrored="$(gh api "repos/${REPO}/commits/${ref}" --jq .sha 2>/dev/null || true)"
    [ "$mirrored" = "$COMMIT" ] && break
    echo "waiting for ${COMMIT} to reach ${REPO}:${ref}"
    sleep 5
  done
  if [ "$mirrored" != "$COMMIT" ]; then
    echo "::warning::${REPO}:${ref} did not reach ${COMMIT} in time (at ${mirrored:-unknown}); skipping macOS dispatch"
    exit 0
  fi
fi

started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
gh workflow run "$WORKFLOW" --repo "$REPO" --ref "$ref" -f "ref=${ref}"

run_id=""
for _ in $(seq 1 60); do
  run_id="$(gh run list --repo "$REPO" --workflow "$WORKFLOW" --branch "$ref" --limit 5 \
    --json databaseId,headSha,createdAt \
    --jq ".[] | select(.headSha == \"${COMMIT}\" and .createdAt >= \"${started_at}\") | .databaseId" \
    2>/dev/null | head -n 1 || true)"
  [ -n "$run_id" ] && break
  sleep 5
done

if [ -z "$run_id" ]; then
  echo "macOS CI run for ${COMMIT} on ${REPO}:${ref} did not appear" >&2
  exit 1
fi

echo "Watching https://github.com/${REPO}/actions/runs/${run_id}"
gh run watch "$run_id" --repo "$REPO" --exit-status
