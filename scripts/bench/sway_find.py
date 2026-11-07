#!/usr/bin/env python3
"""Find a window by exact title in `swaymsg -t get_tree` JSON (stdin).

    swaymsg -t get_tree | python3 -I sway_find.py TITLE id|rect

Prints the con id, or the rectangle as "x,y wxh" (grim -g syntax). Exits 1
when no window has that title.
"""

import json
import sys


def walk(node):
    yield node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child)


def main(argv):
    title, what = argv[1], argv[2]
    tree = json.load(sys.stdin)
    for node in walk(tree):
        if node.get("name") == title and node.get("type") in ("con", "floating_con"):
            if what == "id":
                print(node["id"])
            else:
                r = node["rect"]
                print(f"{r['x']},{r['y']} {r['width']}x{r['height']}")
            return 0
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
