#!/usr/bin/env python3
"""Check that scripts/e2e/agent-transcript.sh's two cards are drawn.

The agent-transcript example opens a tool card (8 rows, background
TOOL) and, below it, a diff card (6 rows, background DIFF). Given a
screenshot with both on the screen, check that the tool card is one band
of its colour, that the band is a whole number of cell rows (8 of them),
and that the diff card spans 6 such rows below it. Its +/- lines have
backgrounds of their own, so only its first and last rows are measured.
"""

from __future__ import annotations

import argparse
import importlib.util
import sys
from pathlib import Path

TOOL = (24, 52, 96)
DIFF = (28, 72, 40)
TOOL_ROWS = 8
DIFF_ROWS = 6
TOLERANCE = 16
# A row of a card is mostly card colour; text glyphs cover some.
BAND_ROW_SHARE = 0.5


def load_png_parser():
    """parse_png from the inline-surface analyser, which has no package."""
    sys.dont_write_bytecode = True  # leave no __pycache__ in scripts/
    path = Path(__file__).with_name("analyze-inline-surface-screenshots.py")
    spec = importlib.util.spec_from_file_location("inline_surface_png", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)  # type: ignore[union-attr]
    return module.parse_png


def card_rows(rows, width: int, colour: tuple[int, int, int]) -> list[int]:
    """Pixel rows that are mostly `colour`."""

    def matches(pixel) -> bool:
        return all(abs(pixel[i] - colour[i]) <= TOLERANCE for i in range(3))

    return [
        y
        for y, row in enumerate(rows)
        if sum(1 for pixel in row if matches(pixel)) >= width * BAND_ROW_SHARE
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("screenshot", type=Path)
    args = parser.parse_args()

    width, _, rows = load_png_parser()(args.screenshot)
    tool = card_rows(rows, width, TOOL)
    if not tool:
        print("FAIL: no tool card", file=sys.stderr)
        return 1
    top, bottom = tool[0], tool[-1] + 1
    if bottom - top != len(tool):
        print("FAIL: the tool card band has gaps", file=sys.stderr)
        return 1
    cell = (bottom - top) / TOOL_ROWS
    if cell < 8 or abs(cell - round(cell)) > 0.01:
        print(f"FAIL: tool card is {bottom - top} px, not {TOOL_ROWS} rows", file=sys.stderr)
        return 1
    cell = round(cell)

    diff = card_rows(rows, width, DIFF)
    if not diff:
        print("FAIL: no diff card", file=sys.stderr)
        return 1
    if diff[0] < bottom:
        print("FAIL: the diff card is not below the tool card", file=sys.stderr)
        return 1
    span = diff[-1] + 1 - diff[0]
    if abs(span - DIFF_ROWS * cell) > 1:
        print(
            f"FAIL: diff card spans {span} px, not {DIFF_ROWS} rows of {cell} px",
            file=sys.stderr,
        )
        return 1
    print(f"cards drawn: tool at y={top} ({TOOL_ROWS} x {cell} px), diff at y={diff[0]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
