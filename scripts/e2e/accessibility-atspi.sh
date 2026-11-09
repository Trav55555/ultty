#!/usr/bin/env bash
# Check what a screen reader sees of Ultty's terminal over AT-SPI (ADR 0021
# slice 4), on a virtual X display with a private session bus and AT-SPI
# bus: a `terminal` object whose text is the screen, which follows output
# (`echo hello`) and whose caret moves with the cursor.
#
#   scripts/e2e/accessibility-atspi.sh [artifact-dir]
#
# Needs Xvfb, dbus-run-session, at-spi-bus-launcher, the system Python with
# python3-gi and gir1.2-atspi-2.0, and a Vulkan or GL driver (Mesa's
# lavapipe works without a GPU). Uses $ULTTY and $ULTTYCTL, default
# target/debug/ultty and ulttyctl, built first if unset. A local script,
# not a CI job: it says nothing about what Orca speaks
# (docs/accessibility-test.md).
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ "${1:-}" != "--inner" ]; then
    for tool in Xvfb dbus-run-session gdbus; do
        command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
    done
    if [ -z "${ULTTY:-}" ]; then
        cargo build -q -p ultty --bins
        export ULTTY=target/debug/ultty ULTTYCTL=target/debug/ulttyctl
    fi
    out="${1:-$(mktemp -d)}"
    mkdir -p "$out"
    out=$(cd "$out" && pwd)
    # A session bus of our own: the desktop's AT-SPI state is left alone.
    exec dbus-run-session -- "$0" --inner "$out"
fi

out="$2"
ULTTYCTL="${ULTTYCTL:-target/debug/ulttyctl}"
launcher=$(ls /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher 2>/dev/null | head -1 || true)
[ -n "$launcher" ] || { echo "at-spi-bus-launcher is required" >&2; exit 2; }
mkdir -p "$out/config/ultty"
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-atspi.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" "${launcher_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\naccessibility = true\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

"$launcher" --launch-immediately >"$out/at-spi.log" 2>&1 &
launcher_pid=$!
for _ in $(seq 50); do
    gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
        --method org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
    sleep 0.1
done
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>' >/dev/null

Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

# Without unsetting WAYLAND_DISPLAY, a Wayland session in the environment
# wins over DISPLAY and the window opens on the desktop instead of Xvfb.
env -u WAYLAND_DISPLAY \
    DISPLAY=":$(cat "$out/display")" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="ultty::a11y=debug,info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    tail -40 "$out/ultty.log" >&2
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
    window=$(DISPLAY=":$(cat "$out/display")" xdotool search --pid "$ultty_pid" | head -1)
    [ -n "$window" ] && DISPLAY=":$(cat "$out/display")" xdotool windowfocus --sync "$window" \
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
! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
grep "accessibility tree built" "$out/ultty.log" | tail -3 || true

echo "accessibility-atspi: ok (caret $caret_before -> $caret_after)"
