#!/usr/bin/env bash
# Check what a screen reader sees of Ultty over AT-SPI (ADR 0021 slices 4
# and 5), on a virtual X display with a private session bus and AT-SPI bus:
# a `terminal` object whose text is the screen, which follows output (`echo
# hello`) and whose caret moves with the cursor; then, with the native
# `dashboard` running in it, its list's selected item exposed as selected
# and the others as selectable, the selection following `j`; then, with
# the native `counter-app`, a `button` per button, the focused button as
# the focused object, and Tab moving that focus.
#
#   scripts/e2e/accessibility-atspi.sh [artifact-dir]
#
# Needs Xvfb, dbus-run-session, at-spi-bus-launcher, the system Python with
# python3-gi and gir1.2-atspi-2.0, and a Vulkan or GL driver (Mesa's
# lavapipe works without a GPU); xdotool, if present, focuses the window,
# without which the focus checks are skipped. Uses $ULTTY, $ULTTYCTL,
# $COUNTER_APP and $DASHBOARD, default target/debug/ultty, ulttyctl,
# counter-app and dashboard: all built first if $ULTTY is unset, the two
# examples if theirs is unset. Artifacts go to artifact-dir if given, else to a
# temporary directory removed on success. A local script, not a CI job:
# it says nothing about what Orca speaks (docs/accessibility-test.md).
set -euo pipefail

if [ "${1:-}" != "--inner" ]; then
    # Paths the caller gave are relative to where it ran us.
    absolute() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
    self=$(absolute "$0")
    if [ -n "${ULTTY:-}" ]; then ULTTY=$(absolute "$ULTTY"); fi
    if [ -n "${ULTTYCTL:-}" ]; then ULTTYCTL=$(absolute "$ULTTYCTL"); fi
    if [ -n "${COUNTER_APP:-}" ]; then COUNTER_APP=$(absolute "$COUNTER_APP"); fi
    if [ -n "${DASHBOARD:-}" ]; then DASHBOARD=$(absolute "$DASHBOARD"); fi
    if [ -n "${1:-}" ]; then
        out=$(absolute "$1")
        keep=true
    else
        out=$(mktemp -d)
        keep=false
    fi
    cd "$(dirname "$self")/../.."
    for tool in Xvfb dbus-run-session gdbus; do
        command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
    done
    if [ -z "${ULTTY:-}" ]; then
        cargo build -q -p ultty --bins -p counter-app -p dashboard
        ULTTY=$PWD/target/debug/ultty
        ULTTYCTL=$PWD/target/debug/ulttyctl
        COUNTER_APP=$PWD/target/debug/counter-app
        DASHBOARD=$PWD/target/debug/dashboard
    fi
    if [ -z "${COUNTER_APP:-}" ]; then
        cargo build -q -p counter-app
        COUNTER_APP=$PWD/target/debug/counter-app
    fi
    if [ -z "${DASHBOARD:-}" ]; then
        cargo build -q -p dashboard
        DASHBOARD=$PWD/target/debug/dashboard
    fi
    export ULTTY ULTTYCTL COUNTER_APP DASHBOARD
    mkdir -p "$out"
    # A session bus of our own: the desktop's AT-SPI state is left alone.
    exec dbus-run-session -- "$self" --inner "$out" "$keep"
fi

cd "$(dirname "$0")/../.."
out="$2"
keep="$3"
ULTTYCTL="${ULTTYCTL:-$PWD/target/debug/ulttyctl}"
COUNTER_APP="${COUNTER_APP:-$PWD/target/debug/counter-app}"
DASHBOARD="${DASHBOARD:-$PWD/target/debug/dashboard}"
launcher=$(ls /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher 2>/dev/null | head -1 || true)
[ -n "$launcher" ] || { echo "at-spi-bus-launcher is required" >&2; exit 2; }
mkdir -p "$out/config/ultty"
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-atspi.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${launcher_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\naccessibility = true\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

# Xvfb first: on X11 the launcher publishes the AT-SPI bus address on the
# display's root window, which must be Xvfb's, not the caller's desktop.
rm -f "$out/display"
Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }

# Everything from here (the launcher, the client, Ultty) is on Xvfb.
# Without unsetting WAYLAND_DISPLAY, a Wayland session in the environment
# wins over DISPLAY and the window opens on the desktop instead.
DISPLAY=":$(cat "$out/display")"
export DISPLAY
unset WAYLAND_DISPLAY

"$launcher" --launch-immediately >"$out/at-spi.log" 2>&1 &
launcher_pid=$!
for _ in $(seq 50); do
    gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
        --method org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
    sleep 0.1
done
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>' >/dev/null

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

env XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="ultty::a11y=debug,info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    tail -40 "$out/ultty.log" >&2
    echo "artifacts in $out" >&2
    exit 1
}

# This instance's own control socket, so ulttyctl cannot reach another.
socket="$runtime/ultty-ctl-$ultty_pid.sock"
for _ in $(seq 600); do
    [ -S "$socket" ] && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
[ -S "$socket" ] || fail "no control socket within 60 s"
ctl() { XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid" "$ULTTYCTL" "$@"; }
# The shell has drawn its prompt once the output goes quiet.
ctl wait-for --quiet 500 --timeout 30000 >/dev/null || fail "the shell never settled"

read_terminal() { /usr/bin/python3 -I scripts/e2e/atspi_terminal.py "$ultty_pid" "$@"; }

before=$(read_terminal --timeout 30) || fail "no terminal object on the AT-SPI bus: $before"
echo "before: $before" | tee "$out/before.json"
echo "$before" | grep -q '"role": "terminal"' || fail "the object is not a terminal: $before"

# Xvfb runs no window manager, so nothing focuses the window; without
# focus AT-SPI reports no focused object. Focus it when xdotool is there.
focus_checked=false
if command -v xdotool >/dev/null; then
    window=$(xdotool search --pid "$ultty_pid" | head -1)
    [ -n "$window" ] && xdotool windowfocus --sync "$window" \
        >"$out/xdotool.log" 2>&1 && focus_checked=true
fi

ctl send-text $'echo hello\r' >/dev/null || fail "ulttyctl send-text failed"
after=$(read_terminal --wait-for $'hello\n$ ' --timeout 15) ||
    fail "the terminal's text never showed the output: $after"
echo "after: $after" | tee "$out/after.json"

caret() { sed -n 's/.*"caret": \([-0-9]*\).*/\1/p'; }
caret_before=$(echo "$before" | caret)
caret_after=$(echo "$after" | caret)
[ "$caret_after" -gt "$caret_before" ] ||
    fail "the caret did not move: $caret_before then $caret_after"
if $focus_checked; then
    echo "$after" | grep -q '"focused": true' ||
        fail "the window has focus but the terminal is not the focused object"
fi

# With a screen reader listening and nothing changing on screen, nothing is
# rebuilt and Ultty does not wake beyond what it does without one. `idle
# <name> <max>` measures 5 s after 3 s to settle, allowing <max> context
# switches, and keeps each thread's switches in the artifacts.
switches() {
    cat /proc/"$ultty_pid"/task/*/status 2>/dev/null |
        awk '/_ctxt_switches/ { s += $2 } END { print s + 0 }'
}
builds() { grep -c "accessibility tree built" "$out/ultty.log" || true; }
threads() {
    for task in /proc/"$ultty_pid"/task/*; do
        printf '%s %s\n' "$(cat "$task/comm" 2>/dev/null)" \
            "$(awk '/_ctxt_switches/ { s += $2 } END { print s + 0 }' "$task/status" 2>/dev/null)"
    done
}
idle() {
    local switches_before builds_before woke rebuilt
    sleep 3
    threads >"$out/threads-$1-before.txt"
    switches_before=$(switches)
    builds_before=$(builds)
    sleep 5
    woke=$(( $(switches) - switches_before ))
    rebuilt=$(( $(builds) - builds_before ))
    threads >"$out/threads-$1-after.txt"
    echo "idle with AT active ($1), 5 s: $woke context switches, $rebuilt trees built" |
        tee -a "$out/idle.txt"
    [ "$rebuilt" -eq 0 ] || fail "a still screen ($1) rebuilt the tree $rebuilt times in 5 s"
    [ "$woke" -le "$2" ] || fail "Ultty woke $woke times in 5 s of idle ($1, at most $2)"
}
idle prompt 2

# Slice 5: a native app's regions. counter-app connects over the tab's
# socket (the shell has ULTTY_SOCKET) and shows a full scene.
read_widgets() { /usr/bin/python3 -I scripts/e2e/atspi_widgets.py "$ultty_pid" "$@"; }
# The label of the region the app has focused, from `ulttyctl regions`.
native_focus() {
    ctl --json regions | /usr/bin/python3 -I scripts/e2e/native_focus.py
}
# Selection (ADR 0021 §1 `selected`): dashboard's list marks the item it
# has selected, and its other items are selectable but not selected.
ctl send-text "$(printf %q "$DASHBOARD")"$'\r' >/dev/null || fail "ulttyctl send-text failed"
widgets=$(read_widgets --selected "System Overview" --unselected "Network" --timeout 30) ||
    fail "dashboard's first item is not the selected one: $widgets"
echo "$widgets" >"$out/selected-first.json"
ctl send-text j >/dev/null || fail "ulttyctl send-text failed"
widgets=$(read_widgets --selected "Network" --unselected "System Overview" --timeout 15) ||
    fail "after j the selection did not move to Network: $widgets"
echo "$widgets" >"$out/selected-second.json"
echo "native: selection System Overview -> Network"
ctl send-key c ctrl >/dev/null || fail "ulttyctl send-key failed"
ctl wait-for --quiet 500 --timeout 30000 >/dev/null || fail "the shell never settled after dashboard"

ctl send-text "exec $(printf %q "$COUNTER_APP")"$'\r' >/dev/null || fail "ulttyctl send-text failed"
buttons=(--role button --name "-1" --role button --name "+1"
    --role button --name "Reset")
widgets=$(read_widgets "${buttons[@]}" --timeout 30) ||
    fail "no buttons -1, +1 and Reset on the AT-SPI bus: $widgets"
echo "$widgets" >"$out/widgets.json"
echo "native: -1, +1 and Reset are buttons"
# Slice 7: what `ulttyctl regions` reports agrees.
ctl --json regions | /usr/bin/python3 -I scripts/e2e/native_semantics.py \
    "role=button,label=-1" "role=button,label=+1" "role=button,label=Reset" \
    >"$out/regions.txt" || { cat "$out/regions.txt" >&2; fail "counter-app's regions are not named buttons"; }

focus_moves="not checked (no xdotool)"
if $focus_checked; then
    first=$(native_focus)
    [ -n "$first" ] || fail "counter-app focused no labelled region: $(ctl regions)"
    widgets=$(read_widgets --focused "$first" --timeout 15) ||
        fail "the focused object is not the app's focused button $first: $widgets"
    echo "$widgets" | grep -q '"focused": {"role": "button"' ||
        fail "the focused object is not a button: $widgets"
    ctl send-key tab >/dev/null || fail "ulttyctl send-key failed"
    second=$first
    for _ in $(seq 50); do
        second=$(native_focus)
        [ -n "$second" ] && [ "$second" != "$first" ] && break
        sleep 0.1
    done
    [ "$second" != "$first" ] || fail "Tab did not move the app's focus from $first"
    widgets=$(read_widgets --focused "$second" --timeout 15) ||
        fail "after Tab the focused object is not $second: $widgets"
    echo "$widgets" >"$out/widgets-after-tab.json"
    focus_moves="$first -> $second"
    echo "native: focus $focus_moves"
fi

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
grep "accessibility tree built" "$out/ultty.log" | tail -3 || true
# A connected native app keeps the 250 ms maintenance tick (keepalives,
# animations; MAINTENANCE_POLL_INTERVAL), 20 wakeups in 5 s with or without
# a screen reader. The tree must not be rebuilt on it; the tolerance over
# those 20 absorbs a stray scheduler switch on a loaded machine.
idle counter-app 25

echo "accessibility-atspi: ok (caret $caret_before -> $caret_after; focus $focus_moves)"
# Artifacts are kept when the caller named a directory; on failure, always.
$keep || rm -rf "$out"
