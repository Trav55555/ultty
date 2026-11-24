#!/usr/bin/env bash
# ADR 0028 Slice 1 GUI smoke: verify quick terminal role, `ultty --quick`
# handoff & cold-start, `ulttyctl toggle-quick`/`show-quick`/`hide-quick`,
# X11 position/size/mapped state flipping, PID preservation across hide/show,
# exclusion of hidden quick window from default targets and session saves,
# and keeping the process alive when the last normal window closes while a
# quick terminal exists.
#
#   scripts/e2e/quick-terminal.sh [artifact-dir]
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
rm -f "$out/ultty.log" "$out/ultty-cold.log" "$out/xvfb.log" "$out/display"

runtime=$(mktemp -d "${TMPDIR:-/tmp}/qt.XXXXXX")
chmod 700 "$runtime"
cleanup() {
    kill "${second_pid-}" "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

fail() {
    echo "quick-terminal: $*" >&2
    for log in "$out/ultty.log" "$out/ultty-cold.log" "$out/ultty-hotkey.log" "$out/ultty-hotkey-conflict.log"; do
        [ -s "$log" ] && { echo "--- $log ---" >&2; tail -40 "$log" >&2; }
    done
    exit 1
}

cat >"$out/config/ultty/config.toml" <<'EOF'
[behavior]
cursor_blink = false

[quick_terminal]
position = "top"
size = "40%"
autohide = false
EOF

Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }
display=":$(cat "$out/display")"

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

xdotool_on() {
    DISPLAY="$display" xdotool "$@"
}

# Start a normal Ultty instance first.
env -u WAYLAND_DISPLAY \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,ultty::startup=debug,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!
export XDG_RUNTIME_DIR="$runtime"
export ULTTY_PID="$ultty_pid"

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Ultty never answered on control socket"

# `describe` lists quick terminal operations.
"$ULTTYCTL" --json describe | python3 -c '
import json, sys
ops = set(json.load(sys.stdin)["supported_operations"])
for op in ("toggle_quick", "show_quick", "hide_quick"):
    assert op in ops, f"missing {op}"
' || fail "describe missing quick terminal operations"

# Toggle quick terminal on via `ultty --quick` (handoff to running instance).
env -u WAYLAND_DISPLAY -u ULTTY_PID \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    "$ULTTY" --quick || fail "ultty --quick (show) exited non-zero"

ctl_count=$(ls "$runtime"/ultty-ctl-*.sock 2>/dev/null | wc -l)
[ "$ctl_count" -eq 1 ] || fail "ultty --quick spawned a second process ($ctl_count sockets)"

quick_win=$("$ULTTYCTL" --json windows | python3 -c '
import json, sys
wins = json.load(sys.stdin)["windows"]
quick = [w for w in wins if w.get("quick")]
assert len(quick) == 1, wins
q = quick[0]
assert q["visible"] is True, q
assert q["width"] == 1280 and q["height"] == 320, q
print(q["id"])
') || fail "quick window missing or wrong geometry after first toggle"

# Verify the X11 window with WM_CLASS `ultty-quick` is mapped (`--onlyvisible`).
quick_xid=""
for _ in $(seq 50); do
    quick_xid=$(xdotool_on search --onlyvisible --class ultty-quick 2>/dev/null | head -1 || true)
    [ -n "$quick_xid" ] && break
    sleep 0.1
done
[ -n "$quick_xid" ] || fail "quick window not mapped on X11 with class ultty-quick"

# Record quick pane ID and shell PID.
quick_pane_info=$("$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = [p for p in json.load(sys.stdin)['panes'] if p['window'] == $quick_win]
assert len(panes) == 1, panes
print(panes[0]['id'], panes[0]['pid'])
") || fail "quick window pane not found"
read -r quick_pane quick_shell_pid <<<"$quick_pane_info"

# Toggle quick terminal off via `ultty --quick`.
env -u WAYLAND_DISPLAY -u ULTTY_PID \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    "$ULTTY" --quick || fail "ultty --quick (hide) exited non-zero"

"$ULTTYCTL" --json windows | python3 -c "
import json, sys
wins = {w['id']: w for w in json.load(sys.stdin)['windows']}
assert wins[$quick_win]['quick'] is True and wins[$quick_win]['visible'] is False, wins
assert wins[$quick_win]['focused'] is False, wins
" || fail "quick window did not report visible=false after hide"

# X11 mapped state must flip to unmapped.
unmapped=0
for _ in $(seq 50); do
    vis=$(xdotool_on search --onlyvisible --class ultty-quick 2>/dev/null || true)
    if [ -z "$vis" ]; then
        unmapped=1
        break
    fi
    sleep 0.1
done
[ "$unmapped" -eq 1 ] || fail "quick window remained mapped on X11 after hide"

# Shell process inside hidden quick terminal must still be alive.
kill -0 "$quick_shell_pid" 2>/dev/null || fail "quick shell exited when quick terminal hid"

# Hidden quick terminal must be excluded from default `new-tab` targeting.
new_tab_pane=$("$ULTTYCTL" --json new-tab | python3 -c 'import json, sys; print(json.load(sys.stdin)["pane"])') \
    || fail "new-tab while quick hidden failed"
"$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = {p['id']: p for p in json.load(sys.stdin)['panes']}
assert panes[$new_tab_pane]['window'] == 0, panes
" || fail "new-tab targeted hidden quick window instead of normal window 0"

# `workspace save` must exclude the quick terminal (ADR 0028 Decision 2).
"$ULTTYCTL" workspace save qt-check || fail "workspace save failed"
python3 -c '
import json, sys
data = json.load(open(sys.argv[1]))
assert data["version"] == 2 and len(data["windows"]) == 1, data
' "$out/config/ultty/sessions/qt-check.json" || fail "workspace save included quick terminal window"

# Closing the last normal window must keep the process alive while quick exists.
"$ULTTYCTL" close-window 0 || fail "close-window 0 failed"
sleep 0.3
kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited when normal window closed despite existing quick terminal"

# Toggle quick terminal back on: X11 window re-maps and shell PID is unchanged.
env -u WAYLAND_DISPLAY -u ULTTY_PID \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    "$ULTTY" --quick || fail "ultty --quick (second show) exited non-zero"

remapped=0
for _ in $(seq 50); do
    vis=$(xdotool_on search --onlyvisible --class ultty-quick 2>/dev/null | head -1 || true)
    if [ -n "$vis" ]; then
        remapped=1
        break
    fi
    sleep 0.1
done
[ "$remapped" -eq 1 ] || fail "quick window did not re-map on X11 after second toggle"

"$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = {p['id']: p for p in json.load(sys.stdin)['panes']}
assert panes[$quick_pane]['window'] == $quick_win, panes
assert panes[$quick_pane]['pid'] == $quick_shell_pid, (panes[$quick_pane]['pid'], $quick_shell_pid)
" || fail "quick terminal shell PID changed across hide/show"

# Closing the quick window when no normal windows remain exits cleanly on Linux.
"$ULTTYCTL" close-window "$quick_win" || fail "close-window quick failed"
exited=0
for _ in $(seq 100); do
    if ! kill -0 "$ultty_pid" 2>/dev/null; then
        exited=1
        break
    fi
    sleep 0.1
done
[ "$exited" -eq 1 ] || fail "closing last (quick) window did not exit Ultty on Linux"
wait "$ultty_pid" || fail "Ultty exited with non-zero status"
ultty_pid=""

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"

# Cold-start `ultty --quick` with no running instance starts a quick-only Ultty.
env -u WAYLAND_DISPLAY -u ULTTY_PID \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,ultty::startup=debug,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" --quick >"$out/ultty-cold.log" 2>&1 &
ultty_pid=$!
export ULTTY_PID="$ultty_pid"

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Cold-start quick Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Cold-start quick Ultty never answered on control socket"

"$ULTTYCTL" --json windows | python3 -c '
import json, sys
wins = json.load(sys.stdin)["windows"]
assert len(wins) == 1 and wins[0]["quick"] is True and wins[0]["visible"] is True, wins
' || fail "cold-start ultty --quick did not open a single visible quick window"

[ "$("$ULTTYCTL" hide-quick)" = "hidden" ] || fail "hide-quick did not print hidden"
[ "$("$ULTTYCTL" show-quick)" = "shown" ] || fail "show-quick did not print shown"

kill -TERM "$ultty_pid"
wait "$ultty_pid" || fail "Cold-start quick Ultty exited non-zero on SIGTERM"
ultty_pid=""
! grep -q "panicked at" "$out/ultty-cold.log" || fail "Cold-start quick Ultty logged a panic"

# Slice 3: X11 global hotkey (`quick_terminal.hotkey = "ctrl+grave"`).
# Verify `xdotool key ctrl+grave` toggles the quick terminal on, off, and on
# again while preserving its shell PID, and that a second instance conflicting
# on the same grab logs a clear error without crashing.
cat >"$out/config/ultty/config.toml" <<'EOF'
[behavior]
cursor_blink = false

[quick_terminal]
position = "top"
size = "40%"
autohide = false
hotkey = "ctrl+grave"
EOF

env -u WAYLAND_DISPLAY -u ULTTY_PID \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,ultty::startup=debug,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty-hotkey.log" 2>&1 &
ultty_pid=$!
export ULTTY_PID="$ultty_pid"

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Hotkey Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Hotkey Ultty never answered on control socket"

grep -q "Registered X11 global hotkey for quick terminal" "$out/ultty-hotkey.log" \
    || fail "Hotkey Ultty did not log X11 global hotkey registration"

# Press ctrl+grave via X11 root input: quick terminal appears.
xdotool_on key --clearmodifiers ctrl+grave
hk_quick_win=""
for _ in $(seq 80); do
    hk_quick_win=$("$ULTTYCTL" --json windows 2>/dev/null | python3 -c '
import json, sys
wins = [w for w in json.load(sys.stdin)["windows"] if w.get("quick") and w.get("visible")]
if wins:
    print(wins[0]["id"])
' 2>/dev/null || true)
    [ -n "$hk_quick_win" ] && break
    sleep 0.1
done
[ -n "$hk_quick_win" ] || fail "xdotool key ctrl+grave did not open quick terminal"

hk_shell_pid=$("$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = [p for p in json.load(sys.stdin)['panes'] if p['window'] == $hk_quick_win]
assert len(panes) == 1, panes
print(panes[0]['pid'])
") || fail "hotkey quick terminal pane not found"

# Second ctrl+grave hides the quick terminal without killing its shell.
xdotool_on key --clearmodifiers ctrl+grave
hk_hidden=0
for _ in $(seq 80); do
    if "$ULTTYCTL" --json windows 2>/dev/null | python3 -c "
import json, sys
wins = {w['id']: w for w in json.load(sys.stdin)['windows']}
sys.exit(0 if wins[$hk_quick_win]['visible'] is False else 1)
" 2>/dev/null; then
        hk_hidden=1
        break
    fi
    sleep 0.1
done
[ "$hk_hidden" -eq 1 ] || fail "second ctrl+grave did not hide quick terminal"
kill -0 "$hk_shell_pid" 2>/dev/null || fail "quick shell died on hotkey hide"

# Third ctrl+grave shows it again with the exact same shell PID.
xdotool_on key --clearmodifiers ctrl+grave
hk_reshown=0
for _ in $(seq 80); do
    if "$ULTTYCTL" --json windows 2>/dev/null | python3 -c "
import json, sys
wins = {w['id']: w for w in json.load(sys.stdin)['windows']}
sys.exit(0 if wins[$hk_quick_win]['visible'] is True else 1)
" 2>/dev/null; then
        hk_reshown=1
        break
    fi
    sleep 0.1
done
[ "$hk_reshown" -eq 1 ] || fail "third ctrl+grave did not re-show quick terminal"
"$ULTTYCTL" --json panes | python3 -c "
import json, sys
panes = [p for p in json.load(sys.stdin)['panes'] if p['window'] == $hk_quick_win]
assert len(panes) == 1 and panes[0]['pid'] == $hk_shell_pid, panes
" || fail "hotkey quick terminal PID changed across toggle"

# A second process requesting the same hotkey must report a clear grab conflict
# without crashing.
env -u WAYLAND_DISPLAY -u ULTTY_PID \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" --new-process >"$out/ultty-hotkey-conflict.log" 2>&1 &
second_pid=$!

for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$second_pid.sock" ] && "$ULTTYCTL" --pid "$second_pid" ping >/dev/null 2>&1 && break
    kill -0 "$second_pid" 2>/dev/null || fail "Conflict-test Ultty exited during startup"
    sleep 0.1
done
grep -q "already grabbed" "$out/ultty-hotkey-conflict.log" \
    || fail "Second Ultty did not report hotkey grab conflict"

kill -TERM "$second_pid" 2>/dev/null || true
wait "$second_pid" 2>/dev/null || true
second_pid=""
kill -TERM "$ultty_pid" 2>/dev/null || true
wait "$ultty_pid" 2>/dev/null || true
ultty_pid=""

! grep -q "panicked at" "$out/ultty-hotkey.log" || fail "Hotkey Ultty logged a panic"
! grep -q "panicked at" "$out/ultty-hotkey-conflict.log" || fail "Conflict-test Ultty logged a panic"

echo "quick-terminal: ok ($out)"
