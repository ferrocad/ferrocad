"""Tests for ``tools/conformance.py`` (M3d harness helpers).

Hermetic: exercises the result-classification helpers without needing the
upstream checkout or a built extension.

Run::

    python3 tools/test_conformance.py
"""

import os
import sys
import unittest

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


if __name__ == "__main__":
    unittest.main()
