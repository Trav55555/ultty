#!/usr/bin/env bash
# End-to-end test of examples/counter-app, driven through ulttyctl: open the
# app in its own pane, press keys, and check what it shows.
#
# Run it from a shell inside Ultty (or with ULTTY_PID naming a running
# Ultty), with ulttyctl on PATH. Exits non-zero when a check fails.
# docs/testing-tuis.md walks through it.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo build -q -p counter-app
app="$PWD/target/debug/counter-app"

# ANSI mode draws the app's text into the terminal grid, where `text` and
# `wait-for` read it: no native socket (ULTTY_SOCKET) and no in-band probe.
pane=$(ulttyctl new-tab env -u ULTTY_SOCKET ULTTY_IN_BAND=0 "$app")
trap 'ulttyctl --pane "$pane" send-text q >/dev/null 2>&1 || true' EXIT

fail() {
    echo "FAIL: $*" >&2
    ulttyctl --pane "$pane" text >&2 || true
    exit 1
}

# The count sits alone on its row, inside the card's border.
count_is() {
    ulttyctl --pane "$pane" text | grep -Eq "^[│|] *$1 *[│|]?\$"
}

ulttyctl --pane "$pane" wait-for "Counter" --timeout 10000 >/dev/null ||
    fail "the app never drew its title"
count_is 0 || fail "the count does not start at 0"

for _ in 1 2 3; do
    ulttyctl --pane "$pane" send-text + >/dev/null
done
# Let it redraw: no output for 300 ms.
ulttyctl --pane "$pane" wait-for --quiet 300 --timeout 5000 >/dev/null
count_is 3 || fail "three presses of + did not count to 3"

ulttyctl --pane "$pane" send-text r >/dev/null
ulttyctl --pane "$pane" wait-for --quiet 300 --timeout 5000 >/dev/null
count_is 0 || fail "r did not reset the count"

echo "counter-app: ok"
