#!/usr/bin/env bash
# Check `ultty -e`: the window's only tab runs the given command instead of
# the shell, and Ultty exits when that command does, as when the last tab's
# program exits. Also checks that a bad argument is refused before any
# window opens.
#
#   scripts/e2e/exec-flag.sh [artifact-dir]
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe works without a
# GPU). Uses $ULTTY and $ULTTYCTL, default target/debug/ultty and ulttyctl,
# built first if unset. Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ]; then
    cargo build -q -p ultty --bins
    ULTTY=${ULTTY:-target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-target/debug/ulttyctl}
fi
out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
# Its own runtime dir also keeps ulttyctl from finding any other Ultty.
runtime=$(mktemp -d /tmp/ultty-exec.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

log="$out/ultty.log"
fail() {
    echo "FAIL: $*" >&2
    [ -f "$log" ] && tail -40 "$log" >&2
    exit 1
}

# A bad argument: usage on stderr, status 2, and no window. No display is
# set, so a window could not open anyway; the status shows Ultty never
# tried.
status=0
env -u WAYLAND_DISPLAY -u DISPLAY "$ULTTY" --bogus >"$out/bogus.out" 2>"$out/bogus.err" || status=$?
[ "$status" -eq 2 ] || fail "ultty --bogus exited $status, want 2"
grep -q "^ultty: unknown argument '--bogus'$" "$out/bogus.err" || fail "no error for --bogus"
grep -q "^Usage: ultty" "$out/bogus.err" || fail "no usage after the error"
[ ! -s "$out/bogus.out" ] || fail "ultty --bogus wrote to stdout"
echo "bad argument: refused with status 2"

Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

# Whether process $1 runs. Not `kill -0`: that succeeds on our own child
# after it exits, until it is reaped.
running() {
    grep -q '^State:[[:space:]]*[^Z]' "/proc/$1/status" 2>/dev/null
}

# Without unsetting WAYLAND_DISPLAY, a Wayland session in the environment
# wins over DISPLAY and the window opens on the desktop. SHELL points at a
# program that fails, so a tab that ran the shell would not print "hi".
env -u WAYLAND_DISPLAY \
    DISPLAY=":$(cat "$out/display")" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/false \
    RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" -e sh -c 'echo hi; sleep 5' >"$log" 2>&1 &
ultty_pid=$!

ctl() {
    env XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid" "$ULTTYCTL" "$@"
}
socket="$runtime/ultty-ctl-$ultty_pid.sock"
for _ in $(seq 300); do
    [ -S "$socket" ] && ctl ping >/dev/null 2>&1 && break
    running "$ultty_pid" || fail "Ultty exited during startup"
    sleep 0.1
done
[ -S "$socket" ] || fail "no control socket at $socket within 30 s"

ctl wait-for hi --timeout 4000 >/dev/null || fail "the first tab did not print hi"
ctl --json tabs >"$out/tabs.json"
[ "$(grep -o '"index"' "$out/tabs.json" | wc -l)" -eq 1 ] || fail "want one tab: $(cat "$out/tabs.json")"
echo "-e: the first tab ran the command"

# The command exits after its sleep; its tab closes and, as the last one,
# takes the window with it.
for _ in $(seq 200); do
    running "$ultty_pid" || break
    sleep 0.1
done
running "$ultty_pid" && fail "Ultty still runs 20 s after the command exited"
status=0
wait "$ultty_pid" || status=$?
[ "$status" -eq 0 ] || fail "Ultty exited with status $status"
! grep -q "panicked at" "$log" || fail "Ultty logged a panic"
echo "-e: Ultty exited with the command"

echo "exec-flag: ok"
