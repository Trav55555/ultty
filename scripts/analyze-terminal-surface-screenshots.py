#!/usr/bin/env python3
"""Validate Ultty terminal-surface screenshots captured through `ulttyctl`."""

from __future__ import annotations

import argparse
import struct
import sys
import zlib
from pathlib import Path


TITLE_BAR_HEIGHT = 28
MAGENTA_EDGE_LIMIT = 0.001


Pixel = tuple[int, int, int, int]
Rows = list[list[Pixel]]


def parse_png(path: Path) -> tuple[int, int, Rows]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")

    offset = 8
    width = height = None
    color_type = bit_depth = None
    compressed = bytearray()

    while offset < len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        offset += 12 + length

        if kind == b"IHDR":
            width, height, bit_depth, color_type = struct.unpack(">IIBB", payload[:10])
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break

    if width is None or height is None:
        raise ValueError(f"{path} missing IHDR")
    if bit_depth != 8 or color_type != 6:
        raise ValueError(
            f"{path} must be 8-bit RGBA PNG, got bit_depth={bit_depth} color={color_type}"
        )

    raw = zlib.decompress(compressed)
    stride = width * 4
    rows: Rows = []
    previous = bytearray(stride)
    cursor = 0

    for _ in range(height):
        filter_type = raw[cursor]
        cursor += 1
        scanline = bytearray(raw[cursor : cursor + stride])
        cursor += stride
        unfilter(scanline, previous, filter_type, 4)
        rows.append([tuple(scanline[i : i + 4]) for i in range(0, stride, 4)])  # type: ignore[misc]
        previous = scanline

    return width, height, rows


def unfilter(scanline: bytearray, previous: bytearray, filter_type: int, bpp: int) -> None:
    for i, value in enumerate(scanline):
        left = scanline[i - bpp] if i >= bpp else 0
        up = previous[i]
        up_left = previous[i - bpp] if i >= bpp else 0

        if filter_type == 0:
            continue
        if filter_type == 1:
            scanline[i] = (value + left) & 0xFF
        elif filter_type == 2:
            scanline[i] = (value + up) & 0xFF
        elif filter_type == 3:
            scanline[i] = (value + ((left + up) // 2)) & 0xFF
        elif filter_type == 4:
            scanline[i] = (value + paeth(left, up, up_left)) & 0xFF
        else:
            raise ValueError(f"unsupported PNG filter {filter_type}")


def paeth(left: int, up: int, up_left: int) -> int:
    estimate = left + up - up_left
    left_distance = abs(estimate - left)
    up_distance = abs(estimate - up)
    up_left_distance = abs(estimate - up_left)
    if left_distance <= up_distance and left_distance <= up_left_distance:
        return left
    if up_distance <= up_left_distance:
        return up
    return up_left


def magenta_edge_ratio(rows: Rows) -> float:
    height = len(rows)
    width = len(rows[0])
    edge_pixels = []
    edge_pixels.extend(rows[0])
    edge_pixels.extend(rows[-1])
    edge_pixels.extend(rows[y][0] for y in range(height))
    edge_pixels.extend(rows[y][-1] for y in range(height))
    magenta = sum(1 for r, g, b, _ in edge_pixels if r > 100 and b > 100 and g < 90)
    return magenta / len(edge_pixels)


def pixel_distance(a: Pixel, b: Pixel) -> int:
    return abs(a[0] - b[0]) + abs(a[1] - b[1]) + abs(a[2] - b[2])


def terminal_background(rows: Rows) -> Pixel:
    y = min(len(rows) - 2, TITLE_BAR_HEIGHT + 40)
    x = min(len(rows[0]) - 16, int(len(rows[0]) * 0.72))
    return rows[y][x]


def foreground_pixel_count(rows: Rows, threshold: int = 45) -> int:
    bg = terminal_background(rows)
    count = 0
    for row in rows[TITLE_BAR_HEIGHT + 1 :]:
        for pixel in row:
            if pixel_distance(pixel, bg) > threshold:
                count += 1
    return count


def right_edge_activity(rows: Rows, threshold: int = 18) -> int:
    bg = terminal_background(rows)
    width = len(rows[0])
    count = 0
    for row in rows[TITLE_BAR_HEIGHT + 4 : len(rows) - 4]:
        for pixel in row[max(0, width - 10) : width]:
            if pixel_distance(pixel, bg) > threshold:
                count += 1
    return count


def validate_common(path: Path, label: str) -> list[str]:
    width, height, rows = parse_png(path)
    errors = []

    if width < 200 or height < 120:
        errors.append(f"{label} screenshot is unexpectedly small: {width}x{height}")
    if magenta_edge_ratio(rows) > MAGENTA_EDGE_LIMIT:
        errors.append(f"{label} screenshot contains magenta edge artifact")
    if foreground_pixel_count(rows) < 25:
        errors.append(f"{label} screenshot appears blank below the title band")

    return errors


# Extra right-edge pixels the scrollback capture may show over the empty
# prompt capture. The window border is in both; a scrollbar added ~4500.
RIGHT_EDGE_SLACK = 200


def validate_scrollback(path: Path, prompt_path: Path) -> list[str]:
    _, _, rows = parse_png(path)
    _, _, prompt_rows = parse_png(prompt_path)
    errors = validate_common(path, "scrollback")

    if foreground_pixel_count(rows) < 400:
        errors.append("scrollback screenshot does not contain enough terminal content")
    # Ultty has no scrollbar: history must not change the right edge.
    if right_edge_activity(rows) > right_edge_activity(prompt_rows) + RIGHT_EDGE_SLACK:
        errors.append("scrollback screenshot shows something at the right edge (a scrollbar?)")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prompt", required=True, type=Path)
    parser.add_argument("--scrollback", required=True, type=Path)
    parser.add_argument("--cursor-underline", required=True, type=Path)
    parser.add_argument("--cursor-bar", required=True, type=Path)
    parser.add_argument("--resize", required=True, type=Path)
    args = parser.parse_args()

    errors = []
    errors.extend(validate_common(args.prompt, "prompt"))
    errors.extend(validate_scrollback(args.scrollback, args.prompt))
    errors.extend(validate_common(args.cursor_underline, "cursor-underline"))
    errors.extend(validate_common(args.cursor_bar, "cursor-bar"))
    errors.extend(validate_common(args.resize, "resize"))

    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print("terminal surface screenshot analysis passed:")
    print(f"  {args.prompt}")
    print(f"  {args.scrollback}")
    print(f"  {args.cursor_underline}")
    print(f"  {args.cursor_bar}")
    print(f"  {args.resize}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
