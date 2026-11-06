#!/usr/bin/env bash
# Live checks for bugs that unit tests and CI missed and only a running Ultty
# showed. Each check names the bug it guards. Needs a running Ultty, plus
# counter-app built (cargo build -p counter-app).
#
# Usage: scripts/regression-smoke.sh [artifact-dir]
set -euo pipefail

ARTIFACT_DIR="${1:-/tmp/ultty-regression-smoke}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"
COUNTER_APP="${COUNTER_APP:-target/debug/counter-app}"
mkdir -p "$ARTIFACT_DIR"

fail() {
  echo "regression smoke failed: $*" >&2
  exit 1
}

for bin in "$ULTTYCTL" "$COUNTER_APP"; do
  [ -x "$bin" ] || fail "$bin not built"
done
"$ULTTYCTL" ping >/dev/null || fail "cannot reach Ultty"

tab_count() {
  "$ULTTYCTL" tabs | awk '/^[* ]*[0-9]+:/ { count++ } END { print count + 0 }'
}
START_TABS="$(tab_count)"
cleanup() {
  while [ "$(tab_count)" -gt "$START_TABS" ]; do
    "$ULTTYCTL" close-tab >/dev/null 2>&1 || break
  done
}
trap cleanup EXIT

BASH_BIN="$(command -v bash)"
ENV_BIN="$(command -v env)"

# ulttyctl info deadlocked the UI thread (it re-locked the terminal), so
# every later request hung too.
for attempt in 1 2; do
  timeout 5 "$ULTTYCTL" --read-timeout 4 info >"$ARTIFACT_DIR/info-$attempt.txt" ||
    fail "ulttyctl info did not answer (attempt $attempt)"
done
echo "PASS info answers"

# Shrinking the screen (the tab bar appearing for a second tab) pushed the
# first lines of a fresh tab into scrollback.
"$ULTTYCTL" new-tab "$BASH_BIN" -c \
  'printf "SHRINK_A\nSHRINK_B\nSHRINK_C\n"; printf "SHRINK_DONE\n"; sleep 60' >/dev/null
"$ULTTYCTL" wait-for SHRINK_DONE --timeout 10000 >/dev/null
"$ULTTYCTL" text >"$ARTIFACT_DIR/shrink-screen.txt"
for marker in SHRINK_A SHRINK_B SHRINK_C; do
  grep -q "$marker" "$ARTIFACT_DIR/shrink-screen.txt" ||
    fail "$marker scrolled off a fresh tab's screen"
done
"$ULTTYCTL" close-tab >/dev/null
echo "PASS fresh tab keeps its top lines"

# ulttyctl text --scrollback ignored the flag and returned the screen only.
"$ULTTYCTL" new-tab "$BASH_BIN" -c \
  'for i in $(seq 1 300); do echo "SB_LINE_$i"; done; echo SB_DONE; sleep 60' >/dev/null
"$ULTTYCTL" wait-for SB_DONE --timeout 10000 >/dev/null
"$ULTTYCTL" text --scrollback >"$ARTIFACT_DIR/scrollback.txt"
grep -qx "SB_LINE_1 *" "$ARTIFACT_DIR/scrollback.txt" ||
  fail "text --scrollback is missing the first line of history"
"$ULTTYCTL" close-tab >/dev/null
echo "PASS text --scrollback includes history"

# A ulttui app draws natively over the socket, or in-band when it has no
# socket (ADR 0018). Either way its text never reaches the terminal grid;
# finding it there means the app fell back to ANSI.
check_native() {
  local name="$1"
  shift
  "$ULTTYCTL" new-tab "$@" >/dev/null
  sleep 4
  # An app that crashed would also leave the grid empty; its tab closes.
  [ "$(tab_count)" -gt "$START_TABS" ] || fail "$name: counter-app exited"
  "$ULTTYCTL" text >"$ARTIFACT_DIR/$name-grid.txt"
  "$ULTTYCTL" screenshot "$ARTIFACT_DIR/$name.png" >/dev/null
  if grep -q "Counter" "$ARTIFACT_DIR/$name-grid.txt"; then
    fail "$name: counter-app fell back to ANSI"
  fi
  "$ULTTYCTL" close-tab >/dev/null
  echo "PASS counter-app runs natively ($name)"
}
check_native socket "$COUNTER_APP"
check_native in-band "$ENV_BIN" -u ULTTY_SOCKET -u ULTTY_TTY "$COUNTER_APP"

echo "regression smoke passed: $ARTIFACT_DIR"
