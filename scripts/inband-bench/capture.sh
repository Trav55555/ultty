#!/usr/bin/env bash
# Launch Ultty, run each example app through traced.sh with scripted input.
# Build first: cargo build --release -p ultty -p counter-app -p todo-app \
#   -p form-app -p chat -p dashboard
# Then: python3 corpus.py  (writes corpus.bin for the inband_transport example)
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
R="$(cd "$DIR/../.." && pwd)/target/release"
CTL=$R/ulttyctl
if $CTL ping >/dev/null 2>&1; then echo "Ultty already running" >&2; exit 1; fi
RUST_LOG=warn $R/ultty >"$DIR/ultty.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true' EXIT
for _ in $(seq 100); do $CTL ping >/dev/null 2>&1 && break; sleep 0.1; done

run() { # name secs keys...
  local name=$1 secs=$2; shift 2
  rm -f "$DIR/$name.trace"
  $CTL new-tab "$DIR/traced.sh" "$DIR/$name.trace" "$secs" "$R/$name" >/dev/null
  sleep 1.5
  for k in "$@"; do
    case $k in text:*) $CTL send-text "${k#text:}" >/dev/null ;; *) $CTL send-key $k >/dev/null ;; esac
    sleep 0.25
  done
  $CTL wait-for TRACE-DONE --timeout $(( (secs + 5) * 1000 )) >/dev/null || echo "$name: no TRACE-DONE"
  $CTL screenshot "$DIR/$name.png" >/dev/null 2>&1 || true
  $CTL close-tab >/dev/null 2>&1 || true
  echo "$name: $(stat -c %s "$DIR/$name.trace" 2>/dev/null || echo missing) bytes"
}
run counter-app 8 + + + - up down j k space
run todo-app 10 text:buy\ milk enter text:walk\ dog enter down up space tab
run form-app 10 text:Ada tab text:ada@example.com tab space tab enter
run chat 12 text:hello enter text:tell\ me\ more enter
run dashboard 8 tab tab down
# sysmon stalls after the handshake under the proxy; see the perf doc.
# run sysmon 15
