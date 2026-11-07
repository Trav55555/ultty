#!/bin/sh
# Install Ultty from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/Trav55555/ultty/main/install.sh | sh
#
# Installs ultty and ulttyctl into ~/.local/bin, the desktop entry and icons
# into ~/.local/share, and the xterm-ultty terminfo entry into ~/.terminfo
# when tic is available. No root needed. The download is checked against the
# release's SHA256SUMS before anything is installed.
#
# Environment:
#   ULTTY_VERSION      release tag to install, such as v0.1.0 (default: latest)
#   ULTTY_INSTALL_DIR  where the binaries go (default: ~/.local/bin)
#   ULTTY_RELEASES     releases URL to download from (default: the GitHub repo)

set -eu

RELEASES="${ULTTY_RELEASES:-https://github.com/Trav55555/ultty/releases}"
INSTALL_DIR="${ULTTY_INSTALL_DIR:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"

say() {
  printf 'ultty-install: %s\n' "$*"
}

die() {
  printf 'ultty-install: error: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

# The Rust target triple for this machine.
detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU: $arch" ;;
  esac
  case "$os" in
    Linux) echo "$arch-unknown-linux-gnu" ;;
    Darwin) echo "$arch-apple-darwin" ;;
    *) die "unsupported OS: $os (Ultty runs on Linux and macOS)" ;;
  esac
}

# The tag of the newest release, read from where releases/latest redirects.
latest_version() {
  url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$RELEASES/latest")" ||
    die "could not reach $RELEASES"
  tag="${url##*/}"
  case "$tag" in
    v*) echo "$tag" ;;
    *) die "no published release found at $RELEASES" ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# $1 as one quoted Exec= argument, per the Desktop Entry spec: inside double
# quotes, escape " ` $ and \ with a backslash; then double every backslash
# (the string-value escape) and every % (field codes).
desktop_exec_quote() {
  escaped="$(printf '%s' "$1" | sed -e 's/[\\"`$]/\\&/g' -e 's/\\/\\\\/g' -e 's/%/%%/g')"
  printf '"%s"' "$escaped"
}

install_desktop_entry() {
  extracted="$1"
  [ -f "$extracted/dist/ultty.desktop" ] || return 0
  case "$INSTALL_DIR" in
    *'
'*)
      say "not adding a desktop entry: $INSTALL_DIR contains a newline"
      return 0
      ;;
  esac
  mkdir -p "$DATA_DIR/applications"
  # Launchers do not always have ~/.local/bin on PATH; point at the binary.
  # The path goes through ENVIRON, so awk never interprets it.
  ULTTY_EXEC="$(desktop_exec_quote "$INSTALL_DIR/ultty")" awk '
    /^Exec=ultty/ { print "Exec=" ENVIRON["ULTTY_EXEC"] substr($0, 11); next }
    { print }
  ' "$extracted/dist/ultty.desktop" >"$DATA_DIR/applications/ultty.desktop"
  if [ -d "$extracted/dist/icons/hicolor" ]; then
    mkdir -p "$DATA_DIR/icons"
    cp -R "$extracted/dist/icons/hicolor" "$DATA_DIR/icons/"
  fi
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true
  fi
  say "added Ultty to the application menu"
}

# Compile the xterm-ultty terminfo entry into ~/.terminfo, so terminal.term
# can be set to xterm-ultty. Skipped quietly without tic or in a release
# that does not ship the entry.
install_terminfo() {
  source="$1/dist/terminfo/ultty.terminfo"
  [ -f "$source" ] || return 0
  command -v tic >/dev/null 2>&1 || return 0
  mkdir -p "$HOME/.terminfo"
  if tic -x -o "$HOME/.terminfo" "$source" >/dev/null 2>&1; then
    say "installed the xterm-ultty terminfo entry to $HOME/.terminfo"
  else
    say "could not compile the xterm-ultty terminfo entry; TERM stays xterm-256color"
  fi
}

main() {
  need curl
  need tar
  need uname
  command -v sha256sum >/dev/null 2>&1 || need shasum

  target="$(detect_target)"
  version="${ULTTY_VERSION:-$(latest_version)}"
  archive="ultty-$version-$target.tar.gz"

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT INT TERM

  say "downloading Ultty $version for $target"
  curl -fsSL -o "$tmp/$archive" "$RELEASES/download/$version/$archive" ||
    die "no $target build in release $version"
  curl -fsSL -o "$tmp/SHA256SUMS" "$RELEASES/download/$version/SHA256SUMS" ||
    die "release $version has no SHA256SUMS"

  expected="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")"
  [ -n "$expected" ] || die "SHA256SUMS does not list $archive"
  [ "$(sha256 "$tmp/$archive")" = "$expected" ] || die "checksum mismatch for $archive"

  mkdir -p "$tmp/x"
  tar xzf "$tmp/$archive" -C "$tmp/x"
  for bin in ultty ulttyctl; do
    [ -f "$tmp/x/$bin" ] || die "$archive is missing $bin"
  done

  mkdir -p "$INSTALL_DIR"
  for bin in ultty ulttyctl; do
    # Write beside the target, then rename, so a running ultty is not disturbed.
    cp "$tmp/x/$bin" "$INSTALL_DIR/.$bin.new"
    chmod 755 "$INSTALL_DIR/.$bin.new"
    mv -f "$INSTALL_DIR/.$bin.new" "$INSTALL_DIR/$bin"
  done
  say "installed ultty and ulttyctl to $INSTALL_DIR"

  if [ "$(uname -s)" = Linux ]; then
    install_desktop_entry "$tmp/x"
  fi
  install_terminfo "$tmp/x"

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) say "add $INSTALL_DIR to your PATH to run ultty from a shell" ;;
  esac
  say "done: run ultty"
}

# Everything runs from main, so a download cut short cannot run half a script.
main "$@"
