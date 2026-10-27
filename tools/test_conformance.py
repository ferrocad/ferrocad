"""Tests for ``tools/conformance.py`` (M3d harness helpers).

Hermetic: exercises the result-classification helpers without needing the
upstream checkout or a built extension.

Run::

    python3 tools/test_conformance.py
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))

import conformance  # noqa: E402


class TestClassifyLoadError(unittest.TestCase):
    def test_from_import(self):
        err = ImportError("cannot import name 'Base' from 'FreeCAD'")
        self.assertEqual(conformance.classify_load_error(err), "missing 'FreeCAD.Base'")

    def test_from_import_nested(self):
        err = ImportError("cannot import name 'Quantity' from 'FreeCAD.Units'")
        self.assertEqual(conformance.classify_load_error(err), "missing 'FreeCAD.Units.Quantity'")

    def test_fallback(self):
        self.assertEqual(conformance.classify_load_error(RuntimeError("boom")), "RuntimeError: boom")


class TestShort(unittest.TestCase):
    def test_short_exception(self):
        try:
            raise ValueError("nope")
        except ValueError:
            err = sys.exc_info()
        text = conformance._short(err)
        self.assertIn("ValueError", text)
        self.assertIn("nope", text)


class TestTotals(unittest.TestCase):
    def test_totals(self):
        records = [("a", "pass", ""), ("b", "fail", "x"), ("c", "error", "y"), ("d", "skip", "z")]
        self.assertEqual(
            conformance._totals(records),
            {"pass": 1, "fail": 1, "error": 1, "skip": 1},
        )


class TestDraftGroup(unittest.TestCase):
    def test_file_list(self):
        # The group grows only when a module runs green against this image; the
        # list is asserted so a silent addition or removal is caught in review.
        # `test_modification` is deliberately absent (it still needs `Sketcher`
        # and TechDraw); the GUI-adjacent files are absent by design.
        self.assertEqual(
            conformance.DRAFT_FILES,
            [
                "test_base",
                "test_grid_settings",
                "test_pivy",
                "test_airfoildat",
                "test_import",
                "test_creation",
                "test_array",
                "test_dwg",
                "test_oca",
                "test_svg",
            ],
        )

    def test_paths_cover_the_package_root_and_ext(self):
        self.assertIn("src/Mod/Draft", conformance.DRAFT_DIRS)
        self.assertIn("src/Mod/Draft/drafttests", conformance.DRAFT_DIRS)
        self.assertIn("src/Ext", conformance.DRAFT_DIRS)

    def test_unknown_revision_is_reported_not_raised(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(conformance.upstream_revision(Path(tmp)), "unknown")

    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_revision_of_a_git_checkout(self):
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["git", "-C", tmp, "init", "-q"], check=True)
            subprocess.run(
                [
                    "git", "-C", tmp,
                    "-c", "user.name=D0", "-c", "user.email=d0@example.invalid",
                    "commit", "--allow-empty", "-q", "-m", "init",
                ],
                check=True,
            )
            revision = conformance.upstream_revision(Path(tmp))
        self.assertRegex(revision, r"^[0-9a-f]{40}$")

    def test_render_reports_the_revision(self):
        text = conformance.render([], root=Path("/nonexistent-upstream"))
        self.assertIn("upstream: /nonexistent-upstream @ unknown", text)
        self.assertIn("summary: 0 passed", text)

    def test_render_without_a_root_keeps_the_old_header(self):
        self.assertNotIn("upstream:", conformance.render([]))


if __name__ == "__main__":
    unittest.main()
