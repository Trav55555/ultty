#!/usr/bin/env bash
# Live stress smoke: proves the live stress runner measured every workload.
#
# This is not a regression gate (it has no thresholds; see
# scripts/bench/icount-gate.sh for the deterministic gate). It fails when
# nothing was measured: no reachable Ultty control socket, a missing report,
# a workload row missing, or a row with fewer runs than requested.
#
# It drives release binaries; a debug build (opt-level 1) measures the
# compiler's settings, not Ultty. Build first:
#   cargo build --release -p ultty --bins
#
# ULTTY_PERF_SMOKE_SKIP_OK=1 turns "no live socket" into an explicit skip
# (exit 0, message on stderr) for callers that run without a display.

set -euo pipefail

OUT="${1:-target/bench-results/perf-smoke.md}"
ULTTYCTL="${ULTTYCTL:-target/release/ulttyctl}"
RUNS="${ULTTY_PERF_SMOKE_RUNS:-2}"
TESTS="${ULTTY_PERF_SMOKE_TESTS:-1,6,8}"
TIMEOUT_SECS="${ULTTY_PERF_SMOKE_TIMEOUT_SECS:-60}"

if [[ ! -x "$ULTTYCTL" ]]; then
  echo "perf smoke failed: $ULTTYCTL not found; run cargo build --release -p ultty --bins" >&2
  exit 1
fi

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  if [[ "${ULTTY_PERF_SMOKE_SKIP_OK:-0}" == "1" ]]; then
    echo "perf smoke SKIPPED: no live Ultty control socket, nothing was measured" >&2
    exit 0
  fi
  echo "perf smoke failed: ulttyctl cannot reach a running Ultty control socket." >&2
  echo "Start a release Ultty first, then rerun: $0 [$OUT]" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT")"
scripts/stress-repeat.sh --ulttyctl "$ULTTYCTL" --runs "$RUNS" --tests "$TESTS" \
  --timeout "$TIMEOUT_SECS" --out "$OUT" >/dev/null

if [[ ! -s "$OUT" ]]; then
  echo "perf smoke failed: missing report $OUT" >&2
  exit 1
fi

IFS=',' read -r -a requested_tests <<< "$TESTS"
for test_num in "${requested_tests[@]}"; do
  case "$test_num" in
    1) name='1. throughput-4MiB' label=throughput ;;
    6) name='6. 500-lines-4000-cols' label=wrap ;;
    8) name='8. 20k-scrollback-lines' label=scrollback ;;
    *) continue ;;
  esac

  line=$(grep -F "| \`$name\` |" "$OUT" | head -n 1 || true)
  if [[ -z "$line" ]]; then
    echo "perf smoke failed: $label row missing from $OUT" >&2
    exit 1
  fi
  # | `name` | runs | mean ms | ...
  runs=$(awk -F'|' '{ gsub(/ /, "", $3); print $3 }' <<<"$line")
  mean=$(awk -F'|' '{ gsub(/ ms| /, "", $4); print $4 }' <<<"$line")
  if [[ "$runs" != "$RUNS" ]]; then
    echo "perf smoke failed: $label measured $runs of $RUNS runs ($OUT)" >&2
    exit 1
  fi
  if ! awk -v m="$mean" 'BEGIN { exit !(m > 0) }'; then
    echo "perf smoke failed: $label mean is not a positive time: $mean ($OUT)" >&2
    exit 1
  fi
done

echo "perf smoke passed: $OUT"
