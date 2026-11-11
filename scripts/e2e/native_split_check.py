"""Checks for scripts/e2e/native-split.sh.

    python3 native_split_check.py regions <dump.json> --cols N --rows N [--text T]...
    python3 native_split_check.py no-scenes <dump.json>
    python3 native_split_check.py screenshot <file.png> --width W --height H --right-of X

`regions`: the dump of `ulttyctl --json regions` holds exactly one scene
whose root is N x N cells (the pane's grid) and whose regions contain each
`--text`. `no-scenes`: the dump holds no scene. `screenshot`: the PNG is
W x H and, with Pillow installed, the part right of column X is not all one
colour (the app drew there). Exits non-zero when a check fails.
"""

import argparse
import json
import struct
import sys


def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError) as e:
        print(f"no dump: {e}")
        sys.exit(1)


def check_regions(args) -> list:
    scenes = load(args.dump).get("scenes", [])
    if len(scenes) != 1:
        return [f"{len(scenes)} scenes, expected 1"]
    regions = scenes[0]["regions"]
    root = next((r for r in regions if r["parent"] is None), None)
    failures = []
    if root is None:
        failures.append("no root region")
    elif (root["width"], root["height"]) != (args.cols, args.rows):
        failures.append(
            f"root is {root['width']}x{root['height']} cells, expected {args.cols}x{args.rows}"
        )
    texts = [r.get("text") or "" for r in regions]
    for wanted in args.text:
        if not any(wanted == t.strip() or wanted in t for t in texts):
            failures.append(f"no region text contains {wanted!r} (texts: {texts})")
    return failures


def check_no_scenes(args) -> list:
    scenes = load(args.dump).get("scenes", [])
    return [f"{len(scenes)} scenes, expected none"] if scenes else []


def check_screenshot(args) -> list:
    try:
        with open(args.png, "rb") as f:
            head = f.read(33)
    except OSError as e:
        return [f"no screenshot: {e}"]
    if head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        return ["not a PNG"]
    width, height = struct.unpack(">II", head[16:24])
    failures = []
    if (width, height) != (args.width, args.height):
        failures.append(f"screenshot is {width}x{height}, expected {args.width}x{args.height}")
    try:
        from PIL import Image
    except ImportError:
        print("Pillow not installed; pixel check skipped")
        return failures
    image = Image.open(args.png).convert("RGB")
    right = image.crop((args.right_of, 0, image.width, image.height))
    colours = right.getcolors(maxcolors=1 << 20) or []
    if len(colours) < 2:
        failures.append(f"nothing drawn right of x={args.right_of}: one colour")
    print(f"screenshot {width}x{height}, {len(colours)} colours right of x={args.right_of}")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="check", required=True)
    regions = sub.add_parser("regions")
    regions.add_argument("dump")
    regions.add_argument("--cols", type=float, required=True)
    regions.add_argument("--rows", type=float, required=True)
    regions.add_argument("--text", action="append", default=[])
    regions.set_defaults(run=check_regions)
    none = sub.add_parser("no-scenes")
    none.add_argument("dump")
    none.set_defaults(run=check_no_scenes)
    shot = sub.add_parser("screenshot")
    shot.add_argument("png")
    shot.add_argument("--width", type=int, required=True)
    shot.add_argument("--height", type=int, required=True)
    shot.add_argument("--right-of", type=int, required=True)
    shot.set_defaults(run=check_screenshot)
    args = parser.parse_args()
    failures = args.run(args)
    for failure in failures:
        print(f"check failed: {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
