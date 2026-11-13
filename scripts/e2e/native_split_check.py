"""Checks for scripts/e2e/native-split.sh.

    python3 native_split_check.py regions <dump.json> --cols N --rows N [--text T]...
    python3 native_split_check.py no-scenes <dump.json>
    python3 native_split_check.py screenshot <file.png> --width W --height H
        --inside X0 Y0 X1 Y1 (--min-ink N | --max-ink N)
    python3 native_split_check.py split-inline <file.png>

`regions`: the dump of `ulttyctl --json regions` holds exactly one scene
whose root is N x N cells (the pane's grid) and whose regions contain each
`--text`. `no-scenes`: the dump holds no scene. `screenshot`: the PNG is
W x H and, inside the rectangle (a pane's interior, away from its border),
the pixels that differ from the rectangle's most common colour (its
background) number at least `--min-ink`, or at most `--max-ink`.
`split-inline`: each of the two right quarters contains at least 30 magenta
card pixels. Uses the existing standard-library RGBA screenshot decoder.
Unsupported or malformed PNGs fail the check. Exits non-zero on failure.
"""

import argparse
from collections import Counter
import importlib.util
import json
from pathlib import Path
import struct
import sys
import zlib


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


def screenshot_rows(path):
    """Decode Ultty RGBA screenshots with the existing inline-surface oracle."""
    data = Path(path).read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    # Check structure and checksums before the shared decoder. In particular,
    # a truncated IEND or an unsupported interlace mode cannot look valid.
    offset = 8
    while offset + 12 <= len(data):
        length = struct.unpack(">I", data[offset:offset + 4])[0]
        end = offset + 12 + length
        if end > len(data):
            raise ValueError("truncated PNG chunk")
        kind = data[offset + 4:offset + 8]
        payload = data[offset + 8:end - 4]
        checksum = struct.unpack(">I", data[end - 4:end])[0]
        if zlib.crc32(kind + payload) != checksum:
            raise ValueError("PNG chunk checksum mismatch")
        if offset == 8 and (kind != b"IHDR" or length != 13):
            raise ValueError("missing PNG header")
        if kind == b"IHDR" and (offset != 8 or payload[10:] != b"\0\0\0"):
            raise ValueError("unsupported PNG header")
        if kind == b"IEND":
            if length != 0 or end != len(data):
                raise ValueError("invalid PNG end")
            break
        offset = end
    else:
        raise ValueError("missing PNG end")

    sys.dont_write_bytecode = True
    parser = Path(__file__).resolve().parents[1] / "analyze-inline-surface-screenshots.py"
    spec = importlib.util.spec_from_file_location("inline_surface_png", parser)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    width, height, rows = module.parse_png(Path(path))
    if width <= 0 or height <= 0 or len(rows) != height or any(len(row) != width for row in rows):
        raise ValueError("incomplete PNG pixel rows")
    return width, height, rows


def ink_pixels(rows, rect) -> int:
    """Pixels inside `rect` that are not its most common colour."""
    x0, y0, x1, y1 = rect
    colours = Counter(pixel[:3] for row in rows[y0:y1] for pixel in row[x0:x1])
    if not colours:
        return 0
    return (x1 - x0) * (y1 - y0) - max(colours.values())


def check_screenshot(args) -> list:
    try:
        width, height, rows = screenshot_rows(args.png)
    except (OSError, ValueError, IndexError, struct.error, zlib.error) as e:
        return [f"invalid screenshot: {e}"]
    failures = []
    if (width, height) != (args.width, args.height):
        failures.append(f"screenshot is {width}x{height}, expected {args.width}x{args.height}")
    rect = tuple(args.inside)
    x0, y0, x1, y1 = rect
    if not (0 <= x0 < x1 <= width and 0 <= y0 < y1 <= height):
        return failures + [f"rectangle {rect} is outside screenshot {width}x{height}"]
    ink = ink_pixels(rows, rect)
    print(f"screenshot {width}x{height}: {ink} ink pixels inside {rect}")
    if args.min_ink is not None and ink < args.min_ink:
        failures.append(f"only {ink} ink pixels inside {rect}, expected at least {args.min_ink}")
    if args.max_ink is not None and ink > args.max_ink:
        failures.append(f"{ink} ink pixels inside {rect}, expected at most {args.max_ink}")
    return failures


def check_split_inline(args) -> list:
    try:
        width, _, rows = screenshot_rows(args.png)
    except (OSError, ValueError, IndexError, struct.error, zlib.error) as error:
        return [f"invalid screenshot: {error}"]
    failures = []
    for left, right in ((width // 2, 3 * width // 4), (3 * width // 4, width)):
        magenta = sum(r > 130 and b > 100 and r > g * 1.5 and b > g * 1.4
                      for row in rows for r, g, b, _ in row[left:right])
        print(f"split inline: {magenta} card pixels in pane at x={left}")
        if magenta < 30:
            failures.append(f"only {magenta} card pixels in pane at x={left}")
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
    shot.add_argument("--inside", type=int, nargs=4, required=True, metavar=("X0", "Y0", "X1", "Y1"))
    shot.add_argument("--min-ink", type=int)
    shot.add_argument("--max-ink", type=int)
    shot.set_defaults(run=check_screenshot)
    inline = sub.add_parser("split-inline")
    inline.add_argument("png")
    inline.set_defaults(run=check_split_inline)
    args = parser.parse_args()
    if args.check == "screenshot" and (args.min_ink is None) == (args.max_ink is None):
        parser.error("screenshot takes exactly one of --min-ink and --max-ink")
    failures = args.run(args)
    for failure in failures:
        print(f"check failed: {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
