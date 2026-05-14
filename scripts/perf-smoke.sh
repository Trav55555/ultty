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

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  if [[ "${ULTTY_REQUIRE_LIVE:-0}" == "1" ]]; then
    echo "ulttyctl cannot reach a running Ultty control socket." >&2
    echo "Start Ultty first, then rerun: $0 [$OUT]" >&2
    exit 1
  fi

  echo "perf smoke skipped: no live Ultty control socket" >&2
  exit 0
fi

scripts/stress-repeat.sh --runs "$RUNS" --tests "$TESTS" --out "$OUT" >/dev/null

if [[ ! -s "$OUT" ]]; then
  echo "perf smoke failed: missing report $OUT" >&2
  exit 1
fi

if ! grep -q "| \`1. throughput-4MiB\` |" "$OUT"; then
  echo "perf smoke failed: throughput row missing from $OUT" >&2
  exit 1
fi

if ! grep -q "| \`6. 500-lines-4000-cols\` |" "$OUT"; then
  echo "perf smoke failed: wrap row missing from $OUT" >&2
  exit 1
fi

if ! grep -q "| \`8. 20k-scrollback-lines\` |" "$OUT"; then
  echo "perf smoke failed: scrollback row missing from $OUT" >&2
  exit 1
fi

echo "perf smoke passed: $OUT"
