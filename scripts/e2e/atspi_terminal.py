#!/usr/bin/python3 -I
"""Read the terminal object Ultty exposes over AT-SPI, as a screen reader
would, for scripts/e2e/accessibility-atspi.sh.

    atspi_terminal.py <pid> [--wait-for TEXT] [--timeout SECONDS]

Finds the application with process id <pid> on the accessibility bus,
then its object with role `terminal`, and prints one JSON line: its name,
its text and its caret offset. With --wait-for, polls until the text
contains TEXT. Exits 1 if nothing matching turns up within the timeout.

Needs the system Python's GObject bindings for libatspi (python3-gi,
gir1.2-atspi-2.0).
"""

import argparse
import json
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib  # noqa: E402


def find_app(pid):
    desktop = Atspi.get_desktop(0)
    for index in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(index)
        if app is not None and app.get_process_id() == pid:
            return app
    return None


def find_role(node, role, depth=0):
    if node is None or depth > 16:
        return None
    if node.get_role() == role:
        return node
    for index in range(node.get_child_count()):
        found = find_role(node.get_child_at_index(index), role, depth + 1)
        if found is not None:
            return found
    return None


def read_terminal(pid):
    app = find_app(pid)
    terminal = find_role(app, Atspi.Role.TERMINAL)
    if terminal is None:
        return None
    count = Atspi.Text.get_character_count(terminal)
    return {
        "app": app.get_name(),
        "name": terminal.get_name(),
        "role": terminal.get_role_name(),
        "text": Atspi.Text.get_text(terminal, 0, count),
        "caret": Atspi.Text.get_caret_offset(terminal),
        "focused": terminal.get_state_set().contains(Atspi.StateType.FOCUSED),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("pid", type=int)
    parser.add_argument("--wait-for")
    parser.add_argument("--timeout", type=float, default=15.0)
    args = parser.parse_args()

    deadline = time.monotonic() + args.timeout
    seen = None
    while time.monotonic() < deadline:
        try:
            seen = read_terminal(args.pid)
        except GLib.Error as error:
            seen = {"error": str(error)}
        if seen is not None and "text" in seen:
            if args.wait_for is None or args.wait_for in seen["text"]:
                print(json.dumps(seen))
                return 0
        time.sleep(0.2)
    print(json.dumps(seen))
    return 1


if __name__ == "__main__":
    sys.exit(main())
