#!/usr/bin/env bash
# End-to-end test of a native app in a split pane (ADR 0025): in a fresh
# Ultty on a virtual X display, split the shell's pane and run
# examples/counter-app natively in the new pane, then check that
#   - `regions --pane <app>` shows one scene laid out to the pane's grid,
#     and `regions --pane <shell>` shows none;
#   - keys sent to the app's pane count;
#   - resizing the window re-lays the scene out to the pane's new grid;
#   - a screenshot of the split tab draws something in the app's half.
#
#   scripts/e2e/native-split.sh [artifact-dir]
#
# Needs Xvfb, xdotool, and a Vulkan or GL driver (Mesa's lavapipe works without a GPU).
# Uses $ULTTY, $ULTTYCTL and $COUNTER_APP, default target/debug/*, built
# first if unset. Logs, region dumps and the screenshot are left in the
# artifact dir. Exits non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }
command -v xdotool >/dev/null || { echo "xdotool is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ] || [ -z "${COUNTER_APP:-}" ]; then
    cargo build -q --locked -p ultty --bins -p counter-app
    ULTTY=${ULTTY:-${CARGO_TARGET_DIR:-target}/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-${CARGO_TARGET_DIR:-target}/debug/ulttyctl}
    COUNTER_APP=${COUNTER_APP:-${CARGO_TARGET_DIR:-target}/debug/counter-app}
fi
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")
COUNTER_APP=$(realpath "$COUNTER_APP")

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
# XDG_CONFIG_HOME is ignored unless it is absolute.
out=$(cd "$out" && pwd)
rm -f "$out"/*.json "$out"/*.log "$out"/*.png
# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d "${TMPDIR:-/tmp}/us.XXXXXX")
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
    "$ULTTYCTL" --json panes >&2 || true
    tail -20 "$out/ultty.log" >&2
    exit 1
}

# Talk only to this Ultty: its own control socket, found through its pid.
export XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN ULTTY_PANE
for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Ultty never answered on its control socket"
"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "no shell prompt"

# Field $2 of pane $1, from `panes`.
pane_field() {
    "$ULTTYCTL" --json panes | python3 -c '
import json, sys
print(next(p[sys.argv[2]] for p in json.load(sys.stdin)["panes"] if p["id"] == int(sys.argv[1])))' "$1" "$2"
}
shell_pane=$("$ULTTYCTL" --json panes | python3 -c 'import json,sys; print(json.load(sys.stdin)["panes"][0]["id"])')

# `regions --json` for pane $2, saved as $out/$1.json.
regions() {
    "$ULTTYCTL" --json --pane "$2" regions >"$out/$1.json" 2>/dev/null || true
}
check() {
    python3 scripts/e2e/native_split_check.py "$@"
}
# Poll until `check regions $out/$1.json <args>` passes for pane $2, within 10 s.
wait_regions() {
    local name=$1 pane=$2
    shift 2
    for _ in $(seq 100); do
        regions "$name" "$pane"
        if check regions "$out/$name.json" "$@" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    check regions "$out/$name.json" "$@" || true
    return 1
}

# 1. The app in a right split draws natively, laid out to its pane's grid.
app_pane=$("$ULTTYCTL" --pane "$shell_pane" split right "$COUNTER_APP") || fail "split"
[[ "$app_pane" =~ ^[0-9]+$ ]] || fail "split printed '$app_pane', not a pane id"
cols=$(pane_field "$app_pane" cols)
rows=$(pane_field "$app_pane" rows)
wait_regions app "$app_pane" --cols "$cols" --rows "$rows" --text Counter --text 0 ||
    fail "counter-app did not draw natively in pane $app_pane at ${cols}x${rows}"
regions shell "$shell_pane"
check no-scenes "$out/shell.json" || fail "the shell's pane lists a scene"
echo "native-split: counter-app draws in pane $app_pane (${cols}x${rows}); the shell pane has no scene"

# 2. Keys sent to the app's pane reach the app.
for _ in 1 2 3; do
    "$ULTTYCTL" --pane "$app_pane" send-text + >/dev/null
done
wait_regions count "$app_pane" --cols "$cols" --rows "$rows" --text 3 ||
    fail "three presses of + did not count to 3"
echo "native-split: keys reach the app"

# 3. A window resize re-lays the scene out to the pane's new grid. The
#    reply waits for the window to have the size, even on X11 where the
#    resize is applied after the request.
"$ULTTYCTL" resize 1000 700 >"$out/resize.log" 2>&1 ||
    fail "ulttyctl resize 1000 700 failed: $(cat "$out/resize.log")"
ok=""
for _ in $(seq 100); do
    new_cols=$(pane_field "$app_pane" cols)
    new_rows=$(pane_field "$app_pane" rows)
    if [ "$new_cols" != "$cols" ] || [ "$new_rows" != "$rows" ]; then
        ok=1
        break
    fi
    sleep 0.1
done
[ -n "$ok" ] || fail "the pane did not change size after resize (still ${cols}x${rows})"
wait_regions resized "$app_pane" --cols "$new_cols" --rows "$new_rows" --text 3 ||
    fail "the scene was not laid out to the pane's new grid ${new_cols}x${new_rows}"
echo "native-split: resize re-lays the scene out to ${new_cols}x${new_rows}"

# 4. A screenshot of the split tab shows the app in its half: the counter's
#    card and text are ink inside the right pane (its interior, clear of the
#    1 px border and the tab bar), while the shell's pane, which shows only
#    a prompt and a cursor, has next to none. The second check is what
#    keeps the first honest: a frame with no scene drawn looks like the
#    shell's pane.
"$ULTTYCTL" screenshot "$out/split.png" >/dev/null || fail "screenshot"
check screenshot "$out/split.png" --width 1000 --height 700 \
    --inside 512 60 992 690 --min-ink 1000 ||
    fail "the screenshot does not show the app in its pane"
check screenshot "$out/split.png" --width 1000 --height 700 \
    --inside 8 60 490 690 --max-ink 300 ||
    fail "the shell's pane has more drawn in it than a prompt"
echo "native-split: screenshot has the app's pane"

# Two additional apps deliberately omit the local socket, so the ordinary
# PTY probe uses the same in-band path as SSH. Both must retain independent
# scenes and input even though their foreground groups may differ.
first_inband=$("$ULTTYCTL" --pane "$shell_pane" split down env -u ULTTY_SOCKET "$COUNTER_APP") || fail "first in-band split"
second_inband=$("$ULTTYCTL" --pane "$first_inband" split right env -u ULTTY_SOCKET "$COUNTER_APP") || fail "second in-band split"
for pane in "$first_inband" "$second_inband"; do
    cols=$(pane_field "$pane" cols)
    rows=$(pane_field "$pane" rows)
    wait_regions "inband-$pane" "$pane" --cols "$cols" --rows "$rows" --text Counter --text 0 ||
        fail "in-band app did not draw in pane $pane"
done
"$ULTTYCTL" --pane "$first_inband" send-text + >/dev/null || fail "first in-band input"
cols=$(pane_field "$first_inband" cols)
rows=$(pane_field "$first_inband" rows)
wait_regions inband-count "$first_inband" --cols "$cols" --rows "$rows" --text 1 || fail "first in-band count"
cols=$(pane_field "$second_inband" cols)
rows=$(pane_field "$second_inband" rows)
wait_regions inband-isolated "$second_inband" --cols "$cols" --rows "$rows" --text 0 || fail "sibling in-band scene changed"
# The last split is focused. Send an actual X key and verify that only
# its in-band app advances; a tab-wide broadcast would also increment first.
window=$(env -u WAYLAND_DISPLAY DISPLAY="$display" xdotool search --onlyvisible --pid "$ultty_pid" | head -1)
[ -n "$window" ] || fail "could not find the Ultty window"
env -u WAYLAND_DISPLAY DISPLAY="$display" xdotool windowfocus "$window"
env -u WAYLAND_DISPLAY DISPLAY="$display" xdotool type --window "$window" '+'
wait_regions inband-gui "$second_inband" --cols "$cols" --rows "$rows" --text 1 || fail "GUI key did not reach focused in-band app"
cols=$(pane_field "$first_inband" cols)
rows=$(pane_field "$first_inband" rows)
wait_regions inband-gui-isolated "$first_inband" --cols "$cols" --rows "$rows" --text 1 || fail "GUI key also reached sibling"
"$ULTTYCTL" --pane "$first_inband" send-text q >/dev/null 2>&1 || true
"$ULTTYCTL" --pane "$second_inband" send-text q >/dev/null 2>&1 || true
echo "native-split: independent in-band panes"

"$ULTTYCTL" --pane "$app_pane" send-text q >/dev/null 2>&1 || true
echo "native-split: ok"
