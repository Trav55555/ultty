"""Check a `ulttyctl --json regions` dump of a ulttui-lua app.

    python3 scripts/e2e/lua_regions.py <dump.json> [--list ID --rows N]
        [--id ID]... [--text TEXT]...

--list ID --rows N: the region with metadata id ID has exactly N child
regions, each a one-line text row. --id ID: some region has metadata id ID.
--text TEXT: some region's text contains TEXT. Prints what it found and
exits non-zero when a check fails.
"""

import argparse
import json
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("dump")
    parser.add_argument("--list")
    parser.add_argument("--rows", type=int)
    parser.add_argument("--id", action="append", default=[])
    parser.add_argument("--text", action="append", default=[])
    args = parser.parse_args()

    try:
        with open(args.dump) as f:
            reply = json.load(f)
    except (OSError, ValueError) as e:
        print(f"no regions: {e}")
        return 1
    regions = [r for scene in reply.get("scenes", []) for r in scene["regions"]]
    by_meta = {r.get("metadata", {}).get("id"): r for r in regions}
    texts = [r.get("text") or "" for r in regions]
    failures = []

    if args.list:
        lst = by_meta.get(args.list)
        if lst is None:
            failures.append(f"no region with id {args.list!r}")
        else:
            rows = [r for r in regions if r["parent"] == lst["id"]]
            for r in rows:
                print(f"row: {r.get('text')!r} ({r['content']}, {r['width']}x{r['height']})")
            if len(rows) != args.rows:
                failures.append(f"list {args.list!r} has {len(rows)} rows, expected {args.rows}")
            if any(r["content"] != "text" or not r.get("text") for r in rows):
                failures.append(f"list {args.list!r} has rows without text")
    for wanted in args.id:
        if wanted not in by_meta:
            failures.append(f"no region with id {wanted!r}")
    for wanted in args.text:
        if not any(wanted in t for t in texts):
            failures.append(f"no region text contains {wanted!r}")

    for failure in failures:
        print(f"check failed: {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
