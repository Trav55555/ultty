#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-/tmp/ultty-append-content-smoke}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"

mkdir -p "$OUT_DIR"

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  echo "ulttyctl cannot reach a running Ultty control socket." >&2
  echo "Start Ultty first, then rerun: $0 [$OUT_DIR]" >&2
  exit 1
fi

ctl() {
  local attempt

  for attempt in {1..5}; do
    if "$ULTTYCTL" "$@"; then
      return 0
    fi
    sleep 0.2
  done

  "$ULTTYCTL" "$@"
}

assert_png() {
  local path="$1"
  if [[ ! -s "$path" ]]; then
    echo "missing or empty screenshot: $path" >&2
    exit 1
  fi

  local magic
  magic="$(od -An -tx1 -N8 "$path" | tr -d ' \n')"
  if [[ "$magic" != "89504e470d0a1a0a" ]]; then
    echo "not a PNG screenshot: $path" >&2
    exit 1
  fi
}

tab_count() {
  ctl tabs | awk '/^[* ]*[0-9]+:/ { count++ } END { print count + 0 }'
}

normalize_tabs() {
  while [[ "$(tab_count)" -gt 1 ]]; do
    ctl close-tab >/dev/null 2>&1 || break
  done
}

cleanup() {
  ctl send-key c ctrl >/dev/null 2>&1 || true
  sleep 0.2
  normalize_tabs
}

trap cleanup EXIT
normalize_tabs

ctl new-tab cargo run -p hello-native --bin append-content-smoke >/dev/null
sleep 1.0
ctl screenshot "$OUT_DIR/before-append.png" >/dev/null
assert_png "$OUT_DIR/before-append.png"

sleep 2.0
ctl screenshot "$OUT_DIR/after-append.png" >/dev/null
assert_png "$OUT_DIR/after-append.png"

scripts/analyze-append-content-screenshots.py \
  "$OUT_DIR/before-append.png" \
  "$OUT_DIR/after-append.png"

echo "append-content smoke passed:"
echo "  $OUT_DIR/before-append.png"
echo "  $OUT_DIR/after-append.png"
