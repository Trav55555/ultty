#!/usr/bin/python3 -I
"""Record the announcements an application makes over AT-SPI, for
scripts/e2e/accessibility-live.sh.

    atspi_announcements.py <pid> <ready-file> [--quiet SECONDS]
                           [--timeout SECONDS]

Listens for `object:announcement` events from the application with
process id <pid> and prints one JSON line per event: seconds since the
first one, the politeness (detail1: 1 polite, 2 assertive), the source's
role, and the text. Touches <ready-file> once listening. Stops --quiet
seconds (default 3) after the last announcement, or at --timeout (default
60) with none at all, which exits 1.

Needs the system Python's GObject bindings for libatspi (python3-gi,
gir1.2-atspi-2.0).
"""

import argparse
import json
import pathlib
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("pid", type=int)
    parser.add_argument("ready")
    parser.add_argument("--quiet", type=float, default=3.0)
    parser.add_argument("--timeout", type=float, default=60.0)
    args = parser.parse_args()

    loop = GLib.MainLoop()
    seen = []
    state = {"first": None, "last": None}

    def on_event(event):
        try:
            if event.source.get_process_id() != args.pid:
                return
            role = event.source.get_role_name()
        except GLib.Error:
            role = "?"
        now = time.monotonic()
        if state["first"] is None:
            state["first"] = now
        state["last"] = now
        text = event.any_data if isinstance(event.any_data, str) else str(event.any_data)
        record = {
            "t": round(now - state["first"], 3),
            "politeness": event.detail1,
            "role": role,
            "text": text,
        }
        seen.append(record)
        print(json.dumps(record), flush=True)

    listener = Atspi.EventListener.new(on_event)
    listener.register("object:announcement")
    started = time.monotonic()

    def check():
        now = time.monotonic()
        if state["last"] is not None and now - state["last"] >= args.quiet:
            loop.quit()
            return False
        if state["last"] is None and now - started >= args.timeout:
            loop.quit()
            return False
        return True

    GLib.timeout_add(100, check)
    pathlib.Path(args.ready).touch()
    loop.run()
    listener.deregister("object:announcement")
    return 0 if seen else 1


if __name__ == "__main__":
    sys.exit(main())
