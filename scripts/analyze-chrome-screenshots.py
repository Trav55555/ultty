#!/usr/bin/env python3
"""Validate Ultty chrome screenshots captured through `ulttyctl screenshot`."""

from __future__ import annotations

import argparse
import statistics
import struct
import sys
import zlib
from pathlib import Path


# Chrome geometry in pixels; keep in step with TITLE_BAR_HEIGHT and
# TAB_ROW_HEIGHT in crates/ultty/src/tabs.rs.
TITLE_BAR_HEIGHT = 44
TAB_ROW_HEIGHT = 28


def parse_png(path: Path) -> tuple[int, int, list[list[tuple[int, int, int, int]]]]:
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
        raise ValueError(f"{path} must be 8-bit RGBA PNG, got bit_depth={bit_depth} color={color_type}")

    raw = zlib.decompress(compressed)
    stride = width * 4
    rows: list[list[tuple[int, int, int, int]]] = []
    previous = bytearray(stride)
    cursor = 0

    for _ in range(height):
        filter_type = raw[cursor]
        cursor += 1
        scanline = bytearray(raw[cursor : cursor + stride])
        cursor += stride
        unfilter(scanline, previous, filter_type, 4)
        rows.append(
            [
                tuple(scanline[i : i + 4])  # type: ignore[misc]
                for i in range(0, stride, 4)
            ]
        )
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


def row_color(rows: list[list[tuple[int, int, int, int]]], y: int) -> tuple[float, float, float]:
    """The background color of row y, sampled across its central band.

    Takes the per-channel median, so text that crosses the band (a long
    shell prompt on the first terminal line) does not shift the result.
    """
    row = rows[min(y, len(rows) - 1)]
    # Sample the central trailing band, away from tab labels, the new-tab
    # button, and right-side window controls.
    start = max(4, int(len(row) * 0.42))
    end = min(len(row) - 4, int(len(row) * 0.78))
    sample = row[start:max(start + 1, end)]
    return tuple(
        float(statistics.median(px[channel] for px in sample)) for channel in range(3)
    )


def distance(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return sum(abs(x - y) for x, y in zip(a, b))


def magenta_edge_ratio(rows: list[list[tuple[int, int, int, int]]]) -> float:
    height = len(rows)
    width = len(rows[0])
    edge_pixels = []
    edge_pixels.extend(rows[0])
    edge_pixels.extend(rows[-1])
    edge_pixels.extend(rows[y][0] for y in range(height))
    edge_pixels.extend(rows[y][-1] for y in range(height))
    magenta = sum(1 for r, g, b, _ in edge_pixels if r > 100 and b > 100 and g < 90)
    return magenta / len(edge_pixels)


def validate_single_tab(path: Path) -> list[str]:
    _, height, rows = parse_png(path)
    title = row_color(rows, TITLE_BAR_HEIGHT // 2)
    lower = row_color(rows, TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT // 2)
    terminal = row_color(rows, min(height - 2, TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT + 24))
    errors = []

    if distance(lower, terminal) > 12:
        errors.append("single-tab screenshot appears to contain a lower tab rail")
    if distance(title, terminal) < 2:
        errors.append("single-tab screenshot does not show a distinct title/control band")
    if magenta_edge_ratio(rows) > 0.001:
        errors.append("single-tab screenshot contains magenta edge artifact")

    return errors


def validate_tab_rail(path: Path, label: str) -> list[str]:
    _, height, rows = parse_png(path)
    title = row_color(rows, TITLE_BAR_HEIGHT // 2)
    rail = row_color(rows, TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT // 2)
    terminal = row_color(rows, min(height - 2, TITLE_BAR_HEIGHT + TAB_ROW_HEIGHT + 24))
    errors = []

    if distance(rail, terminal) < 6:
        errors.append(f"{label} screenshot does not show a lower tab rail")
    if distance(title, rail) < 2:
        errors.append(f"{label} title/control band and tab rail are not visually distinct")
    if magenta_edge_ratio(rows) > 0.001:
        errors.append(f"{label} screenshot contains magenta edge artifact")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--single", required=True, type=Path)
    parser.add_argument("--multi", required=True, type=Path)
    parser.add_argument("--dense", required=True, type=Path)
    args = parser.parse_args()

    errors = []
    errors.extend(f"{args.single}: {error}" for error in validate_single_tab(args.single))
    errors.extend(f"{args.multi}: {error}" for error in validate_tab_rail(args.multi, "multi-tab"))
    errors.extend(f"{args.dense}: {error}" for error in validate_tab_rail(args.dense, "dense-tab"))

    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print("chrome screenshot analysis passed:")
    print(f"  {args.single}")
    print(f"  {args.multi}")
    print(f"  {args.dense}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
