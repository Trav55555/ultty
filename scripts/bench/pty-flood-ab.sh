#!/usr/bin/env bash
# Alternating A/B rounds of the pty_flood bench between two prebuilt bench
# binaries, for a quiet machine (see docs/perf/2026-11-11-pty-flood-quiet-rerun.md).
#
#   pty-flood-ab.sh OUT_DIR BIN_A BIN_B [ROUNDS_PER_BATCH [BATCHES]]
#
# Each round runs every case once per arm, the arm order alternating by round
# and by batch, with criterion's 10 samples per run (--warm-up-time 1
# --measurement-time 3). One discarded trial round comes first (batch 0).
# Per run it keeps criterion's sample.json (the raw per-sample times, which
# show whether run times sit on the 16 ms frame grid) and the 1-minute load.
# Output: OUT_DIR/runs.tsv, one row per (batch, round, arm, case) with the
# median sample time in ns, plus OUT_DIR/raw/<batch>-<round>-<arm>-<case>.json.
set -euo pipefail

OUT="${1:?out dir}" BIN_A="${2:?arm A binary}" BIN_B="${3:?arm B binary}"
ROUNDS="${4:-15}" BATCHES="${5:-3}"
CASES=(ascii cjk sgr-dense)
SIZES=(120x40 300x75)

mkdir -p "$OUT/raw"
printf 'batch\tround\tarm\tcase\tsize\tload1\tmedian_ns\n' >"$OUT/runs.tsv"

run_arm() { # batch round arm binary
  local batch=$1 round=$2 arm=$3 bin=$4 corpus size home
  for corpus in "${CASES[@]}"; do
    home="$OUT/crit-$arm"
    rm -rf "$home"
    CRITERION_HOME="$home" PTY_FLOOD_CORPUS="$corpus" "$bin" --bench \
      --warm-up-time 1 --measurement-time 3 >"$OUT/last-$arm.log" 2>&1 || {
      cat "$OUT/last-$arm.log" >&2
      exit 1
    }
    for size in "${SIZES[@]}"; do
      local sample="$home/pty_flood/${corpus}_${size}/new/sample.json"
      [[ -f "$sample" ]] || { echo "missing $sample" >&2; exit 1; }
      cp "$sample" "$OUT/raw/$batch-$round-$arm-$corpus-$size.json"
      python3 -I - "$sample" "$batch" "$round" "$arm" "$corpus" "$size" "$(cut -d' ' -f1 /proc/loadavg)" >>"$OUT/runs.tsv" <<'PY'
import json, statistics, sys
s = json.load(open(sys.argv[1]))
# sample.json holds each sample's total time and its iteration count.
per = sorted(t / i for t, i in zip(s["times"], s["iters"]))
print("\t".join([*sys.argv[2:8], f"{statistics.median(per):.0f}"]))
PY
    done
  done
}

for ((batch = 0; batch <= BATCHES; batch++)); do
  n=$ROUNDS
  ((batch == 0)) && n=1
  for ((round = 1; round <= n; round++)); do
    if (((batch + round) % 2 == 0)); then
      run_arm "$batch" "$round" A "$BIN_A"
      run_arm "$batch" "$round" B "$BIN_B"
    else
      run_arm "$batch" "$round" B "$BIN_B"
      run_arm "$batch" "$round" A "$BIN_A"
    fi
    echo "batch $batch round $round done $(date +%T) load $(cut -d' ' -f1 /proc/loadavg)" >&2
  done
done
