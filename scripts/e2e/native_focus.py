#!/usr/bin/python3 -I
"""Print the label of the region a native app has focused, from
`ulttyctl --json regions` on stdin, for scripts/e2e/accessibility-atspi.sh.

Prints nothing when no app has a focused region with a label.
"""

import json
import sys


def scenes(value):
    """Every app scene in the response: an object with regions and focus."""
    if isinstance(value, dict):
        if "regions" in value and "focused" in value:
            yield value
        for child in value.values():
            yield from scenes(child)
    elif isinstance(value, list):
        for child in value:
            yield from scenes(child)


def main():
    for scene in scenes(json.load(sys.stdin)):
        for region in scene["regions"]:
            if region["id"] == scene["focused"] and region.get("label"):
                print(region["label"])
                return 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
