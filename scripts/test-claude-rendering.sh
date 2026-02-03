#!/bin/bash
# Test Claude Code rendering issues using ulttyctl
#
# Usage: Start ultty, then run this script in another terminal
#
# Before running:
#   ULTTY_PTY_LOG=/tmp/pty.bin ./target/release/ultty
#
# Then in another terminal:
#   ./scripts/test-claude-rendering.sh

set -e

ULTTYCTL="./target/release/ulttyctl"

# Check if ulttyctl is built
if [ ! -f "$ULTTYCTL" ]; then
    echo "Building ulttyctl..."
    cargo build --release -p ultty --bin ulttyctl
fi

# Check if ultty is running
if ! $ULTTYCTL ping 2>/dev/null | grep -q "PONG"; then
    echo "ERROR: ultty is not running"
    echo ""
    echo "Start ultty first with PTY logging:"
    echo "  ULTTY_PTY_LOG=/tmp/pty.bin ./target/release/ultty"
    exit 1
fi

echo "=== ultty Rendering Test for Claude Code ==="
echo ""

# Get terminal info
echo "1. Terminal info:"
$ULTTYCTL info
echo ""

# Capture initial text
echo "2. Initial terminal state:"
$ULTTYCTL text | head -20
echo ""
echo "---"
echo ""

# Send claude command
echo "3. Launching Claude Code..."
$ULTTYCTL send-text "claude"
$ULTTYCTL send-key enter
sleep 3

# Capture text after launch
echo "4. Terminal after Claude Code launch:"
$ULTTYCTL text | head -40
echo ""
echo "---"
echo ""

# Check terminal info again
echo "5. Terminal info with Claude Code running:"
$ULTTYCTL info
echo ""

# Capture screenshot
SCREENSHOT_PATH="/tmp/ultty-claude-screenshot.png"
echo "6. Taking screenshot to: $SCREENSHOT_PATH"
$ULTTYCTL screenshot "$SCREENSHOT_PATH"
echo ""

# Wait a bit then capture again to check for rendering stability
echo "7. Checking rendering stability..."
sleep 2
TEXT1=$($ULTTYCTL text)
sleep 1
TEXT2=$($ULTTYCTL text)

if [ "$TEXT1" = "$TEXT2" ]; then
    echo "   Rendering is stable (no flickering detected)"
else
    echo "   WARNING: Rendering is unstable (content changed without input)"
    echo ""
    echo "   First capture:"
    echo "$TEXT1" | head -10
    echo ""
    echo "   Second capture:"
    echo "$TEXT2" | head -10
fi
echo ""

# Exit Claude Code
echo "8. Exiting Claude Code..."
$ULTTYCTL send-key c ctrl
sleep 1

# Final state
echo "9. Final terminal state:"
$ULTTYCTL text | head -20
echo ""

echo "=== Test Complete ==="
echo ""
echo "PTY log saved to: /tmp/pty.bin (if PTY logging was enabled)"
echo "Screenshot saved to: $SCREENSHOT_PATH"
echo ""
echo "To analyze PTY log:"
echo "  xxd /tmp/pty.bin | head -100"
echo "  grep -ao '\\x1b\\[[0-9;]*[a-zA-Z]' /tmp/pty.bin | sort | uniq -c | sort -rn | head -20"
