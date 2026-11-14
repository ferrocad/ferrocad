"""Tests for the kernel value-range registry (``FerroCAD.register_bounds_provider``).

A workbench answers the one question the shell cannot: which value its kernel actually
completes for a handle (a fillet radius that fails, rather than a radius that could not
possibly fit). These tests cover the registry's routing only; the Part edition's
bisection is exercised where the kernel is.
"""

import unittest

import FerroCAD


class BoundsProviderTests(unittest.TestCase):
    def setUp(self):
        # The registry is process-wide and later suites register their own providers.
        self._saved = list(FerroCAD._BOUNDS_PROVIDERS)
        FerroCAD._BOUNDS_PROVIDERS.clear()

    def tearDown(self):
        FerroCAD._BOUNDS_PROVIDERS[:] = self._saved

    def test_the_first_provider_that_owns_the_element_answers(self):
        FerroCAD.register_bounds_provider(lambda obj, element, manipulator: None)
        FerroCAD.register_bounds_provider(
            lambda obj, element, manipulator: {"min": 0.001, "max": 2.5, "note": "test"}
        )
        answer = FerroCAD.manipulator_bounds(None, "Edge1", "dress_up")
        self.assertEqual(answer["min"], 0.001)
        self.assertEqual(answer["max"], 2.5)
        self.assertEqual(answer["note"], "test")

    def test_no_provider_means_no_opinion(self):
        self.assertIsNone(FerroCAD.manipulator_bounds(None, "Edge1", "dress_up"))

    def test_a_raising_provider_is_skipped(self):
        def broken(*_args):
            raise RuntimeError("boom")

        FerroCAD.register_bounds_provider(broken)
        FerroCAD.register_bounds_provider(
            lambda obj, element, manipulator: {"min": None, "max": 1.0, "note": ""}
        )
        self.assertEqual(FerroCAD.manipulator_bounds(None, "Edge1", "dress_up")["max"], 1.0)

    def test_registration_is_idempotent(self):
        def provider(*_args):
            return None

        FerroCAD.register_bounds_provider(provider)
        FerroCAD.register_bounds_provider(provider)
        self.assertEqual(FerroCAD._BOUNDS_PROVIDERS.count(provider), 1)

    def test_the_registry_is_exported(self):
        self.assertIn("register_bounds_provider", FerroCAD.__all__)
        self.assertIn("manipulator_bounds", FerroCAD.__all__)


if __name__ == "__main__":
    unittest.main()
