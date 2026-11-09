#!/usr/bin/env bash
# Trace what Ultty commits for each real key press, on a private headless
# GNOME Shell (Mutter): no sway or virtual-keyboard support needed.
#
#   scripts/bench/gnome-echo-trace.sh ULTTY OUTDIR [KEYS] [SHELL_CMD...]
#
# Starts `gnome-shell --headless --wayland --no-x11 --virtual-monitor
# 1280x800@60` under its own dbus-run-session with a private
# XDG_RUNTIME_DIR and XDG config/data/cache/state dirs (never the user's
# session), runs ULTTY in it with WAYLAND_DEBUG=client and the pipeline
# trace, presses KEYS keys through Mutter's RemoteDesktop API
# (gnome-keys.py), then stops everything it started. SHELL_CMD is what Ultty
# runs (default `cat`, so the tty echoes each key). Writes OUTDIR/wl.log
# (the client protocol log), ultty.log (the frame trace), keys.log and
# shell.log; summarise with `wl-echo-trace.py OUTDIR/wl.log`.
#
# Extra environment for Ultty (for example MESA_VK_WSI_PRESENT_MODE) can be
# passed with ULTTY_ENV="NAME=value ...".
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${1:-}" == --inner ]]; then
  gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x800@60 >"$OUT/shell.log" 2>&1 &
  shell_pid=$! ultty_pid=""
  trap '[[ -n "$ultty_pid" ]] && kill "$ultty_pid" 2>/dev/null; kill "$shell_pid" 2>/dev/null; wait 2>/dev/null' EXIT
  for _ in $(seq 100); do [[ -S "$XDG_RUNTIME_DIR/wayland-0" ]] && break; sleep 0.1; done
  [[ -S "$XDG_RUNTIME_DIR/wayland-0" ]] || { echo "gnome-shell did not start (see $OUT/shell.log)" >&2; exit 1; }
  sleep 2
  # shellcheck disable=SC2086 # ULTTY_ENV is a list of NAME=value words
  env WAYLAND_DISPLAY=wayland-0 WAYLAND_DEBUG=client RUST_LOG=ultty::perf=info,info \
    ULTTY_TRACE_PIPELINE=1 SHELL="$OUT/shell.sh" ${ULTTY_ENV:-} \
    "$ULTTY" >"$OUT/ultty.log" 2>"$OUT/wl.log" &
  ultty_pid=$!
  sleep 4
  /usr/bin/python3 -I "$HERE/gnome-keys.py" "$KEYS" >"$OUT/keys.log" 2>&1
  sleep 0.5
  exit 0
fi

[[ $# -ge 2 ]] || { sed -n '2,20p' "$0" >&2; exit 2; }
ULTTY="$(realpath "$1")" OUT="$(mkdir -p "$2" && realpath "$2")" KEYS="${3:-20}"
shift $(( $# >= 3 ? 3 : 2 ))
cmd="${*:-cat}"
xdg="$OUT/xdg"
mkdir -p "$xdg"/{config/ultty,data,cache,state}
cat >"$xdg/config/ultty/config.toml" <<EOF
[behavior]
restore_session = false
cursor_blink = false
EOF
printf '#!/bin/sh\nexec %s\n' "$cmd" >"$OUT/shell.sh"
chmod +x "$OUT/shell.sh"
runtime=$(mktemp -d /tmp/ultty-echo.XXXX)
# GNOME's portals mount FUSE file systems in the runtime dir; they go away
# shortly after the session ends.
cleanup() { for _ in 1 2 3 4 5; do rm -rf "$runtime" 2>/dev/null && return; sleep 1; done; }
trap cleanup EXIT
env -i HOME="$HOME" PATH="$PATH" USER="${USER:-}" LANG=C.UTF-8 \
  XDG_RUNTIME_DIR="$runtime" XDG_CONFIG_HOME="$xdg/config" XDG_DATA_HOME="$xdg/data" \
  XDG_CACHE_HOME="$xdg/cache" XDG_STATE_HOME="$xdg/state" \
  ULTTY="$ULTTY" OUT="$OUT" KEYS="$KEYS" ULTTY_ENV="${ULTTY_ENV:-}" \
  dbus-run-session -- bash "$HERE/gnome-echo-trace.sh" --inner >"$OUT/session.log" 2>&1
rm -rf "$xdg"
