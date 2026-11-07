#!/usr/bin/env bash
# Interleaved headless benchmark runs with A/A and A/B support.
#
#   scripts/bench/run.sh vt [options] [-- ultty-bench vt options]
#
# Options:
#   --ab REV_A REV_B   build both revisions (git worktrees, release) and compare
#   --aa               run the current checkout as both arms (noise floor)
#   (neither)          one arm, the current checkout
#   --bin PATH         use this ultty-bench binary instead of building HEAD
#   --runs N           blocks (default 10); each block runs every arm once,
#                      in a random order
#   --cpu LIST         taskset CPU list (default: the last core and its SMT sibling)
#   --out DIR          session directory (default target/bench-results/<date>-vt)
#   --allow-noisy      run even when the 1-minute load is above 12
#
# Results: DIR/results.jsonl (one object per sample), DIR/summary.md and
# DIR/summary.json from analyze.py. Builds go under target/bench-build/ on
# disk, never /tmp (tmpfs on this box).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PY=(/usr/bin/env python3 -I)

die() { echo "run.sh: $*" >&2; exit 1; }

[[ "${1:-}" == "vt" ]] || die "usage: $0 vt [--ab A B | --aa] [--runs N] [--cpu LIST] [--out DIR] [-- args]"
shift

MODE=single REV_A="" REV_B="" BIN="" RUNS=10 CPU="" OUT="" NOISY=0
EXTRA=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ab) MODE=ab; REV_A="${2:?}"; REV_B="${3:?}"; shift 3 ;;
    --aa) MODE=aa; shift ;;
    --bin) BIN="${2:?}"; shift 2 ;;
    --runs) RUNS="${2:?}"; shift 2 ;;
    --cpu) CPU="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    --allow-noisy) NOISY=1; shift ;;
    --) shift; EXTRA=("$@"); break ;;
    *) die "unknown option $1" ;;
  esac
done

OUT="${OUT:-$ROOT/target/bench-results/$(date +%Y-%m-%d-%H%M%S)-vt}"
mkdir -p "$OUT"
RESULTS="$OUT/results.jsonl"
: >"$RESULTS"

if [[ -z "$CPU" ]]; then
  last=$(( $(nproc --all) - 1 ))
  CPU=$(cat "/sys/devices/system/cpu/cpu$last/topology/thread_siblings_list" 2>/dev/null || echo "$last")
fi

load=$(cut -d' ' -f1 /proc/loadavg)
if awk -v l="$load" 'BEGIN { exit !(l > 12) }'; then
  if [[ "$NOISY" == 1 ]]; then
    echo "run.sh: load $load > 12; results will be marked noisy" >&2
  else
    die "load $load > 12; wall-clock A/B is not credible now (use --allow-noisy to run anyway)"
  fi
fi

# build REV -> prints the binary path. REV "HEAD-worktree" means the checkout as is.
build() {
  local rev="$1" src target sha
  if [[ "$rev" == "HEAD-worktree" ]]; then
    src="$ROOT"
    target="${CARGO_TARGET_DIR:-$ROOT/target}"
  else
    sha=$(git -C "$ROOT" rev-parse --verify "$rev^{commit}") || die "unknown revision $rev"
    src="$ROOT/target/bench-build/src-${sha:0:12}"
    target="$ROOT/target/bench-build/target-${sha:0:12}"
    if [[ ! -d "$src" ]]; then
      git -C "$ROOT" worktree add --detach "$src" "$sha" >&2
    fi
  fi
  (cd "$src" && CARGO_TARGET_DIR="$target" cargo build --release -p ultty-bench --locked >&2)
  echo "$target/release/ultty-bench"
}

rev_of() {
  if [[ "$1" == "HEAD-worktree" ]]; then
    git -C "$ROOT" rev-parse --short=12 HEAD
  else
    git -C "$ROOT" rev-parse --short=12 "$1^{commit}"
  fi
}
dirty_of() {
  if [[ "$1" == "HEAD-worktree" ]] && [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]]; then
    echo 1
  else
    echo 0
  fi
}

declare -A ARM_BIN ARM_REV ARM_DIRTY
case "$MODE" in
  ab)
    ARM_BIN[A]=$(build "$REV_A"); ARM_REV[A]=$(rev_of "$REV_A"); ARM_DIRTY[A]=0
    ARM_BIN[B]=$(build "$REV_B"); ARM_REV[B]=$(rev_of "$REV_B"); ARM_DIRTY[B]=0
    ;;
  aa|single)
    bin="${BIN:-$(build HEAD-worktree)}"
    # Copy so a concurrent build into a shared target dir cannot swap it mid-run.
    cp "$bin" "$OUT/ultty-bench"
    ARM_BIN[A]="$OUT/ultty-bench"; ARM_REV[A]=$(rev_of HEAD-worktree); ARM_DIRTY[A]=$(dirty_of HEAD-worktree)
    if [[ "$MODE" == aa ]]; then
      ARM_BIN[B]="$OUT/ultty-bench"; ARM_REV[B]="${ARM_REV[A]}"; ARM_DIRTY[B]="${ARM_DIRTY[A]}"
    fi
    ;;
esac
for arm in "${!ARM_BIN[@]}"; do
  [[ -x "${ARM_BIN[$arm]}" ]] || die "arm $arm: no binary at ${ARM_BIN[$arm]}"
done
arms=("${!ARM_BIN[@]}")

echo "session: $OUT (cpu $CPU, load $load, $RUNS blocks, arms ${arms[*]})" >&2
for ((block = 0; block < RUNS; block++)); do
  # Random arm order per block (randomized complete blocks).
  mapfile -t order < <(printf '%s\n' "${arms[@]}" | shuf)
  for arm in "${order[@]}"; do
    ULTTY_BENCH_REV="${ARM_REV[$arm]}" ULTTY_BENCH_DIRTY="${ARM_DIRTY[$arm]}" \
      taskset -c "$CPU" "${ARM_BIN[$arm]}" vt --runs 1 --block-start "$block" \
      --arm "$arm" --out "$RESULTS" "${EXTRA[@]}" 2>>"$OUT/console.log"
  done
done

[[ -s "$RESULTS" ]] || die "no samples were written to $RESULTS"

analyze=("${PY[@]}" "$ROOT/scripts/bench/analyze.py" "$RESULTS" --markdown --json "$OUT/summary.json")
if [[ "$MODE" == aa ]]; then analyze+=(--aa); fi
"${analyze[@]}" | tee "$OUT/summary.md"
echo "results: $RESULTS" >&2
