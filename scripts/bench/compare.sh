#!/usr/bin/env bash
# Comparative live benchmarks: Ultty, Ghostty, GNOME Terminal, Alacritty.
#
#   scripts/bench/compare.sh --session xvfb|x11|wayland [options]
#
# Every terminal runs the same terminal-neutral drivers (tools/ultty-bench):
#   ubench-feed  throughput, fenced by a cursor position report
#   ubench-echo  echo target for keypress-to-pixels latency (idle and flood)
#   ubench-k2p   X11 probe: XTest injection + GetImage polling
#   k2p-wayland.py  Wayland probe: wtype + grim polling (coarse; see below)
# and the launcher records startup (exec to titled window), RSS/PSS after the
# workload (gnome-terminal-server, not the client), idle CPU and context
# switches from /proc, and memory against tab count.
#
# Sessions:
#   xvfb     private Xvfb + llvmpipe/lavapipe. Claim class live-swrast: the
#            numbers show the pipeline works, not how fast it is.
#   x11      the X server in $DISPLAY (for example Xorg on an NVIDIA GPU, as
#            scripts/bench/aws.sh sets up). Claim class live-realgpu.
#   wayland  private headless sway (wlroots), GPU rendering through the
#            render node (WLR_RENDERER=gles2 or vulkan; NVIDIA needs
#            --unsupported-gpu, which this script passes). On a box without a
#            GPU set WLR_RENDERER=pixman (live-swrast). sway rather than weston
#            because the Wayland probe needs virtual-keyboard and screencopy,
#            which weston lacks.
#
# Options:
#   --terminals LIST   default ultty,ghostty,gnome-terminal,alacritty
#   --geometry CxR     window size in cells, checked with `stty size` inside
#                      each terminal (default 120x40)
#   --font FAMILY      default "DejaVu Sans Mono"; the resolved file and its
#                      hash are recorded
#   --font-size PT     default 11
#   --corpora LIST     default ascii,sgr-dense,unicode-mixed,cursor-redraw
#   --size-mib N       corpus size (default 16)
#   --rounds N         interleaved rounds, terminal order shuffled per round (default 3)
#   --repeat N         feed passes per launch (default 3)
#   --k2p-samples N    0 skips latency (default 200)
#   --flood LIST       flood rates for latency under load, MiB/s or max (default 10,max)
#   --idle-secs N      0 skips idle measurement (default 60)
#   --tabs LIST        tab counts for memory scaling, empty skips (default 1,8,32)
#   --ultty-dir DIR    directory holding release ultty and ulttyctl (default: build)
#   --bench-dir DIR    directory holding release ultty-bench, ubench-* (default: build)
#   --claim CLASS      override the claim class
#   --out DIR          default target/bench-results/<date>-compare-<session>
#
# Output: DIR/results.jsonl, DIR/versions.json, DIR/report.md, DIR/logs/.
# Exit status is non-zero if any measurement failed; the failures are listed
# at the end of report.md and on stderr.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/scripts/bench"

die() { echo "compare.sh: $*" >&2; exit 1; }
log() { echo "[$(date +%T)] $*" >&2; }

SESSION="" TERMINALS="ultty,ghostty,gnome-terminal,alacritty" GEOMETRY="120x40"
FONT="DejaVu Sans Mono" FONT_SIZE=11 CORPORA="ascii,sgr-dense,unicode-mixed,cursor-redraw"
SIZE_MIB=16 ROUNDS=3 REPEAT=3 K2P_SAMPLES=200 FLOOD="10,max" IDLE_SECS=60 TABS="1,8,32"
ULTTY_DIR="" BENCH_DIR="" CLAIM="" OUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --session) SESSION="${2:?}"; shift 2 ;;
    --terminals) TERMINALS="${2:?}"; shift 2 ;;
    --geometry) GEOMETRY="${2:?}"; shift 2 ;;
    --font) FONT="${2:?}"; shift 2 ;;
    --font-size) FONT_SIZE="${2:?}"; shift 2 ;;
    --corpora) CORPORA="${2:?}"; shift 2 ;;
    --size-mib) SIZE_MIB="${2:?}"; shift 2 ;;
    --rounds) ROUNDS="${2:?}"; shift 2 ;;
    --repeat) REPEAT="${2:?}"; shift 2 ;;
    --k2p-samples) K2P_SAMPLES="${2:?}"; shift 2 ;;
    --flood) FLOOD="${2?}"; shift 2 ;;
    --idle-secs) IDLE_SECS="${2:?}"; shift 2 ;;
    --tabs) TABS="${2?}"; shift 2 ;;
    --ultty-dir) ULTTY_DIR="${2:?}"; shift 2 ;;
    --bench-dir) BENCH_DIR="${2:?}"; shift 2 ;;
    --claim) CLAIM="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    -h|--help) sed -n '2,52p' "$0"; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done
case "$SESSION" in
  xvfb) CLAIM="${CLAIM:-live-swrast}" ;;
  x11|wayland) CLAIM="${CLAIM:-live-realgpu}" ;;
  *) die "--session must be xvfb, x11 or wayland" ;;
esac
[[ "$GEOMETRY" =~ ^([0-9]+)x([0-9]+)$ ]] || die "--geometry must be COLSxROWS"
COLS="${BASH_REMATCH[1]}" ROWS="${BASH_REMATCH[2]}"

OUT="${OUT:-$ROOT/target/bench-results/$(date +%Y-%m-%d-%H%M%S)-compare-$SESSION}"
mkdir -p "$OUT"/{bin,cfg,logs,run}
OUT="$(cd "$OUT" && pwd)"
RESULTS="$OUT/results.jsonl"
FAILURES="$OUT/failures.txt"
: >"$RESULTS"
: >"$FAILURES"
fail() { echo "$*" | tee -a "$FAILURES" >&2; }

# ---------------------------------------------------------------- binaries
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ -z "$BENCH_DIR" || -z "$ULTTY_DIR" ]]; then
  log "building release binaries"
  (cd "$ROOT" && cargo build --release --locked -p ultty --bins -p ultty-bench)
fi
# Copies, so a concurrent build into a shared target dir cannot swap them.
for b in ultty-bench ubench-feed ubench-echo ubench-k2p; do
  cp "${BENCH_DIR:-$TARGET_DIR/release}/$b" "$OUT/bin/"
done
for b in ultty ulttyctl; do
  cp "${ULTTY_DIR:-$TARGET_DIR/release}/$b" "$OUT/bin/"
done
BIN="$OUT/bin"

# ---------------------------------------------------------------- corpora
"$BIN/ultty-bench" corpus --out "$OUT/corpus" --size-mib "$SIZE_MIB" --corpus "$CORPORA" \
  2>"$OUT/logs/corpus.log"
CORPUS_FILES=()
IFS=',' read -r -a corpus_names <<<"$CORPORA"
for c in "${corpus_names[@]}"; do CORPUS_FILES+=("$OUT/corpus/$c.txt"); done

# ---------------------------------------------------------------- session
SESSION_PIDS=()
RUNTIME="$(mktemp -d /tmp/ubench.XXXXXX)"
chmod 700 "$RUNTIME"
export XDG_RUNTIME_DIR="$RUNTIME"
unset TMUX
# Every process this run starts inherits UBENCH_RUN, including daemons that
# D-Bus activates on a private bus (dconf, and any portal or gvfs daemon a
# toolkit starts despite the opt-outs below), so they can be found and
# stopped without touching anything else of the user's.
UBENCH_RUN="${RUNTIME##*/}-$$"
export UBENCH_RUN
# Keep GTK from starting the document portal and gvfs: both would mount
# FUSE file systems inside the private runtime dir, and the portal one
# exposes the user's real document store there.
export GTK_USE_PORTAL=0 GDK_DEBUG=no-portals GIO_USE_VFS=local GVFS_DISABLE_FUSE=1

kill_run_processes() {
  local pid
  for pid in $(pgrep -u "$(id -u)" 2>/dev/null); do
    [[ "$pid" == "$$" ]] && continue
    if tr '\0' '\n' 2>/dev/null <"/proc/$pid/environ" | grep -qx "UBENCH_RUN=$UBENCH_RUN"; then
      kill "$pid" 2>/dev/null || true
    fi
  done
}

cleanup() {
  local pid mnt
  for pid in "${LIVE_PIDS[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true; done
  for pid in "${SESSION_PIDS[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true; done
  kill_run_processes
  wait 2>/dev/null || true
  # Unmount anything mounted under the runtime dir before removing it, and
  # never let rm cross into another file system.
  while read -r _ mnt _; do
    case "$mnt" in "$RUNTIME"/*) fusermount3 -u "$mnt" 2>/dev/null || fusermount -u "$mnt" 2>/dev/null || true ;; esac
  done </proc/mounts
  if grep -q " $RUNTIME/" /proc/mounts; then
    echo "compare.sh: leaving $RUNTIME in place: something is still mounted under it" >&2
  else
    rm -rf --one-file-system "$RUNTIME"
  fi
}
trap cleanup EXIT
LIVE_PIDS=()

case "$SESSION" in
  xvfb)
    disp=$(( 120 + RANDOM % 80 ))
    Xvfb ":$disp" -screen 0 2560x1440x24 -nolisten tcp >"$OUT/logs/xvfb.log" 2>&1 &
    SESSION_PIDS+=("$!")
    export DISPLAY=":$disp"
    unset WAYLAND_DISPLAY
    export VK_ICD_FILENAMES="${VK_ICD_FILENAMES:-$(find /usr/share/vulkan/icd.d -name "lvp_icd*.json" | head -1)}"
    for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
    xdpyinfo >/dev/null 2>&1 || die "Xvfb did not start"
    ;;
  x11)
    [[ -n "${DISPLAY:-}" ]] || die "--session x11 needs DISPLAY"
    unset WAYLAND_DISPLAY
    xdpyinfo >/dev/null 2>&1 || die "cannot reach X server $DISPLAY"
    ;;
  wayland)
    command -v sway >/dev/null || die "sway is required for the Wayland session"
    cat >"$OUT/cfg/sway.conf" <<EOF
output HEADLESS-1 resolution 2560x1440 position 0 0
for_window [title=".*"] floating enable, border none, move position 0 0
default_border none
default_floating_border none
focus_follows_mouse no
xwayland disable
EOF
    sway_args=(-c "$OUT/cfg/sway.conf")
    if lsmod 2>/dev/null | grep -q '^nvidia'; then sway_args+=(--unsupported-gpu); fi
    unset DISPLAY
    WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER="${WLR_RENDERER:-gles2}" \
      sway "${sway_args[@]}" >"$OUT/logs/sway.log" 2>&1 &
    SESSION_PIDS+=("$!")
    for _ in $(seq 100); do
      sock=$(find "$RUNTIME" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' | head -1)
      [[ -n "$sock" ]] && break
      sleep 0.1
    done
    [[ -n "${sock:-}" ]] || die "sway did not start (see $OUT/logs/sway.log)"
    export WAYLAND_DISPLAY="${sock##*/}"
    SWAYSOCK=$(find "$RUNTIME" -maxdepth 1 -name 'sway-ipc.*.sock' | head -1)
    export SWAYSOCK
    export GDK_BACKEND=wayland
    ;;
esac

# GNOME Terminal (GTK3) showed no echo for XTest keys on a bare Xvfb with no
# window manager (the likely cause; unconfirmed), so start openbox when nothing manages the screen. Without
# one, GNOME Terminal's latency samples fail and are reported as failures.
WM=none
if [[ "$SESSION" != wayland ]]; then
  if xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q 'window id'; then
    WM=existing
  elif command -v openbox >/dev/null; then
    openbox >"$OUT/logs/openbox.log" 2>&1 &
    SESSION_PIDS+=("$!")
    WM=openbox
    sleep 1
  else
    echo "compare.sh: no window manager (install openbox); GNOME Terminal latency will fail" >&2
  fi
fi

# ---------------------------------------------------------------- versions
FONT_FILE=$(fc-match "$FONT" file | sed 's/^:file=//')
FONT_SHA=$(sha256sum "$FONT_FILE" | cut -d' ' -f1)
ver() { "$@" 2>/dev/null | head -1 | tr -d '"' || echo unknown; }
pkgver() { dpkg-query -W -f='${Version}' "$1" 2>/dev/null || echo none; }
GHOSTTY_BIN=$(command -v ghostty || echo /snap/bin/ghostty)
cat >"$OUT/versions.json" <<EOF
{
  "session": "$SESSION",
  "window_manager": "$WM",
  "claim": "$CLAIM",
  "geometry": "$GEOMETRY",
  "font": "$FONT", "font_size": $FONT_SIZE, "font_file": "$FONT_FILE", "font_sha256": "$FONT_SHA",
  "ultty_rev": "$(git -C "$ROOT" rev-parse --short=12 HEAD)",
  "ghostty": "$(ver "$GHOSTTY_BIN" --version)",
  "ghostty_snap": "$(snap list ghostty 2>/dev/null | awk 'NR==2 {print $2" rev "$3}' || echo none)",
  "gnome_terminal": "$(ver gnome-terminal --version)",
  "gnome_terminal_pkg": "$(pkgver gnome-terminal)",
  "vte_pkg": "$(pkgver libvte-2.91-0)",
  "alacritty": "$(ver alacritty --version)",
  "alacritty_pkg": "$(pkgver alacritty)",
  "sway": "$(ver sway --version)",
  "gpu": "$(ver nvidia-smi --query-gpu=name,driver_version --format=csv,noheader)",
  "kernel": "$(uname -r)"
}
EOF

# ---------------------------------------------------------------- configs
# Ghostty limits scrollback in bytes of its own page memory, not lines.
# Approximation for 10,000 lines: 8 bytes per cell plus a row header.
GHOSTTY_SCROLLBACK=$(( 10000 * (COLS * 8 + 32) ))
mkdir -p "$OUT/cfg/ultty/ultty" "$OUT/cfg/ghostty" "$OUT/cfg/alacritty" "$OUT/cfg/gnome/gtk-3.0"
cat >"$OUT/cfg/ultty/ultty/config.toml" <<EOF
[font]
family = "$FONT"
size = $FONT_SIZE
ligatures = false

[behavior]
scrollback_lines = 10000
restore_session = false
EOF
cat >"$OUT/cfg/ghostty/config" <<EOF
font-family = $FONT
font-size = $FONT_SIZE
font-feature = -calt
font-feature = -liga
font-feature = -dlig
window-padding-x = 0
window-padding-y = 0
window-width = $COLS
window-height = $ROWS
scrollback-limit = $GHOSTTY_SCROLLBACK
shell-integration = none
cursor-style-blink = false
gtk-single-instance = false
confirm-close-surface = false
quit-after-last-window-closed = true
window-decoration = none
EOF
cat >"$OUT/cfg/alacritty/alacritty.toml" <<EOF
[general]
ipc_socket = true

[font]
size = $FONT_SIZE

[font.normal]
family = "$FONT"

[window]
dimensions = { columns = $COLS, lines = $ROWS }
padding = { x = 0, y = 0 }

[scrolling]
history = 10000

[cursor.style]
blinking = "Never"
EOF
# VTE draws a theme padding around the grid; remove it for parity.
echo 'vte-terminal { padding: 0; }' >"$OUT/cfg/gnome/gtk-3.0/gtk.css"

# The program each terminal runs: set a unique title (how the launcher finds
# the window), run the driver, record its exit status, then linger so the
# launcher can sample memory before the window closes.
cat >"$OUT/bin/inner.sh" <<'EOF'
#!/bin/sh
printf '\033]2;%s\007' "$UBENCH_TITLE"
"$@" 2>>"$UBENCH_LOG"
echo $? >"$UBENCH_DONE.tmp" && mv "$UBENCH_DONE.tmp" "$UBENCH_DONE"
sleep "${UBENCH_LINGER:-0}"
EOF
chmod +x "$OUT/bin/inner.sh"

now_ns() { date +%s%N; }

# ---------------------------------------------------------------- launching
# launch TERM TITLE CMD... ; sets TERM_PID (the process that owns the window).
launch() {
  local term="$1" title="$2"; shift 2
  export UBENCH_TITLE="$title" UBENCH_DONE="$OUT/run/$title.done" UBENCH_LOG="$OUT/logs/$title.log"
  rm -f "$UBENCH_DONE"
  case "$term" in
    ultty)
      XDG_CONFIG_HOME="$OUT/cfg/ultty" SHELL="$OUT/run/$title.sh" \
        "$BIN/ultty" >"$OUT/logs/$title.ultty.log" 2>&1 &
      TERM_PID=$!
      ;;
    ghostty)
      "$GHOSTTY_BIN" --config-default-files=false --config-file="$OUT/cfg/ghostty/config" \
        -e "$OUT/run/$title.sh" >"$OUT/logs/$title.ghostty.log" 2>&1 &
      TERM_PID=$!
      ;;
    alacritty)
      alacritty --config-file "$OUT/cfg/alacritty/alacritty.toml" -e "$OUT/run/$title.sh" \
        >"$OUT/logs/$title.alacritty.log" 2>&1 &
      TERM_PID=$!
      ;;
    gnome-terminal)
      # Private bus and dconf, so the user's server is neither reused nor touched.
      UBENCH_TOKEN="$title" XDG_CONFIG_HOME="$OUT/cfg/gnome" \
        dbus-run-session -- "$HERE/gnome-run.sh" "$FONT $FONT_SIZE" "$COLS" "$ROWS" \
        "$OUT/run/$title.sh" >"$OUT/logs/$title.gnome.log" 2>&1 &
      TERM_PID=$!
      ;;
    *) die "unknown terminal $term" ;;
  esac
  LIVE_PIDS+=("$TERM_PID")
}

# Write the per-launch wrapper (Ultty takes it as $SHELL, so it needs a path).
wrapper() {
  local title="$1"; shift
  {
    echo '#!/bin/sh'
    printf 'exec %q' "$BIN/inner.sh"
    printf ' %q' "$@"
    echo
  } >"$OUT/run/$title.sh"
  chmod +x "$OUT/run/$title.sh"
}

# The PID whose memory and CPU belong to the terminal.
owner_pid() {
  local term="$1" title="$2" pid
  if [[ "$term" != gnome-terminal ]]; then echo "$TERM_PID"; return; fi
  for pid in $(pgrep -x gnome-terminal- 2>/dev/null; pgrep -x gnome-terminal-server 2>/dev/null); do
    if tr '\0' '\n' 2>/dev/null <"/proc/$pid/environ" | grep -qx "UBENCH_TOKEN=$title"; then
      echo "$pid"; return
    fi
  done
  echo ""
}

# Wait for the titled window; prints its id (X11) or con id (sway).
find_window() {
  local title="$1" deadline=$(( $(date +%s) + 30 ))
  while (( $(date +%s) < deadline )); do
    if [[ "$SESSION" == wayland ]]; then
      id=$(swaymsg -t get_tree | python3 -I "$HERE/sway_find.py" "$title" id 2>/dev/null || true)
    else
      id=$(xdotool search --onlyvisible --name "^$title\$" 2>/dev/null | head -1 || true)
    fi
    if [[ -n "$id" ]]; then echo "$id"; return 0; fi
    sleep 0.01
  done
  return 1
}

stop() {
  local pid="$1"
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -9 "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

# Stop a launch. For GNOME Terminal the window belongs to the private
# gnome-terminal-server, which is stopped first; then its bus goes.
stop_term() {
  local term="$1" title="$2" server
  if [[ "$term" == gnome-terminal ]]; then
    server=$(owner_pid "$term" "$title")
    [[ -n "$server" ]] && stop "$server"
  fi
  stop "$TERM_PID"
  if [[ "$term" == gnome-terminal ]]; then
    # dbus-run-session can leave its bus and activated services running.
    local pid
    for pid in $(pgrep -u "$(id -u)" 2>/dev/null); do
      if tr '\0' '\n' 2>/dev/null <"/proc/$pid/environ" | grep -qx "UBENCH_TOKEN=$title"; then
        kill "$pid" 2>/dev/null || true
      fi
    done
  fi
}

wait_done() {
  local title="$1" secs="$2" deadline
  deadline=$(( $(date +%s) + secs ))
  while [[ ! -f "$OUT/run/$title.done" ]]; do
    (( $(date +%s) < deadline )) || return 1
    sleep 0.1
  done
  [[ "$(cat "$OUT/run/$title.done")" == 0 ]]
}

# resize_window TERM WIN W H: pixel resize through whatever the session offers.
resize_window() {
  local term="$1" win="$2" w="$3" h="$4"
  if [[ "$term" == ultty ]]; then
    "$BIN/ulttyctl" resize "$w" "$h" >/dev/null
  elif [[ "$SESSION" == wayland ]]; then
    swaymsg "[con_id=$win] resize set $w px $h px" >/dev/null
  else
    xdotool windowsize "$win" "$w" "$h"
  fi
}

# Resize the window until `stty size` inside it matches, from the size
# reports ubench-feed writes while it waits. Ultty has no cells-based window
# size setting, and other terminals can be off by a row or two (decorations,
# rounding). Stops as soon as the size matches, because ubench-feed starts
# measuring at that moment.
fit_window() {
  local term="$1" win="$2" status="$3" w=1200 h=900 rows cols c1 r1 cw ch
  matches() { read -r rows cols <"$status"; [[ "$cols" == "$COLS" && "$rows" == "$ROWS" ]]; }
  for _ in $(seq 100); do [[ -s "$status" ]] && break; sleep 0.1; done
  [[ -s "$status" ]] || return 1
  sleep 0.3
  matches && return 0
  resize_window "$term" "$win" "$w" "$h"; sleep 0.5
  matches && return 0
  r1=$rows c1=$cols
  resize_window "$term" "$win" $((w + 400)) $((h + 300)); sleep 0.5
  matches && return 0
  (( cols > c1 && rows > r1 )) || return 1
  cw=$(( 400 * 1000 / (cols - c1) )); ch=$(( 300 * 1000 / (rows - r1) ))
  w=$(( w + (COLS - c1) * cw / 1000 + cw / 2000 ))
  h=$(( h + (ROWS - r1) * ch / 1000 + ch / 2000 ))
  for _ in 1 2 3 4 5; do
    resize_window "$term" "$win" "$w" "$h"; sleep 0.5
    matches && return 0
    w=$(( w + (COLS - cols) * cw / 1000 )); h=$(( h + (ROWS - rows) * ch / 1000 ))
  done
  return 1
}

emit() { echo "$1" >>"$RESULTS"; }
jenv() { printf '{"session":"%s","claim":"%s","load":%s}' "$SESSION" "$CLAIM" "$(cut -d' ' -f1 /proc/loadavg)"; }

# smaps_rollup Rss and Pss of a PID, in KiB: "rss pss".
mem_of() { awk '/^Rss:/ {r=$2} /^Pss:/ {p=$2} END {print r+0, p+0}' "/proc/$1/smaps_rollup" 2>/dev/null; }

# ---------------------------------------------------------------- workloads
run_feed() {
  local term="$1" round="$2"
  local title="ub-${term}-feed-$round" t0 win pid rss pss
  wrapper "$title" "$BIN/ubench-feed" --out "$RESULTS" --terminal "$term" --repeat "$REPEAT" \
    --block "$round" --claim "$CLAIM" --expect-size "$GEOMETRY" --size-wait 30 \
    --status "$OUT/run/$title.size" --display-session "$SESSION" "${CORPUS_FILES[@]}"
  t0=$(now_ns)
  UBENCH_LINGER=4 launch "$term" "$title"
  if ! win=$(find_window "$title"); then
    fail "$term round $round: window never appeared"; stop_term "$term" "$title"; return
  fi
  emit "{\"suite\":\"startup.window\",\"case\":\"$SESSION\",\"arm\":\"$term\",\"block\":$round,\"value\":$(( ($(now_ns) - t0) / 1000 ))e-3,\"unit\":\"ms\",\"claim\":\"$CLAIM\",\"env\":$(jenv)}"
  if ! fit_window "$term" "$win" "$OUT/run/$title.size"; then
    fail "$term round $round: could not fit the window to $GEOMETRY"
  fi
  if ! wait_done "$title" 1800; then
    fail "$term round $round: ubench-feed failed (see $OUT/logs/$title.log)"
  else
    pid=$(owner_pid "$term" "$title")
    if [[ -n "$pid" ]]; then
      sleep 2
      read -r rss pss < <(mem_of "$pid")
      emit "{\"suite\":\"mem.rss.live\",\"case\":\"after-feed/$GEOMETRY\",\"arm\":\"$term\",\"block\":$round,\"value\":$rss,\"pss_kib\":$pss,\"unit\":\"KiB\",\"claim\":\"$CLAIM\",\"env\":$(jenv)}"
    else
      fail "$term round $round: no owning process to sample memory"
    fi
  fi
  stop_term "$term" "$title"
}

# CPU time (clock ticks) and context switches of all threads of PID.
cpu_and_switches() {
  local pid="$1" ticks sw=0 f
  ticks=$(awk '{print $14 + $15}' "/proc/$pid/stat")
  for f in /proc/"$pid"/task/*/sched; do
    sw=$(( sw + $(awk '/^nr_switches/ {print $3}' "$f" 2>/dev/null || echo 0) ))
  done
  echo "$ticks $sw"
}

run_idle() {
  local term="$1" round="$2"
  local title="ub-${term}-idle-$round" pid t1 s1 t2 s2 hz
  wrapper "$title" sleep $(( IDLE_SECS + 30 ))
  launch "$term" "$title"
  if ! find_window "$title" >/dev/null; then fail "$term idle: no window"; stop_term "$term" "$title"; return; fi
  sleep 3
  pid=$(owner_pid "$term" "$title")
  if [[ -z "$pid" ]]; then fail "$term idle: no owning process"; stop_term "$term" "$title"; return; fi
  read -r t1 s1 < <(cpu_and_switches "$pid")
  sleep "$IDLE_SECS"
  read -r t2 s2 < <(cpu_and_switches "$pid")
  hz=$(getconf CLK_TCK)
  emit "{\"suite\":\"idle.cpu\",\"case\":\"focused-no-blink\",\"arm\":\"$term\",\"block\":$round,\"value\":$(awk -v d=$((t2 - t1)) -v hz="$hz" -v s="$IDLE_SECS" 'BEGIN {print d / hz / s * 60}'),\"unit\":\"cpu-s/min\",\"claim\":\"$CLAIM\",\"env\":$(jenv)}"
  emit "{\"suite\":\"idle.wakeups\",\"case\":\"focused-no-blink\",\"arm\":\"$term\",\"block\":$round,\"value\":$(awk -v d=$((s2 - s1)) -v s="$IDLE_SECS" 'BEGIN {print d / s}'),\"unit\":\"switches/s\",\"claim\":\"$CLAIM\",\"env\":$(jenv)}"
  stop_term "$term" "$title"
}

run_k2p() {
  local term="$1" round="$2" flood="$3"
  local title="ub-${term}-k2p-$flood-$round" win args=() probe_args=() case_name
  # The flood waits for the f key, so the probe calibrates on a still screen.
  [[ "$flood" == idle ]] || { args=(--flood "$flood"); probe_args=(--start-key f); }
  case_name=$([[ "$flood" == idle ]] && echo idle || echo "flood-$flood")
  wrapper "$title" "$BIN/ubench-echo" "${args[@]}"
  launch "$term" "$title"
  if ! win=$(find_window "$title"); then fail "$term k2p: no window"; stop_term "$term" "$title"; return; fi
  sleep 2
  if [[ "$SESSION" == wayland ]]; then
    python3 -I "$HERE/k2p-wayland.py" --title "$title" --terminal "$term" --out "$RESULTS" \
      --samples "$K2P_SAMPLES" --case "$case_name" --claim "$CLAIM" "${probe_args[@]}" \
      2>>"$OUT/logs/$title.probe.log" || fail "$term k2p $case_name failed (see $OUT/logs/$title.probe.log)"
  else
    "$BIN/ubench-k2p" --window "$win" --terminal "$term" --out "$RESULTS" --samples "$K2P_SAMPLES" \
      --case "$case_name" --claim "$CLAIM" --display-session "$SESSION" "${probe_args[@]}" \
      2>>"$OUT/logs/$title.probe.log" || fail "$term k2p $case_name failed (see $OUT/logs/$title.probe.log)"
  fi
  stop_term "$term" "$title"
}

new_tab() {
  local term="$1" title="$2"
  case "$term" in
    ultty) "$BIN/ulttyctl" new-tab sleep 600 >/dev/null ;;
    gnome-terminal)
      DBUS_SESSION_BUS_ADDRESS=$(tr '\0' '\n' <"/proc/$(owner_pid "$term" "$title")/environ" | sed -n 's/^DBUS_SESSION_BUS_ADDRESS=//p') \
        gnome-terminal --tab -- sleep 600 ;;
    alacritty) ALACRITTY_SOCKET=$(find "$RUNTIME" -maxdepth 1 -name 'Alacritty-*.sock' | head -1) \
        alacritty msg create-window -e sleep 600 ;;
    ghostty)
      if [[ "$SESSION" == wayland ]]; then wtype -M ctrl -M shift t -m shift -m ctrl
      else xdotool key --clearmodifiers ctrl+shift+t; fi ;;
  esac
}

run_tabs() {
  local term="$1"
  local title="ub-${term}-tabs" n have=1 pid rss pss kind=tabs
  [[ "$term" == alacritty ]] && kind=windows
  wrapper "$title" sleep 3600
  launch "$term" "$title"
  find_window "$title" >/dev/null || { fail "$term tabs: no window"; stop_term "$term" "$title"; return; }
  sleep 2
  IFS=',' read -r -a counts <<<"$TABS"
  for n in "${counts[@]}"; do
    while (( have < n )); do new_tab "$term" "$title" || { fail "$term: opening tab $have failed"; break 2; }; have=$((have + 1)); sleep 0.3; done
    sleep 3
    pid=$(owner_pid "$term" "$title")
    read -r rss pss < <(mem_of "$pid")
    emit "{\"suite\":\"tabs.scale\",\"case\":\"$kind/$n\",\"arm\":\"$term\",\"block\":0,\"value\":$rss,\"pss_kib\":$pss,\"unit\":\"KiB\",\"equivalent\":$([[ $kind == tabs ]] && echo true || echo false),\"claim\":\"$CLAIM\",\"env\":$(jenv)}"
  done
  stop_term "$term" "$title"
}

# ---------------------------------------------------------------- campaign
IFS=',' read -r -a terms <<<"$TERMINALS"
IFS=',' read -r -a floods <<<"$FLOOD"
for ((round = 0; round < ROUNDS; round++)); do
  mapfile -t order < <(printf '%s\n' "${terms[@]}" | shuf)
  for term in "${order[@]}"; do
    log "round $round: $term throughput"
    run_feed "$term" "$round"
    if (( IDLE_SECS > 0 )); then log "round $round: $term idle"; run_idle "$term" "$round"; fi
    if (( K2P_SAMPLES > 0 )); then
      for f in idle "${floods[@]}"; do
        [[ -n "$f" ]] || continue
        log "round $round: $term latency ($f)"
        run_k2p "$term" "$round" "$f"
      done
    fi
  done
done
if [[ -n "$TABS" ]]; then
  for term in "${terms[@]}"; do log "$term tab scaling"; run_tabs "$term"; done
fi

# ---------------------------------------------------------------- report
{
  echo "# Comparative run: $SESSION ($CLAIM)"
  echo
  echo "Versions: \`$OUT/versions.json\`"
  echo
  echo '```json'
  cat "$OUT/versions.json"
  echo '```'
  echo
  echo "## All samples"
  echo
  python3 -I "$HERE/analyze.py" "$RESULTS" --markdown
  for t in "${terms[@]}"; do
    [[ "$t" == ultty ]] && continue
    echo
    echo "## Ultty vs $t (ratio = $t / ultty)"
    echo
    python3 -I "$HERE/analyze.py" "$RESULTS" --a ultty --b "$t" --markdown --neutral --threshold 0.05 || true
  done
  if [[ -s "$FAILURES" ]]; then
    echo
    echo "## Failures"
    echo
    sed 's/^/- /' "$FAILURES"
  fi
} >"$OUT/report.md" 2>>"$OUT/logs/report.log"

log "report: $OUT/report.md"
if [[ -s "$FAILURES" ]]; then
  echo "compare.sh: $(wc -l <"$FAILURES") measurement(s) failed:" >&2
  cat "$FAILURES" >&2
  exit 1
fi
