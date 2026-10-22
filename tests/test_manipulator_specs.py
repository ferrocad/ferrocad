"""Tests for the declarative manipulator wire (slice G3a).

A mod declares a handle through ``FerroCAD.register_manipulator_provider``; the
shell bridge ``ferrocad_shell.manipulator_specs`` turns the concatenated list into
JSON. The Rust transport that validates every declaration lives in
``crates/ferrocad_gpui/src/manipulators.rs``.

Run: PYTHONPATH=python python3 -m unittest tests.test_manipulator_specs -v
"""

import contextlib
import io
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FerroCAD  # noqa: E402
import ferrocad_shell  # noqa: E402


MINIMAL = {
    "id": "toy.bolt",
    "shape": {"positions": [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]]},
}

OTHER = {
    "id": "toy.nut",
    "action": "nut",
    "element": "Face3",
    "axis": "x",
    "value": 4.0,
    "shape": {"positions": [[0.0, 0.0, 0.0]]},
}


class ManipulatorSpecTests(unittest.TestCase):
    def setUp(self):
        # The registry is process-global; isolate every test from the others (and
        # from any suite a workbench may have imported).
        self._saved = list(FerroCAD._MANIPULATOR_PROVIDERS)
        FerroCAD._MANIPULATOR_PROVIDERS.clear()

    def tearDown(self):
        FerroCAD._MANIPULATOR_PROVIDERS[:] = self._saved

    def test_a_registered_provider_is_returned_by_the_api_and_the_bridge(self):
        seen = []

        def provider(obj, element):
            seen.append((obj, element))
            return [MINIMAL]

        FerroCAD.register_manipulator_provider(provider)

        self.assertEqual(FerroCAD.manipulator_specs("Slab", "Face3"), [MINIMAL])
        self.assertEqual(seen, [("Slab", "Face3")])

        raw = ferrocad_shell.manipulator_specs(
            json.dumps({"object": "Slab", "element": "Face3"})
        )
        self.assertEqual(json.loads(raw), {"specs": [MINIMAL]})

    def test_none_and_empty_lists_contribute_nothing(self):
        FerroCAD.register_manipulator_provider(lambda obj, element: None)
        FerroCAD.register_manipulator_provider(lambda obj, element: [])

        self.assertEqual(FerroCAD.manipulator_specs("Slab"), [])
        self.assertEqual(
            json.loads(ferrocad_shell.manipulator_specs("{}")), {"specs": []}
        )

    def test_two_providers_concatenate_in_registration_order(self):
        FerroCAD.register_manipulator_provider(lambda obj, element: [MINIMAL])
        FerroCAD.register_manipulator_provider(lambda obj, element: [OTHER])

        specs = FerroCAD.manipulator_specs("Slab")
        self.assertEqual([spec["id"] for spec in specs], ["toy.bolt", "toy.nut"])

        raw = ferrocad_shell.manipulator_specs(json.dumps({"object": "Slab"}))
        self.assertEqual(
            [spec["id"] for spec in json.loads(raw)["specs"]],
            ["toy.bolt", "toy.nut"],
        )

    def test_the_same_callable_registers_once(self):
        def provider(obj, element):
            return [MINIMAL]

        FerroCAD.register_manipulator_provider(provider)
        FerroCAD.register_manipulator_provider(provider)

        self.assertEqual(len(FerroCAD._MANIPULATOR_PROVIDERS), 1)
        self.assertEqual(FerroCAD.manipulator_specs("Slab"), [MINIMAL])

    def test_a_raising_provider_is_reported_once_and_skipped(self):
        def explode(obj, element):
            raise RuntimeError("boom")

        FerroCAD.register_manipulator_provider(explode)
        FerroCAD.register_manipulator_provider(lambda obj, element: [MINIMAL])

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            specs = FerroCAD.manipulator_specs("Slab", "Face3")

        self.assertEqual(specs, [MINIMAL], "the surviving sibling still ran")
        report = stderr.getvalue()
        self.assertEqual(
            report.count("FerroCAD: manipulator provider explode raised: boom"), 1
        )

    def test_a_non_serialisable_spec_is_dropped_and_the_valid_ones_survive(self):
        bad = {"id": "toy.bad", "value": object()}
        FerroCAD.register_manipulator_provider(lambda obj, element: [bad, MINIMAL])

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            raw = ferrocad_shell.manipulator_specs(json.dumps({"object": "Slab"}))

        self.assertEqual(json.loads(raw), {"specs": [MINIMAL]})
        self.assertIn("non-serialisable", stderr.getvalue())

    def test_the_payload_defaults_to_an_empty_object_and_no_element(self):
        seen = []

        def provider(obj, element):
            seen.append((obj, element))
            return []

        FerroCAD.register_manipulator_provider(provider)

        self.assertEqual(
            json.loads(ferrocad_shell.manipulator_specs("{}")), {"specs": []}
        )
        self.assertEqual(seen, [("", None)])


if __name__ == "__main__":
    unittest.main()
