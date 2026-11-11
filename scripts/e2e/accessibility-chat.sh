#!/usr/bin/env bash
# Check what a screen reader is told while a ulttui app streams a reply
# (ADR 0021 slice 7, the automated half of B4's done criterion), on a
# virtual X display with a private session bus and AT-SPI bus. The `chat`
# example runs in Ultty: `ulttyctl regions` must show its transcript as a
# polite live log, and after a message is typed, the bot's reply, which
# streams word by word into a `MarkdownView::streaming(true)` with
# `append_chunk`, must reach AT-SPI as `object:announcement` events
# carrying the reply once, in order, polite, with nothing else announced
# but the reply's own node. Twice: a short reply, then one with markdown
# blocks, inline styles, a link with a title (its last word arrives as
# plain text, after the link has begun), a task list item, a long link and
# a long bold phrase (each longer than the 80 bytes a lone opener is held
# for) and a code block, none of whose markers, checkboxes or addresses
# may be spoken.
#
#   scripts/e2e/accessibility-chat.sh [artifact-dir]
#
# Needs what scripts/e2e/accessibility-atspi.sh needs. Uses $ULTTY,
# $ULTTYCTL and $CHAT, default target/debug/ultty, ulttyctl and chat, all
# built first if $ULTTY is unset. Artifacts go to artifact-dir if given,
# else to a temporary directory removed on success. A local script, not a
# CI job: it says nothing about what Orca speaks
# (docs/accessibility-test.md, step 5).
set -euo pipefail

if [ "${1:-}" != "--inner" ]; then
    absolute() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
    self=$(absolute "$0")
    if [ -n "${ULTTY:-}" ]; then ULTTY=$(absolute "$ULTTY"); fi
    if [ -n "${ULTTYCTL:-}" ]; then ULTTYCTL=$(absolute "$ULTTYCTL"); fi
    if [ -n "${CHAT:-}" ]; then CHAT=$(absolute "$CHAT"); fi
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
    build=()
    if [ -z "${ULTTY:-}" ]; then build+=(-p ultty --bins); fi
    if [ -z "${CHAT:-}" ]; then build+=(-p chat); fi
    if [ ${#build[@]} -gt 0 ]; then cargo build -q "${build[@]}"; fi
    ULTTY=${ULTTY:-$PWD/target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-$(dirname "$ULTTY")/ulttyctl}
    CHAT=${CHAT:-$PWD/target/debug/chat}
    export ULTTY ULTTYCTL CHAT
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
runtime=$(mktemp -d /tmp/ultty-chat.XXXXXX)
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
/usr/bin/python3 -I scripts/e2e/atspi_terminal.py "$ultty_pid" --timeout 30 >/dev/null ||
    fail "no terminal object on the AT-SPI bus"

ctl send-text "exec $(printf %q "$CHAT") 2>$(printf %q "$out/chat.log")"$'\r' >/dev/null ||
    fail "ulttyctl send-text failed"
# The transcript is a polite live log, the input is named.
semantics() {
    ctl --json regions | /usr/bin/python3 -I scripts/e2e/native_semantics.py "$@"
}
for _ in $(seq 100); do
    semantics "role=log,label=Messages,live=polite" >"$out/regions.txt" 2>&1 && break
    sleep 0.1
done
semantics "role=log,label=Messages,live=polite" "role=input,label=Message" \
    "role=heading,label=Chat Demo" >"$out/regions.txt" ||
    { cat "$out/regions.txt" >&2; fail "chat's regions lack their semantics"; }
echo "regions: transcript is a polite log, the input is named"

# Send `message`, check the streaming reply is a polite log of its own
# while it streams, and that what AT-SPI announces is `want`: every word
# once, in order, polite, at most four a second, and nothing else but the
# reply's own node. Artifacts are named after `name`.
converse() {
    local name=$1 message=$2 want=$3
    /usr/bin/python3 -I scripts/e2e/atspi_announcements.py "$ultty_pid" "$out/$name.listening" \
        --quiet 3 --timeout 60 >"$out/$name.jsonl" 2>"$out/$name.listener.log" &
    listener_pid=$!
    for _ in $(seq 100); do [ -e "$out/$name.listening" ] && break; sleep 0.1; done
    [ -e "$out/$name.listening" ] || fail "the announcement listener did not start"

    # Typed one key at a time through the window (send-text writes to the
    # pty, which a socket app does not read); Enter sends it.
    local key i
    for ((i = 0; i < ${#message}; i++)); do
        key=${message:i:1}
        ctl send-key "$key" >/dev/null || fail "ulttyctl send-key failed"
    done
    ctl send-key enter >/dev/null || fail "ulttyctl send-key failed"
    # While the reply streams, it is a polite log of its own. Polled as
    # fast as ulttyctl answers, for at most 10 s (the reply takes 40 ms a
    # word; the markdown one about 1.5 s).
    local streaming=false deadline=$((SECONDS + 10))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if semantics "role=log,label=Bot reply,live=polite" >"$out/$name.regions.txt" 2>&1; then
            streaming=true
            break
        fi
    done

    wait "$listener_pid" || fail "no announcements within 60 s: $(cat "$out/$name.listener.log")"
    unset listener_pid
    $streaming || { cat "$out/$name.regions.txt" >&2; fail "$name: the streaming reply was never a polite log"; }

    /usr/bin/python3 -I - "$out/$name.jsonl" "$want" >"$out/$name.verdict.txt" <<'EOF' ||
import json
import sys

every = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
# The streamed text comes as label children of the reply's block regions;
# the reply's own region is announced by name ("Bot reply") once, when it
# appears.
records = [r for r in every if r["role"] == "label"]
others = [r for r in every if r["role"] != "label"]
want = " ".join(sys.argv[2].split())
heard = " ".join(" ".join(r["text"] for r in records).split())
problems = []
if heard != want:
    problems.append(f"text differs:\n  heard {heard!r}\n  want  {want!r}")
times = [r["t"] for r in records]
worst = max((sum(1 for u in times if t <= u < t + 0.95) for t in times), default=0)
if worst > 4:
    problems.append(f"{worst} announcements within one second")
if any(r["politeness"] != 1 for r in records):
    problems.append("an announcement was not polite")
print(f"{len(records)} announcements, at most {worst} in a second")
for r in others:
    print(f"  also: {r['role']} {r['text']!r} announced when added")
if any(r["text"] != "Bot reply" for r in others) or len(others) > 1:
    problems.append("announcements from other objects than the reply")
for r in records:
    print(f"  {r['t']:6.3f} {r['role']}: {r['text']!r}")
for p in problems:
    print("PROBLEM: " + p)
sys.exit(1 if problems else 0)
EOF
        { cat "$out/$name.verdict.txt" >&2; fail "$name: the announcements were wrong"; }
    echo "$name:"
    cat "$out/$name.verdict.txt"

    # Finished, the reply is an ordinary message: no live region but the
    # transcript's.
    semantics "role=log,label=Messages,live=polite" >"$out/$name.regions-after.txt" ||
        fail "the transcript lost its semantics"
    ! grep -q "label='Bot reply'" "$out/$name.regions-after.txt" ||
        fail "$name: the finished reply is still a live log"
}

converse ping ping \
    "Bot: You said: ping. That is 4 characters, streamed to you word by word."
converse markdown markdown \
    "Bot: Here is bold text and inline code. • first item a task that is done • an item
     with a link let x = 1; let y = 2; Read the architecture overview for every detail of how the
     pieces fit together, which takes rather a long time to explain before you start.
     That was markdown, streamed word by word."

! grep -q "panicked at" "$out/ultty.log" || fail "Ultty logged a panic"
echo "accessibility-chat: ok"
$keep || rm -rf "$out"
