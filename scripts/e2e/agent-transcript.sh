#!/usr/bin/env bash
# The acceptance example of plan item B3 (ADR 0020 slice 5): an agent
# transcript whose cards are native in Ultty, scroll away with the shell's
# output and survive copy, and are plain ANSI text under tmux.
#
#   scripts/e2e/agent-transcript.sh [artifact-dir]
#
# In a fresh Ultty on a virtual X display:
#  1. Runs examples/agent-transcript, which prints prose, a tool card that
#     streams fake cargo output, more prose and a diff card, and holds the
#     cards open. Checks `ulttyctl regions` shows both placed, a screenshot
#     shows both drawn at their heights, and `ulttyctl text --scrollback`
#     reads prose, the cards' last text and more prose in order, with none
#     of the fallback text the cards opened with.
#  2. Prints `seq 1 30` under the live cards, closes them, and checks the
#     scrollback again: the same text, in order, followed by the shell's.
#  3. Runs the example again inside tmux, where the probe falls back to
#     ANSI, and checks the screen shows the same card text. Skipped, loudly,
#     without tmux.
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe works without a
# GPU). Uses $ULTTY and $ULTTYCTL, default target/debug/ultty and ulttyctl,
# built first if unset, and builds the example. Screenshots, text dumps and
# logs are left in the artifact dir. Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ]; then
    cargo build -q --locked -p ultty --bins
    ULTTY=${ULTTY:-target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-target/debug/ulttyctl}
fi
cargo build -q --locked -p agent-transcript
example="$PWD/target/debug/agent-transcript"
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")
tmux_bin=$(command -v tmux || true)

# Without an artifact dir, use a temporary one and remove it on success.
if [ -n "${1:-}" ]; then out=$1; keep_out=1; else out=$(mktemp -d); keep_out=0; fi
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
rm -f "$out/go" "$out/close" "$out"/*.png "$out"/text-*.txt "$out"/*client.log
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-agent.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    [ -n "$tmux_bin" ] && "$tmux_bin" -f /dev/null -S "$runtime/tmux.sock" kill-server 2>/dev/null || true
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

shot() {
    # Two quiet frames' worth, so the window has drawn what was asked.
    sleep 0.3
    "$ULTTYCTL" screenshot "$out/$1.png" >/dev/null || fail "screenshot $1"
}

# The transcript's text, in the order it must appear: prose, the tool
# card's last state, prose, the diff card, prose.
transcript=(
    "> Make the parser tests pass."
    "I'll build the workspace and run the tests first."
    "[tool] cargo test --workspace (passed in 3.2s)"
    "test result: ok. 42 passed; 0 failed"
    "All 42 tests pass. One assertion was off by a tab stop;"
    "[diff] src/parse.rs"
    "let stop = 8;"
    "Applied. The suite is green."
    "End of transcript."
)
# Fallback text the cards opened with, which their rows must not keep.
stale=("(queued)" "(running)")

# Check that file $1 holds the remaining arguments as substrings of lines
# in this order, and none of the stale text. Says what is wrong on stderr.
in_order() {
    local file=$1 at=0 n
    shift
    for want in "$@"; do
        n=$(awk -v want="$want" -v after="$at" \
            'NR > after && index($0, want) { print NR; exit }' "$file")
        [ -n "$n" ] || { echo "missing after line $at: $want" >&2; return 1; }
        at=$n
    done
    for old in "${stale[@]}"; do
        ! grep -qF "$old" "$file" || { echo "stale fallback text: $old" >&2; return 1; }
    done
}

# Wait until a line of the screen matches the extended regex $1 (the
# command lines typed below hold the words, so a substring is not enough).
wait_line() {
    for _ in $(seq 200); do
        "$ULTTYCTL" text | grep -Eq "$1" && return 0
        sleep 0.1
    done
    return 1
}

"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "no shell prompt"

# 1. The transcript with live cards. The example's stderr (its card events)
# goes to a log; once `go` exists the shell prints under the cards, and once
# `close` exists the example closes them and exits.
"$ULTTYCTL" send-text "sh -c '$example --hold $out/close 2>$out/client.log & while [ ! -e $out/go ]; do sleep 0.05; done; seq 1 30; wait; echo shell-done'" >/dev/null
"$ULTTYCTL" send-key enter >/dev/null

wait_line '^End of transcript\. *$' ||
    fail "the transcript never finished"
for _ in $(seq 100); do
    [ "$("$ULTTYCTL" regions | grep -c '^surface #.*placed at line')" = 2 ] && break
    sleep 0.1
done
[ "$("$ULTTYCTL" regions | grep -c '^surface #.*placed at line')" = 2 ] ||
    fail "the two cards are not both placed"
grep -q "Native mode" "$out/client.log" || fail "the example did not run natively"
for _ in $(seq 50); do
    "$ULTTYCTL" text --scrollback >"$out/text-live.txt"
    in_order "$out/text-live.txt" "${transcript[@]}" 2>/dev/null && break
    sleep 0.1
done
in_order "$out/text-live.txt" "${transcript[@]}" ||
    fail "the live transcript does not read in order (see $out/text-live.txt)"
"$ULTTYCTL" regions >"$out/regions-live.txt"
shot live
python3 scripts/analyze-agent-transcript-screenshot.py "$out/live.png" ||
    fail "the cards are not drawn (see $out/live.png)"

# 2. Shell output under the live cards, then close them: they freeze into
# their rows, and everything reads in order.
touch "$out/go"
wait_line '^30 *$' || fail "seq 1 30 did not finish"
touch "$out/close"
wait_line '^shell-done *$' || fail "the example never exited"
[ "$(grep -c ' Closed$' "$out/client.log")" = 2 ] || fail "the example did not hear both cards close"
[ "$(grep -c ' Placed$' "$out/client.log")" = 2 ] || fail "the example did not hear both cards placed"
"$ULTTYCTL" regions | grep -q '^surface #' && fail "a card is still listed after close"
"$ULTTYCTL" text --scrollback >"$out/text-closed.txt"
in_order "$out/text-closed.txt" "${transcript[@]}" 1 15 30 shell-done ||
    fail "the closed transcript does not read in order (see $out/text-closed.txt)"
shot closed

# 3. The same example under tmux: the probe falls back to ANSI, and the
# screen tmux draws shows the same text.
if [ -z "$tmux_bin" ]; then
    echo "SKIPPED: the tmux part of agent-transcript.sh needs tmux" >&2
else
    "$ULTTYCTL" send-text "clear; TERM=xterm-256color $tmux_bin -S $runtime/tmux.sock -f /dev/null new-session '$example 2>$out/tmux-client.log; echo tmux-done; sleep 60'" >/dev/null
    "$ULTTYCTL" send-key enter >/dev/null
    wait_line '^tmux-done *$' || fail "the example never finished in tmux"
    grep -q "Fallback mode" "$out/tmux-client.log" || fail "the example did not fall back under tmux"
    "$ULTTYCTL" text >"$out/text-tmux.txt"
    in_order "$out/text-tmux.txt" "${transcript[@]}" tmux-done ||
        fail "tmux does not show the transcript in order (see $out/text-tmux.txt)"
    shot tmux
fi

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
if [ "$keep_out" = 1 ]; then
    echo "agent-transcript: ok ($out)"
else
    rm -rf "$out"
    echo "agent-transcript: ok"
fi
