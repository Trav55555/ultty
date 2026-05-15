#!/usr/bin/env python3
"""Validate before/after screenshots from append-content-smoke."""

from __future__ import annotations

import argparse
import struct
import sys
import zlib
from pathlib import Path


TITLE_BAR_HEIGHT = 56
MAGENTA_EDGE_LIMIT = 0.001

Pixel = tuple[int, int, int, int]
Rows = list[list[Pixel]]


def parse_png(path: Path) -> tuple[int, int, Rows]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")

    offset = 8
    width = height = None
    bit_depth = color_type = None
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
    p = left + up - up_left
    pa = abs(p - left)
    pb = abs(p - up)
    pc = abs(p - up_left)
    if pa <= pb and pa <= pc:
        return left
    if pb <= pc:
        return up
    return up_left


def is_magenta(pixel: Pixel) -> bool:
    r, g, b, _ = pixel
    return r > 180 and b > 180 and g < 80


def magenta_edge_fraction(rows: Rows) -> float:
    if not rows:
        return 0.0
    height = len(rows)
    width = len(rows[0])
    edge_pixels: list[Pixel] = []
    edge_pixels.extend(rows[0])
    edge_pixels.extend(rows[-1])
    for row in rows:
        edge_pixels.append(row[0])
        edge_pixels.append(row[width - 1])
    return sum(1 for pixel in edge_pixels if is_magenta(pixel)) / max(1, len(edge_pixels))


def changed_pixels(before: Rows, after: Rows) -> tuple[int, int]:
    total = 0
    peak_row = 0
    for y in range(TITLE_BAR_HEIGHT, len(before)):
        row_changes = sum(1 for x, pixel in enumerate(before[y]) if after[y][x] != pixel)
        total += row_changes
        peak_row = max(peak_row, row_changes)
    return total, peak_row


def validate(before_path: Path, after_path: Path) -> list[str]:
    errors: list[str] = []
    before_width, before_height, before = parse_png(before_path)
    after_width, after_height, after = parse_png(after_path)

    if (before_width, before_height) != (after_width, after_height):
        errors.append(
            f"screenshot sizes differ: before={before_width}x{before_height} "
            f"after={after_width}x{after_height}"
        )
        return errors

    if before_width < 300 or before_height < 180:
        errors.append(f"screenshots are unexpectedly small: {before_width}x{before_height}")

    for label, rows in (("before", before), ("after", after)):
        if magenta_edge_fraction(rows) > MAGENTA_EDGE_LIMIT:
            errors.append(f"{label} screenshot contains magenta edge artifact")

    total_changed, peak_changed = changed_pixels(before, after)
    if total_changed < 40:
        errors.append(
            f"after screenshot barely changed below title band: {total_changed} pixels"
        )
    if peak_changed < 8:
        errors.append(f"no row shows a focused append-text change: peak={peak_changed} pixels")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    args = parser.parse_args()

    try:
        errors = validate(args.before, args.after)
    except Exception as exc:  # noqa: BLE001
        print(f"append-content screenshot analysis failed: {exc}", file=sys.stderr)
        return 1

    if errors:
        print("append-content screenshot analysis failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print("append-content screenshot analysis passed:")
    print(f"  {args.before}")
    print(f"  {args.after}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
