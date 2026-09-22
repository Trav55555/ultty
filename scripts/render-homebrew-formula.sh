#!/usr/bin/env bash
# Render dist/homebrew/ultty.rb for a release: substitute the version and the
# sha256 placeholders with the checksums of the published tarballs.
set -euo pipefail

TEMPLATE="dist/homebrew/ultty.rb"
OUTPUT=""
VERSION=""
ARTIFACT_DIR="."

usage() {
  cat <<'USAGE'
render-homebrew-formula.sh - fill the Homebrew formula template for a release

USAGE:
  scripts/render-homebrew-formula.sh --version <version> --out <path> [OPTIONS]

OPTIONS:
  --version <version>       Release version without a leading "v"
  --out <path>              Where to write the rendered formula
  --template <path>         Formula template (default: dist/homebrew/ultty.rb)
  --artifact-dir <path>     Directory holding the release tarballs (default: .)
  -h, --help                Show this help

The tarballs must be named ultty-v<version>-<target>.tar.gz for the
aarch64-apple-darwin, x86_64-apple-darwin, and x86_64-unknown-linux-gnu
targets.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:?missing value for --version}"
      shift 2
      ;;
    --out)
      OUTPUT="${2:?missing value for --out}"
      shift 2
      ;;
    --template)
      TEMPLATE="${2:?missing value for --template}"
      shift 2
      ;;
    --artifact-dir)
      ARTIFACT_DIR="${2:?missing value for --artifact-dir}"
      shift 2
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

if [[ -z "$VERSION" || -z "$OUTPUT" ]]; then
  echo "--version and --out are required" >&2
  usage >&2
  exit 2
fi

VERSION="${VERSION#v}"

if [[ ! -s "$TEMPLATE" ]]; then
  echo "formula template $TEMPLATE does not exist or is empty" >&2
  exit 1
fi

sha256_of() {
  local target="$1"
  local path="$ARTIFACT_DIR/ultty-v$VERSION-$target.tar.gz"

  if [[ ! -s "$path" ]]; then
    echo "missing release tarball: $path" >&2
    exit 1
  fi

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | cut -d' ' -f1
  else
    shasum -a 256 "$path" | cut -d' ' -f1
  fi
}

SHA_ARM64="$(sha256_of aarch64-apple-darwin)"
SHA_X86_64="$(sha256_of x86_64-apple-darwin)"
SHA_LINUX="$(sha256_of x86_64-unknown-linux-gnu)"

mkdir -p "$(dirname "$OUTPUT")"

sed \
  -e "s/\"VERSION\"/\"$VERSION\"/" \
  -e "s/\"SHA256_ARM64\"/\"$SHA_ARM64\"/" \
  -e "s/\"SHA256_X86_64\"/\"$SHA_X86_64\"/" \
  -e "s/\"SHA256_LINUX\"/\"$SHA_LINUX\"/" \
  "$TEMPLATE" >"$OUTPUT"

echo "rendered $OUTPUT for version $VERSION"
