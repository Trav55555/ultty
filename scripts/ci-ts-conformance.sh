#!/usr/bin/env bash
# Run the TypeScript client's v1 wire conformance suite against
# crates/ultty-protocol/tests/conformance/v1.json.
#
# Requires Node.js >= 22.6 (native TypeScript stripping) or Bun.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_FILE="$ROOT/clients/typescript/tests/conformance.test.ts"

if command -v node >/dev/null 2>&1; then
  mode=$(node -e '
    const [maj, min] = process.versions.node.split(".").map(Number);
    if (maj >= 24 || (maj === 23 && min >= 6) || (maj === 22 && min >= 18)) {
      console.log("native");
    } else if (maj === 23 || (maj === 22 && min >= 6)) {
      console.log("flagged");
    } else {
      console.log("unsupported");
    }
  ')
  if [[ "$mode" == "native" ]]; then
    echo "ts-conformance: running with node $(node --version)" >&2
    exec node --test "$TEST_FILE"
  elif [[ "$mode" == "flagged" ]]; then
    echo "ts-conformance: running with node $(node --version) (--experimental-strip-types)" >&2
    exec node --experimental-strip-types --test "$TEST_FILE"
  fi
fi

if command -v bun >/dev/null 2>&1; then
  echo "ts-conformance: running with bun $(bun --version)" >&2
  exec bun test "$TEST_FILE"
fi

msg="ts-conformance: skipped (requires Node >= 22.6 or Bun; found $(node --version 2>/dev/null || echo none))"
echo "$msg" >&2
if [[ "${CI_REQUIRE_TS_CONFORMANCE:-0}" == 1 ]]; then
  exit 1
fi
