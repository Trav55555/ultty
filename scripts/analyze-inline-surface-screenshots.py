#!/usr/bin/env python3
"""Check where scripts/e2e/inline-surface.sh's card was drawn.

The card is an opaque magenta surface ROWS rows high. Given screenshots
taken with it on the screen (placed), after output scrolled it off (off),
after scrolling back to the top of scrollback (top) and after scrolling down
a little from there (shifted), check that it is a band ROWS cells high,
absent when off the screen, back at the same place at the top (where its
anchor row is shown again), and whole rows higher, the same size, when
scrolled down from there: it moves line for line with its text.
"""

from __future__ import annotations

import argparse
import struct
import sys
import zlib
from pathlib import Path

CARD = (200, 40, 160)
TOLERANCE = 16
# A row of the band is mostly card colour; the label's glyphs cover some.
BAND_ROW_SHARE = 0.5
# Fewer card-coloured pixels than this in a whole screenshot is no card.
ABSENT_SHARE = 0.0005

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


def is_card(pixel: Pixel) -> bool:
    return all(abs(pixel[i] - CARD[i]) <= TOLERANCE for i in range(3))


def card_counts(path: Path) -> tuple[int, list[int]]:
    width, _, rows = parse_png(path)
    return width, [sum(1 for pixel in row if is_card(pixel)) for row in rows]


def band(path: Path) -> tuple[int, int]:
    """The first and one-past-last pixel row of the card's band."""
    width, counts = card_counts(path)
    rows = [y for y, count in enumerate(counts) if count >= width * BAND_ROW_SHARE]
    if not rows:
        raise SystemExit(f"FAIL: {path.name}: no card band")
    top, bottom = rows[0], rows[-1] + 1
    if bottom - top != len(rows):
        raise SystemExit(f"FAIL: {path.name}: the card band has gaps")
    return top, bottom


def near(a: tuple[int, int], b: tuple[int, int]) -> bool:
    """Equal but for a pixel of rounding at either edge."""
    return abs(a[0] - b[0]) <= 1 and abs(a[1] - b[1]) <= 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("placed", type=Path)
    parser.add_argument("off", type=Path)
    parser.add_argument("top", type=Path)
    parser.add_argument("shifted", type=Path)
    parser.add_argument("--rows", type=int, required=True)
    args = parser.parse_args()

    placed = band(args.placed)
    height = placed[1] - placed[0]
    cell = height / args.rows
    if cell < 6:
        raise SystemExit(f"FAIL: the card is {height} px high, too low for {args.rows} rows")
    print(f"placed: rows {placed[0]}..{placed[1]} ({args.rows} rows of {cell:.0f} px)")

    width, counts = card_counts(args.off)
    if sum(counts) > width * len(counts) * ABSENT_SHARE:
        raise SystemExit(f"FAIL: {args.off.name}: the card is drawn off the screen")
    print("off: no card")

    top = band(args.top)
    if not near(top, placed):
        raise SystemExit(f"FAIL: at the top of scrollback the card is at {top}, not {placed}")
    print(f"top: rows {top[0]}..{top[1]}")

    shifted = band(args.shifted)
    lines = (placed[0] - shifted[0]) / cell
    expected = (placed[0] - round(round(lines) * cell), placed[1] - round(round(lines) * cell))
    if round(lines) < 1 or not near(shifted, expected):
        raise SystemExit(
            f"FAIL: scrolled down, the card is at {shifted}: not whole rows above {placed}"
        )
    print(f"shifted: rows {shifted[0]}..{shifted[1]}, {round(lines)} lines up")
    return 0


if __name__ == "__main__":
    sys.exit(main())
