"""Tests for the persisted viewport GPU adapter setting.

The Dev Tools panel writes
``User parameter:BaseApp/Preferences/View/GpuAdapter`` through
``ferrocad_shell.set_gpu_adapter``; the shell reads it with ``gpu_adapter`` when
it creates the viewport. The grammar is the Rust ``AdapterChoice`` one:
``auto``, ``backend:vulkan|gl|metal|dx12`` or ``adapter:<name substring>``.

A malformed payload or value is a result, never an exception: the shell shows
the error in the panel instead of losing the interpreter.
"""

import json
import unittest

import FreeCAD
import ferrocad_shell

PREFERENCES = "User parameter:BaseApp/Preferences/View"
SETTING = "GpuAdapter"


class GpuAdapterPreferenceTests(unittest.TestCase):
    def setUp(self):
        self.prefs = FreeCAD.ParamGet(PREFERENCES)
        self._previous = self.prefs.GetString(SETTING, "auto")
        self.prefs.RemString(SETTING)

    def tearDown(self):
        self.prefs.RemString(SETTING)
        if self._previous != "auto":
            self.prefs.SetString(SETTING, self._previous)

    def store(self, value):
        return json.loads(ferrocad_shell.set_gpu_adapter(json.dumps({"value": value})))

    def test_the_default_is_auto(self):
        self.assertEqual(ferrocad_shell.gpu_adapter(), "auto")

    def test_a_choice_round_trips_through_the_parameter_store(self):
        outcome = self.store(" adapter:Llvm ")
        self.assertTrue(outcome["ok"], outcome)
        self.assertEqual(outcome["error"], "")
        self.assertEqual(outcome["value"], "adapter:Llvm")
        self.assertEqual(ferrocad_shell.gpu_adapter(), "adapter:Llvm")

    def test_every_accepted_form_is_stored(self):
        for value in (
            "auto",
            "backend:vulkan",
            "BACKEND:GL",
            "backend:metal",
            "backend:dx12",
            "adapter: llvmpipe ",
        ):
            outcome = self.store(value)
            self.assertTrue(outcome["ok"], (value, outcome))
            self.assertEqual(outcome["value"], value.strip())
            self.assertEqual(ferrocad_shell.gpu_adapter(), value.strip())

    def test_a_bad_value_names_the_accepted_forms(self):
        for value in ("", "gpu", "adapter:", "backend:", "backend:swiftshader"):
            outcome = self.store(value)
            self.assertFalse(outcome["ok"], value)
            self.assertEqual(outcome["value"], "")
            self.assertIn("auto", outcome["error"])
            self.assertIn("backend:", outcome["error"])
            self.assertIn("adapter:", outcome["error"])
            self.assertEqual(ferrocad_shell.gpu_adapter(), "auto")

    def test_a_bad_payload_never_raises(self):
        for payload in ("", "not json", "[]", "{}", '{"value": 7}'):
            outcome = json.loads(ferrocad_shell.set_gpu_adapter(payload))
            self.assertFalse(outcome["ok"], payload)
            self.assertEqual(outcome["value"], "")
            self.assertTrue(outcome["error"])


if __name__ == "__main__":
    unittest.main()
