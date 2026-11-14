"""Tests for the kernel probe registry (``FerroCAD.register_manipulator_probe``).

A workbench answers the questions the shell cannot: which value its kernel actually
completes for a handle (a fillet radius that fails, rather than a radius that could not
possibly fit), and which value an operation the user is re-opening already holds. These
tests cover the registry's routing only; the Part edition's bisection and provenance are
exercised where the kernel is.
"""

import unittest

import FerroCAD


class ProbeProviderTests(unittest.TestCase):
    def setUp(self):
        # The registry is process-wide and later suites register their own providers.
        self._saved = list(FerroCAD._PROBE_PROVIDERS)
        FerroCAD._PROBE_PROVIDERS.clear()

    def tearDown(self):
        FerroCAD._PROBE_PROVIDERS[:] = self._saved

    def test_the_first_provider_that_owns_the_element_answers(self):
        FerroCAD.register_manipulator_probe(lambda obj, element, manipulator: None)
        FerroCAD.register_manipulator_probe(
            lambda obj, element, manipulator: {
                "values": [2.5],
                "min": 0.001,
                "max": 2.5,
                "note": "test",
            }
        )
        answer = FerroCAD.manipulator_probe(None, "Edge1", "dress_up")
        self.assertEqual(answer["values"], [2.5])
        self.assertEqual(answer["min"], 0.001)
        self.assertEqual(answer["max"], 2.5)
        self.assertEqual(answer["note"], "test")

    def test_no_provider_means_no_opinion(self):
        self.assertIsNone(FerroCAD.manipulator_probe(None, "Edge1", "dress_up"))

    def test_a_raising_provider_is_skipped(self):
        def broken(*_args):
            raise RuntimeError("boom")

        FerroCAD.register_manipulator_probe(broken)
        FerroCAD.register_manipulator_probe(
            lambda obj, element, manipulator: {"min": None, "max": 1.0, "note": ""}
        )
        self.assertEqual(
            FerroCAD.manipulator_probe(None, "Edge1", "dress_up")["max"], 1.0
        )

    def test_registration_is_idempotent(self):
        def provider(*_args):
            return None

        FerroCAD.register_manipulator_probe(provider)
        FerroCAD.register_manipulator_probe(provider)
        self.assertEqual(FerroCAD._PROBE_PROVIDERS.count(provider), 1)

    def test_the_registry_is_exported(self):
        self.assertIn("register_manipulator_probe", FerroCAD.__all__)
        self.assertIn("manipulator_probe", FerroCAD.__all__)


if __name__ == "__main__":
    unittest.main()
