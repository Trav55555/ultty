#!/usr/bin/env bash
# Agent-session soak (plan item D5): run Ultty on a virtual X display with
# agent-shaped output for a long time and sample it every minute.
#
#   scripts/bench/soak.sh [minutes] [out-dir]     # default 600 minutes
#
# Two tabs run scripts/bench/agent-stream.py: the one on screen and one in
# the background. Every 5 minutes the window is resized. Every minute one
# CSV row goes to <out-dir>/samples.csv: RSS, its anonymous part (heap
# and stacks; a leak shows here), memory swapped out, GPU memory that lives in host
# memory (memfd; zero on a GPU with its own memory), CPU seconds, frames
# drawn and their median and p99 time. scripts/bench/soak-report.py reads
# it. Needs Xvfb; builds release binaries first.
set -euo pipefail
cd "$(dirname "$0")/../.."
minutes=${1:-600}
out=${2:-target/soak-$(date +%Y%m%d-%H%M%S)}
mkdir -p "$out"
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

cargo build -q --release -p ultty --bins
# A private runtime dir holds this Ultty's control socket and no other's,
# so ulttyctl cannot reach a window on the desktop. Socket paths are short
# (108 bytes), so it lives directly under /tmp.
runtime=$(mktemp -d /tmp/ultty-soak.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

Xvfb -displayfd 3 -screen 0 1920x1200x24 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; exit 1; }

# Frame times come from the pipeline trace; the log is appended to so it
# can be truncated after each sample.
: >"$out/ultty.log"
# Without unsetting WAYLAND_DISPLAY, a Wayland session in the environment
# wins over DISPLAY and the window opens on the desktop instead of Xvfb.
export XDG_RUNTIME_DIR="$runtime"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN
env -u WAYLAND_DISPLAY DISPLAY=":$(cat "$out/display")" ULTTY_TRACE_PIPELINE=1 \
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
frame_stats() {
    # frames, median us, p99 us of pipeline_render_frame since the last call
    sed 's/\x1b\[[0-9;]*m//g' "$out/ultty.log" | grep -o 'pipeline_render_frame.*elapsed_us=[0-9]*' |
        sed 's/.*elapsed_us=//' | sort -n |
        awk '{a[NR]=$1} END{ if (NR==0) { print "0,0,0"; exit }
                            p=int(NR*0.99); if (p<1) p=1
                            print NR "," a[int((NR+1)/2)] "," a[p] }'
    : >"$out/ultty.log"
}

echo "minute,rss_kib,anon_kib,swap_kib,gpu_kib,cpu_s,frames,frame_p50_us,frame_p99_us" >"$out/samples.csv"
hz=$(getconf CLK_TCK)
sizes=("1600 1000" "1200 800" "1920 1150" "1000 700")
for minute in $(seq 1 "$minutes"); do
    sleep 60
    kill -0 "$ultty_pid" 2>/dev/null || { echo "Ultty exited at minute $minute" >&2; exit 1; }
    if (( minute % 5 == 0 )); then
        # shellcheck disable=SC2086 # width and height are two words
        ctl resize ${sizes[$(( (minute / 5) % 4 ))]} >/dev/null || true
    fi
    rss=$(awk '/^Rss:/{print $2}' "/proc/$ultty_pid/smaps_rollup")
    anon=$(awk '/^RssAnon:/{print $2}' "/proc/$ultty_pid/status")
    swap=$(awk '/^VmSwap:/{print $2}' "/proc/$ultty_pid/status")
    cpu=$(awk -v hz="$hz" '{printf "%.2f", ($14+$15)/hz}' "/proc/$ultty_pid/stat")
    echo "$minute,$rss,$anon,$swap,$(gpu_kib),$cpu,$(frame_stats)" >>"$out/samples.csv"
done
echo "samples in $out/samples.csv"
python3 scripts/bench/soak-report.py "$out/samples.csv"
