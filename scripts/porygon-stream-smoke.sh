#!/usr/bin/env bash
set -euo pipefail

# porygon-tui is excluded from the workspace, so this runs against its own
# manifest rather than `cargo test -p porygon-tui`.
cargo test --manifest-path examples/porygon-tui/Cargo.toml porygon_stream_smoke "$@"
