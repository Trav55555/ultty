#!/usr/bin/env bash
# Check what a screen reader is told while a native app streams into a
# polite live region (ADR 0021 slice 6), on a virtual X display with a
# private session bus and AT-SPI bus: the `live-region` example of
# ultty-client streams twelve lines in five-character pieces every 20 ms,
# and the AT-SPI `object:announcement` events must carry every line, in
# order, each once, no more than four a second, polite, and no line cut
# inside a word (the stream is fast enough for that). Then, with the
# stream ended and the app still connected, a still screen must rebuild
# nothing and wake no more than the app's maintenance tick does.
#
#   scripts/e2e/accessibility-live.sh [artifact-dir]
#
# Needs what scripts/e2e/accessibility-atspi.sh needs. Uses $ULTTY,
# $ULTTYCTL and $LIVE_REGION, default target/debug/ultty, ulttyctl and
# examples/live-region, all built first if $ULTTY is unset. Artifacts go to
# artifact-dir if given, else to a temporary directory removed on success.
# A local script, not a CI job: it says nothing about what Orca speaks
# (docs/accessibility-test.md, step 5).
set -euo pipefail

if [ "${1:-}" != "--inner" ]; then
    absolute() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
    self=$(absolute "$0")
    if [ -n "${ULTTY:-}" ]; then ULTTY=$(absolute "$ULTTY"); fi
    if [ -n "${ULTTYCTL:-}" ]; then ULTTYCTL=$(absolute "$ULTTYCTL"); fi
    if [ -n "${LIVE_REGION:-}" ]; then LIVE_REGION=$(absolute "$LIVE_REGION"); fi
    if [ -n "${1:-}" ]; then
        out=$(absolute "$1")
        keep=true
    else
        out=$(mktemp -d)
        keep=false
    fi
    cd "$(dirname "$self")/../.."
    for tool in Xvfb dbus-run-session gdbus; do
        command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
    done
    if [ -z "${ULTTY:-}" ]; then
        cargo build -q -p ultty --bins
        ULTTY=$PWD/target/debug/ultty
        ULTTYCTL=$PWD/target/debug/ulttyctl
    fi
    if [ -z "${LIVE_REGION:-}" ]; then
        cargo build -q -p ultty-client --example live-region
        LIVE_REGION=$PWD/target/debug/examples/live-region
    fi
    export ULTTY ULTTYCTL LIVE_REGION
    mkdir -p "$out"
    # A session bus of our own: the desktop's AT-SPI state is left alone.
    exec dbus-run-session -- "$self" --inner "$out" "$keep"
fi

cd "$(dirname "$0")/../.."
out="$2"
keep="$3"
launcher=$(ls /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher 2>/dev/null | head -1 || true)
[ -n "$launcher" ] || { echo "at-spi-bus-launcher is required" >&2; exit 2; }
mkdir -p "$out/config/ultty"
runtime=$(mktemp -d /tmp/ultty-live.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${listener_pid-}" "${ultty_pid-}" "${launcher_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\naccessibility = true\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

# Xvfb first: the launcher publishes the AT-SPI bus address on its root
# window. WAYLAND_DISPLAY is unset so nothing opens on the desktop.
rm -f "$out/display"
Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }
DISPLAY=":$(cat "$out/display")"
export DISPLAY
unset WAYLAND_DISPLAY

"$launcher" --launch-immediately >"$out/at-spi.log" 2>&1 &
launcher_pid=$!
for _ in $(seq 50); do
    gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
        --method org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
    sleep 0.1
done
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>' >/dev/null

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

env XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="ultty::a11y=debug,info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    tail -40 "$out/ultty.log" >&2
    echo "artifacts in $out" >&2
    exit 1
}

socket="$runtime/ultty-ctl-$ultty_pid.sock"
for _ in $(seq 600); do
    [ -S "$socket" ] && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
[ -S "$socket" ] || fail "no control socket within 60 s"
ctl() { XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid" "$ULTTYCTL" "$@"; }
ctl wait-for --quiet 500 --timeout 30000 >/dev/null || fail "the shell never settled"
# The tree is up once the terminal object is on the bus.
/usr/bin/python3 -I scripts/e2e/atspi_terminal.py "$ultty_pid" --timeout 30 >/dev/null ||
    fail "no terminal object on the AT-SPI bus"

/usr/bin/python3 -I scripts/e2e/atspi_announcements.py "$ultty_pid" "$out/listening" \
    --quiet 3 --timeout 60 >"$out/announcements.jsonl" 2>"$out/listener.log" &
listener_pid=$!
for _ in $(seq 100); do [ -e "$out/listening" ] && break; sleep 0.1; done
[ -e "$out/listening" ] || fail "the announcement listener did not start"

ctl send-text "exec $(printf %q "$LIVE_REGION") 12 30 2>$(printf %q "$out/live-region.log")"$'\r' >/dev/null ||
    fail "ulttyctl send-text failed"
wait "$listener_pid" || fail "no announcements within 60 s: $(cat "$out/listener.log")"
unset listener_pid
grep -q "streamed" "$out/live-region.log" || fail "live-region did not finish: $(cat "$out/live-region.log")"

/usr/bin/python3 -I - "$out/announcements.jsonl" 12 >"$out/verdict.txt" <<'EOF' ||
import json
import sys

every = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
# AccessKit also announces a live node's name when the node is added: the
# region itself, "Reply", once when the app shows it. The streamed text
# comes as label children.
records = [r for r in every if r["role"] == "label"]
others = [r for r in every if r["role"] != "label"]
lines = int(sys.argv[2])
expected = [f"Line {n} of the reply, streamed in small pieces." for n in range(1, lines + 1)]
spoken = " ".join(r["text"] for r in records)
heard = " ".join(spoken.split())
want = " ".join(expected)
problems = []
if heard != want:
    problems.append(f"text differs:\n  heard {heard!r}\n  want  {want!r}")
times = [r["t"] for r in records]
# At most four in any one second (a little slack for D-Bus delivery).
worst = max(sum(1 for u in times if t <= u < t + 0.95) for t in times)
if worst > 4:
    problems.append(f"{worst} announcements within one second")
if any(not r["text"].endswith(".") for r in records):
    problems.append("an announcement ends inside a line")
if any(r["politeness"] != 1 for r in records):
    problems.append("an announcement was not polite")
if any(len(r["text"].encode()) > 2048 for r in records):
    problems.append("an announcement over 2 KiB")
span = times[-1] - times[0] if len(times) > 1 else 0
print(f"{len(records)} announcements over {span:.2f} s, at most {worst} in a second")
for r in others:
    print(f"  also: {r['role']} {r['text']!r} announced when added")
if len(others) > 1:
    problems.append(f"{len(others)} announcements from other objects")
for r in records:
    print(f"  {r['t']:6.3f} {r['role']}: {r['text']!r}")
for p in problems:
    print("PROBLEM: " + p)
sys.exit(1 if problems else 0)
EOF
    { cat "$out/verdict.txt" >&2; fail "the announcements were wrong"; }
cat "$out/verdict.txt"

# The stream is over and the app holds its session open: a still screen
# rebuilds nothing, and Ultty wakes only for the 250 ms maintenance tick a
# connected app has with or without a screen reader (20 in 5 s).
switches() {
    cat /proc/"$ultty_pid"/task/*/status 2>/dev/null |
        awk '/_ctxt_switches/ { s += $2 } END { print s + 0 }'
}
builds() { grep -c "accessibility tree built" "$out/ultty.log" || true; }
sleep 2
switches_before=$(switches)
builds_before=$(builds)
sleep 5
woke=$(( $(switches) - switches_before ))
rebuilt=$(( $(builds) - builds_before ))
echo "idle after the stream, AT active, 5 s: $woke context switches, $rebuilt trees built" |
    tee "$out/idle.txt"
[ "$rebuilt" -eq 0 ] || fail "a still screen rebuilt the tree $rebuilt times in 5 s"
[ "$woke" -le 25 ] || fail "Ultty woke $woke times in 5 s of idle (at most 25)"

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
echo "accessibility-live: ok"
$keep || rm -rf "$out"
