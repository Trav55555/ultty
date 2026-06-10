#!/usr/bin/env bash
set -euo pipefail

ARTIFACT_DIR="${1:-/tmp/ultty-terminal-workflow-smoke}"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"

mkdir -p "$ARTIFACT_DIR"

if [ ! -x "$ULTTYCTL" ]; then
  echo "ulttyctl not found at $ULTTYCTL; run cargo build -p ultty --bin ulttyctl" >&2
  exit 2
fi

"$ULTTYCTL" ping >"$ARTIFACT_DIR/ping.txt"
"$ULTTYCTL" --json info >"$ARTIFACT_DIR/info.json"

python3 -m json.tool "$ARTIFACT_DIR/info.json" >"$ARTIFACT_DIR/info.pretty.json"

initial_tab_count() {
  "$ULTTYCTL" tabs | awk '/^[* ]*[0-9]+:/ { count++ } END { print count + 0 }'
}

START_TAB_COUNT="$(initial_tab_count)"

cleanup() {
  "$ULTTYCTL" send-key c ctrl >/dev/null 2>&1 || true
  while [[ "$(initial_tab_count)" -gt "$START_TAB_COUNT" ]]; do
    "$ULTTYCTL" close-tab >/dev/null 2>&1 || break
  done
}
trap cleanup EXIT

ENV_BIN="$(command -v env)"
BASH_BIN="$(command -v bash)"

SENTINEL="ULTTY_WORKFLOW_SMOKE_DONE_$(date +%s)"
SCRIPT=$'set -e\n'
SCRIPT+='printf "ULTTY_WORKFLOW_SMOKE_START\n"'
SCRIPT+=$'\n'
SCRIPT+='printf "shell editing: alpha beta\n"'
SCRIPT+=$'\n'
SCRIPT+='(sleep 0.1; printf "job-control-child\n") & wait'
SCRIPT+=$'\n'
SCRIPT+='printf "pager-lines\none\ntwo\nthree\n" | sed -n "1,3p"'
SCRIPT+=$'\n'
SCRIPT+="printf \"$SENTINEL\n\""
SCRIPT+=$'\n'
SCRIPT+='sleep 30'

"$ULTTYCTL" new-tab "$ENV_BIN" "$BASH_BIN" -lc "$SCRIPT" >"$ARTIFACT_DIR/new-tab.txt"
sleep 0.5
"$ULTTYCTL" wait-for "$SENTINEL" --timeout 10000 >"$ARTIFACT_DIR/wait-for.txt"
"$ULTTYCTL" text --scrollback >"$ARTIFACT_DIR/text.txt"
"$ULTTYCTL" search "$SENTINEL" >"$ARTIFACT_DIR/search.txt"

grep -q "$SENTINEL" "$ARTIFACT_DIR/text.txt"
grep -q "$SENTINEL" "$ARTIFACT_DIR/search.txt"

echo "terminal workflow smoke passed: $ARTIFACT_DIR"
