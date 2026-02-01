#!/bin/bash
# Run ultty with claude for testing
# Kills any existing ultty processes first

set -e

# Kill existing instances
pkill -f "target/release/ultty" 2>/dev/null || true
sleep 0.5

# Build if needed
cargo build --release 2>&1 | tail -5

# Run with logging
LOG_FILE="/tmp/ultty-claude-test.log"
RUST_LOG=info ./target/release/ultty -e claude 2>"$LOG_FILE" &
PID=$!

echo "Started ultty (PID: $PID)"
echo "Log file: $LOG_FILE"
echo ""
echo "Commands:"
echo "  scripts/screenshot.sh  - Take full screenshot"
echo "  tail -f $LOG_FILE      - Watch logs"
echo "  kill $PID              - Stop terminal"
