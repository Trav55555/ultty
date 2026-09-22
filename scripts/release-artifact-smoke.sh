#!/usr/bin/env bash
set -euo pipefail

EXPECTED_VERSION=""
RUN_BINARIES=0
TARBALLS=()
DEBS=()
RPMS=()
FORMULAS=()
RENDERED_FORMULAS=()

usage() {
  cat <<'USAGE'
release-artifact-smoke.sh - verify Ultty release artifacts without launching the GUI

USAGE:
  scripts/release-artifact-smoke.sh [OPTIONS]

OPTIONS:
  --expected-version <version>  Expected version string (default: workspace version)
  --tarball <path>             Verify a release tarball
  --deb <path>                 Verify a .deb package
  --rpm <path>                 Verify an .rpm package
  --homebrew <path>            Verify the Homebrew formula test/install metadata
  --homebrew-rendered <path>   Verify a rendered formula: metadata plus no
                               leftover VERSION/SHA256_ placeholders and
                               64-hex-character sha256 values
  --run-binaries               Run extracted ultty and ulttyctl --version checks
  -h, --help                   Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expected-version)
      EXPECTED_VERSION="${2:?missing value for --expected-version}"
      shift 2
      ;;
    --tarball)
      TARBALLS+=("${2:?missing value for --tarball}")
      shift 2
      ;;
    --deb)
      DEBS+=("${2:?missing value for --deb}")
      shift 2
      ;;
    --rpm)
      RPMS+=("${2:?missing value for --rpm}")
      shift 2
      ;;
    --homebrew)
      FORMULAS+=("${2:?missing value for --homebrew}")
      shift 2
      ;;
    --homebrew-rendered)
      RENDERED_FORMULAS+=("${2:?missing value for --homebrew-rendered}")
      shift 2
      ;;
    --run-binaries)
      RUN_BINARIES=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$EXPECTED_VERSION" ]]; then
  EXPECTED_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
fi

if [[ -z "$EXPECTED_VERSION" ]]; then
  echo "could not determine expected version" >&2
  exit 1
fi

if [[ "${#TARBALLS[@]}" -eq 0 && "${#DEBS[@]}" -eq 0 && "${#RPMS[@]}" -eq 0 \
  && "${#FORMULAS[@]}" -eq 0 && "${#RENDERED_FORMULAS[@]}" -eq 0 ]]; then
  echo "no artifacts supplied" >&2
  usage >&2
  exit 2
fi

require_list_entry() {
  local list="$1"
  local expected="$2"
  local label="$3"

  if ! grep -Fxq "$expected" "$list"; then
    echo "$label missing expected entry: $expected" >&2
    echo "contents:" >&2
    sed -n '1,160p' "$list" >&2
    exit 1
  fi
}

run_version_checks() {
  local bin_dir="$1"
  local label="$2"

  local ultty="$bin_dir/ultty"
  local ulttyctl="$bin_dir/ulttyctl"

  if [[ ! -x "$ultty" ]]; then
    echo "$label missing executable ultty at $ultty" >&2
    exit 1
  fi
  if [[ ! -x "$ulttyctl" ]]; then
    echo "$label missing executable ulttyctl at $ulttyctl" >&2
    exit 1
  fi

  local ultty_version
  ultty_version="$("$ultty" --version)"
  if [[ "$ultty_version" != "ultty $EXPECTED_VERSION" ]]; then
    echo "$label unexpected ultty --version output: $ultty_version" >&2
    exit 1
  fi

  local ulttyctl_version
  ulttyctl_version="$("$ulttyctl" --version)"
  if [[ "$ulttyctl_version" != "ulttyctl $EXPECTED_VERSION" ]]; then
    echo "$label unexpected ulttyctl --version output: $ulttyctl_version" >&2
    exit 1
  fi
}

verify_tarball() {
  local artifact="$1"
  local label="tarball $artifact"

  if [[ ! -s "$artifact" ]]; then
    echo "$label does not exist or is empty" >&2
    exit 1
  fi

  local list
  list="$(mktemp)"
  tar tzf "$artifact" | sed 's#^\./##' | sort >"$list"

  require_list_entry "$list" "ultty" "$label"
  require_list_entry "$list" "ulttyctl" "$label"
  require_list_entry "$list" "dist/ultty.desktop" "$label"
  require_list_entry "$list" "dist/icons/hicolor/scalable/apps/ultty.svg" "$label"
  require_list_entry "$list" "dist/icons/hicolor/256x256/apps/ultty.png" "$label"

  if [[ "$RUN_BINARIES" -eq 1 ]]; then
    local unpack_dir
    unpack_dir="$(mktemp -d)"
    tar xzf "$artifact" -C "$unpack_dir"
    run_version_checks "$unpack_dir" "$label"
  fi
}

verify_deb() {
  local artifact="$1"
  local label="deb $artifact"

  if [[ ! -s "$artifact" ]]; then
    echo "$label does not exist or is empty" >&2
    exit 1
  fi
  if ! command -v dpkg-deb >/dev/null 2>&1; then
    echo "dpkg-deb is required to verify .deb artifacts" >&2
    exit 1
  fi

  local list
  list="$(mktemp)"
  dpkg-deb -c "$artifact" | awk '{ print $NF }' | sed 's#^\./##' | sort >"$list"

  require_list_entry "$list" "usr/bin/ultty" "$label"
  require_list_entry "$list" "usr/bin/ulttyctl" "$label"
  require_list_entry "$list" "usr/share/applications/ultty.desktop" "$label"
  require_list_entry "$list" "usr/share/icons/hicolor/scalable/apps/ultty.svg" "$label"
  require_list_entry "$list" "usr/share/icons/hicolor/256x256/apps/ultty.png" "$label"

  local unpack_dir
  unpack_dir="$(mktemp -d)"
  dpkg-deb -x "$artifact" "$unpack_dir"
  run_version_checks "$unpack_dir/usr/bin" "$label"
}

verify_rpm() {
  local artifact="$1"
  local label="rpm $artifact"

  if [[ ! -s "$artifact" ]]; then
    echo "$label does not exist or is empty" >&2
    exit 1
  fi
  if ! command -v rpm >/dev/null 2>&1; then
    echo "rpm is required to verify .rpm artifacts" >&2
    exit 1
  fi

  local list
  list="$(mktemp)"
  rpm -qpl "$artifact" | sort >"$list"

  require_list_entry "$list" "/usr/bin/ultty" "$label"
  require_list_entry "$list" "/usr/bin/ulttyctl" "$label"
  require_list_entry "$list" "/usr/share/applications/ultty.desktop" "$label"
  require_list_entry "$list" "/usr/share/icons/hicolor/scalable/apps/ultty.svg" "$label"
  require_list_entry "$list" "/usr/share/icons/hicolor/256x256/apps/ultty.png" "$label"
}

verify_homebrew_formula() {
  local formula="$1"
  local label="homebrew formula $formula"

  if [[ ! -s "$formula" ]]; then
    echo "$label does not exist or is empty" >&2
    exit 1
  fi

  grep -Fq 'shell_output("#{bin}/ultty --version")' "$formula" || {
    echo "$label must test ultty --version" >&2
    exit 1
  }
  grep -Fq 'shell_output("#{bin}/ulttyctl --version")' "$formula" || {
    echo "$label must test ulttyctl --version" >&2
    exit 1
  }
  grep -Fq '(share/"applications").install "dist/ultty.desktop"' "$formula" || {
    echo "$label must install the Linux desktop entry" >&2
    exit 1
  }
  grep -Fq '(share/"icons").install "dist/icons/hicolor"' "$formula" || {
    echo "$label must install Linux hicolor icons" >&2
    exit 1
  }
}

verify_rendered_homebrew_formula() {
  local formula="$1"
  local label="rendered homebrew formula $formula"

  verify_homebrew_formula "$formula"

  if grep -nE '(^|[^A-Za-z0-9_])(VERSION|SHA256_[A-Z0-9_]+)([^A-Za-z0-9_]|$)' "$formula" >&2; then
    echo "$label still contains template placeholders" >&2
    exit 1
  fi

  local sha_lines
  sha_lines="$(grep -c '^[[:space:]]*sha256 ' "$formula" || true)"
  if [[ "$sha_lines" -lt 3 ]]; then
    echo "$label must declare a sha256 for each published tarball, found $sha_lines" >&2
    exit 1
  fi

  local line
  while IFS= read -r line; do
    if [[ ! "$line" =~ ^[[:space:]]*sha256\ \"[0-9a-f]{64}\"[[:space:]]*$ ]]; then
      echo "$label has a sha256 that is not 64 hex characters: $line" >&2
      exit 1
    fi
  done < <(grep '^[[:space:]]*sha256 ' "$formula")

  local version
  version="$(sed -n 's/^[[:space:]]*version "\(.*\)"[[:space:]]*$/\1/p' "$formula" | head -n1)"
  if [[ -z "$version" ]]; then
    echo "$label does not declare a version" >&2
    exit 1
  fi
  if [[ -n "$EXPECTED_VERSION" && "$version" != "$EXPECTED_VERSION" ]]; then
    echo "$label declares version $version, expected $EXPECTED_VERSION" >&2
    exit 1
  fi
}

for artifact in "${TARBALLS[@]}"; do
  verify_tarball "$artifact"
done

for artifact in "${DEBS[@]}"; do
  verify_deb "$artifact"
done

for artifact in "${RPMS[@]}"; do
  verify_rpm "$artifact"
done

for formula in "${FORMULAS[@]}"; do
  verify_homebrew_formula "$formula"
done

for formula in "${RENDERED_FORMULAS[@]}"; do
  verify_rendered_homebrew_formula "$formula"
done

echo "release artifact smoke passed"
