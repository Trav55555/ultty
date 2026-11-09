#!/usr/bin/env bash
# Run one Gitea Actions step on the ultty runner pool (Fargate tasks defined in
# the gitea-server repository) and pass its exit status through.
#
# Usage: scripts/ci-runner-step.sh <command> [args...]
#
# While the command runs, the pool's activity parameter is stamped every minute;
# without it the scaler stops the task after its idle window, mid-job. When
# RUSTC_WRAPPER is sccache, compiles are cached in the shared S3 bucket under
# SCCACHE_S3_KEY_PREFIX, with credentials exported from the task role.
set -euo pipefail

ACTIVITY_SSM="${ULTTY_RUNNER_ACTIVITY_SSM:-/gitea/prod/ultty_runner_last_activity}"

heartbeat() {
  while true; do
    ssm-put "$ACTIVITY_SSM" "$(date +%s)" || true
    sleep 60
  done
}
heartbeat &
heartbeat_pid=$!
trap 'kill "$heartbeat_pid" 2>/dev/null || true' EXIT

if [ "${RUSTC_WRAPPER:-}" = sccache ]; then
  eval "$(aws configure export-credentials --format env)"
  account="$(aws sts get-caller-identity --query Account --output text)"
  export SCCACHE_BUCKET="gitea-ci-cache-$account-${SCCACHE_REGION:?}"
fi

"$@"
