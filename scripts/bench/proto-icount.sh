#!/usr/bin/env bash
# ADR 0022 prototype: callgrind instruction counts inside the timed pass,
# as icount-gate.sh collects them, for the reference (`run_vt_pass`) and
# prototype variants A (`run_proto_a_pass`) and C (`run_proto_c_pass`).
#
#   scripts/bench/proto-icount.sh [--jobs N] [--out DIR]
#
# Prints one line per case: corpus size geometry arm instructions.
# Needs valgrind and a release ultty-bench built with `--features proto`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
JOBS=4 OUT="$ROOT/target/bench-results/proto-icount"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --jobs) JOBS="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 1 ;;
  esac
done
command -v valgrind >/dev/null || { echo "valgrind is not installed" >&2; exit 1; }
mkdir -p "$OUT"
(cd "$ROOT" && cargo build --release --locked -p ultty-bench --features proto >&2)
cp "${CARGO_TARGET_DIR:-$ROOT/target}/release/ultty-bench" "$OUT/ultty-bench"

one() {
  local corpus="$1" size="$2" geometry="$3" arm="$4" suite fn file ir
  case "$arm" in
    ref) suite=vt fn='ultty_bench::run_vt_pass' ;;
    A) suite=proto-a fn='ultty_bench::run_proto_a_pass' ;;
    C) suite=proto-c fn='ultty_bench::run_proto_c_pass' ;;
  esac
  file="$OUT/callgrind.$corpus.$size.$geometry.$arm.out"
  valgrind --tool=callgrind --callgrind-out-file="$file" --toggle-collect="$fn" \
    "$OUT/ultty-bench" vt --suite "$suite" --corpus "$corpus" --size-mib "$size" \
    --runs 1 --warmup 0 --geometry "$geometry" --out /dev/null >/dev/null 2>"$file.log"
  ir=$(awk '/^(summary|totals):/ {print $2; exit}' "$file")
  echo "$corpus $size $geometry $arm $ir"
}
export -f one
export OUT

for corpus in ascii wide-wrap scroll sgr-dense cjk; do
  for size in 1 8; do
    for geometry in 120x40 300x75; do
      for arm in ref A C; do
        echo "$corpus $size $geometry $arm"
      done
    done
  done
done | xargs -P "$JOBS" -L 1 bash -c 'one "$@"' _ | sort
