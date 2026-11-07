#!/usr/bin/env bash
# Runs ON the benchmark instance (Ubuntu 24.04), driven by scripts/bench-aws.sh.
# Every step is idempotent: rerunning skips work that is already done, and
# any failure stops the step with a message.
#
#   aws-remote.sh setup gpu|cpu     packages, Rust, comparison terminals, driver
#   aws-remote.sh build BUNDLE REF  check out REF from a git bundle, build release
#   aws-remote.sh run gpu|cpu       run the suite; results under ~/ubench/results
#   aws-remote.sh needs-reboot      exit 0 when a reboot is pending (driver install)
#
# Pinned versions (see docs/perf/README.md, "AWS runner"):
#   Rust          from rust-toolchain.toml (rustup, minimal profile)
#   Alacritty     0.16.1, built with `cargo install --locked` (Ubuntu 24.04's
#                 apt package is older; this matches the local comparison box)
#   Ghostty       snap latest/stable, must report 1.4.0 (EXPECT_GHOSTTY), then held
#   GNOME Terminal Ubuntu 24.04 apt package; the version is recorded, and
#                 EXPECT_GNOME_TERMINAL (a dpkg version) fails the setup on drift
#   NVIDIA driver nvidia-driver-${NVIDIA_DRIVER:-550} (GPU role only)
set -euo pipefail

WORK="${UBENCH_WORK:-$HOME/ubench}"
SRC="$WORK/src"
TARGET="$WORK/target"
RESULTS="$WORK/results"
ALACRITTY_VERSION="${ALACRITTY_VERSION:-0.16.1}"
EXPECT_GHOSTTY="${EXPECT_GHOSTTY:-1.4.0}"
NVIDIA_DRIVER="${NVIDIA_DRIVER:-550}"
export PATH="$HOME/.cargo/bin:$WORK/alacritty/bin:/snap/bin:$PATH"

die() { echo "aws-remote: $*" >&2; exit 1; }
log() { echo "[aws-remote $(date +%T)] $*" >&2; }

require_ubuntu_2404() {
  local id version
  id=$(awk -F= '$1 == "ID" {gsub(/"/, "", $2); print $2}' /etc/os-release)
  version=$(awk -F= '$1 == "VERSION_ID" {gsub(/"/, "", $2); print $2}' /etc/os-release)
  [[ "$id" == ubuntu && "$version" == 24.04 ]] || die "expected Ubuntu 24.04, found $id $version"
}

apt_install() {
  local missing=()
  for p in "$@"; do dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || missing+=("$p"); done
  if (( ${#missing[@]} )); then
    log "apt install ${missing[*]}"
    sudo DEBIAN_FRONTEND=noninteractive apt-get update -q
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${missing[@]}"
  fi
}

setup_common() {
  require_ubuntu_2404
  apt_install build-essential pkg-config cmake git curl ca-certificates python3 valgrind \
    libwayland-dev libxkbcommon-dev libxkbcommon-x11-0 libx11-xcb1 libxcursor1 libxrandr2 libxi6 \
    libfontconfig1-dev libfreetype-dev libxcb-xfixes0-dev libvulkan1 vulkan-tools \
    fonts-dejavu-core fontconfig
  if ! command -v rustup >/dev/null; then
    log "installing rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
  fi
}

setup_terminals() {
  apt_install xvfb xserver-xorg-core x11-utils x11-xserver-utils xdotool dbus-x11 dbus-user-session \
    gnome-terminal dconf-cli sway grim wtype mesa-vulkan-drivers mesa-utils
  if [[ -n "${EXPECT_GNOME_TERMINAL:-}" ]]; then
    have=$(dpkg-query -W -f='${Version}' gnome-terminal)
    [[ "$have" == "$EXPECT_GNOME_TERMINAL" ]] || die "gnome-terminal is $have, expected $EXPECT_GNOME_TERMINAL"
  fi
  if ! snap list ghostty >/dev/null 2>&1; then
    log "installing Ghostty (snap)"
    sudo snap install ghostty --classic --channel=latest/stable
  fi
  have=$(/snap/bin/ghostty --version 2>/dev/null | awk '/^Ghostty/ {print $2; exit}')
  [[ "$have" == "$EXPECT_GHOSTTY" ]] || die "Ghostty is $have, expected $EXPECT_GHOSTTY (set EXPECT_GHOSTTY to accept it)"
  sudo snap refresh --hold ghostty >/dev/null
  if [[ "$("$WORK/alacritty/bin/alacritty" --version 2>/dev/null)" != "alacritty $ALACRITTY_VERSION"* ]]; then
    log "building Alacritty $ALACRITTY_VERSION"
    rustup toolchain install stable --profile minimal
    cargo +stable install alacritty --version "$ALACRITTY_VERSION" --locked --root "$WORK/alacritty"
  fi
}

setup_nvidia() {
  if nvidia-smi >/dev/null 2>&1; then
    log "NVIDIA driver present: $(nvidia-smi --query-gpu=name,driver_version --format=csv,noheader)"
  else
    lspci | grep -qi nvidia || die "no NVIDIA GPU on this instance; use the cpu role"
    log "installing nvidia-driver-$NVIDIA_DRIVER (reboot required)"
    apt_install "nvidia-driver-$NVIDIA_DRIVER" "nvidia-utils-$NVIDIA_DRIVER"
    # GBM and DRM modesetting, needed by the wlroots (sway) session.
    echo 'options nvidia-drm modeset=1' | sudo tee /etc/modprobe.d/ubench-nvidia-drm.conf >/dev/null
    sudo touch /var/run/reboot-required
  fi
  # Headless Xorg on the GPU: no monitor is attached (a T4 has no outputs),
  # so the screen is virtual. XTest and GetImage work on it.
  if [[ ! -f /etc/X11/ubench-xorg.conf ]] && nvidia-smi >/dev/null 2>&1; then
    busid=$(nvidia-xconfig --query-gpu-info | awk -F'PCI:' '/PCI BusID/ {print "PCI:"$2; exit}')
    sudo nvidia-xconfig -o /etc/X11/ubench-xorg.conf --allow-empty-initial-configuration \
      --use-display-device=None --virtual=2560x1440 --busid="$busid" >/dev/null
  fi
}

cmd_setup() {
  local role="$1"
  setup_common
  case "$role" in
    gpu) setup_terminals; setup_nvidia ;;
    cpu) : ;;
    *) die "role must be gpu or cpu" ;;
  esac
  log "setup $role done"
}

cmd_build() {
  local bundle="$1" ref="$2"
  [[ -f "$bundle" ]] || die "no bundle at $bundle"
  git bundle verify "$bundle" >/dev/null || die "bundle $bundle is not valid"
  # A fresh clone of the bundle each build: cheap, and never stale. The
  # cargo target directory lives outside it, so builds stay incremental.
  rm -rf "$SRC"
  git clone --quiet --no-checkout "$bundle" "$SRC"
  sha=$(git -C "$SRC" rev-parse --verify "$ref^{commit}") || die "commit $ref is not in the bundle"
  git -C "$SRC" checkout --quiet --force --detach "$sha"
  local channel
  channel=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$SRC/rust-toolchain.toml")
  [[ -n "$channel" ]] || die "no channel in rust-toolchain.toml"
  rustup toolchain list | grep -q "^$channel" || rustup toolchain install "$channel" --profile minimal
  (cd "$SRC" && CARGO_TARGET_DIR="$TARGET" cargo build --release --locked -p ultty --bins -p ultty-bench)
  echo "$sha" >"$WORK/built-rev"
  log "built $sha"
}

session_x11() {
  if ! DISPLAY=:0 xdpyinfo >/dev/null 2>&1; then
    log "starting Xorg :0 on the GPU"
    nohup sudo Xorg :0 -config /etc/X11/ubench-xorg.conf -noreset -nolisten tcp -novtswitch \
      >"$WORK/xorg.log" 2>&1 &
    for _ in $(seq 100); do DISPLAY=:0 xdpyinfo >/dev/null 2>&1 && break; sleep 0.2; done
    DISPLAY=:0 xdpyinfo >/dev/null 2>&1 || die "Xorg did not start (see $WORK/xorg.log)"
    # Any local user may connect: the instance is single-purpose.
    sudo env DISPLAY=:0 XAUTHORITY= xhost +local: >/dev/null 2>&1 || true
  fi
  DISPLAY=:0 glxinfo -B 2>/dev/null | grep -i 'renderer' || true
}

cmd_run() {
  local role="$1" stamp
  [[ -f "$WORK/built-rev" ]] || die "nothing built yet; run the build step"
  stamp="$(date +%Y%m%d-%H%M%S)-$(cut -c1-12 "$WORK/built-rev")"
  mkdir -p "$RESULTS/$stamp"
  cd "$SRC"
  export CARGO_TARGET_DIR="$TARGET"
  case "$role" in
    cpu)
      scripts/bench/run.sh vt --aa --runs "${RUNS:-10}" --bin "$TARGET/release/ultty-bench" \
        --out "$RESULTS/$stamp/vt-aa" -- --suite vt,parse --geometry 80x24,120x40,300x75
      scripts/bench/icount-gate.sh --update --bin "$TARGET/release/ultty-bench" --out "$RESULTS/$stamp/icount"
      cp bench/baselines/icount.json "$RESULTS/$stamp/icount/baseline-this-machine.json"
      ;;
    gpu)
      local extra=() status=0
      read -r -a extra <<<"${COMPARE_ARGS:-}"
      session_x11
      # Run both sessions even if the first reports failures; fail at the end.
      DISPLAY=:0 scripts/bench/compare.sh --session x11 --ultty-dir "$TARGET/release" \
        --bench-dir "$TARGET/release" --out "$RESULTS/$stamp/x11" "${extra[@]}" || status=1
      WLR_RENDERER="${WLR_RENDERER:-gles2}" scripts/bench/compare.sh --session wayland \
        --ultty-dir "$TARGET/release" --bench-dir "$TARGET/release" --out "$RESULTS/$stamp/wayland" \
        "${extra[@]}" || status=1
      # Headless VT numbers on the same box, for context (not a dedicated core).
      scripts/bench/run.sh vt --runs "${RUNS:-10}" --bin "$TARGET/release/ultty-bench" \
        --allow-noisy --out "$RESULTS/$stamp/vt" || status=1
      echo "$stamp" >"$RESULTS/latest"
      (( status == 0 )) || die "some measurements failed; see $RESULTS/$stamp/*/report.md"
      ;;
    *) die "role must be gpu or cpu" ;;
  esac
  echo "$stamp" >"$RESULTS/latest"
  log "results in $RESULTS/$stamp"
}

case "${1:-}" in
  setup) cmd_setup "${2:?role}" ;;
  build) cmd_build "${2:?bundle}" "${3:?ref}" ;;
  run) cmd_run "${2:?role}" ;;
  needs-reboot) [[ -f /var/run/reboot-required ]] ;;
  *) die "usage: $0 setup|build|run|needs-reboot ..." ;;
esac
