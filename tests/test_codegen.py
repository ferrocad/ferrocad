"""Smoke tests for the M3c generated bindings (``ferrocad_gen``).

Verifies the generated PyO3 module exposes the same class/method/attribute
surface as the upstream ``.pyi`` stubs it was generated from. Skips cleanly when
the module isn't built or the upstream checkout isn't present.

Run (after ``./build.sh``)::

    PYTHONPATH=python python3 tests/test_codegen.py
"""

import inspect
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "tools"))

try:
    import ferrocad_gen as fc_gen  # noqa: E402
except ImportError:  # not built
    fc_gen = None

import inventory  # noqa: E402

SLICE = ("Document", "DocumentObject", "PropertyContainer")
UPSTREAM = os.path.join(os.path.dirname(__file__), "..", "..", "freecad-upstream")


def _model():
    root = os.path.abspath(UPSTREAM)
    if not os.path.isdir(os.path.join(root, "src")):
        return {}
    return {
        cls.name: cls
        for mod in inventory.discover(Path(root))
        if mod.area == "App"
        for cls in mod.classes
        if cls.name in SLICE
    }


@unittest.skipUnless(fc_gen is not None, "fc_gen extension not built")
class TestGeneratedSurface(unittest.TestCase):
    def test_classes_exposed(self):
        for name in SLICE:
            self.assertTrue(hasattr(fc_gen, name), f"missing class {name}")
            self.assertTrue(inspect.isclass(getattr(fc_gen, name)))

    def test_surface_matches_model(self):
        model = _model()
        if not model:
            self.skipTest("upstream checkout not present")
        for name in SLICE:
            cls = getattr(fc_gen, name)
            src = model[name]
            for m in src.methods:
                self.assertTrue(
                    hasattr(cls, m.name), f"{name}.{m.name} missing from generated class"
                )
            for a in src.attributes:
                self.assertTrue(
                    hasattr(cls, a.name), f"{name}.{a.name} missing from generated class"
                )

    def test_signature_defaults(self):
        # int default is preserved literally.
        sig = inspect.signature(fc_gen.PropertyContainer.getPropertyByName)
        self.assertEqual(sig.parameters["checkOwner"].default, 0)
        # str default (from `= ...`) is a real empty string.
        sig = inspect.signature(fc_gen.PropertyContainer.getPropertyStatus)
        self.assertEqual(sig.parameters["name"].default, "")


if __name__ == "__main__":
    unittest.main()
