#!/usr/bin/env bash
# Repeat the live Ultty stress battery through real tabs and aggregate variance.
#
# Requires a running Ultty control socket:
#   cargo run -p ultty --bin ultty
#
# Examples:
#   scripts/stress-repeat.sh --runs 5
#   scripts/stress-repeat.sh --runs 3 --tests 1,6 --out /tmp/ultty-stress.md

set -euo pipefail

RUNS=3
TESTS=""
OUT=""
TIMEOUT_SECS=60
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"

usage() {
  cat <<'USAGE'
Usage: scripts/stress-repeat.sh [options]

Options:
  --runs N          Number of repeat runs (default: 3)
  --tests LIST      Comma-separated stress test numbers, e.g. 1,6
  --out PATH        Write a markdown report to PATH as well as stdout
  --timeout SECS    Per-run timeout while waiting for DONE (default: 60)
  --ulttyctl PATH   Path to ulttyctl (default: target/debug/ulttyctl)
  -h, --help        Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs)
      RUNS="$2"
      shift 2
      ;;
    --tests)
      TESTS="$2"
      shift 2
      ;;
    --out)
      OUT="$2"
      shift 2
      ;;
    --timeout)
      TIMEOUT_SECS="$2"
      shift 2
      ;;
    --ulttyctl)
      ULTTYCTL="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$RUNS" in
  ''|*[!0-9]*)
    echo "--runs must be a positive integer" >&2
    exit 2
    ;;
esac

if [[ "$RUNS" -lt 1 ]]; then
  echo "--runs must be at least 1" >&2
  exit 2
fi

case "$TIMEOUT_SECS" in
  ''|*[!0-9]*)
    echo "--timeout must be a positive integer" >&2
    exit 2
    ;;
esac

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  echo "ulttyctl cannot reach a running Ultty control socket." >&2
  echo "Start Ultty first, then rerun this script." >&2
  exit 1
fi

ROOT="$(git rev-parse --show-toplevel)"
STRESS_SCRIPT="$ROOT/scripts/stress.sh"
if [[ ! -x "$STRESS_SCRIPT" && ! -f "$STRESS_SCRIPT" ]]; then
  echo "missing stress script: $STRESS_SCRIPT" >&2
  exit 1
fi

ENV_BIN="$(command -v env)"
BASH_BIN="$(command -v bash)"

test_args=()
if [[ -n "$TESTS" ]]; then
  IFS=',' read -r -a test_args <<< "$TESTS"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ultty-stress-repeat.XXXXXX")"
KEEP_TMP=0
cleanup() {
  if [[ "$KEEP_TMP" == "1" ]]; then
    return
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

poll_done() {
  local log="$1"
  local deadline=$((SECONDS + TIMEOUT_SECS))
  while [[ "$SECONDS" -le "$deadline" ]]; do
    if [[ -f "$log" ]] && grep -q '^DONE$' "$log"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

RUN_LOGS=()
for run in $(seq 1 "$RUNS"); do
  log="$TMP_DIR/run-$run.log"
  RUN_LOGS+=("$log")
  echo "starting live stress run $run/$RUNS" >&2
  "$ULTTYCTL" new-tab "$ENV_BIN" "ULTTY_STRESS_LOG=$log" "$BASH_BIN" "$STRESS_SCRIPT" "${test_args[@]}" >/dev/null
  if ! poll_done "$log"; then
    echo "timed out waiting for run $run to finish; log: $log" >&2
    [[ -f "$log" ]] && cat "$log" >&2
    KEEP_TMP=1
    echo "preserving stress-repeat temp directory: $TMP_DIR" >&2
    exit 1
  fi
done

{
  echo "# Ultty Live Stress Repeat Report"
  echo
  echo "| Field | Value |"
  echo "|-------|-------|"
  echo "| Date | $(date -Iseconds) |"
  echo "| Commit | \`$(git rev-parse HEAD)\` |"
  echo "| Runs | $RUNS |"
  echo "| Tests | ${TESTS:-all} |"
  echo "| Driver | \`$ULTTYCTL new-tab $ENV_BIN ULTTY_STRESS_LOG=<log> $BASH_BIN scripts/stress.sh\` |"
  echo
  echo "## Summary"
  echo
  awk '
    /^[[:space:]]*[0-9]+\./ {
      ms = $(NF - 1)
      key = $0
      sub(/^[[:space:]]*/, "", key)
      sub(/[[:space:]]+[0-9]+[[:space:]]+ms$/, "", key)
      if (!(key in seen)) {
        seen[key] = 1
        order[++order_count] = key
        min[key] = ms
        max[key] = ms
      }
      count[key]++
      sum[key] += ms
      sumsq[key] += ms * ms
      if (ms < min[key]) min[key] = ms
      if (ms > max[key]) max[key] = ms
    }
    END {
      print "| Workload | Runs | Mean | Stddev | Min | Max | Spread |"
      print "|----------|------|------|--------|-----|-----|--------|"
      for (i = 1; i <= order_count; i++) {
        key = order[i]
        mean = sum[key] / count[key]
        if (count[key] > 1) {
          variance = (sumsq[key] - (sum[key] * sum[key] / count[key])) / (count[key] - 1)
          if (variance < 0) variance = 0
          stddev = sqrt(variance)
        } else {
          stddev = 0
        }
        spread = max[key] - min[key]
        printf "| `%s` | %d | %.1f ms | %.1f ms | %d ms | %d ms | %d ms |\n",
          key, count[key], mean, stddev, min[key], max[key], spread
      }
    }
  ' "${RUN_LOGS[@]}"
  echo
  echo "## Raw Runs"
  echo
  for i in "${!RUN_LOGS[@]}"; do
    run=$((i + 1))
    echo "### Run $run"
    echo
    echo '```text'
    grep -v '^DONE$' "${RUN_LOGS[$i]}"
    echo '```'
    echo
  done
} | tee ${OUT:+"$OUT"}
