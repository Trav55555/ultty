#!/usr/bin/env bash
# ADR 0027 slice 1 GUI smoke: open two windows in a single Ultty process via
# the main menu ("New window"), type distinct commands into both windows via
# X11 input (xdotool), verify each pane receives only its own input, close the
# second window while keeping the first alive, and verify closing the last
# window exits the process cleanly.
#
#   scripts/e2e/multi-window.sh [artifact-dir]
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }
command -v xdotool >/dev/null || { echo "xdotool is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ]; then
    cargo build -q --locked -p ultty --bins
    ULTTY=${ULTTY:-${CARGO_TARGET_DIR:-target}/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-${CARGO_TARGET_DIR:-target}/debug/ulttyctl}
fi
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
out=$(cd "$out" && pwd)
rm -f "$out/ultty.log" "$out/xvfb.log" "$out/display" "$out/startup.png"

runtime=$(mktemp -d "${TMPDIR:-/tmp}/mw.XXXXXX")
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }
display=":$(cat "$out/display")"

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

env -u WAYLAND_DISPLAY \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,ultty::startup=debug,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    "$ULTTYCTL" panes >&2 || true
    tail -30 "$out/ultty.log" >&2
    exit 1
}

xdotool_on() { env -u WAYLAND_DISPLAY DISPLAY="$display" xdotool "$@"; }

export XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Ultty never answered on its control socket"

# Wait for the GPU renderer to finish startup so window 2 uses the shared GPU.
gpu_ready=0
for _ in $(seq 120); do
    if "$ULTTYCTL" screenshot "$out/startup.png" >/dev/null 2>&1; then
        gpu_ready=1
        break
    fi
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited while waiting for GPU"
    sleep 0.25
done
[ "$gpu_ready" -eq 1 ] || fail "Ultty GPU never became ready"

"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "window 1 shell prompt missing"

win1=$(xdotool_on search --onlyvisible --pid "$ultty_pid" | head -1 || true)
[ -n "$win1" ] || fail "could not find first X11 window for pid $ultty_pid"

pane1=$("$ULTTYCTL" --json panes | python3 -c 'import json,sys; panes=json.load(sys.stdin)["panes"]; assert len(panes)==1; print(panes[0]["id"])')

# Open the main menu (top-left Menu button) and click "New window".
xdotool_on windowraise "$win1"
xdotool_on windowfocus --sync "$win1"
xdotool_on mousemove --window "$win1" 24 22
xdotool_on click 1
sleep 0.3
xdotool_on mousemove --window "$win1" 60 88
xdotool_on click 1

# Wait for a second pane and a second X11 window owned by the SAME process.
pane2=""
for _ in $(seq 100); do
    pane2=$("$ULTTYCTL" --json panes | python3 -c '
import json, sys
panes = json.load(sys.stdin)["panes"]
if len(panes) == 2:
    print(panes[1]["id"])
' 2>/dev/null || true)
    [ -n "$pane2" ] && break
    sleep 0.1
done
[ -n "$pane2" ] || fail "New window did not create a second pane in-process"

win2=""
for _ in $(seq 100); do
    wins=$(xdotool_on search --onlyvisible --pid "$ultty_pid" || true)
    count=$(printf '%s\n' "$wins" | grep -c '[0-9]' || true)
    if [ "$count" -eq 2 ]; then
        win2=$(printf '%s\n' "$wins" | grep -v "^${win1}$" | head -1)
        break
    fi
    sleep 0.1
done
[ -n "$win2" ] || fail "expected 2 visible X11 windows owned by pid $ultty_pid"

# Verify only one ultty process owns sockets in this runtime dir.
ctl_count=$(ls "$runtime"/ultty-ctl-*.sock 2>/dev/null | wc -l)
[ "$ctl_count" -eq 1 ] || fail "expected 1 control socket (in-process windows), found $ctl_count"

"$ULTTYCTL" --pane "$pane2" wait-for '$' --timeout 20000 >/dev/null || fail "window 2 shell prompt missing"

# Place the two windows side by side so clicks and focus are unambiguous.
xdotool_on windowmove "$win1" 20 40
xdotool_on windowmove "$win2" 440 120
sleep 0.2

# Focus window 1 and type a marker via X11 keyboard input.
xdotool_on windowraise "$win1"
xdotool_on windowfocus --sync "$win1"
xdotool_on mousemove --window "$win1" 200 200
xdotool_on click 1
sleep 0.2
xdotool_on type --delay 10 "echo WIN1_OK"
xdotool_on key Return

"$ULTTYCTL" --pane "$pane1" wait-for 'WIN1_OK' --timeout 10000 >/dev/null || fail "window 1 did not receive keyboard input"

# Focus window 2 and type a different marker via X11 keyboard input.
xdotool_on windowraise "$win2"
xdotool_on windowfocus --sync "$win2"
xdotool_on mousemove --window "$win2" 200 200
xdotool_on click 1
sleep 0.2
xdotool_on type --delay 10 "echo WIN2_OK"
xdotool_on key Return

"$ULTTYCTL" --pane "$pane2" wait-for 'WIN2_OK' --timeout 10000 >/dev/null || fail "window 2 did not receive keyboard input"

# Verify input was routed strictly to the focused window.
if "$ULTTYCTL" --pane "$pane1" text | grep -q 'WIN2_OK'; then
    fail "window 2 input leaked into window 1"
fi
if "$ULTTYCTL" --pane "$pane2" text | grep -q 'WIN1_OK'; then
    fail "window 1 input leaked into window 2"
fi

# Close window 2 via its CSD Close button (top-right of the 800px title bar):
# process and window 1 must remain alive and interactive.
xdotool_on windowraise "$win2"
xdotool_on windowfocus --sync "$win2"
xdotool_on mousemove --window "$win2" 776 22
xdotool_on click 1
for _ in $(seq 100); do
    pane_count=$("$ULTTYCTL" --json panes | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["panes"]))' 2>/dev/null || echo 0)
    [ "$pane_count" -eq 1 ] && break
    sleep 0.1
done
[ "$pane_count" -eq 1 ] || fail "closing window 2 did not drop to 1 pane (got $pane_count)"
kill -0 "$ultty_pid" 2>/dev/null || fail "closing window 2 exited the whole process"

xdotool_on windowraise "$win1"
xdotool_on windowfocus --sync "$win1"
xdotool_on mousemove --window "$win1" 200 200
xdotool_on click 1
sleep 0.2
xdotool_on type --delay 10 "echo STILL_ALIVE"
xdotool_on key Return
"$ULTTYCTL" --pane "$pane1" wait-for 'STILL_ALIVE' --timeout 10000 >/dev/null || fail "window 1 stopped responding after window 2 closed"

# Close window 1 (the last window) via its CSD Close button: on Linux the
# process must exit cleanly.
xdotool_on mousemove --window "$win1" 776 22
xdotool_on click 1
exited=0
for _ in $(seq 100); do
    if ! kill -0 "$ultty_pid" 2>/dev/null; then
        exited=1
        break
    fi
    sleep 0.1
done
[ "$exited" -eq 1 ] || fail "closing the last window did not exit Ultty on Linux"
wait "$ultty_pid" || fail "Ultty exited with non-zero status"
ultty_pid=""

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"

echo "multi-window: ok ($out)"
