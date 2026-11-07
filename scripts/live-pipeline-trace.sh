#!/usr/bin/env bash
# Capture opt-in live pipeline timing while running the stress smoke.
#
# The script builds release binaries, launches a private Ultty (its own
# XDG_RUNTIME_DIR, so it never talks to or collides with a running Ultty)
# with `ULTTY_TRACE_PIPELINE=1`, runs selected live stress workloads
# through ulttyctl, and writes a trace log and stress report under the
# chosen output directory.
#
# Trace lines are only emitted at `ultty::perf=info`; the default RUST_LOG
# below includes it. With RUST_LOG=warn the trace is empty and the script
# fails rather than reporting nothing.
#
# The result is live evidence for whatever display it ran on. Under Xvfb
# with lavapipe it is `live-swrast` (correctness and relative CPU-side
# work only); see docs/perf/README.md for what each claim class supports.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT_DIR="${1:-target/bench-results/live-pipeline-trace}"
RUNS="${ULTTY_TRACE_RUNS:-2}"
TESTS="${ULTTY_TRACE_TESTS:-1,6,8}"

mkdir -p "$OUT_DIR/bin"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

TRACE_LOG="$OUT_DIR/pipeline.log"
STRESS_REPORT="$OUT_DIR/stress-report.md"
SUMMARY="$OUT_DIR/pipeline-summary.md"
SUMMARY_JSON="$OUT_DIR/pipeline-summary.json"
GATE_REPORT="$OUT_DIR/pipeline-gate.md"
MAX_PARSE_MS="${ULTTY_TRACE_MAX_PARSE_MS:-900}"
MAX_DRAIN_MS="${ULTTY_TRACE_MAX_DRAIN_MS:-1000}"
MAX_RENDER_TOTAL_MS="${ULTTY_TRACE_MAX_RENDER_TOTAL_MS:-400}"
MAX_RENDER_FRAME_MS="${ULTTY_TRACE_MAX_RENDER_FRAME_MS:-50}"

# Release build (opt-level 3, LTO). Copy the binaries so a concurrent build
# into a shared target dir cannot replace them mid-run.
cargo build --release -p ultty --bins
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cp "$TARGET_DIR/release/ultty" "$TARGET_DIR/release/ulttyctl" "$OUT_DIR/bin/"
ULTTYCTL="$OUT_DIR/bin/ulttyctl"
export ULTTYCTL

RUNTIME_DIR="$(mktemp -d /tmp/ultty-trace.XXXXXX)"
chmod 700 "$RUNTIME_DIR"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
unset TMUX

RUST_LOG="${RUST_LOG:-ultty=info,ultty::perf=info,wgpu=warn}" \
ULTTY_TRACE_PIPELINE=1 \
"$OUT_DIR/bin/ultty" >"$TRACE_LOG" 2>&1 &
ULTTY_PID=$!

cleanup() {
  if kill -0 "$ULTTY_PID" >/dev/null 2>&1; then
    kill "$ULTTY_PID" >/dev/null 2>&1 || true
    wait "$ULTTY_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$RUNTIME_DIR"
}
trap cleanup EXIT

for _ in $(seq 1 100); do
  if "$ULTTYCTL" ping >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  echo "Ultty control socket did not become ready." >&2
  echo "Trace log: $TRACE_LOG" >&2
  exit 1
fi

ULTTY_PERF_SMOKE_RUNS="$RUNS" \
ULTTY_PERF_SMOKE_TESTS="$TESTS" \
ULTTY_PERF_SMOKE_TIMEOUT_SECS="${ULTTY_TRACE_TIMEOUT_SECS:-180}" \
scripts/perf-smoke.sh "$STRESS_REPORT"

if ! grep -q "pipeline_pty_drain" "$TRACE_LOG"; then
  echo "trace missing pipeline_pty_drain entries: $TRACE_LOG" >&2
  exit 1
fi

if ! grep -q "pipeline_parse_grid" "$TRACE_LOG"; then
  echo "trace missing pipeline_parse_grid entries: $TRACE_LOG" >&2
  exit 1
fi

if ! grep -q "pipeline_render_frame" "$TRACE_LOG"; then
  echo "trace missing pipeline_render_frame entries: $TRACE_LOG" >&2
  exit 1
fi

awk \
  -v json="$SUMMARY_JSON" \
  -v gate="$GATE_REPORT" \
  -v max_parse_ms="$MAX_PARSE_MS" \
  -v max_drain_ms="$MAX_DRAIN_MS" \
  -v max_render_total_ms="$MAX_RENDER_TOTAL_MS" \
  -v max_render_frame_ms="$MAX_RENDER_FRAME_MS" '
  /pipeline_parse_grid/ {
    for (i = 1; i <= NF; i++) {
      if ($i ~ /^bytes=/) { split($i, a, "="); parse_bytes += a[2] }
      if ($i ~ /^actions=/) { split($i, a, "="); actions += a[2] }
      if ($i ~ /^print_actions=/) { split($i, a, "="); print_actions += a[2] }
      if ($i ~ /^elapsed_us=/) { split($i, a, "="); parse_us += a[2]; parse_entries++ }
    }
  }
  /pipeline_pty_drain/ {
    for (i = 1; i <= NF; i++) {
      if ($i ~ /^read_batches=/) { split($i, a, "="); read_batches += a[2] }
      if ($i ~ /^read_bytes=/) { split($i, a, "="); read_bytes += a[2] }
      if ($i ~ /^elapsed_us=/) { split($i, a, "="); drain_us += a[2]; drain_entries++ }
    }
  }
  /pipeline_render_scheduled/ { render_scheduled++ }
  /pipeline_render_frame/ {
    for (i = 1; i <= NF; i++) {
      if ($i ~ /^elapsed_us=/) {
        split($i, a, "=")
        render_us += a[2]
        render_frames++
        if (a[2] > max_render_us) max_render_us = a[2]
      }
    }
  }
  END {
    parse_ms = parse_us / 1000
    drain_ms = drain_us / 1000
    render_ms = render_us / 1000
    max_render_ms = max_render_us / 1000
    if (parse_us > render_us * 4) {
      dominant = "parser/grid and PTY drain"
    } else if (render_us > parse_us) {
      dominant = "rendering"
    } else {
      dominant = "mixed"
    }

    failed = 0
    if (parse_ms > max_parse_ms) failed = 1
    if (drain_ms > max_drain_ms) failed = 1
    if (render_ms > max_render_total_ms) failed = 1
    if (max_render_ms > max_render_frame_ms) failed = 1
    # Nothing measured is a failure, not a pass with zeros.
    if (parse_bytes == 0 || parse_entries == 0 || render_frames == 0) failed = 1

    print "# Ultty Live Pipeline Trace Summary"
    print ""
    print "| Metric | Value |"
    print "|--------|-------|"
    printf "| Parser/grid entries | %d |\n", parse_entries
    printf "| Parser/grid bytes | %d |\n", parse_bytes
    printf "| Parser/grid actions | %d |\n", actions
    printf "| Parser/grid print actions | %d |\n", print_actions
    printf "| Parser/grid time | %.3f ms |\n", parse_ms
    printf "| PTY drain entries | %d |\n", drain_entries
    printf "| PTY read batches | %d |\n", read_batches
    printf "| PTY drain bytes | %d |\n", read_bytes
    printf "| PTY drain time | %.3f ms |\n", drain_ms
    printf "| Render scheduled | %d |\n", render_scheduled
    printf "| Render frames | %d |\n", render_frames
    printf "| Render frame time | %.3f ms |\n", render_ms
    printf "| Max render frame | %.3f ms |\n", max_render_ms
    print ""
    printf "Dominant cost: %s.\n", dominant

    print "# Ultty Live Pipeline Gate" > gate
    print "" >> gate
    printf "Status: %s\n\n", failed ? "FAIL" : "PASS" >> gate
    print "| Metric | Value | Limit |" >> gate
    print "|--------|-------|-------|" >> gate
    printf "| Parser/grid time | %.3f ms | %.3f ms |\n", parse_ms, max_parse_ms >> gate
    printf "| PTY drain time | %.3f ms | %.3f ms |\n", drain_ms, max_drain_ms >> gate
    printf "| Render frame time | %.3f ms | %.3f ms |\n", render_ms, max_render_total_ms >> gate
    printf "| Max render frame | %.3f ms | %.3f ms |\n", max_render_ms, max_render_frame_ms >> gate

    print "{" > json
    printf "  \"status\": \"%s\",\n", failed ? "fail" : "pass" >> json
    printf "  \"dominant_cost\": \"%s\",\n", dominant >> json
    printf "  \"parser_grid_entries\": %d,\n", parse_entries >> json
    printf "  \"parser_grid_bytes\": %d,\n", parse_bytes >> json
    printf "  \"parser_grid_actions\": %d,\n", actions >> json
    printf "  \"parser_grid_print_actions\": %d,\n", print_actions >> json
    printf "  \"parser_grid_ms\": %.3f,\n", parse_ms >> json
    printf "  \"pty_drain_entries\": %d,\n", drain_entries >> json
    printf "  \"pty_read_batches\": %d,\n", read_batches >> json
    printf "  \"pty_drain_bytes\": %d,\n", read_bytes >> json
    printf "  \"pty_drain_ms\": %.3f,\n", drain_ms >> json
    printf "  \"render_scheduled\": %d,\n", render_scheduled >> json
    printf "  \"render_frames\": %d,\n", render_frames >> json
    printf "  \"render_frame_ms\": %.3f,\n", render_ms >> json
    printf "  \"max_render_frame_ms\": %.3f,\n", max_render_ms >> json
    print "  \"thresholds\": {" >> json
    printf "    \"parser_grid_ms\": %.3f,\n", max_parse_ms >> json
    printf "    \"pty_drain_ms\": %.3f,\n", max_drain_ms >> json
    printf "    \"render_frame_ms\": %.3f,\n", max_render_total_ms >> json
    printf "    \"max_render_frame_ms\": %.3f\n", max_render_frame_ms >> json
    print "  }" >> json
    print "}" >> json

    if (failed) {
      print "live pipeline gate failed: " gate > "/dev/stderr"
      exit 2
    }
  }
' "$TRACE_LOG" > "$SUMMARY"

echo "live pipeline trace captured:"
echo "  trace:  $TRACE_LOG"
echo "  stress: $STRESS_REPORT"
echo "  summary: $SUMMARY"
echo "  json:   $SUMMARY_JSON"
echo "  gate:   $GATE_REPORT"
