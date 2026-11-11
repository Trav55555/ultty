#!/usr/bin/env bash
# Check that Ultty leaves no runtime files behind: an instance stopped by
# SIGTERM, SIGHUP or SIGINT removes its sockets and token on the way out,
# and a new instance sweeps away the ones a killed instance left.
#
#   scripts/e2e/runtime-file-cleanup.sh [artifact-dir]
#
# Needs Xvfb, python3 and a Vulkan or GL driver (Mesa's lavapipe works
# without a GPU). Uses $ULTTY, default target/debug/ultty, built first if
# unset. Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ]; then
    cargo build -q -p ultty --bin ultty
    ULTTY=target/debug/ultty
fi
out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-rtf.XXXXXX)
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

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

log=""
fail() {
    echo "FAIL: $*" >&2
    [ -n "$log" ] && tail -40 "$log" >&2
    ls -la "$runtime" >&2
    exit 1
}

# Whether process $1 runs. Not `kill -0`: that succeeds on our own child
# after it exits, until it is reaped.
running() {
    grep -q '^State:[[:space:]]*[^Z]' "/proc/$1/status" 2>/dev/null
}

# Job control, so the instances below do not start with SIGINT ignored, as
# background jobs of a script otherwise do (and Ultty keeps an inherited
# ignore).
set -m

# Start Ultty with its log in $out/ultty-$1.log and wait for its first
# frame, by which point its sockets and token exist.
start_ultty() {
    log="$out/ultty-$1.log"
    # Without unsetting WAYLAND_DISPLAY, a Wayland session in the
    # environment wins over DISPLAY and the window opens on the desktop.
    env -u WAYLAND_DISPLAY \
        DISPLAY=":$(cat "$out/display")" \
        XDG_RUNTIME_DIR="$runtime" \
        XDG_CONFIG_HOME="$out/config" \
        SHELL=/bin/sh \
        RUST_LOG="ultty::startup=debug,info,wgpu_core=warn,wgpu_hal=warn" \
        "$ULTTY" >"$log" 2>&1 &
    ultty_pid=$!
    for _ in $(seq 600); do
        grep -qs "drawing the first frame" "$log" && break
        running "$ultty_pid" || fail "Ultty exited during startup"
        sleep 0.1
    done
    grep -q "drawing the first frame" "$log" || fail "no first frame within 60 s"
    for file in "ultty-ctl-$ultty_pid.sock" "ultty-auth-$ultty_pid.token" "ultty-$ultty_pid-tab0.sock"; do
        [ -e "$runtime/$file" ] || fail "Ultty did not create $file"
    done
}

# Send signal $1 to the running Ultty, wait for it to exit, and check it
# took its files with it.
stop_ultty() {
    kill -"$1" "$ultty_pid"
    for _ in $(seq 200); do
        running "$ultty_pid" || break
        sleep 0.1
    done
    running "$ultty_pid" && fail "Ultty still runs 20 s after SIG$1"
    local status=0
    wait "$ultty_pid" || status=$?
    [ "$status" -eq 0 ] || fail "Ultty exited with status $status after SIG$1"
    grep -q "Received signal" "$log" || fail "Ultty did not log SIG$1"
    left=$(cd "$runtime" && ls -d "ultty-$ultty_pid-"* "ultty-ctl-$ultty_pid.sock" "ultty-auth-$ultty_pid.token" 2>/dev/null || true)
    [ -z "$left" ] || fail "SIG$1 left behind: $left"
    ! grep -q "panicked at" "$log" || fail "Ultty logged a panic"
    echo "SIG$1: exited cleanly, no files left"
}

# What a SIGKILLed instance leaves: sockets nobody listens on and a token,
# under the pid of a process that is gone.
true &
dead=$!
wait "$dead"
python3 - "$runtime" "$dead" <<'EOF'
import socket, sys
runtime, pid = sys.argv[1], sys.argv[2]
for name in (f"ultty-{pid}-tab0.sock", f"ultty-ctl-{pid}.sock"):
    s = socket.socket(socket.AF_UNIX)
    s.bind(f"{runtime}/{name}")
    s.close()
EOF
echo stale >"$runtime/ultty-auth-$dead.token"
# Not Ultty's to remove: the shared token and other programs' files.
echo shared >"$runtime/ultty-auth.token"
echo other >"$runtime/ultty-notes.txt"

start_ultty sweep
for file in "ultty-$dead-tab0.sock" "ultty-ctl-$dead.sock" "ultty-auth-$dead.token"; do
    [ ! -e "$runtime/$file" ] || fail "the stale $file was not swept"
done
for file in ultty-auth.token ultty-notes.txt; do
    [ -e "$runtime/$file" ] || fail "the sweep removed $file"
done
grep -q "Removed 3 runtime files" "$log" || fail "the sweep did not log 3 removals"
echo "sweep: removed the dead instance's 3 files, kept the rest"
stop_ultty TERM

start_ultty hup
stop_ultty HUP

start_ultty int
stop_ultty INT

echo "runtime-file-cleanup: ok"
