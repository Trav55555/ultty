#!/usr/bin/env bash
# Diagnose long soft-wrap wall-clock time by comparing generator-only output,
# live child timing, and app-side pipeline totals from the traced Ultty run.

set -euo pipefail

OUT_DIR="${1:-/tmp/ultty-softwrap-pty-probe}"
RUNS="${ULTTY_SOFTWRAP_PROBE_RUNS:-2}"

mkdir -p "$OUT_DIR"

GENERATOR_LOG="$OUT_DIR/generator.log"
LIVE_DIR="$OUT_DIR/live"
REPORT="$OUT_DIR/softwrap-pty-probe.md"

ROOT="$(git rev-parse --show-toplevel)"

: > "$GENERATOR_LOG"

for run in $(seq 1 "$RUNS"); do
  log="$OUT_DIR/generator-$run.log"
  ULTTY_STRESS_LOG="$log" bash "$ROOT/scripts/stress.sh" 6 >/dev/null 2>"$OUT_DIR/generator-$run.stderr"
  ms="$(awk '/6\. 500-lines-4000-cols/ { print $(NF - 1) }' "$log")"
  if [[ -z "$ms" ]]; then
    echo "missing generator timing in $log" >&2
    exit 1
  fi
  echo "$ms" >> "$GENERATOR_LOG"
done

generator_mean="$(awk '{ sum += $1; count++ } END { printf "%.1f", sum / count }' "$GENERATOR_LOG")"

ULTTY_TRACE_RUNS="$RUNS" \
ULTTY_TRACE_TESTS=6 \
"$ROOT/scripts/live-pipeline-trace.sh" "$LIVE_DIR"

live_mean="$(
  awk -F'|' '/`6\. 500-lines-4000-cols`/ {
    gsub(/^[ \t]+|[ \t]+$/, "", $4)
    sub(/ ms$/, "", $4)
    print $4
  }' "$LIVE_DIR/stress-report.md"
)"

parser_ms="$(jq -r '.parser_grid_ms' "$LIVE_DIR/pipeline-summary.json")"
drain_ms="$(jq -r '.pty_drain_ms' "$LIVE_DIR/pipeline-summary.json")"
render_ms="$(jq -r '.render_frame_ms' "$LIVE_DIR/pipeline-summary.json")"
max_render_ms="$(jq -r '.max_render_frame_ms' "$LIVE_DIR/pipeline-summary.json")"
read_batches="$(jq -r '.pty_read_batches' "$LIVE_DIR/pipeline-summary.json")"
render_frames="$(jq -r '.render_frames' "$LIVE_DIR/pipeline-summary.json")"

awk \
  -v runs="$RUNS" \
  -v generator_mean="$generator_mean" \
  -v live_mean="$live_mean" \
  -v parser_ms="$parser_ms" \
  -v drain_ms="$drain_ms" \
  -v render_ms="$render_ms" \
  -v max_render_ms="$max_render_ms" \
  -v read_batches="$read_batches" \
  -v render_frames="$render_frames" \
  -v report="$REPORT" '
  BEGIN {
    parser_mean = parser_ms / runs
    drain_mean = drain_ms / runs
    render_mean = render_ms / runs
    app_mean = parser_mean + drain_mean + render_mean
    generator_gap = live_mean - generator_mean
    unaccounted = live_mean - app_mean

    print "# Soft-Wrap PTY Probe" > report
    print "" >> report
    print "| Field | Value |" >> report
    print "|-------|-------|" >> report
    printf "| Runs | %d |\n", runs >> report
    printf "| Generator-only mean | %.1f ms |\n", generator_mean >> report
    printf "| Live child mean | %.1f ms |\n", live_mean >> report
    printf "| Live minus generator estimate | %.1f ms |\n", generator_gap >> report
    print "" >> report
    print "## App-Side Pipeline Mean Per Run" >> report
    print "" >> report
    print "| Component | Mean |" >> report
    print "|-----------|------|" >> report
    printf "| Parser/grid | %.1f ms |\n", parser_mean >> report
    printf "| PTY drain | %.1f ms |\n", drain_mean >> report
    printf "| Render frames | %.1f ms |\n", render_mean >> report
    printf "| Combined measured app-side work | %.1f ms |\n", app_mean >> report
    printf "| Live minus measured app-side estimate | %.1f ms |\n", unaccounted >> report
    print "" >> report
    print "## Scheduling Counters" >> report
    print "" >> report
    print "| Counter | Value |" >> report
    print "|---------|-------|" >> report
    printf "| PTY read batches | %d |\n", read_batches >> report
    printf "| Render frames | %d |\n", render_frames >> report
    printf "| Max render frame | %.3f ms |\n", max_render_ms >> report
    print "" >> report
    print "## Interpretation" >> report
    print "" >> report
    print "The generator-only timing is the lower bound for producing the long-line" >> report
    print "payload when stdout is redirected. The live child mean includes time spent" >> report
    print "blocked on the PTY while Ultty drains and renders. The app-side totals come" >> report
    print "from the pipeline trace and are divided by the number of live runs." >> report
  }
'

echo "soft-wrap PTY probe captured:"
echo "  report: $REPORT"
echo "  live:   $LIVE_DIR"
