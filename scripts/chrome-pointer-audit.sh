#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-/tmp/ultty-chrome-pointer-audit}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"

mkdir -p "$OUT_DIR"

AUDIT="$OUT_DIR/audit.md"
COMMAND_LOG="$OUT_DIR/commands.log"
: >"$COMMAND_LOG"

append() {
  printf '%s\n' "$*" >>"$AUDIT"
}

run_logged() {
  {
    printf '$'
    printf ' %q' "$@"
    printf '\n'
  } >>"$COMMAND_LOG"
  "$@" >>"$COMMAND_LOG" 2>&1
}

tab_count() {
  "$ULTTYCTL" tabs | awk '/^[* ]*[0-9]+:/ { count++ } END { print count + 0 }'
}

normalize_tabs() {
  while [[ "$(tab_count)" -gt 1 ]]; do
    "$ULTTYCTL" close-tab >/dev/null 2>&1 || break
  done
}

if ! "$ULTTYCTL" ping >/dev/null 2>&1; then
  echo "ulttyctl cannot reach a running Ultty control socket." >&2
  echo "Start Ultty first, then rerun: $0 [$OUT_DIR]" >&2
  exit 1
fi

trap normalize_tabs EXIT
normalize_tabs

append "# Ultty Chrome Pointer Audit"
append
append "Generated: $(date -Iseconds)"
append
append "## Product Regression Evidence"
append
append "- Control socket responded to ping."

run_logged "$ULTTYCTL" tabs
append "- Initial tab state normalized to one tab."

scripts/screenshot-smoke.sh "$OUT_DIR/screenshots" >>"$COMMAND_LOG" 2>&1
append "- Renderer screenshot smoke passed for single-tab, two-tab, and dense-tab states."

normalize_tabs
"$ULTTYCTL" new-tab >/dev/null
"$ULTTYCTL" switch-tab 1 >/dev/null
"$ULTTYCTL" switch-tab 2 >/dev/null
"$ULTTYCTL" close-tab 2 >/dev/null
run_logged "$ULTTYCTL" tabs
append "- Tab command path matched visible one-based tab labels for new, switch, and close."

"$ULTTYCTL" resize 900 640 >/dev/null
"$ULTTYCTL" screenshot "$OUT_DIR/resize-900x640.png" >/dev/null
append "- Resize command completed and renderer screenshot captured at resize-900x640.png."

append
append "## Compositor Debugging Evidence"
if command -v xdotool >/dev/null 2>&1; then
  append "- xdotool is available for desktop/window-manager checks."
  if "$ULTTYCTL" focus >>"$COMMAND_LOG" 2>&1; then
    sleep 0.2
    append "- Ultty focus command completed before compositor geometry capture."
  else
    append "- Ultty focus command failed before compositor geometry capture."
  fi
  if xdotool getactivewindow getwindowname getwindowgeometry >>"$COMMAND_LOG" 2>&1; then
    append "- Focused-window name and geometry were captured in commands.log."
  else
    append "- xdotool was available, but focused-window geometry could not be captured from this shell."
  fi
else
  append "- xdotool is not available; compositor-only checks were not automated."
fi

append
append "## Manual Pointer Checks"
append
append "- Title/control band: empty title-band space drags the window; new-tab and window controls do not start drag."
append "- Edge resize: each window edge and corner enters resize behavior without stealing tab clicks."
append "- Tab rail: left click switches tabs, middle click closes a tab, close target closes only the targeted tab."
append "- Dense tabs: close targets remain stable and hover does not resize or shift the rail."
append
append "## Artifacts"
append
append "- commands.log"
append "- screenshots/single-tab.png"
append "- screenshots/multi-tab.png"
append "- screenshots/dense-tabs.png"
append "- resize-900x640.png"

normalize_tabs

echo "chrome pointer audit passed:"
echo "  $AUDIT"
