#!/usr/bin/env python3
"""The measuring helpers of b1-agent-console.py, without a display.

    /usr/bin/python3 -I scripts/bench/test_b1_agent_console.py

Copy fidelity, the copy rows and the scroll offset are each judged by one
small function; these tests pin that each one reports the faults it is
there to find, so a clean result from the harness means a clean copy.
"""
import importlib.util
import os
import sys
import unittest

from PIL import Image, ImageDraw

sys.dont_write_bytecode = True  # loading the harness would leave a __pycache__ behind
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("b1", os.path.join(HERE, "b1-agent-console.py"))
b1 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b1)


class CopyErrors(unittest.TestCase):
    SOURCE = ["```rust", "fn main() {", "    let x = 1;", "}", "```"]

    def test_an_exact_copy_has_no_errors(self):
        got = b1.copy_errors("\n".join(self.SOURCE) + "\n", self.SOURCE)
        self.assertEqual((got["chars_wrong"], got["trailing_space_lines"],
                          got["indent_not_in_source"], got["breaks_not_in_source"]), (0, 0, 0, 0))

    def test_trailing_spaces_are_counted_per_line(self):
        copied = "```rust  \nfn main() {\n    let x = 1;   \n}\n```"
        got = b1.copy_errors(copied, self.SOURCE)
        self.assertEqual(got["trailing_space_lines"], 2)
        self.assertEqual(got["chars_wrong"], 5)

    def test_indentation_not_in_the_source_is_counted(self):
        copied = "```rust\n  fn main() {\n    let x = 1;\n}\n```"
        self.assertEqual(b1.copy_errors(copied, self.SOURCE)["indent_not_in_source"], 1)

    def test_a_wrapped_line_broken_in_two_is_an_extra_break(self):
        source = ["a long paragraph that the terminal wrapped"]
        got = b1.copy_errors("a long paragraph that\nthe terminal wrapped", source)
        self.assertEqual(got["breaks_not_in_source"], 1)
        self.assertEqual(got["chars_wrong"], 1)


class Rows(unittest.TestCase):
    def test_wide_characters_take_two_columns_and_wrap_whole(self):
        self.assertEqual(b1.wrapped_rows("abcd", 4), 1)
        self.assertEqual(b1.wrapped_rows("abcde", 4), 2)
        # The wide character does not fit in the last column, so it wraps.
        self.assertEqual(b1.wrapped_rows("abc漢", 4), 2)
        self.assertEqual(b1.wrapped_rows("", 4), 1)

    def test_locate_finds_a_block_by_its_first_and_last_lines(self):
        lines = ["> prompt", "", "```rust", "fn main() {", "}", "```", "", "after"]
        self.assertEqual(b1.locate(lines, ["```rust", "fn main() {", "}", "```"], 0), (2, 5))
        self.assertIsNone(b1.locate(lines, ["missing"], 0))


class SourceBlocks(unittest.TestCase):
    def test_prose_splits_at_blank_lines_and_fences_stay_whole(self):
        path = os.path.join(os.path.dirname(HERE), "..", "examples", "agent-console",
                            "fixtures", "short.jsonl")
        blocks = b1.source_blocks(path)
        kinds = [k for k, _ in blocks]
        self.assertEqual(kinds.count("code"), 1)
        self.assertEqual(kinds.count("diff"), 1)
        code = next(b for k, b in blocks if k == "code")
        self.assertEqual((code[0], code[-1]), ("```rust", "```"))
        # The diff sits where it happened, between the prose around it.
        self.assertEqual(kinds.index("diff"), kinds.index("code") + 5)


def screen(lines, height=60, width=80):
    """A fake screenshot: each text line drawn in its own 10-pixel row."""
    img = Image.new("RGB", (width, height), (0, 0, 0))
    draw = ImageDraw.Draw(img)
    for i, text in enumerate(lines):
        draw.text((2, i * 10), text, fill=(255, 255, 255))
    return img


class ViewShift(unittest.TestCase):
    LINES = ["a1", "b22", "c333", "d4444", "e55555", "f666666"]

    def test_a_held_view_does_not_shift(self):
        self.assertEqual(b1.view_shift(screen(self.LINES), screen(self.LINES), 30), 0)

    def test_content_that_moved_up_one_row_shifts_minus_ten_pixels(self):
        later = self.LINES[1:] + ["g7"]
        self.assertEqual(b1.view_shift(screen(self.LINES), screen(later), 30), -10)

    def test_content_that_moved_down_two_rows_shifts_twenty_pixels(self):
        later = ["y8", "z99"] + self.LINES[:4]
        self.assertEqual(b1.view_shift(screen(self.LINES), screen(later), 30), 20)

    def test_content_that_left_the_screen_is_none(self):
        other = ["x" * n for n in range(1, 7)]
        self.assertIsNone(b1.view_shift(screen(self.LINES), screen(other), 30))


if __name__ == "__main__":
    unittest.main()
