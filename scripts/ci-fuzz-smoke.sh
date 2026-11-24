#!/usr/bin/env bash
# Run the CI fuzz targets with a single parallel build followed by two balanced
# concurrent waves, capping concurrency at 2 ASan processes to stay within runner
# RAM limits.
#
# Usage:
#   scripts/ci-fuzz-smoke.sh [--mode smoke|nightly] [--base <commit>] [nightly-toolchain]
#
# When SCCACHE_BUCKET is set and the AWS CLI is available, restores each target's
# corpus from S3 before running (so PR smoke replays real edge cases instead of
# starting from empty inputs). In --mode nightly, also minimizes each target's
# corpus with -merge=1 and uploads it back to S3.
set -euo pipefail

MODE="smoke"
BASE=""
TOOLCHAIN=""
while [ $# -gt 0 ]; do
  case "$1" in
    --mode) MODE="${2:?}"; shift 2 ;;
    --base) BASE="${2:?}"; shift 2 ;;
    -h|--help)
      echo "Usage: $0 [--mode smoke|nightly] [--base <commit>] [nightly-toolchain]"
      exit 0
      ;;
    -*) echo "ci-fuzz-smoke: unknown option: $1" >&2; exit 2 ;;
    *)
      [ -z "$TOOLCHAIN" ] || { echo "ci-fuzz-smoke: unexpected argument: $1" >&2; exit 2; }
      TOOLCHAIN="$1"
      shift
      ;;
  esac
done

case "$MODE" in
  smoke|nightly) ;;
  *) echo "ci-fuzz-smoke: invalid --mode '$MODE' (expected smoke or nightly)" >&2; exit 2 ;;
esac
TOOLCHAIN="${TOOLCHAIN:-nightly}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TARGETS=(vt_terminal decode_app decode_terminal in_band_split kitty_graphics image_decode)

if [ "$MODE" = smoke ] && [ -n "$BASE" ] && [ "${CI_FUZZ_FORCE:-0}" != 1 ]; then
  given="$BASE"
  [[ "$given" =~ ^0+$ ]] && given="$(git rev-parse HEAD^1)"
  if base_sha="$(git merge-base HEAD "$given" 2>/dev/null)"; then
    if git diff --quiet "$base_sha" HEAD -- \
      crates/ultty-vt crates/ultty-protocol crates/ultty-client fuzz Cargo.toml Cargo.lock rust-toolchain.toml; then
      echo "ci-fuzz-smoke: no changes in fuzzed crates, fuzz/, or workspace lockfile/toolchain; skipping" >&2
      if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        printf '### Fuzz (smoke)\n\nSkipped: no changes in `ultty-vt`, `ultty-protocol`, `ultty-client`, `fuzz/`, or workspace lockfile/toolchain.\n' >>"$GITHUB_STEP_SUMMARY"
      fi
      exit 0
    fi
  fi
fi

S3_CORPUS_PREFIX=""
if [ -n "${SCCACHE_BUCKET:-}" ] && command -v aws >/dev/null 2>&1; then
  S3_CORPUS_PREFIX="s3://${SCCACHE_BUCKET}/${SCCACHE_S3_KEY_PREFIX:-ultty/}fuzz-corpus"
fi

count_files() {
  local dir="$1"
  if [ -d "$dir" ]; then
    find "$dir" -maxdepth 1 -type f | wc -l | tr -d ' '
  else
    echo 0
  fi
}

cd "$ROOT/fuzz"

declare -A BEFORE_COUNTS=()
for target in "${TARGETS[@]}"; do
  mkdir -p "corpus/$target"
  if [ -n "$S3_CORPUS_PREFIX" ]; then
    aws s3 sync --only-show-errors "$S3_CORPUS_PREFIX/$target/" "corpus/$target/" ||
      echo "::warning::could not restore fuzz corpus for $target from $S3_CORPUS_PREFIX/$target/" >&2
  fi
  BEFORE_COUNTS["$target"]="$(count_files "corpus/$target")"
done

# Pass the host target explicitly: prebuilt musl cargo-fuzz binaries (such as
# from taiki-e/install-action on Linux) otherwise default --target to
# x86_64-unknown-linux-musl, which fails under AddressSanitizer.
FUZZ_TARGET_TRIPLE="${FUZZ_TARGET_TRIPLE:-$(rustc "+$TOOLCHAIN" -vV | awk '/^host: /{print $2; exit}')}"
[ -n "$FUZZ_TARGET_TRIPLE" ] || { echo "ci-fuzz-smoke: could not determine rustc host triple for +$TOOLCHAIN" >&2; exit 1; }

# Build all targets in one parallel Cargo invocation.
cargo "+$TOOLCHAIN" fuzz build --target "$FUZZ_TARGET_TRIPLE"

run_wave() {
  local log_a log_b pid_a pid_b status_a=0 status_b=0
  log_a="$(mktemp)"
  log_b="$(mktemp)"
  trap 'rm -f "$log_a" "$log_b"' RETURN

  (eval "$1") >"$log_a" 2>&1 &
  pid_a=$!
  (eval "$2") >"$log_b" 2>&1 &
  pid_b=$!

  wait "$pid_a" || status_a=$?
  wait "$pid_b" || status_b=$?

  cat "$log_a"
  cat "$log_b"

  if [ "$status_a" -ne 0 ] || [ "$status_b" -ne 0 ]; then
    return 1
  fi
}

if [ "$MODE" = nightly ]; then
  T_VT=600
  T_DECODE=180
  T_SPLIT=240
  T_KITTY=300
  T_IMAGE=300
else
  T_VT=60
  T_DECODE=20
  T_SPLIT=30
  T_KITTY=40
  T_IMAGE=40
fi

echo "==> Wave 1 ($MODE): vt_terminal (${T_VT}s) || (decode_app ${T_DECODE}s + decode_terminal ${T_DECODE}s + in_band_split ${T_SPLIT}s)"
run_wave \
  "cargo +\"$TOOLCHAIN\" fuzz run --target \"$FUZZ_TARGET_TRIPLE\" vt_terminal -- -max_total_time=$T_VT -max_len=4096" \
  "cargo +\"$TOOLCHAIN\" fuzz run --target \"$FUZZ_TARGET_TRIPLE\" decode_app -- -max_total_time=$T_DECODE && \
   cargo +\"$TOOLCHAIN\" fuzz run --target \"$FUZZ_TARGET_TRIPLE\" decode_terminal -- -max_total_time=$T_DECODE && \
   cargo +\"$TOOLCHAIN\" fuzz run --target \"$FUZZ_TARGET_TRIPLE\" in_band_split -- -max_total_time=$T_SPLIT -max_len=4096"

echo "==> Wave 2 ($MODE): kitty_graphics (${T_KITTY}s) || image_decode (${T_IMAGE}s)"
run_wave \
  "cargo +\"$TOOLCHAIN\" fuzz run --target \"$FUZZ_TARGET_TRIPLE\" kitty_graphics -- -max_total_time=$T_KITTY -max_len=8192" \
  "cargo +\"$TOOLCHAIN\" fuzz run --target \"$FUZZ_TARGET_TRIPLE\" image_decode -- -max_total_time=$T_IMAGE -max_len=65536 -rss_limit_mb=4096"

if [ "$MODE" = nightly ] && [ -n "$S3_CORPUS_PREFIX" ]; then
  echo "==> Minimizing and syncing corpus back to $S3_CORPUS_PREFIX"
  for target in "${TARGETS[@]}"; do
    merged="$(mktemp -d)"
    if cargo "+$TOOLCHAIN" fuzz run --target "$FUZZ_TARGET_TRIPLE" "$target" "$merged" "corpus/$target" -- -merge=1 >/dev/null 2>&1; then
      rm -rf "corpus/$target"
      mv "$merged" "corpus/$target"
    else
      rm -rf "$merged"
      echo "::warning::corpus merge failed for $target; syncing unmerged corpus" >&2
    fi
    aws s3 sync --only-show-errors --delete "corpus/$target/" "$S3_CORPUS_PREFIX/$target/" ||
      echo "::warning::could not upload fuzz corpus for $target to $S3_CORPUS_PREFIX/$target/" >&2
  done
fi

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Fuzz ($MODE)"
    echo ""
    echo "| Target | Corpus before | Corpus after |"
    echo "|---|---:|---:|"
    for target in "${TARGETS[@]}"; do
      printf '| `%s` | %s | %s |\n' "$target" "${BEFORE_COUNTS[$target]}" "$(count_files "corpus/$target")"
    done
    echo ""
  } >>"$GITHUB_STEP_SUMMARY"
fi
