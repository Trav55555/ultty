#!/usr/bin/env bash
# Agent-session soak (plan item D5): run Ultty on a virtual X display with
# agent-shaped output for a long time and sample it every minute.
#
#   scripts/bench/soak.sh [minutes] [out-dir]     # default 600 minutes
#
# Two tabs run scripts/bench/agent-stream.py: the one on screen and one in
# the background. Every 5 minutes the window is resized. Every minute one
# CSV row goes to <out-dir>/samples.csv: RSS, GPU memory that lives in host
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
cleanup() { kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true; }
trap cleanup EXIT

Xvfb -displayfd 3 -screen 0 1920x1200x24 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; exit 1; }

# Frame times come from the pipeline trace; the log is appended to so it
# can be truncated after each sample.
: >"$out/ultty.log"
DISPLAY=":$(cat "$out/display")" ULTTY_TRACE_PIPELINE=1 \
    RUST_LOG=info,ultty::perf=info target/release/ultty >>"$out/ultty.log" 2>&1 &
ultty_pid=$!
export ULTTY_PID=$ultty_pid PATH="$PWD/target/release:$PATH"
for _ in $(seq 80); do ulttyctl ping >/dev/null 2>&1 && break; sleep 0.25; done
ulttyctl ping >/dev/null

stream="$PWD/scripts/bench/agent-stream.py"
ulttyctl new-tab /usr/bin/python3 "$stream" --seed 2 >/dev/null
ulttyctl new-tab /usr/bin/python3 "$stream" --seed 1 >/dev/null
ulttyctl close-tab 1 >/dev/null   # the starting shell; the streams remain

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

echo "minute,rss_kib,gpu_kib,cpu_s,frames,frame_p50_us,frame_p99_us" >"$out/samples.csv"
hz=$(getconf CLK_TCK)
sizes=("1600 1000" "1200 800" "1920 1150" "1000 700")
for minute in $(seq 1 "$minutes"); do
    sleep 60
    kill -0 "$ultty_pid" 2>/dev/null || { echo "Ultty exited at minute $minute" >&2; exit 1; }
    if (( minute % 5 == 0 )); then
        # shellcheck disable=SC2086 # width and height are two words
        ulttyctl resize ${sizes[$(( (minute / 5) % 4 ))]} >/dev/null || true
    fi
    rss=$(awk '/^Rss:/{print $2}' "/proc/$ultty_pid/smaps_rollup")
    cpu=$(awk -v hz="$hz" '{printf "%.2f", ($14+$15)/hz}' "/proc/$ultty_pid/stat")
    echo "$minute,$rss,$(gpu_kib),$cpu,$(frame_stats)" >>"$out/samples.csv"
done
echo "samples in $out/samples.csv"
python3 scripts/bench/soak-report.py "$out/samples.csv"
