#!/bin/bash
# Take a screenshot of the ultty window specifically
OUT="${1:-/tmp/ultty-screenshot.png}"

# Find ultty window using swaymsg or similar
# For now, use slurp for region selection or full screen
if command -v slurp &> /dev/null; then
    GEOM=$(swaymsg -t get_tree | jq -r '.. | select(.app_id? == "ultty" or .name? == "Ultty") | .rect | "\(.x),\(.y) \(.width)x\(.height)"' | head -1)
    if [ -n "$GEOM" ]; then
        grim -g "$GEOM" "$OUT"
    else
        # Fallback to full screen
        grim "$OUT"
    fi
else
    grim "$OUT"
fi
echo "Screenshot saved to: $OUT"
