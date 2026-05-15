#!/usr/bin/env bash
set -euo pipefail

cargo test -p porygon-tui porygon_stream_smoke "$@"
