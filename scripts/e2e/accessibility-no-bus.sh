#!/usr/bin/env bash
# Start Ultty with `accessibility = true` and no session D-Bus, on a virtual
# X display, and check it still draws its first frame: the AccessKit
# adapter must neither fail nor block startup when there is no bus (headless
# CI, ADR 0021).
#
#   scripts/e2e/accessibility-no-bus.sh [artifact-dir]
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe works without a
# GPU). Uses $ULTTY, default target/debug/ultty, built first if unset.
# Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ]; then
    cargo build -q -p ultty --bin ultty
    ULTTY=target/debug/ultty
fi
out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-a11y.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\naccessibility = true\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

# Xvfb picks a free display and writes its number to fd 3.
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
# The bus address names a socket that does not exist: no session bus at all
# (an unset address would fall back to $XDG_RUNTIME_DIR/bus).
env -u WAYLAND_DISPLAY \
    DISPLAY=":$(cat "$out/display")" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/no-such-bus" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    RUST_LOG="ultty::startup=debug,info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    tail -40 "$out/ultty.log" >&2
    exit 1
}

for _ in $(seq 600); do
    grep -q "drawing the first frame" "$out/ultty.log" && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
grep -q "drawing the first frame" "$out/ultty.log" || fail "no first frame within 60 s"
grep -q "accessibility adapter ready" "$out/ultty.log" ||
    fail "the accessibility adapter was not created"
# Give the adapter's thread time to try the bus, then check nothing broke.
sleep 1
kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited after its first frame"
! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"

echo "accessibility-no-bus: ok"
