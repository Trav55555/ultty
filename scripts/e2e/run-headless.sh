#!/usr/bin/env bash
# Run a command against a fresh Ultty window on a virtual X display, as if
# from a shell inside it: for end-to-end tests in CI or over ssh.
#
#   scripts/e2e/run-headless.sh scripts/e2e/counter-app.sh
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe or llvmpipe works
# without a GPU). Uses target/release/ultty and ulttyctl, built first.
# Exits with the command's status.
set -euo pipefail
cd "$(dirname "$0")/../.."
[ $# -gt 0 ] || { echo "usage: $0 <command> [args...]" >&2; exit 2; }
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

cargo build -q --release -p ultty --bins
work=$(mktemp -d)
# A private runtime dir holds this Ultty's control socket and no other's,
# so neither this script nor the command can reach a window on the
# desktop. Socket paths are short (108 bytes), so it lives under /tmp.
runtime=$(mktemp -d /tmp/ultty-headless.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$work" "$runtime"
}
trap cleanup EXIT

# Xvfb picks a free display and writes its number to fd 3.
Xvfb -displayfd 3 -screen 0 1280x800x24 3>"$work/display" 2>"$work/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$work/display" ] && break; sleep 0.1; done
[ -s "$work/display" ] || { echo "Xvfb did not start" >&2; cat "$work/xvfb.log" >&2; exit 1; }

# Without this, a Wayland session in the environment wins over DISPLAY
# and the window opens on the desktop instead of Xvfb.
export XDG_RUNTIME_DIR="$runtime"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN
env -u WAYLAND_DISPLAY DISPLAY=":$(cat "$work/display")" target/release/ultty >"$work/ultty.log" 2>&1 &
ultty_pid=$!
export ULTTY_PID=$ultty_pid PATH="$PWD/target/release:$PATH"
for _ in $(seq 80); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && ulttyctl --pid "$ultty_pid" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || break
    sleep 0.25
done
ulttyctl --pid "$ultty_pid" ping >/dev/null || { echo "Ultty did not start" >&2; cat "$work/ultty.log" >&2; exit 1; }

"$@"
