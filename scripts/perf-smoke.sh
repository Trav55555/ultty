#!/usr/bin/env bash
# CI-friendly perf smoke for the live Ultty stress runner.
#
# By default this script skips when no live control socket is available, which
# makes it safe to call from generic CI. Set ULTTY_REQUIRE_LIVE=1 to fail when
# Ultty is not running.

set -euo pipefail

OUT="${1:-/tmp/ultty-perf-smoke.md}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"
RUNS="${ULTTY_PERF_SMOKE_RUNS:-2}"
TESTS="${ULTTY_PERF_SMOKE_TESTS:-1,6,8}"
TIMEOUT_SECS="${ULTTY_PERF_SMOKE_TIMEOUT_SECS:-60}"

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  if [[ "${ULTTY_REQUIRE_LIVE:-0}" == "1" ]]; then
    echo "ulttyctl cannot reach a running Ultty control socket." >&2
    echo "Start Ultty first, then rerun: $0 [$OUT]" >&2
    exit 1
  fi

  echo "perf smoke skipped: no live Ultty control socket" >&2
  exit 0
fi

scripts/stress-repeat.sh --runs "$RUNS" --tests "$TESTS" --timeout "$TIMEOUT_SECS" --out "$OUT" >/dev/null

if [[ ! -s "$OUT" ]]; then
  echo "perf smoke failed: missing report $OUT" >&2
  exit 1
fi

IFS=',' read -r -a requested_tests <<< "$TESTS"
for test_num in "${requested_tests[@]}"; do
  case "$test_num" in
    1)
      row='| `1. throughput-4MiB` |'
      label="throughput"
      ;;
    6)
      row='| `6. 500-lines-4000-cols` |'
      label="wrap"
      ;;
    8)
      row='| `8. 20k-scrollback-lines` |'
      label="scrollback"
      ;;
    *)
      row=""
      label=""
      ;;
  esac

  if [[ -n "$row" ]] && ! grep -Fq "$row" "$OUT"; then
    echo "perf smoke failed: $label row missing from $OUT" >&2
    exit 1
  fi
done

echo "perf smoke passed: $OUT"
