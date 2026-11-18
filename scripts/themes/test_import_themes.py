"""Tests for scripts/import-themes.py. Run:
python3 -S -m unittest discover -s scripts/themes -p 'test_*.py'"""

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"

spec = importlib.util.spec_from_file_location("import_themes", HERE.parent / "import-themes.py")
importer = importlib.util.module_from_spec(spec)
sys.modules["import_themes"] = importer
spec.loader.exec_module(importer)


def run(selection_text, out, sources=None):
    sel = Path(out) / "selection.txt"
    sel.write_text(selection_text, encoding="utf-8")
    args = ["--upstream", str(FIXTURES), "--selection", str(sel), "--out", str(Path(out) / "out")]
    args += ["--sources", str(sources or Path(out) / "none")]
    return importer.main(args)


class ImportThemes(unittest.TestCase):
    def test_converts_a_sample_upstream_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(run("Sample Dark\n", tmp), 0)
            got = (Path(tmp) / "out" / "sample-dark.toml").read_text()
            self.assertEqual(got, (FIXTURES / "expected-sample-dark.toml").read_text())
            notice = (Path(tmp) / "out" / "THIRD_PARTY_THEMES.md").read_text()
            self.assertIn("| Sample Dark | `sample-dark` | MIT |", notice)
            self.assertIn("Copyright (c) Someone", notice)

    def test_rerun_replaces_old_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            run("Sample Dark\n", tmp)
            stale = Path(tmp) / "out" / "stale.toml"
            stale.write_text("x")
            run("Sample Dark => other-name\n", tmp)
            self.assertFalse(stale.exists())
            self.assertTrue((Path(tmp) / "out" / "other-name.toml").exists())
            self.assertFalse((Path(tmp) / "out" / "sample-dark.toml").exists())

    def test_refuses_copyleft_and_unknown_licences(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(run("Copyleft Dark\n", tmp), 1)
            self.assertEqual(run("Mystery Dark\n", tmp), 1)
            self.assertFalse((Path(tmp) / "out" / "mystery-dark.toml").exists())

    def test_a_checked_licence_override_admits_an_unknown_scheme(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(run("Mystery Dark | MIT | https://example.com/l\n", tmp), 0)
            text = (Path(tmp) / "out" / "mystery-dark.toml").read_text()
            self.assertIn("Theme licence: MIT. Source: https://example.com/l", text)

    def test_missing_scheme_and_bad_colours_are_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(run("No Such Theme\n", tmp), 1)
        with self.assertRaises(importer.SchemeError):
            importer.parse_ghostty("palette = 0=#12345\n", "bad")
        with self.assertRaises(importer.SchemeError):
            importer.parse_ghostty("background = #000000\n", "short")

    def test_license_expressions(self):
        self.assertTrue(importer.license_allowed("MIT OR Apache-2.0"))
        self.assertFalse(importer.license_allowed("GPL-3.0-or-later"))
        self.assertFalse(importer.license_allowed("unknown"))

    def test_local_sources_carry_their_licence_in_headers(self):
        with tempfile.TemporaryDirectory() as tmp:
            sources = Path(tmp) / "src"
            sources.mkdir()
            body = (FIXTURES / "ghostty" / "Sample Dark").read_text()
            (sources / "Local One").write_text("# license: MIT\n# source: https://example.com/x\n" + body)
            self.assertEqual(run("", tmp, sources), 0)
            self.assertTrue((Path(tmp) / "out" / "local-one.toml").exists())
            (sources / "Local One").write_text(body)
            self.assertEqual(run("", tmp, sources), 1)


if __name__ == "__main__":
    unittest.main()
