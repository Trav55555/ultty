#!/usr/bin/env bash
# ADR 0027 slices 1, 3, and 4 GUI smoke: open windows in a single Ultty process
# via the main menu ("New window") and `ulttyctl new-window`, verify X11 input
# isolation, `describe`, `windows`, `--window` / `--pane` targeting (`new-tab`,
# `resize`, `screenshot`), `window_opened`/`window_closed` subscribe events,
# closing a window while keeping another alive, closing the last window, and
# SessionData v2 multi-window save/restore with logical window sizes.
#
#   scripts/e2e/multi-window.sh [artifact-dir]
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }
command -v xdotool >/dev/null || { echo "xdotool is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ]; then
    cargo build -q --locked -p ultty --bins
    ULTTY=${ULTTY:-${CARGO_TARGET_DIR:-target}/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-${CARGO_TARGET_DIR:-target}/debug/ulttyctl}
fi
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
out=$(cd "$out" && pwd)
rm -f "$out/ultty.log" "$out/xvfb.log" "$out/display" "$out/startup.png"

runtime=$(mktemp -d "${TMPDIR:-/tmp}/mw.XXXXXX")
chmod 700 "$runtime"
cleanup() {
    kill "${sub_pid-}" "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }
display=":$(cat "$out/display")"

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

env -u WAYLAND_DISPLAY \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,ultty::startup=debug,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    "$ULTTYCTL" panes >&2 || true
    tail -30 "$out/ultty.log" >&2
    exit 1
}

xdotool_on() { env -u WAYLAND_DISPLAY DISPLAY="$display" xdotool "$@"; }

export XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN ULTTY_PANE

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Ultty never answered on its control socket"

# Wait for the GPU renderer to finish startup so window 2 uses the shared GPU.
gpu_ready=0
for _ in $(seq 120); do
    if "$ULTTYCTL" screenshot "$out/startup.png" >/dev/null 2>&1; then
        gpu_ready=1
        break
    fi
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited while waiting for GPU"
    sleep 0.25
done
[ "$gpu_ready" -eq 1 ] || fail "Ultty GPU never became ready"

"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "window 1 shell prompt missing"

win1=$(xdotool_on search --onlyvisible --pid "$ultty_pid" | head -1 || true)
[ -n "$win1" ] || fail "could not find first X11 window for pid $ultty_pid"

pane1=$("$ULTTYCTL" --json panes | python3 -c 'import json,sys; panes=json.load(sys.stdin)["panes"]; assert len(panes)==1 and panes[0]["window"]==0; print(panes[0]["id"])')

# Verify describe advertises the slice 3 window control operations.
"$ULTTYCTL" --json describe | python3 -c '
import json, sys
ops = set(json.load(sys.stdin)["supported_operations"])
for op in ("list_windows", "new_window", "close_window"):
    assert op in ops, f"missing {op} in describe"
' || fail "describe missing window operations"

# Start an unfiltered V2 event subscriber to capture window_opened/window_closed
# and pane events carrying the window field.
rm -f "$out/events.jsonl"
"$ULTTYCTL" subscribe-v2 >"$out/events.jsonl" 2>&1 &
sub_pid=$!
for _ in $(seq 50); do
    grep -q '"subscribed_v2"' "$out/events.jsonl" 2>/dev/null && break
    sleep 0.05
done
grep -q '"subscribed_v2"' "$out/events.jsonl" 2>/dev/null || fail "subscribe-v2 never connected"

"$ULTTYCTL" --json windows | python3 -c '
import json, sys
wins = json.load(sys.stdin)["windows"]
assert len(wins) == 1 and wins[0]["id"] == 0 and wins[0]["tabs"] == [0], wins
' || fail "initial windows listing mismatch"

# Open the main menu (top-left Menu button) and click "New window".
xdotool_on windowraise "$win1"
xdotool_on windowfocus --sync "$win1"
xdotool_on mousemove --window "$win1" 24 22
xdotool_on click 1
sleep 0.3
xdotool_on mousemove --window "$win1" 60 88
xdotool_on click 1

# Wait for a second pane and a second X11 window owned by the SAME process.
pane2=""
for _ in $(seq 100); do
    pane2=$("$ULTTYCTL" --json panes | python3 -c '
import json, sys
panes = json.load(sys.stdin)["panes"]
if len(panes) == 2:
    print(panes[1]["id"])
' 2>/dev/null || true)
    [ -n "$pane2" ] && break
    sleep 0.1
done
[ -n "$pane2" ] || fail "New window did not create a second pane in-process"

win2=""
for _ in $(seq 100); do
    wins=$(xdotool_on search --onlyvisible --pid "$ultty_pid" || true)
    count=$(printf '%s\n' "$wins" | grep -c '[0-9]' || true)
    if [ "$count" -eq 2 ]; then
        win2=$(printf '%s\n' "$wins" | grep -v "^${win1}$" | head -1)
        break
    fi
    sleep 0.1
done
[ -n "$win2" ] || fail "expected 2 visible X11 windows owned by pid $ultty_pid"

# Verify only one ultty process owns sockets in this runtime dir.
ctl_count=$(ls "$runtime"/ultty-ctl-*.sock 2>/dev/null | wc -l)
[ "$ctl_count" -eq 1 ] || fail "expected 1 control socket (in-process windows), found $ctl_count"

"$ULTTYCTL" --pane "$pane2" wait-for '$' --timeout 20000 >/dev/null || fail "window 2 shell prompt missing"

# Place the two windows side by side so clicks and focus are unambiguous.
xdotool_on windowmove "$win1" 20 40
xdotool_on windowmove "$win2" 440 120
sleep 0.2

# Focus window 1 and type a marker via X11 keyboard input.
xdotool_on windowraise "$win1"
xdotool_on windowfocus --sync "$win1"
xdotool_on mousemove --window "$win1" 200 200
xdotool_on click 1
sleep 0.2
xdotool_on type --delay 10 "echo WIN1_OK"
xdotool_on key Return

"$ULTTYCTL" --pane "$pane1" wait-for 'WIN1_OK' --timeout 10000 >/dev/null || fail "window 1 did not receive keyboard input"

# Focus window 2 and type a different marker via X11 keyboard input.
xdotool_on windowraise "$win2"
xdotool_on windowfocus --sync "$win2"
xdotool_on mousemove --window "$win2" 200 200
xdotool_on click 1
sleep 0.2
xdotool_on type --delay 10 "echo WIN2_OK"
xdotool_on key Return

"$ULTTYCTL" --pane "$pane2" wait-for 'WIN2_OK' --timeout 10000 >/dev/null || fail "window 2 did not receive keyboard input"

# Verify input was routed strictly to the focused window.
if "$ULTTYCTL" --pane "$pane1" text | grep -q 'WIN2_OK'; then
    fail "window 2 input leaked into window 1"
fi
if "$ULTTYCTL" --pane "$pane2" text | grep -q 'WIN1_OK'; then
    fail "window 1 input leaked into window 2"
fi

# Close window 2 via its CSD Close button (top-right of the 800px title bar):
# process and window 1 must remain alive and interactive.
xdotool_on windowraise "$win2"
xdotool_on windowfocus --sync "$win2"
xdotool_on mousemove --window "$win2" 776 22
xdotool_on click 1
for _ in $(seq 100); do
    pane_count=$("$ULTTYCTL" --json panes | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["panes"]))' 2>/dev/null || echo 0)
    [ "$pane_count" -eq 1 ] && break
    sleep 0.1
done
[ "$pane_count" -eq 1 ] || fail "closing window 2 did not drop to 1 pane (got $pane_count)"
kill -0 "$ultty_pid" 2>/dev/null || fail "closing window 2 exited the whole process"

xdotool_on windowraise "$win1"
xdotool_on windowfocus --sync "$win1"
xdotool_on mousemove --window "$win1" 200 200
xdotool_on click 1
sleep 0.2
xdotool_on type --delay 10 "echo STILL_ALIVE"
xdotool_on key Return
"$ULTTYCTL" --pane "$pane1" wait-for 'STILL_ALIVE' --timeout 10000 >/dev/null || fail "window 1 stopped responding after window 2 closed"

# Close window 1 (the last window) via `ulttyctl close-window` after testing
# slice 3 `new-window`, `--window`, `--pane` window targeting, and events.
win3_json=$("$ULTTYCTL" --json new-window) || fail "ulttyctl new-window failed"
win3_id=$(printf '%s' "$win3_json" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["window"]>=1; print(d["window"])')
win3_pane=$(printf '%s' "$win3_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])')

"$ULTTYCTL" --pane "$win3_pane" wait-for '$' --timeout 20000 >/dev/null || fail "new-window shell prompt missing"

# Still one control socket for the process.
ctl_count=$(ls "$runtime"/ultty-ctl-*.sock 2>/dev/null | wc -l)
[ "$ctl_count" -eq 1 ] || fail "expected 1 control socket after new-window, found $ctl_count"

# `--window 0 new-tab` opens in window 0 even though win3 is focused.
tab_in_win0=$("$ULTTYCTL" --window 0 new-tab) || fail "--window 0 new-tab failed"
"$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = {p['id']: p for p in json.load(sys.stdin)['panes']}
assert panes[$tab_in_win0]['window'] == 0, panes
assert panes[$win3_pane]['window'] == $win3_id, panes
" || fail "--window 0 new-tab opened in wrong window"

# `--pane <win3_pane> new-tab` opens in win3 even after focusing window 0.
"$ULTTYCTL" --window 0 focus || fail "--window 0 focus failed"
tab_in_win3=$("$ULTTYCTL" --pane "$win3_pane" new-tab) || fail "--pane new-tab failed"
"$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = {p['id']: p for p in json.load(sys.stdin)['panes']}
assert panes[$tab_in_win3]['window'] == $win3_id, panes
" || fail "--pane new-tab opened in wrong window"

# `--window 0 resize` resizes only window 0; `--window <win3_id> screenshot` captures win3.
"$ULTTYCTL" --window 0 resize 920 640 || fail "--window 0 resize failed"
"$ULTTYCTL" --json windows | python3 -c "
import json, sys
wins = {w['id']: w for w in json.load(sys.stdin)['windows']}
assert wins[0]['width'] == 920 and wins[0]['height'] == 640, wins
assert wins[$win3_id]['width'] != 920 or wins[$win3_id]['height'] != 640, wins
assert len(wins[0]['tabs']) == 2 and len(wins[$win3_id]['tabs']) == 2, wins
" || fail "window resize or tab ownership mismatch"

# Wait briefly for win3's GPU surface to become ready, then screenshot it via --window.
win3_shot=0
for _ in $(seq 80); do
    if "$ULTTYCTL" --window "$win3_id" screenshot "$out/win3.png" >/dev/null 2>&1; then
        win3_shot=1
        break
    fi
    sleep 0.1
done
[ "$win3_shot" -eq 1 ] && [ -s "$out/win3.png" ] || fail "--window $win3_id screenshot failed"

# Save the 2-window layout via `workspace save` and verify SessionData v2 schema.
"$ULTTYCTL" workspace save mw-s4 || fail "workspace save mw-s4 failed"
ws_file="$out/config/ultty/sessions/mw-s4.json"
[ -s "$ws_file" ] || fail "workspace save did not write $ws_file"
python3 -c '
import json, sys
data = json.load(open(sys.argv[1]))
assert data["version"] == 2, data
assert "tabs" not in data, data
wins = data["windows"]
assert len(wins) == 2, wins
assert len(wins[0]["tabs"]) == 2 and len(wins[1]["tabs"]) == 2, wins
assert wins[0]["size"]["width"] == 920.0 and wins[0]["size"]["height"] == 640.0, wins
assert wins[1]["size"]["width"] == 800.0 and wins[1]["size"]["height"] == 600.0, wins
' "$ws_file" || fail "saved v2 workspace file mismatch"

# Unknown window IDs fail cleanly.
if "$ULTTYCTL" --window 999 focus >/dev/null 2>&1; then
    fail "expected --window 999 focus to fail"
fi

# `close-window <win3_id>` closes only win3 and keeps window 0 alive.
"$ULTTYCTL" close-window "$win3_id" || fail "close-window $win3_id failed"
"$ULTTYCTL" --json windows | python3 -c '
import json, sys
wins = json.load(sys.stdin)["windows"]
assert [w["id"] for w in wins] == [0], wins
' || fail "windows list after close-window mismatch"

# Verify subscribe stream recorded window_opened, window_closed, and pane window IDs.
for _ in $(seq 50); do
    grep -q '"window_closed"' "$out/events.jsonl" 2>/dev/null && break
    sleep 0.05
done
kill "$sub_pid" 2>/dev/null || true
wait "$sub_pid" 2>/dev/null || true
sub_pid=""
python3 -c '
import json, sys
records = [json.loads(line) for line in open(sys.argv[1]) if line.strip().startswith("{")]
events = [r["event"] for r in records if isinstance(r.get("event"), dict)]
kinds = [e.get("event") for e in events]
assert "window_opened" in kinds, kinds
assert "window_closed" in kinds, kinds
pane_opened = [e for e in events if e.get("event") == "pane_opened"]
assert pane_opened and all("window" in e for e in pane_opened), pane_opened
' "$out/events.jsonl" || fail "subscribe stream missing window events or window field"

# Close window 0 (the last window) via `close-window`: on Linux the process
# must exit cleanly.
"$ULTTYCTL" close-window 0 || fail "close-window 0 failed"
exited=0
for _ in $(seq 100); do
    if ! kill -0 "$ultty_pid" 2>/dev/null; then
        exited=1
        break
    fi
    sleep 0.1
done
[ "$exited" -eq 1 ] || fail "closing the last window did not exit Ultty on Linux"
wait "$ultty_pid" || fail "Ultty exited with non-zero status"
ultty_pid=""

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"

# Slice 4: restore the saved 2-window v2 session via `restore_session = true`,
# verify both windows reopen in-process with their saved sizes and tabs, then
# exit cleanly via SIGTERM so ShutdownSignal saves both windows back to last.json.
printf '[behavior]\ncursor_blink = false\nrestore_session = true\n' >"$out/config/ultty/config.toml"
cp "$ws_file" "$out/config/ultty/sessions/last.json"

env -u WAYLAND_DISPLAY \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,ultty::startup=debug,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty-restore.log" 2>&1 &
ultty_pid=$!
export ULTTY_PID="$ultty_pid"

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Restored Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Restored Ultty never answered on control socket"

restored_ok=0
for _ in $(seq 120); do
    if "$ULTTYCTL" --json windows 2>/dev/null | python3 -c '
import json, sys
wins = json.load(sys.stdin)["windows"]
if len(wins) != 2:
    sys.exit(1)
assert len(wins[0]["tabs"]) == 2 and len(wins[1]["tabs"]) == 2, wins
assert wins[0]["width"] == 920 and wins[0]["height"] == 640, wins
assert wins[1]["width"] == 800 and wins[1]["height"] == 600, wins
' 2>/dev/null; then
        restored_ok=1
        break
    fi
    kill -0 "$ultty_pid" 2>/dev/null || fail "Restored Ultty exited while restoring windows"
    sleep 0.1
done
[ "$restored_ok" -eq 1 ] || fail "Session v2 restore did not reopen 2 windows with saved sizes and tabs"

ctl_count=$(ls "$runtime"/ultty-ctl-*.sock 2>/dev/null | wc -l)
[ "$ctl_count" -eq 1 ] || fail "expected 1 control socket after multi-window restore, found $ctl_count"

kill -TERM "$ultty_pid"
exited=0
for _ in $(seq 100); do
    if ! kill -0 "$ultty_pid" 2>/dev/null; then
        exited=1
        break
    fi
    sleep 0.1
done
[ "$exited" -eq 1 ] || fail "Restored Ultty did not exit cleanly on SIGTERM"
wait "$ultty_pid" || fail "Restored Ultty exited with non-zero status on SIGTERM"
ultty_pid=""

python3 -c '
import json, sys
data = json.load(open(sys.argv[1]))
assert data["version"] == 2 and len(data["windows"]) == 2, data
assert len(data["windows"][0]["tabs"]) == 2 and len(data["windows"][1]["tabs"]) == 2, data
' "$out/config/ultty/sessions/last.json" || fail "ShutdownSignal did not preserve both windows in last.json"

! grep -q "panicked at" "$out/ultty-restore.log" || fail "Restored Ultty logged a panic"

echo "multi-window: ok ($out)"
