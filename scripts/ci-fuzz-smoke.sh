#!/usr/bin/env bash
# Run the CI fuzz smoke targets with a single parallel build followed by two
# balanced concurrent waves (120s + 60s = 180s instead of 360s serial), capping
# concurrency at 2 ASan processes to stay within runner RAM limits.
#
# Usage: scripts/ci-fuzz-smoke.sh [nightly-toolchain]
set -euo pipefail

TOOLCHAIN="${1:-nightly}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/fuzz"

# Build all targets in one parallel Cargo invocation.
cargo "+$TOOLCHAIN" fuzz build

run_wave() {
  local log_a log_b pid_a pid_b status_a=0 status_b=0
  log_a="$(mktemp)"
  log_b="$(mktemp)"
  trap 'rm -f "$log_a" "$log_b"' RETURN

  (eval "$1") >"$log_a" 2>&1 &
  pid_a=$!
  (eval "$2") >"$log_b" 2>&1 &
  pid_b=$!

  wait "$pid_a" || status_a=$?
  wait "$pid_b" || status_b=$?

  cat "$log_a"
  cat "$log_b"

  if [ "$status_a" -ne 0 ] || [ "$status_b" -ne 0 ]; then
    return 1
  fi
}

echo "==> Wave 1 (120s): vt_terminal || (decode_app + decode_terminal + in_band_split)"
run_wave \
  "cargo +\"$TOOLCHAIN\" fuzz run vt_terminal -- -max_total_time=120 -max_len=4096" \
  "cargo +\"$TOOLCHAIN\" fuzz run decode_app -- -max_total_time=30 && \
   cargo +\"$TOOLCHAIN\" fuzz run decode_terminal -- -max_total_time=30 && \
   cargo +\"$TOOLCHAIN\" fuzz run in_band_split -- -max_total_time=60 -max_len=4096"

echo "==> Wave 2 (60s): kitty_graphics || image_decode"
run_wave \
  "cargo +\"$TOOLCHAIN\" fuzz run kitty_graphics -- -max_total_time=60 -max_len=8192" \
  "cargo +\"$TOOLCHAIN\" fuzz run image_decode -- -max_total_time=60 -max_len=65536 -rss_limit_mb=4096"
