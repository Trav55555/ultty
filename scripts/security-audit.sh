#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-audit >/dev/null 2>&1; then
  echo "cargo-audit is required. Install with: cargo install cargo-audit --version 0.22.1 --locked" >&2
  exit 1
fi

cargo audit \
  --deny warnings \
  --ignore RUSTSEC-2024-0436 \
  --ignore RUSTSEC-2026-0002 \
  --ignore RUSTSEC-2026-0097
