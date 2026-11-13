"""CLI screenshot regressions using Python's standard library only."""

import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib


CHECK = Path(__file__).with_name("native_split_check.py")


def chunk(kind, value):
    return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value))


def png(path, first=0, second=0, shell=0):
    """Opaque RGBA screenshot with independently populated pane rectangles."""
    width, height = 100, 20
    rows = [[(0, 0, 0, 255)] * width for _ in range(height)]
    for left, count, span in [(0, shell, 50), (50, first, 25), (75, second, 25)]:
        for index in range(count):
            rows[index // span][left + index % span] = (200, 40, 160, 255)

    raw = b"".join(b"\0" + bytes(component for pixel in row for component in pixel) for row in rows)
    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


class ScreenshotCheckTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.image = Path(self.directory.name) / "capture.png"

    def run_check(self, *arguments):
        # -S reproduces the runner's missing site-packages without relying on
        # whether the developer's Python has Pillow installed.
        return subprocess.run([sys.executable, "-S", str(CHECK), *map(str, arguments)],
                              capture_output=True, text=True, timeout=5)

    def test_both_split_cards_pass_at_the_exact_pixel_threshold(self):
        png(self.image, first=30, second=30)
        result = self.run_check("split-inline", self.image)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_or_subthreshold_sibling_never_borrows_other_pane_pixels(self):
        for first, second, shell in [(29, 30, 0), (30, 29, 0), (60, 0, 0), (0, 0, 200)]:
            with self.subTest(first=first, second=second, shell=shell):
                png(self.image, first=first, second=second, shell=shell)
                result = self.run_check("split-inline", self.image)
                self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_native_ink_minimum_maximum_and_dimensions_without_pillow(self):
        png(self.image, shell=30)
        arguments = ["screenshot", self.image, "--width", 100, "--height", 20,
                     "--inside", 0, 0, 50, 20]
        for option, threshold, expected in [("--min-ink", 30, 0), ("--min-ink", 31, 1),
                                            ("--max-ink", 30, 0), ("--max-ink", 29, 1)]:
            with self.subTest(option=option, threshold=threshold):
                result = self.run_check(*arguments, option, threshold)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        arguments[3] = 101
        self.assertNotEqual(self.run_check(*arguments, "--min-ink", 1).returncode, 0)

    def test_malformed_and_unsupported_pngs_fail_closed(self):
        png(self.image, first=30, second=30)
        original = self.image.read_bytes()
        header = chunk(b"IHDR", struct.pack(">IIBBBBB", 100, 20, 8, 6, 0, 0, 0))
        prefix = b"\x89PNG\r\n\x1a\n" + header
        bad_scanlines = [prefix + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
                         for raw in [b"\0" * (20 * 401 - 1), b"\0" * (20 * 401 + 1),
                                     b"\x05" + b"\0" * (20 * 401 - 1)]]
        for data in [b"not a png", original[:45], original[:-12],
                     original[:24] + b"\x10" + original[25:], *bad_scanlines]:
            with self.subTest(bytes=len(data)):
                self.image.write_bytes(data)
                for command in [("split-inline", self.image),
                                ("screenshot", self.image, "--width", 100, "--height", 20,
                                 "--inside", 0, 0, 100, 20, "--min-ink", 1)]:
                    self.assertNotEqual(self.run_check(*command).returncode, 0)

    def test_region_and_no_scene_checks_still_run_without_image_packages(self):
        dump = Path(self.directory.name) / "regions.json"
        dump.write_text(json.dumps({"scenes": [{"regions": [
            {"id": 0, "parent": None, "width": 10, "height": 5, "text": "Counter"}]}]}))
        result = self.run_check("regions", dump, "--cols", 10, "--rows", 5, "--text", "Counter")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(self.run_check("no-scenes", dump).returncode, 0)
        dump.write_text(json.dumps({"scenes": []}))
        self.assertEqual(self.run_check("no-scenes", dump).returncode, 0)


if __name__ == "__main__":
    unittest.main()
