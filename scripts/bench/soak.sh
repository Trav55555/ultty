#!/usr/bin/env bash
# Agent-session soak (plan item D5): run Ultty on a virtual X display with
# agent-shaped output for a long time and sample it every minute.
#
#   scripts/bench/soak.sh [minutes] [out-dir]     # default 150 minutes
#
# 150 minutes is 30 of warm-up and two hours measured: enough to see a
# growth of 1 MiB an hour against the minute-to-minute noise (under 1 MiB),
# and for draw time to settle once scrollback and the glyph atlas are full.
# Leaks over many more hours of output are checked without a clock by the
# agent-session case in crates/ultty/benches/memory.rs.
#
# Two tabs run scripts/bench/agent-stream.py: the one on screen and one in
# the background. Every 5 minutes the window is resized. Every minute one
# CSV row goes to <out-dir>/samples.csv: RSS, its anonymous part (heap
# and stacks; a leak shows here), memory swapped out, GPU memory that lives in host
# memory (memfd; zero on a GPU with its own memory), CPU seconds, frames
# drawn and their median and p99 time, the median and p99 of Ultty's own
# draw work in those frames (draw_us: building and submitting the frame,
# without waiting for the GPU or the display), and the window size.
# scripts/bench/soak-report.py reads it.
#
# Frame time includes presenting; draw time does not, so its drift per
# window size is comparable between sessions. The GPU Ultty used is
# written to <out-dir>/gpu.
#
# Environment:
#   SOAK_SESSION=xvfb  (default) a private Xvfb. No GPU can present to it,
#                      so Ultty renders with llvmpipe (software): memory is
#                      valid, frame times are the CPU's.
#   SOAK_SESSION=sway  a private headless sway, so Ultty renders on a GPU.
#                      WLR_RENDER_DRM_DEVICE=/dev/dri/renderDN picks sway's
#                      GPU; MESA_VK_DEVICE_SELECT=vid:did (with
#                      MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE=1) picks
#                      Ultty's.
#   SOAK_SESSION=x11   the X server in $DISPLAY (e.g. a headless Xorg on a
#                      GPU instance)
#   SOAK_NO_BUILD=1    use the binaries already in target/release
set -euo pipefail
cd "$(dirname "$0")/../.."
minutes=${1:-150}
out=${2:-target/soak-$(date +%Y%m%d-%H%M%S)}
mkdir -p "$out"
session=${SOAK_SESSION:-xvfb}
case "$session" in
    xvfb) command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; } ;;
    sway) command -v sway >/dev/null || { echo "sway is required" >&2; exit 2; } ;;
    x11) [ -n "${DISPLAY:-}" ] || { echo "SOAK_SESSION=x11 needs DISPLAY" >&2; exit 2; } ;;
    *) echo "SOAK_SESSION must be xvfb, sway or x11" >&2; exit 2 ;;
esac

[ -n "${SOAK_NO_BUILD:-}" ] || cargo build -q --release -p ultty --bins --features heap-stats
# A private runtime dir holds this Ultty's control socket and no other's,
# so ulttyctl cannot reach a window on the desktop. Socket paths are short
# (108 bytes), so it lives directly under /tmp.
runtime=$(mktemp -d /tmp/ultty-soak.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${display_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

export XDG_RUNTIME_DIR="$runtime"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN
# Ultty must reach only this script's display: a Wayland session in the
# environment wins over DISPLAY, so it is unset unless sway is ours.
case "$session" in
    xvfb)
        Xvfb -displayfd 3 -screen 0 1920x1200x24 3>"$out/display" 2>"$out/display.log" &
        display_pid=$!
        for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
        [ -s "$out/display" ] || { echo "Xvfb did not start" >&2; exit 1; }
        display_env=(-u WAYLAND_DISPLAY DISPLAY=":$(cat "$out/display")")
        ;;
    x11)
        display_env=(-u WAYLAND_DISPLAY DISPLAY="$DISPLAY")
        ;;
    sway)
        # Floating, so the window takes the sizes ulttyctl resize asks for.
        cat >"$out/sway.conf" <<'EOF'
output HEADLESS-1 mode --custom 2560x1440@60Hz position 0 0
for_window [title=".*"] floating enable, border none, move position 0 0
default_border none
default_floating_border none
xwayland disable
EOF
        env -u DISPLAY -u WAYLAND_DISPLAY -u SWAYSOCK WLR_BACKENDS=headless \
            WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER="${WLR_RENDERER:-gles2}" \
            sway -c "$out/sway.conf" >"$out/display.log" 2>&1 &
        display_pid=$!
        for _ in $(seq 100); do
            sock=$(find "$runtime" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' | head -1)
            [ -n "$sock" ] && break
            sleep 0.1
        done
        [ -n "${sock:-}" ] || { echo "sway did not start (see $out/display.log)" >&2; exit 1; }
        display_env=(-u DISPLAY WAYLAND_DISPLAY="${sock##*/}")
        ;;
esac

# Frame times come from the pipeline trace; the log is appended to so it
# can be truncated after each sample.
: >"$out/ultty.log"
env "${display_env[@]}" ULTTY_TRACE_PIPELINE=1 \
    RUST_LOG=info,ultty::perf=info target/release/ultty >>"$out/ultty.log" 2>&1 &
ultty_pid=$!
export ULTTY_PID=$ultty_pid PATH="$PWD/target/release:$PATH"
# --pid: this Ultty or an error, never another one.
ctl() { ulttyctl --pid "$ultty_pid" "$@"; }
for _ in $(seq 80); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && ctl ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || { echo "Ultty exited during startup" >&2; exit 1; }
    sleep 0.25
done
ctl ping >/dev/null || { echo "Ultty $ultty_pid never answered on its control socket" >&2; exit 1; }
# The startup log is truncated with the first sample; keep the adapter line.
sed 's/\x1b\[[0-9;]*m//g' "$out/ultty.log" | grep -o 'GPU: .*' | head -1 >"$out/gpu" || true
echo "$session: $(cat "$out/gpu")"

stream="$PWD/scripts/bench/agent-stream.py"
ctl new-tab /usr/bin/python3 "$stream" --seed 2 >/dev/null
ctl new-tab /usr/bin/python3 "$stream" --seed 1 >/dev/null
ctl close-tab 1 >/dev/null   # the starting shell; the streams remain
# Fail now, not after hours of samples, if the tabs did not land here.
tabs=$(ctl --json panes | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["panes"]))')
[ "$tabs" = 2 ] || { echo "expected 2 streaming panes in Ultty $ultty_pid, found $tabs" >&2; exit 1; }

gpu_kib() {
    awk '/^[0-9a-f]+-[0-9a-f]+ /{m=($0 ~ /memfd/)} m && /^Rss:/{s+=$2} END{print s+0}' \
        "/proc/$ultty_pid/smaps"
}
# median and p99 of the numbers on stdin, as "p50,p99"
p50_p99() {
    sort -n | awk '{a[NR]=$1} END{ if (NR==0) { print "0,0"; exit }
                                   p=int(NR*0.99); if (p<1) p=1
                                   print a[int((NR+1)/2)] "," a[p] }'
}
# The trace since the last sample, to $out/minute.log; the live log restarts.
take_log() {
    sed 's/\x1b\[[0-9;]*m//g' "$out/ultty.log" >"$out/minute.log"
    : >"$out/ultty.log"
}
frame_stats() {
    # frames, total median and p99 us, draw median and p99 us of
    # pipeline_render_frame in the minute's log
    local frames
    frames=$(grep 'pipeline_render_frame' "$out/minute.log" || true)
    echo "$(printf '%s\n' "$frames" | grep -c elapsed_us= || true),$(
        printf '%s\n' "$frames" | grep -o 'elapsed_us=[0-9]*' | cut -d= -f2 | p50_p99),$(
        printf '%s\n' "$frames" | grep -o 'draw_us=[0-9]*' | cut -d= -f2 | p50_p99)"
}
heap_kib() {
    # Live Rust heap from the minute's last heap_stats line (ultty built
    # with --features heap-stats); empty without it.
    local bytes
    bytes=$(grep -o 'heap_live_bytes=[0-9]*' "$out/minute.log" | tail -1 | cut -d= -f2 || true)
    [ -z "$bytes" ] || echo $((bytes / 1024))
}

echo "minute,rss_kib,anon_kib,swap_kib,gpu_kib,cpu_s,frames,frame_p50_us,frame_p99_us,draw_p50_us,draw_p99_us,width,height,heap_kib" >"$out/samples.csv"
hz=$(getconf CLK_TCK)
sizes=("1600 1000" "1200 800" "1920 1150" "1000 700")
# Start at the first size so every sample's window size is known.
# shellcheck disable=SC2086 # width and height are two words
ctl resize ${sizes[0]} >/dev/null
for minute in $(seq 1 "$minutes"); do
    sleep 60
    kill -0 "$ultty_pid" 2>/dev/null || { echo "Ultty exited at minute $minute" >&2; exit 1; }
    # The window size these frames were drawn at (set before this minute).
    size=${sizes[$(( ((minute - 1) / 5) % 4 ))]}
    if (( minute % 5 == 0 )); then
        # shellcheck disable=SC2086 # width and height are two words
        ctl resize ${sizes[$(( (minute / 5) % 4 ))]} >/dev/null || true
    fi
    rss=$(awk '/^Rss:/{print $2}' "/proc/$ultty_pid/smaps_rollup")
    anon=$(awk '/^RssAnon:/{print $2}' "/proc/$ultty_pid/status")
    swap=$(awk '/^VmSwap:/{print $2}' "/proc/$ultty_pid/status")
    cpu=$(awk -v hz="$hz" '{printf "%.2f", ($14+$15)/hz}' "/proc/$ultty_pid/stat")
    take_log
    echo "$minute,$rss,$anon,$swap,$(gpu_kib),$cpu,$(frame_stats),${size/ /,},$(heap_kib)" >>"$out/samples.csv"
done
echo "samples in $out/samples.csv"
python3 scripts/bench/soak-report.py "$out/samples.csv"
