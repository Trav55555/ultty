#!/usr/bin/env bash
# The agent console (ADR 0031 slice 2) in a headless Ultty, both modes.
#
#   scripts/e2e/agent-console.sh [artifact-dir]
#
# In a fresh Ultty on a virtual X display:
#  1. Replays fixtures/short.jsonl natively (inline cards) at 2x speed,
#     approving with --auto-approve. While the first tool card streams,
#     checks `ulttyctl regions` shows a placed surface and takes a
#     screenshot. Afterwards checks `ulttyctl text --scrollback` reads the
#     prompt, the prose, the frozen tool, diff and approval cards and the
#     final status in order, with no live view left behind and no surface
#     still listed.
#  2. Replays it again with the socket unset and ULTTY_IN_BAND=0, so the
#     console falls back to ANSI, and checks the same text in order.
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
if [ -z "${AGENT_CONSOLE:-}" ]; then
    cargo build -q --locked -p agent-console
    AGENT_CONSOLE="$PWD/target/debug/agent-console"
fi
console=$(realpath "$AGENT_CONSOLE")
fixture="$PWD/examples/agent-console/fixtures/short.jsonl"
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")

# Without an artifact dir, use a temporary one and remove it on success.
if [ -n "${1:-}" ]; then out=$1; keep_out=1; else out=$(mktemp -d); keep_out=0; fi
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
rm -f "$out"/*.png "$out"/text-*.txt "$out"/*client.log
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-console.XXXXXX)
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
    sleep 0.3
    "$ULTTYCTL" screenshot "$out/$1.png" >/dev/null || fail "screenshot $1"
}

# The transcript's text, in order: the prompt, prose (including code that
# keeps its indentation), the first tool card frozen with its result, the
# diff card, the approval card with its answer, the second tool card, the
# closing prose and the last status.
transcript=(
    "> Tabs after wide characters land one column short in the pager."
    "I'll start by reading how \`expand_tabs\` handles tab stops."
    '```rust'
    "            let pad = stop - out.len() % stop;"
    "   - \`Config::load\` reads \`tab_width\` but never passes it down."
    "[bash] cargo test -p pager parse:: (failed, exit 101, 10 lines, 3.9s)"
    "test result: FAILED. 1 passed; 3 failed"
    "Three failures, all from the two problems above."
    "[diff] src/parse.rs"
    "+            column += ch.width().unwrap_or(0);"
    "[approve] cargo test -p pager (Allow once)"
    "> 1. Allow once"
    "[bash] cargo test -p pager (ok, 2 lines, 2.1s)"
    "The suite is green"
    "I found none in this repository."
    "example-model-1 | in 921 | out 424 | idle | 9.6s"
)
# What a live view shows, which the finished transcript must not keep.
stale=("(running" "| thinking |" "| editing |")

# Check that file $1 holds the remaining arguments as substrings of lines
# in this order, and none of the stale text.
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
        ! grep -qF -- "$old" "$file" || { echo "stale live text: $old" >&2; return 1; }
    done
}

wait_line() {
    for _ in $(seq "${2:-300}"); do
        "$ULTTYCTL" text | grep -Eq "$1" && return 0
        sleep 0.1
    done
    return 1
}

"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "no shell prompt"

# 1. Native.
"$ULTTYCTL" send-text "$console replay $fixture --speed 2 --auto-approve 1 2>$out/client.log; echo native-done" >/dev/null
"$ULTTYCTL" send-key enter >/dev/null
wait_line 'cargo test -p pager parse:: \(running' || fail "the first tool card never showed"
"$ULTTYCTL" regions >"$out/regions-live.txt"
grep -q '^surface #.*placed at line' "$out/regions-live.txt" ||
    fail "no surface placed while the tool runs (see $out/regions-live.txt)"
shot live
wait_line '^native-done *$' 400 || fail "the native replay never finished"
grep -q "Native mode" "$out/client.log" || fail "the console did not run natively"
"$ULTTYCTL" text --scrollback >"$out/text-native.txt"
in_order "$out/text-native.txt" "${transcript[@]}" native-done ||
    fail "the native transcript does not read in order (see $out/text-native.txt)"
"$ULTTYCTL" regions | grep -q '^surface #' && fail "a card is still listed after the replay"
shot native

# 2. ANSI, in the same window.
"$ULTTYCTL" send-text "clear; env -u ULTTY_SOCKET ULTTY_IN_BAND=0 $console replay $fixture --speed 8 --auto-approve 1 2>$out/ansi-client.log; echo ansi-done" >/dev/null
"$ULTTYCTL" send-key enter >/dev/null
wait_line '^ansi-done *$' 400 || fail "the ANSI replay never finished"
grep -q "Fallback mode" "$out/ansi-client.log" || fail "the console did not fall back to ANSI"
# Only what follows the command line, so the native run cannot pass for it.
"$ULTTYCTL" text --scrollback |
    awk '/ULTTY_IN_BAND=0/ { n = NR } { line[NR] = $0 } END { for (i = n + 1; i <= NR; i++) print line[i] }' \
    >"$out/text-ansi.txt"
in_order "$out/text-ansi.txt" "${transcript[@]}" ansi-done ||
    fail "the ANSI transcript does not read in order (see $out/text-ansi.txt)"
shot ansi

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
if [ "$keep_out" = 1 ]; then
    echo "agent-console: ok ($out)"
else
    rm -rf "$out"
    echo "agent-console: ok"
fi
