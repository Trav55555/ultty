#!/usr/bin/env bash
# Open an inline surface (ADR 0020) in a fresh Ultty on a virtual X
# display, print `seq 1 200` under it, and check from screenshots and
# `ulttyctl regions` that the card is drawn over its own rows: on the screen
# when placed, gone when the output scrolled it off, back where its text is
# after scrolling to the top of scrollback, and moving line for line with
# that text. `ulttyctl text --scrollback` reads the card's text in its rows,
# in order with the output around it, live and after the card is closed.
#
#   scripts/e2e/inline-surface.sh [artifact-dir]
#
# Needs Xvfb, xdotool (to turn the mouse wheel) and a Vulkan or GL driver
# (Mesa's lavapipe works without a GPU). Uses $ULTTY and $ULTTYCTL, default
# target/debug/ultty and ulttyctl, built first if unset, and builds
# ultty-client's inline-surface example. Screenshots and logs are left in
# the artifact dir. Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }
command -v xdotool >/dev/null || { echo "xdotool is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ]; then
    cargo build -q --locked -p ultty --bins
    ULTTY=${ULTTY:-target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-target/debug/ulttyctl}
fi
cargo build -q --locked -p ultty-client --example inline-surface
client="$PWD/target/debug/examples/inline-surface"
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
rm -f "$out/go" "$out/close" "$out"/*.png "$out"/text-*.txt "$out/client.log"
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-inline.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

# Xvfb picks a free display and writes its number to fd 3.
Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }
display=":$(cat "$out/display")"

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

# Without unsetting WAYLAND_DISPLAY, a Wayland session in the environment
# wins over DISPLAY and the window opens on the desktop instead of Xvfb.
# PS1 fixes the prompt the wait below looks for: sh gives root (CI) "# ".
env -u WAYLAND_DISPLAY \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    "$ULTTYCTL" regions >&2 || true
    tail -20 "$out/ultty.log" >&2
    exit 1
}

# Talk only to this Ultty: its own control socket, found through its pid.
export XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN
for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Ultty never answered on its control socket"

# The `surface #9000 (3 rows, <state> at line <n>)` line of `regions`.
surface_line() {
    "$ULTTYCTL" regions | grep '^surface #9000 ' || true
}
wait_for_state() {
    for _ in $(seq 100); do
        surface_line | grep -q "$1" && return 0
        sleep 0.1
    done
    fail "the surface never became $1 (now: $(surface_line))"
}
shot() {
    # Two quiet frames' worth, so the window has drawn what was asked.
    sleep 0.3
    "$ULTTYCTL" screenshot "$out/$1.png" >/dev/null || fail "screenshot $1"
}

"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "no shell prompt"
# Five lines first, so the card sits mid-screen. The card waits for `go`
# before `seq 1 200` prints under it; sh -c keeps job-control chatter and
# prompts out of the text. The client's stderr goes to a log.
# Once `close` exists the client closes the card and exits.
"$ULTTYCTL" send-text "sh -c 'seq 1 5; $client 60 $out/close 2>$out/client.log & while [ ! -e $out/go ]; do sleep 0.05; done; seq 1 200; wait'" >/dev/null
"$ULTTYCTL" send-key enter >/dev/null

wait_for_state "placed at line"
line=$(surface_line | sed -n 's/.* at line \([0-9]*\))$/\1/p')
[ -n "$line" ] || fail "no anchor line in: $(surface_line)"
# The card's rows hold its text instead of the fallback text, once the
# first frame is written into them.
# Line `n` (1-based) of `text --scrollback`, trailing blanks dropped.
text_line() {
    sed -n "${2}p" "$1" | sed 's/ *$//'
}
card_text() {
    "$ULTTYCTL" text --scrollback >"$out/text-$1.txt"
    [ "$(text_line "$out/text-$1.txt" "$line")" = "5" ] &&
        [ "$(text_line "$out/text-$1.txt" "$((line + 1))")" = "INLINE CARD" ] &&
        [ -z "$(text_line "$out/text-$1.txt" "$((line + 2))")" ] &&
        [ -z "$(text_line "$out/text-$1.txt" "$((line + 3))")" ] &&
        ! grep -q FALLBACK "$out/text-$1.txt"
}
for _ in $(seq 50); do card_text placed && break; sleep 0.1; done
card_text placed || fail "the card's rows do not read its text (see $out/text-placed.txt)"
shot placed

touch "$out/go"
for _ in $(seq 200); do
    "$ULTTYCTL" text | grep -Eq '^200 *$' && break
    sleep 0.1
done
"$ULTTYCTL" text | grep -Eq '^200 *$' || fail "seq 1 200 did not finish"
wait_for_state "hidden at line $line"
shot off

# Turn the wheel up past the top of scrollback, then a few notches down.
xdotool_on() { env -u WAYLAND_DISPLAY DISPLAY="$display" xdotool "$@"; }
xdotool_on mousemove 600 400
xdotool_on click --repeat 300 --delay 2 4
wait_for_state "placed at line $line"
shot top
xdotool_on click --repeat 3 --delay 20 5
sleep 0.3
wait_for_state "placed at line $line"
shot shifted

python3 scripts/analyze-inline-surface-screenshots.py \
    "$out/placed.png" "$out/off.png" "$out/top.png" "$out/shifted.png" \
    --rows 3 || fail "the card is not where its text is"

# Close the card: it freezes into its rows as text, between `seq 1 5` and
# `seq 1 200`.
touch "$out/close"
for _ in $(seq 100); do
    grep -q "surface 9000 Closed" "$out/client.log" && break
    sleep 0.1
done
grep -q "surface 9000 Closed" "$out/client.log" || fail "the app never heard Closed"
[ -z "$(surface_line)" ] || fail "the surface is still listed: $(surface_line)"
card_text closed || fail "the closed card's rows do not read its text (see $out/text-closed.txt)"
[ "$(text_line "$out/text-closed.txt" "$((line + 4))")" = "1" ] ||
    fail "the output after the card is not where it was"

grep -q "surface 9000 Placed" "$out/client.log" || fail "the app never heard Placed"
grep -q "surface 9000 Hidden" "$out/client.log" || fail "the app never heard Hidden"
grep -q "surface 9000 Shown" "$out/client.log" || fail "the app never heard Shown"
! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"

echo "inline-surface: ok ($out)"
