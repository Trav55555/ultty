#!/usr/bin/python3 -I
"""List the objects Ultty exposes over AT-SPI, as a screen reader would
walk them, for scripts/e2e/accessibility-atspi.sh.

    atspi_widgets.py <pid> [--role ROLE --name NAME]... [--focused NAME]
                     [--timeout SECONDS]

Finds the application with process id <pid> on the accessibility bus and
prints one JSON line: every object under it with its role name, name and
whether it is focused, and the focused object. Polls until an object of
each ROLE named NAME (pairs in order, e.g. `--role button --name
+1`) exists and, with --focused, until the focused object is named NAME.
Exits 1 if that does not happen within the timeout.

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


def walk(node, out, depth=0):
    if node is None or depth > 32:
        return
    out.append(
        {
            "role": node.get_role_name(),
            "name": node.get_name(),
            "focused": node.get_state_set().contains(Atspi.StateType.FOCUSED),
        }
    )
    for index in range(node.get_child_count()):
        walk(node.get_child_at_index(index), out, depth + 1)


def read_objects(pid):
    app = find_app(pid)
    if app is None:
        return None
    objects = []
    for index in range(app.get_child_count()):
        walk(app.get_child_at_index(index), objects)
    focused = [o for o in objects if o["focused"]]
    return {"objects": objects, "focused": focused[-1] if focused else None}


def satisfied(seen, wanted, focused):
    present = {(o["role"], o["name"]) for o in seen["objects"]}
    if any(pair not in present for pair in wanted):
        return False
    if focused is not None:
        return seen["focused"] is not None and seen["focused"]["name"] == focused
    return True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("pid", type=int)
    parser.add_argument("--role", action="append", default=[])
    parser.add_argument("--name", action="append", default=[])
    parser.add_argument("--focused")
    parser.add_argument("--timeout", type=float, default=15.0)
    args = parser.parse_args()
    if len(args.role) != len(args.name):
        parser.error("give --role and --name in pairs")
    wanted = list(zip(args.role, args.name))

    deadline = time.monotonic() + args.timeout
    seen = None
    while time.monotonic() < deadline:
        try:
            seen = read_objects(args.pid)
        except GLib.Error as error:
            seen = {"error": str(error)}
        if seen is not None and "objects" in seen and satisfied(seen, wanted, args.focused):
            print(json.dumps(seen))
            return 0
        time.sleep(0.2)
    print(json.dumps(seen))
    return 1


if __name__ == "__main__":
    sys.exit(main())
