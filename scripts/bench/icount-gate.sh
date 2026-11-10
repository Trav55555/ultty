#!/usr/bin/env bash
# Deterministic performance gate: instruction counts and heap bytes.
#
#   scripts/bench/icount-gate.sh            compare with bench/baselines/icount.json
#   scripts/bench/icount-gate.sh --update   rewrite the baseline (commit it with a reason)
#
# Instruction counts come from callgrind, collected only inside the timed
# functions of `ultty-bench vt` (run_vt_pass / run_parse_pass), so corpus
# generation and process startup are excluded. They barely move with machine
# load, so a 3% threshold does not flake the way wall-clock time would on a
# shared box. Heap bytes come from the memory bench (`cargo test -p ultty
# --bench memory`), which fails on its own +5% budgets.
#
# Options:
#   --update            write the baseline instead of comparing
#   --threshold PCT     instruction regression threshold (default 3)
#   --bin PATH          use this release ultty-bench instead of building it
#   --baseline PATH     read (or, with --update, write) this baseline file
#   --corpus LIST       default: every corpus
#   --size-mib N        default 1
#   --skip-memory       do not run the heap-bytes gate
#   --out DIR           default target/bench-results/icount
#
# Needs valgrind (apt install valgrind). Counts depend on the compiler and
# its flags, so the baseline records rustc and valgrind versions; a mismatch
# fails with instructions to regenerate rather than comparing apples to pears.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BASELINE="$ROOT/bench/baselines/icount.json"
die() { echo "icount-gate: $*" >&2; exit 1; }

UPDATE=0 THRESHOLD=3 BIN="" CORPUS=all SIZE=1 MEMORY=1 OUT="$ROOT/target/bench-results/icount"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --update) UPDATE=1; shift ;;
    --threshold) THRESHOLD="${2:?}"; shift 2 ;;
    --bin) BIN="${2:?}"; shift 2 ;;
    --baseline) BASELINE="${2:?}"; shift 2 ;;
    --corpus) CORPUS="${2:?}"; shift 2 ;;
    --size-mib) SIZE="${2:?}"; shift 2 ;;
    --skip-memory) MEMORY=0; shift ;;
    --out) OUT="${2:?}"; shift 2 ;;
    *) die "unknown option $1" ;;
  esac
done

command -v valgrind >/dev/null || die "valgrind is not installed (apt install valgrind)"
mkdir -p "$OUT"
if [[ -z "$BIN" ]]; then
  (cd "$ROOT" && cargo build --release --locked -p ultty-bench >&2)
  BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/ultty-bench"
fi
cp "$BIN" "$OUT/ultty-bench"
BIN="$OUT/ultty-bench"

if [[ "$CORPUS" == all ]]; then
  CORPUS="ascii,sgr-dense,unicode-mixed,cjk,emoji-zwj,combining,cursor-redraw,scroll,scroll-region,alt-screen,wide-wrap"
fi
RUSTC_VERSION=$(rustc -V)
VALGRIND_VERSION=$(valgrind --version)

# measure SUITE CORPUS -> instruction count inside the timed function.
measure() {
  local suite="$1" corpus="$2" fn file
  case "$suite" in
    # Exact names: a wildcard would also match the parser's generic
    # instantiations over the pass's closures (`...<ultty_bench::run_vt_pass::
    # {closure#0}>`), and entering one of those toggles collection off again.
    vt) fn='ultty_bench::run_vt_pass' ;;
    parse) fn='ultty_bench::run_parse_pass' ;;
  esac
  file="$OUT/callgrind.$suite.$corpus.out"
  valgrind --tool=callgrind --callgrind-out-file="$file" --toggle-collect="$fn" \
    "$BIN" vt --suite "$suite" --corpus "$corpus" --size-mib "$SIZE" --runs 1 --warmup 0 \
    --geometry 120x40 --out /dev/null >/dev/null 2>"$OUT/valgrind.$suite.$corpus.log" \
    || die "valgrind failed for $suite/$corpus (see $OUT/valgrind.$suite.$corpus.log)"
  awk '/^(summary|totals):/ {print $2; exit}' "$file"
}

CURRENT="$OUT/current.json"
{
  echo "{"
  echo "  \"rustc\": \"$RUSTC_VERSION\","
  echo "  \"valgrind\": \"$VALGRIND_VERSION\","
  echo "  \"size_mib\": $SIZE,"
  echo "  \"geometry\": \"120x40\","
  echo "  \"instructions\": {"
  first=1
  IFS=',' read -r -a corpora <<<"$CORPUS"
  for corpus in "${corpora[@]}"; do
    for suite in parse vt; do
      ir=$(measure "$suite" "$corpus")
      [[ "$ir" =~ ^[0-9]+$ && "$ir" -gt 0 ]] || die "no instruction count for $suite/$corpus: is the binary built with symbols for run_${suite}_pass?"
      [[ $first == 1 ]] || echo ","
      first=0
      printf '    "%s/%s": %s' "$suite" "$corpus" "$ir"
      echo "$suite/$corpus $ir" >&2
    done
  done
  echo
  echo "  }"
  echo "}"
} >"$CURRENT"

if [[ "$UPDATE" == 1 ]]; then
  mkdir -p "$(dirname "$BASELINE")"
  cp "$CURRENT" "$BASELINE"
  echo "icount-gate: baseline written to $BASELINE" >&2
else
  [[ -f "$BASELINE" ]] || die "no baseline at $BASELINE; run with --update first"
  python3 -I - "$BASELINE" "$CURRENT" "$THRESHOLD" <<'PY'
import json, sys
base, cur, threshold = json.load(open(sys.argv[1])), json.load(open(sys.argv[2])), float(sys.argv[3])
for key in ("rustc", "valgrind", "size_mib", "geometry"):
    if base[key] != cur[key]:
        sys.exit(f"icount-gate: baseline {key} is {base[key]!r}, this run has {cur[key]!r}; "
                 "regenerate the baseline with --update on the new toolchain (in its own commit)")
worst, failed = 0.0, []
print(f"{'case':<28} {'baseline':>14} {'current':>14} {'change':>8}")
for case, b in sorted(base["instructions"].items()):
    c = cur["instructions"].get(case)
    if c is None:
        failed.append(f"{case}: missing from this run")
        continue
    change = (c - b) / b * 100
    worst = max(worst, change)
    flag = "  REGRESSED" if change > threshold else ""
    print(f"{case:<28} {b:>14} {c:>14} {change:>+7.2f}%{flag}")
    if change > threshold:
        failed.append(f"{case}: {change:+.2f}% instructions (threshold +{threshold}%)")
if failed:
    print("\nicount-gate FAILED:\n  " + "\n  ".join(failed))
    sys.exit(1)
print(f"\nicount-gate passed (worst change {worst:+.2f}%, threshold +{threshold}%)")
PY
fi

if [[ "$MEMORY" == 1 ]]; then
  (cd "$ROOT" && cargo test --locked -p ultty --bench memory)
fi
