#!/bin/bash
# Take a screenshot using XDG portal (works on GNOME Wayland)
OUT="${1:-/tmp/ultty-screenshot.png}"

# Use Python to call the portal since it handles the interactive dialog
python3 << 'EOF' - "$OUT"
import sys
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib, Gio

output_path = sys.argv[1] if len(sys.argv) > 1 else '/tmp/ultty-screenshot.png'

def on_response(portal, result, user_data):
    try:
        success, uri = portal.call_finish(result)
        if success and uri:
            # Copy from portal location to our destination
            import shutil
            src = uri.replace('file://', '')
            shutil.copy(src, output_path)
            print(f"Screenshot saved to: {output_path}")
        else:
            print("Screenshot cancelled or failed")
    except Exception as e:
        print(f"Error: {e}")
    finally:
        Gtk.main_quit()

# For non-interactive, just use import from ImageMagick if available
import subprocess
try:
    # Try to capture the root window with import
    result = subprocess.run(['import', '-window', 'root', output_path],
                          capture_output=True, timeout=5)
    if result.returncode == 0:
        print(f"Screenshot saved to: {output_path}")
    else:
        print("Screenshot failed")
except Exception as e:
    print(f"Error: {e}")
EOF
