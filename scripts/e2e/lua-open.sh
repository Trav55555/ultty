#!/usr/bin/env bash
# Open Luau tools from Ultty itself with `ulttyctl lua open` (ADR 0023) in a
# fresh Ultty on a virtual X display, and check through `ulttyctl`:
#
#   - `lua open panes.luau --exec ulttyctl` opens a tab drawing natively,
#     with a list row per pane;
#   - `lua open panes.luau` without a grant shows the banner naming the
#     flag to add, and runs nothing;
#   - `lua open hello.luau --split right` opens a split pane next to the
#     shell, drawn with ANSI, and says so on stderr.
#
#   scripts/e2e/lua-open.sh [artifact-dir]
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe works without a GPU).
# Uses $ULTTY and $ULTTYCTL, default target/debug/*, built first if unset;
# `ulttyctl lua open` runs the ulttui-lua next to ulttyctl. Logs and region
# dumps are left in the artifact dir. Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ]; then
    cargo build -q --locked -p ultty --bins -p ulttui-lua
    ULTTY=${ULTTY:-target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-target/debug/ulttyctl}
fi
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")
[ -x "$(dirname "$ULTTYCTL")/ulttui-lua" ] || { echo "no ulttui-lua next to $ULTTYCTL" >&2; exit 2; }

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty" "$out/bin"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
rm -f "$out"/*.json "$out"/*.log "$out"/*.luau "$out"/*.txt
# The tool runs `ulttyctl` by name, from the PATH Ultty hands its children:
# put this build first.
ln -sf "$ULTTYCTL" "$out/bin/ulttyctl"
cp examples/lua/panes.luau "$out/panes.luau"
cp examples/lua/hello.luau "$out/hello.luau"
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
    PATH="$out/bin:$PATH" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    "$ULTTYCTL" panes >&2 || true
    tail -20 "$out/ultty.log" >&2
    for log in "$out"/*.log; do
        case "$log" in *ultty.log|*xvfb.log) ;; *) echo "== $log" >&2; tail -10 "$log" >&2 ;; esac
    done
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

pane_count() {
    "$ULTTYCTL" --json panes | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["panes"]))'
}
# The tab index of pane $1, from `panes`.
tab_of() {
    "$ULTTYCTL" --json panes | python3 -c '
import json, sys
print(next(p["tab_index"] for p in json.load(sys.stdin)["panes"] if p["id"] == int(sys.argv[1])))' "$1"
}
# `regions --json` for pane $2's tab, saved as $out/$1.json.
regions() {
    "$ULTTYCTL" --json --pane "$2" regions >"$out/$1.json" 2>/dev/null || true
}
check_regions() {
    python3 scripts/e2e/lua_regions.py "$out/$1.json" "${@:2}"
}
# Poll until `check_regions $1 ...` passes, within 10 s.
wait_regions() {
    local name=$1 pane=$2
    shift 2
    for _ in $(seq 100); do
        regions "$name" "$pane"
        if check_regions "$name" "$@" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    check_regions "$name" "$@" || true
    return 1
}

# 1. A granted tool in a new tab draws natively: a row per pane (the shell
#    tab and its own).
granted=$("$ULTTYCTL" lua open "$out/panes.luau" --exec ulttyctl --log "$out/granted.log") ||
    fail "lua open with a grant"
[[ "$granted" =~ ^[0-9]+$ ]] || fail "lua open printed '$granted', not a pane id"
[ "$(tab_of "$granted")" = 1 ] || fail "the granted tool is not in a tab of its own"
wait_regions granted "$granted" --list panes --rows 2 --text "Ultty panes" ||
    fail "the granted tool did not list a row per pane"
echo "lua-open: granted tool lists $(pane_count) panes natively in pane $granted"

# 2. Without a grant the same script's `init` fails at its `run{}`, so the
#    pane shows only the banner naming the flag.
denied=$("$ULTTYCTL" lua open "$out/panes.luau" --log "$out/denied.log") ||
    fail "lua open without a grant"
wait_regions denied "$denied" --id ulttui-lua-error \
    --text "panes.luau:10: run: ulttyctl is not allowed; open the script with --exec ulttyctl" ||
    fail "the denied tool did not show the banner naming --exec ulttyctl"
python3 scripts/e2e/lua_regions.py "$out/denied.json" --id panes >/dev/null 2>&1 &&
    fail "the denied tool drew its list, so its init ran past run{}"
echo "lua-open: denied tool shows the banner in pane $denied"

# 3. A split next to the shell draws with ANSI and says so.
"$ULTTYCTL" switch-tab 1 >/dev/null || fail "switch-tab 1"
split=$("$ULTTYCTL" lua open "$out/hello.luau" --split right 2>"$out/note.txt") ||
    fail "lua open --split"
grep -q "draws with ANSI" "$out/note.txt" || fail "no ANSI note for --split (got: $(cat "$out/note.txt"))"
[ "$(tab_of "$split")" = 0 ] || fail "the split pane is not in the shell's tab"
ok=""
for _ in $(seq 100); do
    if "$ULTTYCTL" --pane "$split" text 2>/dev/null | grep -q "Count: 0"; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || { "$ULTTYCTL" --pane "$split" text >&2 || true; fail "hello.luau did not draw in the split pane"; }
# Keys reach it through the pane: the counter goes up.
"$ULTTYCTL" --pane "$split" send-key up >/dev/null
ok=""
for _ in $(seq 50); do
    if "$ULTTYCTL" --pane "$split" text 2>/dev/null | grep -q "Count: 1"; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "the split pane did not take a key"
echo "lua-open: split pane $split draws hello.luau with ANSI"

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
echo "lua-open: ok ($out)"
