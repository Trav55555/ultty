#!/usr/bin/env bash
# Run Ultty headless under Xvfb with Mesa's software Vulkan (lavapipe) and
# put it through the live smokes. Used by the CI "GUI smoke" job; runs the
# same way locally on Linux with xvfb and mesa-vulkan-drivers installed.
#
# Usage: scripts/ci-gui-smoke.sh [artifact-dir]
# Expects debug builds of ultty, ulttyctl, hello-native, counter-app, and
# agent-transcript.
set -euo pipefail

OUT="${1:-/tmp/ultty-ci-gui-smoke}"
mkdir -p "$OUT"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DISPLAY_NUM="${GUI_SMOKE_DISPLAY:-:99}"
LVP_ICD="${GUI_SMOKE_VK_ICD:-$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1)}"
[ -n "$LVP_ICD" ] || { echo "no lavapipe ICD found (install mesa-vulkan-drivers)" >&2; exit 2; }

# Unix socket paths are short (108 bytes), so the runtime dir must be too.
RUNTIME_DIR="$(mktemp -d /tmp/ultty-gui.XXXXXX)"
chmod 700 "$RUNTIME_DIR"
CONFIG_DIR="$OUT/config"
mkdir -p "$CONFIG_DIR"

XVFB_PID=""
ULTTY_PID=""
cleanup() {
  [ -n "$ULTTY_PID" ] && kill "$ULTTY_PID" 2>/dev/null || true
  [ -n "$XVFB_PID" ] && kill "$XVFB_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$RUNTIME_DIR"
}
trap cleanup EXIT

Xvfb "$DISPLAY_NUM" -screen 0 1280x800x24 -nolisten tcp >"$OUT/xvfb.log" 2>&1 &
XVFB_PID=$!
sleep 1

export DISPLAY="$DISPLAY_NUM"
unset WAYLAND_DISPLAY
export VK_ICD_FILENAMES="$LVP_ICD"
export XDG_RUNTIME_DIR="$RUNTIME_DIR"
export XDG_CONFIG_HOME="$CONFIG_DIR"
export ULTTYCTL="$ROOT/target/debug/ulttyctl"

RUST_LOG="${RUST_LOG:-info,wgpu_core=warn,wgpu_hal=warn}" target/debug/ultty >"$OUT/ultty.log" 2>&1 &
ULTTY_PID=$!
for _ in $(seq 1 120); do
  "$ULTTYCTL" ping >/dev/null 2>&1 && break
  if ! kill -0 "$ULTTY_PID" 2>/dev/null; then
    echo "Ultty exited during startup:" >&2
    tail -40 "$OUT/ultty.log" >&2
    exit 1
  fi
  sleep 0.5
done
"$ULTTYCTL" ping >/dev/null || { echo "Ultty never answered ulttyctl ping" >&2; tail -40 "$OUT/ultty.log" >&2; exit 1; }

scripts/live-smoke-gate.sh "$OUT/gate"
scripts/terminal-workflow-smoke.sh "$OUT/terminal-workflow"
scripts/regression-smoke.sh "$OUT/regression"
# A second Ultty on its own display, with the accessibility adapter and no
# session bus.
ULTTY="$ROOT/target/debug/ultty" scripts/e2e/accessibility-no-bus.sh "$OUT/accessibility"
# Instances stopped by signals, and one sweeping a killed instance's files,
# on a display and runtime dir of their own.
ULTTY="$ROOT/target/debug/ultty" scripts/e2e/runtime-file-cleanup.sh "$OUT/runtime-file-cleanup"
# A third, for an inline surface over scrolling output (ADR 0020). It turns
# the mouse wheel with xdotool; runners without it skip the check, loudly.
if command -v xdotool >/dev/null 2>&1; then
  ULTTY="$ROOT/target/debug/ultty" ULTTYCTL="$ULTTYCTL" \
    scripts/e2e/inline-surface.sh "$OUT/inline-surface"
else
  echo "SKIPPED: scripts/e2e/inline-surface.sh needs xdotool" >&2
fi
# A fourth, for the agent transcript example (plan item B3): cards in the
# scrollback in Ultty, and as ANSI text under tmux. Without tmux the script
# skips that part, loudly.
ULTTY="$ROOT/target/debug/ultty" ULTTYCTL="$ULTTYCTL" \
  scripts/e2e/agent-transcript.sh "$OUT/agent-transcript"

# Catch panics that did not take the process down.
if grep -q "panicked at" "$OUT/ultty.log"; then
  echo "Ultty logged a panic:" >&2
  grep -A5 "panicked at" "$OUT/ultty.log" >&2
  exit 1
fi
echo "GUI smoke passed: $OUT"
