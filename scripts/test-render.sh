#!/bin/bash
# Test rendering by running ultty with a test command, then dumping state

# Kill any existing ultty
pkill -f "target/release/ultty" 2>/dev/null
sleep 0.5

# Start ultty with test script in background
./target/release/ultty -e "bash -c '
# Clear and position cursor
printf \"\033[2J\033[H\"

# Draw box with cursor positioning (simulating Claude-like output)
printf \"\033[38;5;208m\"  # Orange

# Set scroll region and origin mode like Claude does
printf \"\033[3;20r\"     # Set scroll region rows 3-20
printf \"\033[?6h\"       # Enable origin mode (DECOM)

# Now cursor positions are relative to scroll region
printf \"\033[1;1H\"      # This should go to row 3 (region top), col 1
printf \"Line 1 in region\"

printf \"\033[2;1H\"      # This should go to row 4, col 1
printf \"Line 2 in region\"

printf \"\033[5;10H\"     # Row 7, col 10
printf \"Positioned text\"

# Reset origin mode
printf \"\033[?6l\"

# Now draw outside region
printf \"\033[1;1H\"      # Actual row 1
printf \"Outside region (top)\"

printf \"\033[22;1H\"     # Row 22 (below scroll region)
printf \"Outside region (bottom)\"

printf \"\033[0m\"        # Reset colors

echo
echo \"Press Ctrl+Shift+D to dump terminal state\"
echo \"Dump will be saved to /tmp/ultty-debug-*.txt\"
sleep 30
'" &

ULTTY_PID=$!
echo "Started ultty (PID: $ULTTY_PID)"
echo ""
echo "To test:"
echo "1. Look at the terminal window"
echo "2. Press Ctrl+Shift+D to create debug dump"
echo "3. Check /tmp/ultty-debug-*.txt"
echo ""
echo "Waiting for ultty to exit or press Ctrl+C..."
wait $ULTTY_PID 2>/dev/null
