#!/usr/bin/env bash
# Runs ON the benchmark instance (Ubuntu 24.04, or 26.04 when Ghostty is
# compared), driven by scripts/bench-aws.sh.
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
#   Ghostty       1.4.0 built from the signed source release with Zig 0.17.0
#                 (both checked against pinned SHA-256s and the projects'
#                 minisign keys). The snap bundles its own Mesa, which cannot
#                 drive the NVIDIA driver, so it never opened a window.
#   GNOME Terminal Ubuntu 24.04 apt package; the version is recorded, and
#                 EXPECT_GNOME_TERMINAL (a dpkg version) fails the setup on drift
#   NVIDIA driver nvidia-driver-${NVIDIA_DRIVER:-580} (GPU role only; 550 on
#                 24.04 pulled in 580 anyway)
set -euo pipefail

WORK="${UBENCH_WORK:-$HOME/ubench}"
SRC="$WORK/src"
TARGET="$WORK/target"
RESULTS="$WORK/results"
ALACRITTY_VERSION="${ALACRITTY_VERSION:-0.16.1}"
EXPECT_GHOSTTY="${EXPECT_GHOSTTY:-1.4.0}"
GHOSTTY_SHA256=cb188ac6c3a8278d5fd7bfef15d8c017104c87f638b5560eb38024a71ae0f005
GHOSTTY_MINISIGN_KEY=RWQlAjJC23149WL2sEpT/l0QKy7hMIFhYdQOFy0Z7z7PbneUgvlsnYcV
ZIG_VERSION=0.17.0
ZIG_SHA256=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
ZIG_MINISIGN_KEY=RWSGOq2NVecA2UPNdBUZykf1CCb147pkmdtYxgb3Ti+JO/wCYvhbAb/U
NVIDIA_DRIVER="${NVIDIA_DRIVER:-580}"
# /snap/bin last: the source-built Ghostty in /usr/local/bin must win.
export PATH="$HOME/.cargo/bin:$WORK/alacritty/bin:$PATH:/snap/bin"

die() { echo "aws-remote: $*" >&2; exit 1; }
log() { echo "[aws-remote $(date +%T)] $*" >&2; }

require_ubuntu_2404() {
  local id version
  id=$(awk -F= '$1 == "ID" {gsub(/"/, "", $2); print $2}' /etc/os-release)
  version=$(awk -F= '$1 == "VERSION_ID" {gsub(/"/, "", $2); print $2}' /etc/os-release)
  # 26.04 for a run that includes Ghostty: 1.4 needs GTK 4.18 and
  # libadwaita 1.7, newer than 24.04 ships.
  [[ "$id" == ubuntu && ( "$version" == 24.04 || "$version" == 26.04 ) ]] ||
    die "expected Ubuntu 24.04 or 26.04, found $id $version"
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

# fetch_verified URL SHA256 MINISIGN_KEY: download URL and its .minisig into
# $WORK/dl, check both the pinned hash and the signature, print the path.
fetch_verified() {
  local url="$1" sha="$2" key="$3" file
  mkdir -p "$WORK/dl"
  file="$WORK/dl/$(basename "$url")"
  [[ -f "$file" ]] || curl -fsSL -o "$file" "$url"
  [[ -f "$file.minisig" ]] || curl -fsSL -o "$file.minisig" "$url.minisig"
  echo "$sha  $file" | sha256sum -c --quiet - || die "$file does not match its pinned SHA-256"
  minisign -Vqm "$file" -P "$key" || die "$file has no valid signature"
  echo "$file"
}

setup_ghostty() {
  local have
  have=$(/usr/local/bin/ghostty --version 2>/dev/null | awk '/^Ghostty/ {print $2; exit}' || true)
  if [[ "$have" == "$EXPECT_GHOSTTY" ]]; then return; fi
  log "building Ghostty $EXPECT_GHOSTTY from source (Zig $ZIG_VERSION)"
  apt_install minisign libgtk-4-dev libadwaita-1-dev gettext libxml2-utils
  local zig_tar ghostty_tar build="$WORK/ghostty-build"
  zig_tar=$(fetch_verified "https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz" \
    "$ZIG_SHA256" "$ZIG_MINISIGN_KEY")
  ghostty_tar=$(fetch_verified "https://release.files.ghostty.org/$EXPECT_GHOSTTY/ghostty-$EXPECT_GHOSTTY.tar.gz" \
    "$GHOSTTY_SHA256" "$GHOSTTY_MINISIGN_KEY")
  # Ubuntu 24.04 does not package gtk4-layer-shell, so Ghostty builds its
  # bundled copy (-fno-sys=gtk4-layer-shell).
  rm -rf "$build" && mkdir -p "$build"
  tar -xJf "$zig_tar" -C "$build"
  tar -xzf "$ghostty_tar" -C "$build"
  (cd "$build/ghostty-$EXPECT_GHOSTTY" &&
    "$build/zig-x86_64-linux-$ZIG_VERSION/zig" build -Doptimize=ReleaseFast \
      -fno-sys=gtk4-layer-shell -p "$build/install")
  sudo install -m 0755 "$build/install/bin/ghostty" /usr/local/bin/ghostty
  # The bundled gtk4-layer-shell is a shared library next to the binary.
  if [[ -d "$build/install/lib" ]]; then
    sudo cp -a "$build/install/lib/." /usr/local/lib/
    sudo ldconfig
  fi
  sudo cp -r "$build/install/share/." /usr/local/share/
  have=$(/usr/local/bin/ghostty --version 2>/dev/null | awk '/^Ghostty/ {print $2; exit}')
  [[ "$have" == "$EXPECT_GHOSTTY" ]] || die "built Ghostty reports $have, expected $EXPECT_GHOSTTY"
}

setup_terminals() {
  apt_install xvfb xserver-xorg-core x11-utils x11-xserver-utils xdotool dbus-x11 dbus-user-session \
    gnome-terminal dconf-cli sway grim wtype mesa-vulkan-drivers mesa-utils openbox
  if [[ -n "${EXPECT_GNOME_TERMINAL:-}" ]]; then
    have=$(dpkg-query -W -f='${Version}' gnome-terminal)
    [[ "$have" == "$EXPECT_GNOME_TERMINAL" ]] || die "gnome-terminal is $have, expected $EXPECT_GNOME_TERMINAL"
  fi
  setup_ghostty
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
  # The headless sway session opens the GPU's render node itself; it is
  # root:render 0660. The group applies from the next SSH session, which
  # every later step of bench-aws.sh is.
  if ! id -nG | tr ' ' '\n' | grep -qx render; then
    sudo usermod -aG render,video "$USER"
  fi
  # Headless Xorg on the GPU: no monitor is attached (a T4 has no outputs),
  # so the screen is virtual. XTest and GetImage work on it.
  if [[ ! -f /etc/X11/ubench-xorg.conf ]] && nvidia-smi >/dev/null 2>&1; then
    busid=$(nvidia-xconfig --query-gpu-info | awk -F'PCI:' '/PCI BusID/ {print "PCI:"$2; exit}')
    # No --use-display-device=None: driver 580 refuses it with a virtual
    # screen ("not supported with virtual display").
    sudo nvidia-xconfig -o /etc/X11/ubench-xorg.conf --allow-empty-initial-configuration \
      --virtual=2560x1440 --busid="$busid" >/dev/null
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
  # A fresh clone of the bundle each build: cheap, and never stale. The
  # cargo target directory lives outside it, so builds stay incremental.
  rm -rf "$SRC"
  # Cloning checks the bundle (`git bundle verify` needs a repository).
  git clone --quiet --no-checkout "$bundle" "$SRC" || die "bundle $bundle is not valid"
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
