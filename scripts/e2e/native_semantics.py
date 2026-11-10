#!/usr/bin/python3 -I
"""Check the semantics `ulttyctl --json regions` reports for a native
app's regions (ADR 0021), for scripts/e2e/accessibility-chat.sh.

    ulttyctl --json regions | native_semantics.py FIELD=VALUE[,FIELD=VALUE…]…

Each argument describes one region by the fields it must have (role,
label, live); every one must match some region of some scene. Prints the
regions' semantics, one per line, and exits 1 naming what is missing.
"""

import json
import sys


def regions(value):
    """Every region in the response: an object with an id and a role."""
    if isinstance(value, dict):
        if "id" in value and "role" in value:
            yield value
        for child in value.values():
            yield from regions(child)
    elif isinstance(value, list):
        for child in value:
            yield from regions(child)


def main():
    found = list(regions(json.load(sys.stdin)))
    for region in found:
        print(
            f"{region['id']}: {region['role']} label={region.get('label')!r} "
            f"live={region.get('live')}"
        )
    missing = []
    for want in sys.argv[1:]:
        fields = dict(part.split("=", 1) for part in want.split(","))
        if not any(all(str(r.get(k)) == v for k, v in fields.items()) for r in found):
            missing.append(want)
    for want in missing:
        print(f"MISSING: a region with {want}")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
