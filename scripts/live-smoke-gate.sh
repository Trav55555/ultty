#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-/tmp/ultty-live-smoke-gate}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"

mkdir -p "$OUT_DIR"

assert_png() {
  local path="$1"
  if [[ ! -s "$path" ]]; then
    echo "live smoke gate failed: missing or empty screenshot: $path" >&2
    exit 1
  fi

  local magic
  magic="$(od -An -tx1 -N8 "$path" | tr -d ' \n')"
  if [[ "$magic" != "89504e470d0a1a0a" ]]; then
    echo "live smoke gate failed: not a PNG screenshot: $path" >&2
    exit 1
  fi
}

run_health_check() {
  local phase="$1"
  local smoke="$2"
  local health_dir="$OUT_DIR/health/$smoke/$phase"

  mkdir -p "$health_dir"

  if ! "$ULTTYCTL" ping >"$health_dir/ping.txt" 2>"$health_dir/ping.err"; then
    echo "live smoke gate failed: $smoke $phase ping failed; see $health_dir" >&2
    exit 1
  fi

  if ! "$ULTTYCTL" tabs >"$health_dir/tabs.txt" 2>"$health_dir/tabs.err"; then
    echo "live smoke gate failed: $smoke $phase tabs failed; see $health_dir" >&2
    exit 1
  fi

  if ! "$ULTTYCTL" info >"$health_dir/info.txt" 2>"$health_dir/info.err"; then
    echo "live smoke gate failed: $smoke $phase info failed; see $health_dir" >&2
    exit 1
  fi

  if ! "$ULTTYCTL" screenshot "$health_dir/screenshot.png" >"$health_dir/screenshot.txt" 2>"$health_dir/screenshot.err"; then
    echo "live smoke gate failed: $smoke $phase screenshot failed; see $health_dir" >&2
    exit 1
  fi
  assert_png "$health_dir/screenshot.png"
}

run_smoke() {
  local name="$1"
  shift

  local smoke_dir="$OUT_DIR/$name"
  local log="$OUT_DIR/$name.log"

  echo "==> $name" | tee -a "$OUT_DIR/summary.txt"
  run_health_check before "$name"

  mkdir -p "$smoke_dir"
  if ! "$@" "$smoke_dir" >"$log" 2>&1; then
    echo "live smoke gate failed: $name failed; see $log and $smoke_dir" >&2
    exit 1
  fi

  run_health_check after "$name"
  echo "PASS $name" | tee -a "$OUT_DIR/summary.txt"
}

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  echo "ulttyctl cannot reach a running Ultty control socket." >&2
  echo "Start Ultty first, then rerun: $0 [$OUT_DIR]" >&2
  exit 1
fi

: >"$OUT_DIR/summary.txt"

run_smoke chrome-screenshot scripts/screenshot-smoke.sh
run_smoke terminal-surface scripts/terminal-surface-smoke.sh
run_smoke native-richtext scripts/native-richtext-smoke.sh
run_smoke append-content scripts/append-content-smoke.sh

echo "live smoke gate passed:"
echo "  $OUT_DIR"
