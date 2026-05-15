#!/usr/bin/env python3
"""Validate the hello-native screenshot captured by native-richtext-smoke."""

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


def average_region(rows: Rows, x0: int, y0: int, x1: int, y1: int) -> tuple[float, float, float]:
    samples = [
        pixel
        for row in rows[max(0, y0) : min(len(rows), y1)]
        for pixel in row[max(0, x0) : min(len(row), x1)]
    ]
    if not samples:
        return (0.0, 0.0, 0.0)
    return (
        sum(px[0] for px in samples) / len(samples),
        sum(px[1] for px in samples) / len(samples),
        sum(px[2] for px in samples) / len(samples),
    )


def distance(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return sum(abs(x - y) for x, y in zip(a, b))


def luminance(rgb: tuple[float, float, float]) -> float:
    return 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]


def foreground_count(rows: Rows, bg: tuple[float, float, float], threshold: float = 70.0) -> int:
    count = 0
    for row in rows[TITLE_BAR_HEIGHT + 1 :]:
        for r, g, b, _ in row:
            if distance((r, g, b), bg) > threshold:
                count += 1
    return count


def validate(path: Path) -> list[str]:
    width, height, rows = parse_png(path)
    errors = []

    if width < 400 or height < 300:
        errors.append(f"native screenshot is unexpectedly small: {width}x{height}")
    if magenta_edge_ratio(rows) > MAGENTA_EDGE_LIMIT:
        errors.append("native screenshot contains magenta edge artifact")

    header = average_region(rows, 24, TITLE_BAR_HEIGHT + 8, width - 24, TITLE_BAR_HEIGHT + 48)
    spinner = average_region(rows, 24, TITLE_BAR_HEIGHT + 96, width - 24, TITLE_BAR_HEIGHT + 128)
    body = average_region(rows, int(width * 0.45), int(height * 0.55), int(width * 0.82), int(height * 0.82))

    if distance(header, body) < 35:
        errors.append("native screenshot does not show a distinct header band")
    if distance(spinner, body) < 20:
        errors.append("native screenshot does not show a distinct widget/status band")
    if luminance(body) > 130:
        errors.append("native body background is too bright for reliable text contrast")
    if foreground_count(rows, body) < 350:
        errors.append("native screenshot does not contain enough visible text/widget content")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("image", type=Path)
    args = parser.parse_args()

    errors = validate(args.image)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print("native rich text screenshot analysis passed:")
    print(f"  {args.image}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
