#!/usr/bin/env bash
# Headless screenshots of the command palette on a private Xvfb.
# usage: palette-shot.sh <out-dir> <WxH> [theme]
set -euo pipefail
repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
out=$1; size=$2; theme=${3-}
mkdir -p "$out"
work=$(mktemp -d "$out/work.XXXX")
runtime=$(mktemp -d /tmp/ucp.XXXXXX); chmod 700 "$runtime"
mkdir -p "$work/config/ultty" "$work/home"
if [ "$theme" = light ]; then printf '[colors]\nforeground = "#1e1e1e"\nbackground = "#fafafa"\ncursor = "#2f6fdf"\n' >"$work/config/ultty/config.toml"; fi
cleanup() {
    kill "${ultty_pid-}" 2>/dev/null || true
    sleep 0.5
    kill "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT
Xvfb -displayfd 3 -screen 0 "${size}x24" 3>"$work/display" 2>"$work/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$work/display" ] && break; sleep 0.2; done
disp=":$(cat "$work/display")"
export XDG_RUNTIME_DIR="$runtime" XDG_CONFIG_HOME="$work/config"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN ULTTY_PID
env -u WAYLAND_DISPLAY DISPLAY="$disp" "$repo/target/debug/ultty" >"$work/ultty.log" 2>&1 &
ultty_pid=$!
ctl() { "$repo/target/debug/ulttyctl" --pid "$ultty_pid" "$@"; }
for _ in $(seq 240); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && ctl ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || break
    sleep 0.5
done
ctl ping >/dev/null
if [ -n "${RESIZE-}" ]; then ctl resize $RESIZE || true; sleep 2; fi
sleep 2
win=$(DISPLAY="$disp" xdotool search --pid "$ultty_pid" | head -1)
DISPLAY="$disp" xdotool windowfocus "$win" || true
key() { DISPLAY="$disp" xdotool key --window "$win" "$@"; sleep 1; }
type_() { DISPLAY="$disp" xdotool type --window "$win" --delay 80 "$1"; sleep 1; }
ctl send-text 'echo palette-test\r' >/dev/null || true
sleep 1
key ctrl+shift+p
ctl screenshot "$out/open.png"
type_ "sp"
key Down
ctl screenshot "$out/filtered.png"
key Escape
ctl screenshot "$out/closed.png"
# Run an action from the palette: split right.
key ctrl+shift+p
type_ "split right"
key Return
sleep 1
ctl screenshot "$out/after-split.png"
ctl panes > "$out/panes.txt" 2>&1 || true
# Typed characters must not have reached the shell.
key ctrl+shift+p
type_ "zzq"
ctl screenshot "$out/nomatch.png"
key Escape
ctl text > "$out/text.txt" 2>&1 || true
echo done
