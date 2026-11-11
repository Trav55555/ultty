#!/usr/bin/env bash
# Open Luau tools from Ultty itself with `ulttyctl lua open` (ADR 0024) in a
# fresh Ultty on a virtual X display, and check through `ulttyctl`:
#
#   - `lua open panes.luau --exec 'ulttyctl --json panes'` opens a tab
#     drawing natively, with a list row per pane, in the caller's directory;
#   - `lua open panes.luau` without a grant shows the banner naming the
#     flag to add, and runs nothing;
#   - `--pane 0 lua open hello.luau --split right` opens a split pane next
#     to the shell in tab 1, drawn with ANSI, and says so on stderr;
#   - `lua open` with `--exec sh` or a missing file fails on its own stderr
#     and opens no pane;
#   - an error only ulttui-lua finds (an unwritable --log) stays in its
#     pane until a key is pressed.
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
ULTTUI_LUA="$(dirname "$ULTTYCTL")/ulttui-lua"
[ -x "$ULTTUI_LUA" ] || { echo "no ulttui-lua next to $ULTTYCTL" >&2; exit 2; }

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

# A grant that would allow anything is refused before any UI, naming the
# flag that says so.
if "$ULTTUI_LUA" --exec sh "$out/hello.luau" 2>"$out/refused.txt"; then
    echo "FAIL: ulttui-lua --exec sh started" >&2; exit 1
fi
grep -q -- '--allow-exec' "$out/refused.txt" ||
    { echo "FAIL: refusing --exec sh did not name --allow-exec" >&2; cat "$out/refused.txt" >&2; exit 1; }
echo "lua-open: --exec sh refused in favour of --allow-exec"
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
# Field $2 of pane $1, from `panes`.
pane_field() {
    "$ULTTYCTL" --json panes | python3 -c '
import json, sys
print(next(p[sys.argv[2]] for p in json.load(sys.stdin)["panes"] if p["id"] == int(sys.argv[1])))' "$1" "$2"
}
tab_of() { pane_field "$1" tab_index; }
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
#    tab and its own). Relative paths and the pane's directory are the
#    caller's.
granted=$(cd "$out" && "$ULTTYCTL" lua open panes.luau --exec 'ulttyctl --json panes' --log granted.log) ||
    fail "lua open with a grant"
[[ "$granted" =~ ^[0-9]+$ ]] || fail "lua open printed '$granted', not a pane id"
[ "$(tab_of "$granted")" = 1 ] || fail "the granted tool is not in a tab of its own"
[ "$(pane_field "$granted" cwd)" = "$out" ] ||
    fail "the granted tool runs in $(pane_field "$granted" cwd), not the caller's $out"
[ -f "$out/granted.log" ] || fail "a relative --log was not made absolute against the caller's directory"
wait_regions granted "$granted" --list panes --rows 2 --text "Ultty panes" ||
    fail "the granted tool did not list a row per pane"
echo "lua-open: granted tool lists $(pane_count) panes natively in pane $granted"

# 2. Without a grant the same script's `init` fails at its `run{}`, so the
#    pane shows only the banner naming the flag.
denied=$("$ULTTYCTL" lua open "$out/panes.luau" --log "$out/denied.log") ||
    fail "lua open without a grant"
wait_regions denied "$denied" --id ulttui-lua-error \
    --text "panes.luau:10: run: ulttyctl --json panes is not allowed; open the script with --exec 'ulttyctl --json panes'" ||
    fail "the denied tool did not show the banner naming the grant to add"
python3 scripts/e2e/lua_regions.py "$out/denied.json" --id panes >/dev/null 2>&1 &&
    fail "the denied tool drew its list, so its init ran past run{}"
echo "lua-open: denied tool shows the banner in pane $denied"

# 3. A split next to pane 0 (the shell, in a tab that is not the visible
#    one) draws with ANSI and says so.
split=$("$ULTTYCTL" --pane 0 lua open "$out/hello.luau" --split right 2>"$out/note.txt") ||
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

# 4. From inside a pane, `split` with no --pane splits that pane (Ultty
#    sets ULTTY_PANE), not the visible tab's focused one.
"$ULTTYCTL" --pane 0 send-text 'ulttyctl split down' >/dev/null
"$ULTTYCTL" --pane 0 send-key enter >/dev/null
ok=""
for _ in $(seq 100); do
    panes_in_tab0=$("$ULTTYCTL" --json panes | python3 -c '
import json, sys
print(sum(1 for p in json.load(sys.stdin)["panes"] if p["tab_index"] == 0))')
    if [ "$panes_in_tab0" = 3 ]; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "split from inside pane 0 did not add a pane to its tab (tab 0 has $panes_in_tab0 panes)"
echo "lua-open: split from inside pane 0 lands in its tab"

# 5. What ulttui-lua would refuse at startup, `lua open` refuses itself,
#    on its own stderr, and opens no pane: a grant on a program runner and
#    a missing file.
before=$(pane_count)
if "$ULTTYCTL" lua open "$out/hello.luau" --exec sh 2>"$out/open-refused.txt"; then
    fail "lua open --exec sh succeeded"
fi
grep -q -- '--allow-exec' "$out/open-refused.txt" ||
    fail "lua open --exec sh did not name --allow-exec (got: $(cat "$out/open-refused.txt"))"
if "$ULTTYCTL" lua open "$out/missing.luau" 2>"$out/open-missing.txt"; then
    fail "lua open of a missing file succeeded"
fi
grep -q 'no such file' "$out/open-missing.txt" ||
    fail "lua open of a missing file did not say so (got: $(cat "$out/open-missing.txt"))"
[ "$(pane_count)" = "$before" ] || fail "a refused lua open opened a pane"
echo "lua-open: refused grant and missing file reported by ulttyctl, no pane opened"

# 6. An error only ulttui-lua can find (here a log it cannot create) stays
#    on screen in its pane until a key is pressed, then the pane closes.
held=$("$ULTTYCTL" lua open "$out/hello.luau" --log "$out/no-such-dir/x.log") ||
    fail "lua open with an unwritable log"
ok=""
for _ in $(seq 100); do
    if "$ULTTYCTL" --pane "$held" text 2>/dev/null | grep -q "press any key to close"; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "pane $held did not hold its startup error"
"$ULTTYCTL" --pane "$held" text | grep -q "cannot create log file" ||
    fail "pane $held does not say why it could not start"
# Still there a moment later: it waits for the key, not a timer.
sleep 1
[ "$(pane_count)" = "$((before + 1))" ] || fail "pane $held closed before a key was pressed"
"$ULTTYCTL" --pane "$held" send-key enter >/dev/null
ok=""
for _ in $(seq 100); do
    if [ "$(pane_count)" = "$before" ]; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "pane $held did not close after a key"
echo "lua-open: startup error held in pane $held until a key"

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
echo "lua-open: ok ($out)"
