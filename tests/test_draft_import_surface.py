"""App/facade surface exercised by the upstream Draft import tests.

Three small gaps blocked or masked the Draft import group:

* ``Quantity.__eq__`` rejected a bare number, but Draft compares a length
  property against a number (``obj.Length == 12`` in
  ``drafttests/test_modification.py:197``).
* ``FreeCAD.getHomePath()`` was missing; ``drafttests/test_dxf.py:54`` joins it
  with ``Mod/Draft/drafttests/Issue24314.dxf``.
* ``TechDraw.projectToSVG`` was missing, so the SVG regression tests could not
  patch it out to force Draft's own arc fallback
  (``drafttests/test_svg.py:174``).

Run::

    PYTHONPATH=python python3 -m pytest tests/test_draft_import_surface.py -q
"""

from __future__ import annotations

import os
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

import FreeCAD


class QuantityEqualityTests(unittest.TestCase):
    def test_quantity_equals_a_bare_number(self):
        # The unit does not take part: `obj.Length == 12` means 12 in mm.
        self.assertTrue(FreeCAD.Units.Quantity("12 mm") == 12)
        self.assertFalse(FreeCAD.Units.Quantity("12 mm") == 13)
        self.assertTrue(12 == FreeCAD.Units.Quantity("12 mm"))
        self.assertFalse(FreeCAD.Units.Quantity("12 mm") != 12)

    def test_quantity_equality_is_unchanged(self):
        self.assertTrue(
            FreeCAD.Units.Quantity("12 mm") == FreeCAD.Units.Quantity("12 mm")
        )
        self.assertFalse(
            FreeCAD.Units.Quantity("12 mm") == FreeCAD.Units.Quantity("12.5 mm")
        )

    def test_unsupported_operands_are_not_equal(self):
        self.assertFalse(FreeCAD.Units.Quantity("12 mm") == "12 mm")
        self.assertIsNotNone(FreeCAD.Units.Quantity("12 mm") != "12 mm")


class HomePathTests(unittest.TestCase):
    def test_home_owns_the_mod_tree(self):
        home = FreeCAD.getHomePath()
        self.assertTrue(home.endswith(os.sep), home)
        self.assertTrue(os.path.isdir(os.path.join(home, "Mod")), home)

    def test_home_follows_a_source_checkout_workbench(self):
        # An upstream checkout puts Draft at <home>/Mod/Draft/__init__.py, which
        # is how the conformance fixture path resolves.
        saved = sys.modules.get("Draft")
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "src"
            package = home / "Mod" / "Draft"
            package.mkdir(parents=True)
            (package / "__init__.py").write_text("")
            module = types.ModuleType("Draft")
            module.__file__ = str(package / "__init__.py")
            sys.modules["Draft"] = module
            try:
                self.assertEqual(
                    FreeCAD.getHomePath(), os.path.join(str(home), "")
                )
            finally:
                if saved is None:
                    sys.modules.pop("Draft", None)
                else:
                    sys.modules["Draft"] = saved


class TechDrawProjectionTests(unittest.TestCase):
    def test_projection_is_unavailable_but_callable(self):
        import TechDraw

        # No HLR in FerroCAD: "" tells Draft's SVG exporter to generate the
        # path itself, which the exporter already handles.
        self.assertEqual(TechDraw.projectToSVG(object(), None), "")

    def test_projection_can_be_patched(self):
        # The upstream regression tests replace it to force the fallback.
        import TechDraw

        with mock.patch.object(TechDraw, "projectToSVG", return_value=""):
            self.assertEqual(TechDraw.projectToSVG(object()), "")


if __name__ == "__main__":
    unittest.main()
