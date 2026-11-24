#!/usr/bin/env bash
# Run Ultty headless under Xvfb with Mesa's software Vulkan (lavapipe) and
# put it through the live smokes and isolated E2E suites. Used by the CI
# "GUI smoke" job; runs the same way locally on Linux with xvfb and
# mesa-vulkan-drivers installed.
#
# Usage: scripts/ci-gui-smoke.sh [--shard all|core|native-lua|sessions] [--jobs N] [artifact-dir]
# Expects debug builds of ultty, ulttyctl, ultty-session, hello-native,
# counter-app, agent-transcript, agent-console, ulttui-lua, and the
# ultty-client inline-surface example.
set -euo pipefail

SHARD="all"
JOBS="${GUI_SMOKE_JOBS:-2}"
OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --shard) SHARD="${2:?}"; shift 2 ;;
    --jobs) JOBS="${2:?}"; shift 2 ;;
    -h|--help)
      echo "Usage: $0 [--shard all|core|native-lua|sessions] [--jobs N] [artifact-dir]"
      exit 0
      ;;
    -*) echo "ci-gui-smoke: unknown option: $1" >&2; exit 2 ;;
    *)
      [ -z "$OUT" ] || { echo "ci-gui-smoke: unexpected argument: $1" >&2; exit 2; }
      OUT="$1"
      shift
      ;;
  esac
done

case "$SHARD" in
  all|core|native-lua|sessions) ;;
  *) echo "ci-gui-smoke: invalid --shard '$SHARD' (expected all, core, native-lua, or sessions)" >&2; exit 2 ;;
esac
[[ "$JOBS" =~ ^[1-9][0-9]*$ ]] || { echo "ci-gui-smoke: --jobs must be a positive integer (got $JOBS)" >&2; exit 2; }

OUT="${OUT:-/tmp/ultty-ci-gui-smoke}"
mkdir -p "$OUT/logs" "$OUT/results"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/debug"

LVP_ICD="${GUI_SMOKE_VK_ICD:-$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1)}"
[ -n "$LVP_ICD" ] || { echo "no lavapipe ICD found (install mesa-vulkan-drivers)" >&2; exit 2; }

unset WAYLAND_DISPLAY XAUTHORITY
export VK_ICD_FILENAMES="$LVP_ICD"

ULTTY_BIN="$BIN_DIR/ultty"
ULTTYCTL_BIN="$BIN_DIR/ulttyctl"
ULTTY_SESSION_BIN="$BIN_DIR/ultty-session"
COUNTER_BIN="$BIN_DIR/counter-app"
AGENT_TRANSCRIPT_BIN="$BIN_DIR/agent-transcript"
AGENT_CONSOLE_BIN="$BIN_DIR/agent-console"
ULTTUI_LUA_BIN="$BIN_DIR/ulttui-lua"
INLINE_APP_BIN="$BIN_DIR/examples/inline-surface"
FRAME_APP_BIN="$BIN_DIR/examples/session-frame-fixture"

XVFB_PID=""
ULTTY_PID=""
RUNTIME_DIR=""
CHILD_PIDS=()

stop_shared_ultty() {
  if [ -n "$ULTTY_PID" ]; then
    kill "$ULTTY_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$ULTTY_PID" 2>/dev/null || break
      sleep 0.1
    done
    wait "$ULTTY_PID" 2>/dev/null || true
    ULTTY_PID=""
  fi
  if [ -n "$XVFB_PID" ]; then
    kill "$XVFB_PID" 2>/dev/null || true
    wait "$XVFB_PID" 2>/dev/null || true
    XVFB_PID=""
  fi
  if [ -n "$RUNTIME_DIR" ]; then
    rm -rf "$RUNTIME_DIR"
    RUNTIME_DIR=""
  fi
}

cleanup() {
  local pid
  for pid in "${CHILD_PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  stop_shared_ultty
  wait 2>/dev/null || true
}
trap cleanup EXIT

CHECK_INDEX=0
record_result() {
  local idx="$1" name="$2" status="$3" secs="$4" note="${5:-}"
  printf '%s\t%s\t%s\t%s\n' "$name" "$status" "$secs" "$note" >"$OUT/results/$(printf '%03d' "$idx")-$name.tsv"
}

run_sync_check() {
  local name="$1"
  shift
  CHECK_INDEX=$((CHECK_INDEX + 1))
  local idx="$CHECK_INDEX"
  local log="$OUT/logs/$name.log"
  local t0 t1 secs
  t0="$(date +%s)"
  echo "==> [$name] starting"
  if "$@" >"$log" 2>&1; then
    t1="$(date +%s)"
    secs=$((t1 - t0))
    echo "PASS [$name] (${secs}s)"
    record_result "$idx" "$name" "PASS" "$secs" ""
  else
    t1="$(date +%s)"
    secs=$((t1 - t0))
    echo "FAIL [$name] (${secs}s)" >&2
    tail -40 "$log" >&2 || true
    record_result "$idx" "$name" "FAIL" "$secs" "see logs/$name.log"
  fi
}

# Phase 1: shared-instance smokes on a dedicated Xvfb display.
if [ "$SHARD" = all ] || [ "$SHARD" = core ]; then
  # Unix socket paths are short (108 bytes), so the runtime dir must be too.
  RUNTIME_DIR="$(mktemp -d /tmp/ultty-gui.XXXXXX)"
  chmod 700 "$RUNTIME_DIR"
  CONFIG_DIR="$OUT/config"
  mkdir -p "$CONFIG_DIR"

  if [ -n "${GUI_SMOKE_DISPLAY:-}" ]; then
    DISPLAY_NUM="$GUI_SMOKE_DISPLAY"
    Xvfb "$DISPLAY_NUM" -screen 0 1280x800x24 -nolisten tcp >"$OUT/xvfb.log" 2>&1 &
    XVFB_PID=$!
    sleep 1
  else
    rm -f "$OUT/display"
    Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$OUT/display" 2>"$OUT/xvfb.log" &
    XVFB_PID=$!
    for _ in $(seq 1 50); do [ -s "$OUT/display" ] && break; sleep 0.1; done
    [ -s "$OUT/display" ] || { echo "Xvfb did not start" >&2; cat "$OUT/xvfb.log" >&2; exit 1; }
    DISPLAY_NUM=":$(cat "$OUT/display")"
  fi

  (
    export DISPLAY="$DISPLAY_NUM"
    export XDG_RUNTIME_DIR="$RUNTIME_DIR"
    export XDG_CONFIG_HOME="$CONFIG_DIR"
    export ULTTYCTL="$ULTTYCTL_BIN"

    RUST_LOG="${RUST_LOG:-info,wgpu_core=warn,wgpu_hal=warn}" "$ULTTY_BIN" >"$OUT/ultty.log" 2>&1 &
    shared_ultty_pid=$!
    echo "$shared_ultty_pid" >"$OUT/shared-ultty.pid"
    for _ in $(seq 1 120); do
      "$ULTTYCTL" ping >/dev/null 2>&1 && break
      if ! kill -0 "$shared_ultty_pid" 2>/dev/null; then
        echo "Ultty exited during startup:" >&2
        tail -40 "$OUT/ultty.log" >&2
        exit 1
      fi
      sleep 0.5
    done
    "$ULTTYCTL" ping >/dev/null || { echo "Ultty never answered ulttyctl ping" >&2; tail -40 "$OUT/ultty.log" >&2; exit 1; }
    # With async GPU startup (ADR 0030), ulttyctl ping succeeds as soon as the
    # control socket binds, before the wgpu renderer has drawn its first frame.
    # Wait for the first screenshot to succeed so live-smoke-gate does not race
    # the GPU startup thread.
    gpu_ready=0
    for _ in $(seq 1 120); do
      if "$ULTTYCTL" screenshot "$OUT/startup-ready.png" >/dev/null 2>&1; then
        gpu_ready=1
        break
      fi
      if ! kill -0 "$shared_ultty_pid" 2>/dev/null; then
        echo "Ultty exited while waiting for GPU startup:" >&2
        tail -40 "$OUT/ultty.log" >&2
        exit 1
      fi
      sleep 0.25
    done
    rm -f "$OUT/startup-ready.png"
    [ "$gpu_ready" -eq 1 ] || { echo "Ultty GPU never became ready for screenshots" >&2; tail -40 "$OUT/ultty.log" >&2; exit 1; }
  )
  ULTTY_PID="$(cat "$OUT/shared-ultty.pid")"

  DISPLAY="$DISPLAY_NUM" XDG_RUNTIME_DIR="$RUNTIME_DIR" XDG_CONFIG_HOME="$CONFIG_DIR" ULTTYCTL="$ULTTYCTL_BIN" \
    run_sync_check live-smoke-gate scripts/live-smoke-gate.sh "$OUT/gate"
  DISPLAY="$DISPLAY_NUM" XDG_RUNTIME_DIR="$RUNTIME_DIR" XDG_CONFIG_HOME="$CONFIG_DIR" ULTTYCTL="$ULTTYCTL_BIN" \
    run_sync_check terminal-workflow-smoke scripts/terminal-workflow-smoke.sh "$OUT/terminal-workflow"
  DISPLAY="$DISPLAY_NUM" XDG_RUNTIME_DIR="$RUNTIME_DIR" XDG_CONFIG_HOME="$CONFIG_DIR" ULTTYCTL="$ULTTYCTL_BIN" COUNTER_APP="$COUNTER_BIN" \
    run_sync_check regression-smoke scripts/regression-smoke.sh "$OUT/regression"

  # Free the shared Ultty + Xvfb before launching the parallel isolated-Xvfb wave.
  stop_shared_ultty

  # Catch panics that did not take the process down.
  CHECK_INDEX=$((CHECK_INDEX + 1))
  if grep -q "panicked at" "$OUT/ultty.log"; then
    echo "FAIL [shared-ultty-panic-check]: Ultty logged a panic:" >&2
    grep -A5 "panicked at" "$OUT/ultty.log" >&2
    record_result "$CHECK_INDEX" "shared-ultty-panic-check" "FAIL" "0" "panic in ultty.log"
  else
    record_result "$CHECK_INDEX" "shared-ultty-panic-check" "PASS" "0" ""
  fi
fi

# Phase 2: isolated E2E suites (each owns its own Xvfb -displayfd / runtime dir)
# run in parallel up to --jobs workers.
queue_check() {
  local name="$1"
  shift
  CHECK_INDEX=$((CHECK_INDEX + 1))
  local idx="$CHECK_INDEX"
  local log="$OUT/logs/$name.log"

  while (( $(jobs -rp | wc -l) >= JOBS )); do
    sleep 0.2
  done

  (
    t0="$(date +%s)"
    echo "==> [$name] starting"
    # Non-interactive background jobs inherit SIGINT/SIGQUIT=SIG_IGN under
    # POSIX; restore SIG_DFL before exec so signal tests (e.g. runtime-file-cleanup)
    # receive real SIGINT delivery.
    if python3 -S -c 'import os, signal, sys; signal.signal(signal.SIGINT, signal.SIG_DFL); signal.signal(signal.SIGQUIT, signal.SIG_DFL); os.execvp(sys.argv[1], sys.argv[1:])' "$@" >"$log" 2>&1; then
      t1="$(date +%s)"
      secs=$((t1 - t0))
      echo "PASS [$name] (${secs}s)"
      record_result "$idx" "$name" "PASS" "$secs" ""
    else
      t1="$(date +%s)"
      secs=$((t1 - t0))
      echo "FAIL [$name] (${secs}s)" >&2
      tail -40 "$log" >&2 || true
      record_result "$idx" "$name" "FAIL" "$secs" "see logs/$name.log"
    fi
  ) &
  CHILD_PIDS+=("$!")
}

record_skip() {
  local name="$1" reason="$2"
  CHECK_INDEX=$((CHECK_INDEX + 1))
  echo "SKIPPED [$name]: $reason" >&2
  record_result "$CHECK_INDEX" "$name" "SKIP" "0" "$reason"
}

if [ "$SHARD" = all ] || [ "$SHARD" = core ]; then
  queue_check accessibility-no-bus env ULTTY="$ULTTY_BIN" scripts/e2e/accessibility-no-bus.sh "$OUT/accessibility"
  queue_check runtime-file-cleanup env ULTTY="$ULTTY_BIN" scripts/e2e/runtime-file-cleanup.sh "$OUT/runtime-file-cleanup"
  queue_check exec-flag env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" scripts/e2e/exec-flag.sh "$OUT/exec-flag"
  if command -v xdotool >/dev/null 2>&1; then
    queue_check multi-window env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" \
      scripts/e2e/multi-window.sh "$OUT/multi-window"
    queue_check quick-terminal env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" \
      scripts/e2e/quick-terminal.sh "$OUT/quick-terminal"
  else
    record_skip multi-window "needs xdotool"
    record_skip quick-terminal "needs xdotool"
  fi
fi

if [ "$SHARD" = all ] || [ "$SHARD" = native-lua ]; then
  queue_check native-split-oracle python3 -S -m unittest discover -s scripts/e2e -p test_native_split_check.py
  if command -v xdotool >/dev/null 2>&1; then
    queue_check inline-surface env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" INLINE_APP="$INLINE_APP_BIN" \
      scripts/e2e/inline-surface.sh "$OUT/inline-surface"
    queue_check native-split env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" COUNTER_APP="$COUNTER_BIN" \
      scripts/e2e/native-split.sh "$OUT/native-split"
  else
    record_skip inline-surface "needs xdotool"
    record_skip native-split "needs xdotool"
  fi
  queue_check agent-transcript env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" AGENT_TRANSCRIPT="$AGENT_TRANSCRIPT_BIN" \
    scripts/e2e/agent-transcript.sh "$OUT/agent-transcript"
  queue_check agent-console env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" AGENT_CONSOLE="$AGENT_CONSOLE_BIN" \
    scripts/e2e/agent-console.sh "$OUT/agent-console"
  queue_check override-push env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" COUNTER_APP="$COUNTER_BIN" \
    scripts/e2e/override-push.sh "$OUT/override-push"
  queue_check lua-open env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" \
    scripts/e2e/lua-open.sh "$OUT/lua-open"
  queue_check lua-panes env ULTTY="$ULTTY_BIN" ULTTYCTL="$ULTTYCTL_BIN" ULTTUI_LUA="$ULTTUI_LUA_BIN" \
    scripts/e2e/lua-panes.sh "$OUT/lua-panes"
fi

if [ "$SHARD" = all ] || [ "$SHARD" = sessions ]; then
  queue_check deploy-bundle env ULTTY_DEPLOY_TEST_BIN_DIR="$BIN_DIR" \
    python3 -S -m unittest discover -s scripts/deploy -p 'test_*.py'
  rm -rf "$OUT/session-reconnect" "$OUT/control-pid" "$OUT/control-workflows" "$OUT/session-control"
  queue_check session-reconnect python3 -S scripts/e2e/session-reconnect.py \
    --ultty "$ULTTY_BIN" \
    --session-bin "$ULTTY_SESSION_BIN" \
    --counter "$COUNTER_BIN" \
    --output "$OUT/session-reconnect"
  queue_check session-host env \
    ULTTY_SESSION="$ULTTY_SESSION_BIN" \
    COUNTER_APP="$COUNTER_BIN" \
    INLINE_APP="$INLINE_APP_BIN" \
    FRAME_APP="$FRAME_APP_BIN" \
    python3 -S scripts/e2e/session-host.py
  queue_check session-control bash -c '
    set -euo pipefail
    tmp="$(mktemp -d /tmp/sc.XXXXXX)"
    trap "rm -rf \"$tmp\"" EXIT
    mkdir -p "$tmp/t"
    ULTTY_SESSION="$1" ULTTYCTL="$2" COUNTER_APP="$3" ULTTUI_LUA="$4" \
      CARGO_TARGET_DIR="$tmp/t" TMPDIR="$tmp" \
      python3 -S scripts/e2e/session-control.py "$5"
  ' _ "$ULTTY_SESSION_BIN" "$ULTTYCTL_BIN" "$COUNTER_BIN" "$ULTTUI_LUA_BIN" "$OUT/session-control"
  queue_check control-pid python3 -S scripts/e2e/control-pid.py \
    --session-bin "$ULTTY_SESSION_BIN" \
    --ctl-bin "$ULTTYCTL_BIN" \
    --output "$OUT/control-pid"
  queue_check control-workflows python3 -S scripts/e2e/control-workflows.py \
    --ultty "$ULTTY_BIN" \
    --ulttyctl "$ULTTYCTL_BIN" \
    --counter "$COUNTER_BIN" \
    --lua "$ULTTUI_LUA_BIN" \
    --output "$OUT/control-workflows"
fi

for pid in "${CHILD_PIDS[@]}"; do
  wait "$pid" || true
done
CHILD_PIDS=()

# Summarize all checks and fail if any failed.
passed=0
failed=0
skipped=0
echo ""
printf '%-26s %-6s %6s  %s\n' "Check" "Status" "Time" "Notes"
printf '%-26s %-6s %6s  %s\n' "--------------------------" "------" "------" "-----"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### GUI smoke (shard: \`$SHARD\`, jobs: $JOBS)"
    echo ""
    echo "| Check | Status | Time | Notes |"
    echo "|---|---|---:|---|"
  } >>"$GITHUB_STEP_SUMMARY"
fi

for res in "$OUT/results/"*.tsv; do
  [ -f "$res" ] || continue
  IFS=$'\t' read -r name status secs note <"$res"
  case "$status" in
    PASS) passed=$((passed + 1)) ;;
    FAIL) failed=$((failed + 1)) ;;
    SKIP) skipped=$((skipped + 1)) ;;
  esac
  printf '%-26s %-6s %5ss  %s\n' "$name" "$status" "$secs" "$note"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '| `%s` | %s | %ss | %s |\n' "$name" "$status" "$secs" "$note" >>"$GITHUB_STEP_SUMMARY"
  fi
done
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  printf '\n**Totals:** %d passed, %d failed, %d skipped\n\n' "$passed" "$failed" "$skipped" >>"$GITHUB_STEP_SUMMARY"
fi

echo ""
echo "GUI smoke totals: $passed passed, $failed failed, $skipped skipped ($OUT)"
if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "GUI smoke passed: $OUT"
