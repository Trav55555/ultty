#!/usr/bin/env bash
# Start GNOME Terminal with a benchmark profile. compare.sh runs this inside
# `dbus-run-session` with a private XDG_CONFIG_HOME, so gsettings writes go
# to a throwaway dconf database and the gnome-terminal-server that starts is
# private to this run (never the user's).
#
#   gnome-run.sh "FONT SIZE" COLS ROWS CMD [ARGS...]
set -euo pipefail

font="$1" cols="$2" rows="$3"
shift 3
[[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] || { echo "gnome-run.sh: needs a private session bus" >&2; exit 1; }

export GSETTINGS_BACKEND=dconf
uuid=$(gsettings get org.gnome.Terminal.ProfilesList default | tr -d "'")
profile="org.gnome.Terminal.Legacy.Profile:/org/gnome/terminal/legacy/profiles:/:$uuid/"
gsettings set "$profile" use-system-font false
gsettings set "$profile" font "$font"
gsettings set "$profile" default-size-columns "$cols"
gsettings set "$profile" default-size-rows "$rows"
gsettings set "$profile" scrollback-unlimited false
gsettings set "$profile" scrollback-lines 10000
gsettings set "$profile" cursor-blink-mode off
gsettings set "$profile" audible-bell false
gsettings set org.gnome.Terminal.Legacy.Settings confirm-close false
gsettings set org.gnome.Terminal.Legacy.Settings default-show-menubar false

# Fail loudly if the settings did not stick (for example, no dconf service).
[[ "$(gsettings get "$profile" default-size-columns)" == "$cols" ]] || {
  echo "gnome-run.sh: profile settings were not stored" >&2
  exit 1
}

exec gnome-terminal --wait -- "$@"
