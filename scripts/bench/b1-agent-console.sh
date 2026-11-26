#!/usr/bin/env bash
# B1's four numbers for the agent console (ADR 0031 section 4): scroll held
# while output streams, copy fidelity, resize, memory. Measured for
# examples/agent-console in native mode (in Ultty) and ANSI mode (in Ultty
# with the socket unset and ULTTY_IN_BAND=0, and in Alacritty and Ghostty
# as references).
#
#   scripts/bench/b1-agent-console.sh [options]
#
# Options:
#   --targets LIST        default ultty-native,ultty-ansi,alacritty,ghostty
#                         (the references need ultty-ansi for the copy rows)
#   --numbers LIST        default scroll,copy,resize,memory
#   --memory-minutes N    how long the memory replay runs (default 12.5:
#                         two-hours.jsonl at --speed 10 lasts 12 minutes)
#   --memory-warmup N     minutes skipped before the memory trend (default 8)
#   --memory-scrollback N scrollback lines in the memory replay (default
#                         2000, reached about 7 minutes in; the other
#                         numbers use 10,000)
#   --font FAMILY         default "DejaVu Sans Mono"
#   --font-size PT        default 11
#   --no-build            use target/release as it is
#   --out DIR             default target/bench-results/<date>-b1
#
# Everything runs on a private Xvfb (Ultty through Mesa's lavapipe), with
# WAYLAND_DISPLAY unset and a private runtime dir per launch, so nothing
# reaches the desktop session. scripts/bench/b1-agent-console.py does the
# measuring and documents each method. Output: DIR/results.json,
# DIR/report.md, DIR/shots/ (screens compared for resize), DIR/run/ (each
# launch's wrapper, logs and text), DIR/memory-*.csv.
#
# Needs Xvfb, xdotool, alacritty, ghostty, and python3 with Pillow (built
# with XCB, for screenshots of the display).
set -euo pipefail
# Test and benchmark runs stay out of the user's Ultty log file.
export ULTTY_LOG_FILE=0

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/scripts/bench"

die() { echo "b1-agent-console.sh: $*" >&2; exit 1; }

TARGETS="ultty-native,ultty-ansi,alacritty,ghostty" NUMBERS="scroll,copy,resize,memory"
MEMORY_MINUTES=12.5 MEMORY_WARMUP=8 MEMORY_SCROLLBACK=2000
FONT="DejaVu Sans Mono" FONT_SIZE=11 BUILD=1 OUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --targets) TARGETS="${2:?}"; shift 2 ;;
    --numbers) NUMBERS="${2:?}"; shift 2 ;;
    --memory-minutes) MEMORY_MINUTES="${2:?}"; shift 2 ;;
    --memory-warmup) MEMORY_WARMUP="${2:?}"; shift 2 ;;
    --memory-scrollback) MEMORY_SCROLLBACK="${2:?}"; shift 2 ;;
    --font) FONT="${2:?}"; shift 2 ;;
    --font-size) FONT_SIZE="${2:?}"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    --out) OUT="${2:?}"; shift 2 ;;
    -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done
for tool in Xvfb xdotool xdpyinfo; do command -v "$tool" >/dev/null || die "$tool is required"; done
# The system interpreter: Pillow must be importable without a virtualenv.
PYTHON=/usr/bin/python3
"$PYTHON" -I -c 'from PIL import ImageGrab, features; assert features.check("xcb")' 2>/dev/null ||
  die "$PYTHON needs Pillow with XCB support"

OUT="${OUT:-$ROOT/target/bench-results/$(date +%Y-%m-%d)-b1}"
mkdir -p "$OUT"/{bin,cfg}
OUT="$(cd "$OUT" && pwd)"

if [[ "$BUILD" == 1 ]]; then
  # heap-stats, as soak.sh: Ultty logs its live Rust heap for the memory trend.
  (cd "$ROOT" && cargo build --release --locked -p ultty --bins --features heap-stats \
    && cargo build --release --locked -p agent-console)
fi
# Copies, so a concurrent build cannot swap them mid-run.
for b in ultty ulttyctl agent-console; do cp "$ROOT/target/release/$b" "$OUT/bin/"; done

GHOSTTY_BIN=$(command -v ghostty || echo /snap/bin/ghostty)
# write_configs DIR LINES: each terminal's config with LINES of scrollback.
write_configs() {
  local dir="$1" lines="$2"
  mkdir -p "$dir/ultty/ultty"
  # Ghostty limits scrollback in bytes of page memory, not lines; this is
  # roughly LINES lines of 150 columns, as compare.sh estimates.
  cat >"$dir/ghostty.conf" <<EOF
font-family = $FONT
font-size = $FONT_SIZE
font-feature = -calt
font-feature = -liga
font-feature = -dlig
window-padding-x = 0
window-padding-y = 0
window-width = 120
window-height = 30
scrollback-limit = $(( lines * (150 * 8 + 32) ))
mouse-scroll-multiplier = 1
resize-overlay = never
shell-integration = none
cursor-style-blink = false
gtk-single-instance = false
confirm-close-surface = false
quit-after-last-window-closed = true
window-decoration = none
EOF
  cat >"$dir/alacritty.toml" <<EOF
[font]
size = $FONT_SIZE

[font.normal]
family = "$FONT"

[window]
dimensions = { columns = 120, lines = 30 }
padding = { x = 0, y = 0 }

[scrolling]
history = $lines
multiplier = 1

[cursor.style]
blinking = "Never"
EOF
  cat >"$dir/ultty/ultty/config.toml" <<EOF
[font]
family = "$FONT"
size = $FONT_SIZE
ligatures = false

[behavior]
scrollback_lines = $lines
restore_session = false
cursor_blink = false
EOF
}
write_configs "$OUT/cfg" 10000
# The memory replay keeps less scrollback, so every terminal reaches its cap
# during the warm-up and the trend after it shows growth beyond scrollback.
write_configs "$OUT/cfg/memory" "$MEMORY_SCROLLBACK"

# Unix socket paths are short (108 bytes), so the runtime dir is under /tmp.
RUNTIME="$(mktemp -d /tmp/ub1.XXXXXX)"
chmod 700 "$RUNTIME"
B1_RUN="${RUNTIME##*/}-$$"
export B1_RUN
unset TMUX ULTTY_SOCKET ULTTY_PID ULTTY_AUTH_TOKEN WAYLAND_DISPLAY
export GTK_USE_PORTAL=0 GDK_DEBUG=no-portals GIO_USE_VFS=local GVFS_DISABLE_FUSE=1

# Every process this run starts inherits B1_RUN; stop exactly those.
kill_run_processes() {
  local pid
  for pid in $(pgrep -u "$(id -u)" 2>/dev/null); do
    [[ "$pid" == "$$" ]] && continue
    if tr '\0' '\n' 2>/dev/null <"/proc/$pid/environ" | grep -qx "B1_RUN=$B1_RUN"; then
      kill "$pid" 2>/dev/null || true
    fi
  done
}
cleanup() {
  [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
  kill_run_processes
  wait 2>/dev/null || true
  rm -rf --one-file-system "$RUNTIME"
}
trap cleanup EXIT

Xvfb -displayfd 3 -screen 0 2560x2000x24 -nolisten tcp 3>"$OUT/display" 2>"$OUT/xvfb.log" &
XVFB_PID=$!
for _ in $(seq 50); do [[ -s "$OUT/display" ]] && break; sleep 0.1; done
[[ -s "$OUT/display" ]] || die "Xvfb did not start (see $OUT/xvfb.log)"
DISPLAY=":$(cat "$OUT/display")"
export DISPLAY
xdpyinfo >/dev/null 2>&1 || die "cannot reach Xvfb on $DISPLAY"
VK_ICD_FILENAMES="${VK_ICD_FILENAMES:-$(find /usr/share/vulkan/icd.d -name 'lvp_icd*.json' | head -1)}"
export VK_ICD_FILENAMES

"$PYTHON" -I "$HERE/b1-agent-console.py" --out "$OUT" --bin "$OUT/bin" --cfg "$OUT/cfg" \
  --runtime "$RUNTIME" --ghostty "$GHOSTTY_BIN" --targets "$TARGETS" --numbers "$NUMBERS" \
  --memory-minutes "$MEMORY_MINUTES" --memory-warmup "$MEMORY_WARMUP"
cat "$OUT/report.md"
