#!/bin/bash
# Send keystrokes to ultty window using wtype (Wayland) or xdotool (X11)
# Usage: send-keys.sh "text to type"

TEXT="$1"

if [ -z "$TEXT" ]; then
    echo "Usage: $0 'text to type'"
    exit 1
fi

if [ "$XDG_SESSION_TYPE" = "wayland" ]; then
    if command -v wtype &> /dev/null; then
        # Focus ultty window first if possible
        swaymsg '[app_id="ultty"] focus' 2>/dev/null || true
        sleep 0.1
        wtype "$TEXT"
    else
        echo "wtype not installed. Install with: sudo apt install wtype"
        exit 1
    fi
else
    if command -v xdotool &> /dev/null; then
        xdotool type "$TEXT"
    else
        echo "xdotool not installed. Install with: sudo apt install xdotool"
        exit 1
    fi
fi
