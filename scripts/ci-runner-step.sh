#!/usr/bin/env bash
# Run one Gitea Actions step on the ultty runner pool (Fargate tasks defined in
# the gitea-server repository) and pass its exit status through.
#
# Usage: scripts/ci-runner-step.sh <command> [args...]
#
# While the command runs, the pool's activity parameter is stamped every minute;
# without it the scaler stops the task after its idle window, mid-job. When
# RUSTC_WRAPPER is sccache, compiles are cached in the shared S3 bucket under
# SCCACHE_S3_KEY_PREFIX, with credentials exported from the task role. If the
# sccache server will not start, the step compiles without the cache rather
# than failing. scripts/ci-runner-step-test.sh tests this script.
set -euo pipefail

ACTIVITY_SSM="${ULTTY_RUNNER_ACTIVITY_SSM:-/gitea/prod/ultty_runner_last_activity}"
# sccache's client gives a starting server 10 s; a fresh task's first start
# sometimes needs longer (2026-11-09 to 11-13: ten jobs failed that way).
SCCACHE_START_ATTEMPTS=3

heartbeat() {
  # Sleep in the background and wait, so a TERM ends the sleep too: an
  # orphaned sleep would hold the step's output open for up to a minute.
  local sleep_pid=""
  trap 'kill "$sleep_pid" 2>/dev/null; exit 0' TERM
  while true; do
    ssm-put "$ACTIVITY_SSM" "$(date +%s)" || true
    sleep 60 &
    sleep_pid=$!
    wait "$sleep_pid"
  done
}
heartbeat &
heartbeat_pid=$!
trap 'kill "$heartbeat_pid" 2>/dev/null || true' EXIT

# Connect to the sccache server, starting it if needed. A server that missed
# the client's timeout often finishes starting, so retry before giving up.
start_sccache() {
  local attempt
  for attempt in $(seq "$SCCACHE_START_ATTEMPTS"); do
    if sccache --show-stats >/dev/null; then
      return 0
    fi
    echo "sccache server not up (attempt $attempt of $SCCACHE_START_ATTEMPTS)" >&2
    [ "$attempt" -lt "$SCCACHE_START_ATTEMPTS" ] && sleep 2
  done
  return 1
}

if [ "${RUSTC_WRAPPER:-}" = sccache ]; then
  eval "$(aws configure export-credentials --format env)"
  account="$(aws sts get-caller-identity --query Account --output text)"
  export SCCACHE_BUCKET="gitea-ci-cache-$account-${SCCACHE_REGION:?}"
  if ! start_sccache; then
    echo "::warning::sccache server did not start; compiling without the cache"
    unset RUSTC_WRAPPER
  fi
fi

"$@"
