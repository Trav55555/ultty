#!/usr/bin/env bash
# Run an app in this Ultty tab with its native socket traffic logged.
# Usage: traced.sh <trace-out> <seconds> <app> [args...]
set -u
OUT="$1"; SECS="$2"; shift 2
SOCK="$(mktemp -u "${TMPDIR:-/tmp}/inband-proxy.XXXXXX.sock")"
python3 "$(dirname "$0")/proxy.py" "$SOCK" "$ULTTY_SOCKET" "$OUT" &
for _ in $(seq 50); do [ -S "$SOCK" ] && break; sleep 0.05; done
ULTTY_SOCKET="$SOCK" RUST_LOG=${APP_LOG:-warn} timeout "$SECS" "$@" 2>"$OUT.err"
rm -f "$SOCK"
echo TRACE-DONE
sleep 1
