#!/usr/bin/env bash
# Run examples/lua/panes.luau with ulttui-lua in a fresh Ultty on a virtual X
# display and check, through `ulttyctl regions`, that its list has a row per
# pane, and that rewriting a label in (a copy of) the script shows up within
# a second. Then run examples/lua/hello.luau on a plain pty, outside Ultty,
# and check its ANSI output.
#
#   scripts/e2e/lua-panes.sh [artifact-dir]
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe works without a GPU).
# Uses $ULTTY, $ULTTYCTL and $ULTTUI_LUA, default target/debug/*, built first
# if unset. Logs and region dumps are left in the artifact dir. Exits
# non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ] || [ -z "${ULTTUI_LUA:-}" ]; then
    cargo build -q --locked -p ultty --bins -p ulttui-lua
    ULTTY=${ULTTY:-target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-target/debug/ulttyctl}
    ULTTUI_LUA=${ULTTUI_LUA:-target/debug/ulttui-lua}
fi
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")
ULTTUI_LUA=$(realpath "$ULTTUI_LUA")

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty" "$out/bin"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
rm -f "$out"/*.json "$out"/*.log "$out/panes.luau"
# The script runs `ulttyctl` by name: put this build first on its PATH.
ln -sf "$ULTTYCTL" "$out/bin/ulttyctl"
cp examples/lua/panes.luau "$out/panes.luau"
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-lua.XXXXXX)
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
    [ -f "$out/lua.log" ] && tail -20 "$out/lua.log" >&2
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
"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "no shell prompt"

# A second tab, so there is more than one pane to list; then back to the
# first, where the app runs.
"$ULTTYCTL" new-tab >/dev/null || fail "new-tab"
for _ in $(seq 50); do
    [ "$("$ULTTYCTL" --json panes | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["panes"]))')" = 2 ] && break
    sleep 0.1
done
"$ULTTYCTL" switch-tab 1 >/dev/null || fail "switch-tab 1"
app_pane=$("$ULTTYCTL" --json panes | python3 -c '
import json, sys
print(next(p["id"] for p in json.load(sys.stdin)["panes"] if p["tab_index"] == 0))')

"$ULTTYCTL" send-text "PATH=$out/bin:\$PATH exec $ULTTUI_LUA --allow-exec --log $out/lua.log $out/panes.luau" >/dev/null
"$ULTTYCTL" send-key enter >/dev/null

# `regions --json` for the app's tab, saved as $out/<name>.json.
regions() {
    "$ULTTYCTL" --json --pane "$app_pane" regions >"$out/$1.json" 2>/dev/null || true
}
# Texts of the regions whose ancestor has id metadata $2, from dump $1.
check_regions() {
    python3 scripts/e2e/lua_regions.py "$out/$1.json" "${@:2}"
}

ok=""
for _ in $(seq 100); do
    regions rows
    if check_regions rows --list panes --rows 2 --text "Ultty panes" >/dev/null 2>&1; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || { check_regions rows --list panes --rows 2 --text "Ultty panes" || true; fail "no list with a row per pane"; }
check_regions rows --list panes --rows 2 --text "Ultty panes"

# Rewrite the title in the running copy, the way an editor saves (write a
# new file, rename it over the old one), and time how long until `regions`
# shows it.
sed 's/"Ultty panes"/"Pane list (edited)"/' "$out/panes.luau" >"$out/panes.luau.new"
start=$(date +%s%N)
mv "$out/panes.luau.new" "$out/panes.luau"
seen=""
while [ $(( ($(date +%s%N) - start) / 1000000 )) -lt 1000 ]; do
    regions edited
    if check_regions edited --text "Pane list (edited)" --list panes --rows 2 >/dev/null 2>&1; then
        seen=$(( ($(date +%s%N) - start) / 1000000 ))
        break
    fi
    sleep 0.02
done
[ -n "$seen" ] || fail "the edited title did not reach regions within 1 s"
echo "lua-panes: edit visible in regions after ${seen} ms"

# A broken save keeps the app (and its rows) and shows the error banner.
printf 'return app {\n' >"$out/panes.luau.new"
mv "$out/panes.luau.new" "$out/panes.luau"
ok=""
for _ in $(seq 50); do
    regions broken
    if check_regions broken --list panes --rows 2 --id ulttui-lua-error --text "panes.luau:2:" >/dev/null 2>&1; then
        ok=1
        break
    fi
    sleep 0.05
done
[ -n "$ok" ] || { check_regions broken --list panes --rows 2 --id ulttui-lua-error --text "panes.luau:2:" || true; fail "a broken save did not show the error banner over the running app"; }

# Reload latency, natively: hello.luau in the second tab, saved 20 times.
"$ULTTYCTL" switch-tab 2 >/dev/null || fail "switch-tab 2"
hello_pane=$("$ULTTYCTL" --json panes | python3 -c '
import json, sys
print(next(p["id"] for p in json.load(sys.stdin)["panes"] if p["tab_index"] == 1))')
cp examples/lua/hello.luau "$out/hello.luau"
"$ULTTYCTL" --pane "$hello_pane" send-text "exec $ULTTUI_LUA --log $out/hello.log $out/hello.luau" >/dev/null
"$ULTTYCTL" --pane "$hello_pane" send-key enter >/dev/null
ok=""
for _ in $(seq 100); do
    "$ULTTYCTL" --json --pane "$hello_pane" regions >"$out/hello.json" 2>/dev/null || true
    if python3 scripts/e2e/lua_regions.py "$out/hello.json" --id count --text "Count: 0" >/dev/null 2>&1; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "hello.luau never drew natively"
python3 scripts/e2e/lua_reload_latency.py --script "$out/hello.luau" --log "$out/hello.log" |
    tee "$out/latency-native.txt"

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
echo "lua-panes: native ok ($out)"

python3 scripts/e2e/lua_ansi_pty.py "$ULTTUI_LUA" examples/lua/hello.luau "$out/hello-ansi.bin"
