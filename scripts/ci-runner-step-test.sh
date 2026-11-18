#!/usr/bin/env bash
# Run scripts/ci-runner-step.sh against fake sccache, aws and ssm-put commands
# and check that a step still runs when the sccache server is slow to start
# or never starts, that it keeps the cache when the server does come up, and
# that the step's exit status passes through.
#
#   scripts/ci-runner-step-test.sh [path/to/ci-runner-step.sh]
set -euo pipefail

STEP_SH="$(realpath "${1:-$(dirname "$0")/ci-runner-step.sh}")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"

# sccache fails its first $SCCACHE_FAILS server probes (counted in a file),
# the way a server that misses the client's startup timeout does.
cat >"$work/bin/sccache" <<'SH'
#!/bin/sh
count_file="$FAKE_STATE/sccache-probes"
n=$(($(cat "$count_file" 2>/dev/null || echo 0) + 1))
echo "$n" >"$count_file"
case "$1" in
  --start-server | --show-stats)
    if [ "$n" -le "${SCCACHE_FAILS:-0}" ]; then
      echo "sccache: error: Timed out waiting for server startup. Maybe the remote service is unreachable?" >&2
      exit 2
    fi
    exit 0 ;;
esac
# Invoked as the rustc wrapper: fine once the server is up.
[ "$n" -gt "${SCCACHE_FAILS:-0}" ] || exit 2
SH
cat >"$work/bin/aws" <<'SH'
#!/bin/sh
case "$1" in
  configure) echo 'export AWS_ACCESS_KEY_ID=test' ;;
  sts) echo 123456789012 ;;
esac
SH
printf '#!/bin/sh\nexit 0\n' >"$work/bin/ssm-put"
# A stand-in for cargo: runs the wrapper the way cargo would, if one is set.
cat >"$work/bin/fake-cargo" <<'SH'
#!/bin/sh
if [ -n "${RUSTC_WRAPPER:-}" ]; then
  "$RUSTC_WRAPPER" rustc -vV || { echo "wrapper failed" >&2; exit 101; }
fi
echo "wrapper=${RUSTC_WRAPPER:-none}"
exit "${FAKE_CARGO_EXIT:-0}"
SH
chmod 755 "$work/bin/"*

failures=0
# run_step NAME SCCACHE_FAILS CARGO_EXIT -> prints combined output, sets $status
run_step() {
  rm -rf "$work/state" && mkdir "$work/state"
  status=0
  out="$(PATH="$work/bin:$PATH" FAKE_STATE="$work/state" SCCACHE_FAILS="$2" \
    FAKE_CARGO_EXIT="$3" RUSTC_WRAPPER=sccache SCCACHE_REGION=us-east-1 \
    ULTTY_RUNNER_ACTIVITY_SSM=/test/activity \
    "$STEP_SH" fake-cargo 2>&1)" || status=$?
}
expect() { # NAME CONDITION-DESCRIPTION TEST...
  local name="$1" what="$2"
  shift 2
  if ! "$@"; then
    echo "FAIL $name: $what (status $status)" >&2
    printf '%s\n' "$out" | sed 's/^/  | /' >&2
    failures=$((failures + 1))
  fi
}
contains() { case "$out" in *"$1"*) return 0 ;; esac; return 1; }

run_step "server up at once" 0 0
expect "server up at once" "step runs with the cache" contains "wrapper=sccache"
expect "server up at once" "exit status 0" test "$status" -eq 0

run_step "server slow to start" 1 0
expect "server slow to start" "step keeps the cache after a retry" contains "wrapper=sccache"
expect "server slow to start" "exit status 0" test "$status" -eq 0

run_step "server never starts" 99 0
expect "server never starts" "step runs without the cache" contains "wrapper=none"
expect "server never starts" "a warning says so" contains "compiling without the cache"
expect "server never starts" "exit status 0" test "$status" -eq 0

run_step "step fails" 0 3
expect "step fails" "the step's exit status passes through" test "$status" -eq 3

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "ci-runner-step.sh: all checks passed"
