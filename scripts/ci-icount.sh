#!/usr/bin/env bash
# CI gate for VT instruction counts: measure the merge-base and this commit in
# the same job, with the same toolchain, valgrind and libc, and fail when any
# case regresses past scripts/bench/icount-gate.sh's threshold.
#
# The committed bench/baselines/icount.json is a developer-machine file; it
# records that machine's rustc and valgrind, and counts shift by up to a few
# percent with libc alone, so CI does not compare against it.
#
# Usage: scripts/ci-icount.sh <base-commit>
# Needs valgrind (installed with apt when missing) and a history that holds the
# base commit.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
die() { echo "ci-icount: $*" >&2; exit 1; }

given="${1:?usage: ci-icount.sh <base-commit>}"
# A new branch push reports an all-zero "before": fall back to the first parent.
[[ "$given" =~ ^0+$ ]] && given=$(git rev-parse HEAD^1)
base=$(git merge-base HEAD "$given") || die "no merge-base of HEAD and $given (shallow checkout?)"
echo "ci-icount: comparing HEAD $(git rev-parse --short HEAD) with base $(git rev-parse --short "$base")" >&2

if ! command -v valgrind >/dev/null; then
  apt=(apt-get)
  [[ "$(id -u)" == 0 ]] || apt=(sudo apt-get)
  "${apt[@]}" update -qq
  "${apt[@]}" install -y -qq --no-install-recommends valgrind
fi

work=$(mktemp -d)
trap 'git worktree remove --force "$work/base" 2>/dev/null || true; rm -rf "$work"' EXIT
git worktree add --detach "$work/base" "$base" >&2

(cd "$work/base" && CARGO_TARGET_DIR="$work/base-target" cargo build --release --locked -p ultty-bench >&2)
scripts/bench/icount-gate.sh --update --skip-memory \
  --bin "$work/base-target/release/ultty-bench" \
  --baseline "$work/base.json" --out "$work/base-out"

# Heap budgets run in the Test job; this one is instruction counts only.
scripts/bench/icount-gate.sh --skip-memory --baseline "$work/base.json" --out "$work/head-out"
