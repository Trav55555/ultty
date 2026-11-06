#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-audit >/dev/null 2>&1; then
  echo "cargo-audit is required. Install with: cargo install cargo-audit --version 0.22.1 --locked" >&2
  exit 1
fi

# Every ignore below is a warning-class advisory (unmaintained or unsound)
# held open by cosmic-text 0.19 / winit 0.30 (ttf-parser) or by notify-rust's
# zbus 5 dependency.
# Rationale per advisory lives in docs/security-audit.md. Vulnerability-class
# advisories are never ignored here.
cargo audit \
  --deny warnings \
  --ignore RUSTSEC-2026-0192 \
  --ignore RUSTSEC-2026-0370 \
  --ignore RUSTSEC-2026-0371 \
  --ignore RUSTSEC-2026-0372
