#!/usr/bin/env bash
# Run scripts/ci-runner-step.sh against fake sccache, aws and ssm-put commands
# and check that a step still runs when the sccache server is slow to start
# or never starts or dies mid-step, that it keeps the cache when the server does come up, and
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
    echo "Compile requests 1 (cache hits 1)"
    exit 0 ;;
esac
# Invoked as the rustc wrapper: fails while the server is down, and its first
# $SCCACHE_WRAP_FAILS calls fail even though the start check passed (a server
# that died or restarted mid-build).
[ "$n" -gt "${SCCACHE_FAILS:-0}" ] || exit 2
wrap_file="$FAKE_STATE/sccache-wrapped"
w=$(($(cat "$wrap_file" 2>/dev/null || echo 0) + 1))
echo "$w" >"$wrap_file"
if [ "$w" -le "${SCCACHE_WRAP_FAILS:-0}" ]; then
  echo "sccache: error: Timed out waiting for server startup. Maybe the remote service is unreachable?" >&2
  exit 2
fi
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
echo run >>"$FAKE_STATE/cargo-runs"
[ -z "${FAKE_CARGO_MSG:-}" ] || echo "$FAKE_CARGO_MSG" >&2
if [ -n "${RUSTC_WRAPPER:-}" ]; then
  "$RUSTC_WRAPPER" rustc -vV || { echo "wrapper failed" >&2; exit 101; }
fi
echo "wrapper=${RUSTC_WRAPPER:-none}"
echo "rustflags=${RUSTFLAGS:-}"
exit "${FAKE_CARGO_EXIT:-0}"
SH
chmod 755 "$work/bin/"*

failures=0
# run_step NAME SCCACHE_FAILS CARGO_EXIT [WRAP_FAILS] -> sets $out, $status, $runs
run_step() {
  rm -rf "$work/state" && mkdir "$work/state"
  status=0
  out="$(PATH="$work/bin:$PATH" FAKE_STATE="$work/state" SCCACHE_FAILS="$2" \
    SCCACHE_WRAP_FAILS="${4:-0}" \
    FAKE_CARGO_EXIT="$3" RUSTC_WRAPPER=sccache SCCACHE_REGION=us-east-1 \
    ULTTY_RUNNER_ACTIVITY_SSM=/test/activity \
    "$STEP_SH" fake-cargo 2>&1)" || status=$?
  runs="$(wc -l <"$work/state/cargo-runs")"
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

run_step "step fails" 0 3
expect "step fails" "an ordinary failure is not retried" test "$runs" -eq 1

FAKE_CARGO_MSG="error: could not compile (sccache: error: not ours)" \
  run_step "ordinary compile error" 0 101
expect "ordinary compile error" "not retried" test "$runs" -eq 1
expect "ordinary compile error" "exit status 101" test "$status" -eq 101

run_step "server dies mid-step" 0 0 1
expect "server dies mid-step" "step is rerun once" test "$runs" -eq 2
expect "server dies mid-step" "rerun has no cache" contains "wrapper=none"
expect "server dies mid-step" "a warning says so" contains "::warning::sccache failed mid-step"
expect "server dies mid-step" "exit status 0" test "$status" -eq 0

# The rerun's own result is the step's result.
run_step "mid-step failure, then real failure" 0 3 1
expect "mid-step failure, then real failure" "exit status 3" test "$status" -eq 3
expect "mid-step failure, then real failure" "rerun once only" test "$runs" -eq 2

CI_SCCACHE_STATS=1 run_step "stats printed when enabled" 0 0
expect "stats printed when enabled" "prints sccache stats" contains "Compile requests 1 (cache hits 1)"
expect "stats printed when enabled" "exit status 0" test "$status" -eq 0

printf '#!/bin/sh\nexit 0\n' >"$work/bin/mold"
chmod 755 "$work/bin/mold"
CI_AUTO_LINKER=1 RUSTFLAGS="-Dwarnings" run_step "auto-linker detects mold" 0 0
expect "auto-linker detects mold" "appends -fuse-ld=mold" contains "rustflags=-Dwarnings -C link-arg=-fuse-ld=mold"
expect "auto-linker detects mold" "exit status 0" test "$status" -eq 0

CI_AUTO_LINKER=0 RUSTFLAGS="-Dwarnings" run_step "auto-linker disabled" 0 0
expect "auto-linker disabled" "leaves RUSTFLAGS unchanged" contains "rustflags=-Dwarnings"
rm -f "$work/bin/mold"

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "ci-runner-step.sh: all checks passed"
