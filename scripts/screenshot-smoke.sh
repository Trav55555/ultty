#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-/tmp/ultty-screenshot-smoke}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"

mkdir -p "$OUT_DIR"

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  echo "ulttyctl cannot reach a running Ultty control socket." >&2
  echo "Start Ultty first, then rerun: $0 [$OUT_DIR]" >&2
  exit 1
fi

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
  "$ULTTYCTL" tabs | awk '/^[* ]*[0-9]+:/ { count++ } END { print count + 0 }'
}

while [[ "$(tab_count)" -gt 1 ]]; do
  "$ULTTYCTL" close-tab >/dev/null
done

"$ULTTYCTL" screenshot "$OUT_DIR/single-tab.png" >/dev/null
assert_png "$OUT_DIR/single-tab.png"

"$ULTTYCTL" new-tab >/dev/null
cleanup() {
  "$ULTTYCTL" close-tab >/dev/null 2>&1 || true
}
trap cleanup EXIT

"$ULTTYCTL" screenshot "$OUT_DIR/multi-tab.png" >/dev/null
assert_png "$OUT_DIR/multi-tab.png"

scripts/analyze-chrome-screenshots.py \
  --single "$OUT_DIR/single-tab.png" \
  --multi "$OUT_DIR/multi-tab.png"

echo "screenshot smoke passed:"
echo "  $OUT_DIR/single-tab.png"
echo "  $OUT_DIR/multi-tab.png"
