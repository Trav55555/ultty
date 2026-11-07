#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-/tmp/ultty-terminal-surface-smoke}"
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

normalize_tabs() {
  while [[ "$(tab_count)" -gt 1 ]]; do
    "$ULTTYCTL" close-tab >/dev/null 2>&1 || break
  done
}

send_line() {
  "$ULTTYCTL" send-text "$1" >/dev/null
  "$ULTTYCTL" send-key enter >/dev/null
  sleep 0.2
}

capture_png() {
  local name="$1"
  "$ULTTYCTL" screenshot "$OUT_DIR/$name" >/dev/null
  assert_png "$OUT_DIR/$name"
}

capture_resize_evidence() {
  local width="$1"
  local height="$2"
  local evidence="$OUT_DIR/resize-evidence.txt"

  {
    printf 'before: '
    "$ULTTYCTL" info
  } >"$evidence"

  # Under X11 the platform resizes asynchronously, so ulttyctl resize can
  # report "not applied"; record its answer and resize with xdotool instead.
  {
    printf 'resize: '
    "$ULTTYCTL" resize "$width" "$height" 2>&1 || true
  } >>"$evidence"
  if command -v xdotool >/dev/null 2>&1; then
    "$ULTTYCTL" focus >/dev/null 2>&1 || true
    sleep 0.2
    xdotool getactivewindow windowsize "$width" "$height" >/dev/null 2>&1 || true
  fi

  sleep 0.2
  {
    printf 'after: '
    "$ULTTYCTL" info
    if command -v xdotool >/dev/null 2>&1; then
      printf '\nxdotool active geometry:\n'
      xdotool getactivewindow getwindowgeometry || true
    fi
  } >>"$evidence"
}

trap normalize_tabs EXIT
normalize_tabs

capture_png "prompt.png"

send_line "seq 1 180"
"$ULTTYCTL" wait-for 180 --timeout 3000 >/dev/null
capture_png "scrollback.png"

send_line "printf '\\033[3 q'"
capture_png "cursor-underline.png"

send_line "printf '\\033[5 q'"
capture_png "cursor-bar.png"

send_line "printf '\\033[1 q'"

"$ULTTYCTL" selection >"$OUT_DIR/selection.txt"

capture_resize_evidence 900 640
capture_png "resize-900x640.png"

scripts/analyze-terminal-surface-screenshots.py \
  --prompt "$OUT_DIR/prompt.png" \
  --scrollback "$OUT_DIR/scrollback.png" \
  --cursor-underline "$OUT_DIR/cursor-underline.png" \
  --cursor-bar "$OUT_DIR/cursor-bar.png" \
  --resize "$OUT_DIR/resize-900x640.png"

echo "terminal surface smoke passed:"
echo "  $OUT_DIR/prompt.png"
echo "  $OUT_DIR/scrollback.png"
echo "  $OUT_DIR/cursor-underline.png"
echo "  $OUT_DIR/cursor-bar.png"
echo "  $OUT_DIR/selection.txt"
echo "  $OUT_DIR/resize-evidence.txt"
echo "  $OUT_DIR/resize-900x640.png"
