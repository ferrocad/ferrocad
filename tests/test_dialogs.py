"""Tests for the non-blocking dialog protocol (A3).

A command may return a dialog spec; the facade keeps ``on_accept`` under a
single-use token and the shell accepts or cancels it later. These tests drive the
protocol directly, without the gpui shell. See ``docs/developer/workbenches/actions-and-tools.md`` §6.
"""

import json
import unittest

import FreeCADGui

#: Values each accepted callback saw, so a test can prove it ran (once).
SEEN = []


class _Dialog:
    def Activated(self):
        return {
            "title": "Ask",
            "accept": "Do it",
            "cancel": "Nope",
            "fields": [
                {"name": "N", "label": "N", "kind": "int", "value": 2, "min": 1},
                {"name": "Mode", "kind": "enum", "values": ["a", "b"], "value": "a"},
            ],
            "on_accept": self.accept,
        }

    def accept(self, values):
        SEEN.append(values)
        return {"status": f"got {values['N']} {values['Mode']}"}


class _Wizard:
    def Activated(self):
        return {"title": "One", "on_accept": self.second}

    def second(self, values):
        return {
            "title": "Two",
            "fields": [{"name": "Name", "kind": "text"}],
            "on_accept": lambda more: {"status": f"done {more['Name']}"},
        }


class _Raising:
    def Activated(self):
        return {"title": "Boom", "on_accept": lambda values: 1 / 0}


class DialogProtocolTests(unittest.TestCase):
    def setUp(self):
        SEEN.clear()
        self.names = []

    def tearDown(self):
        for name in self.names:
            FreeCADGui.removeCommand(name)
        FreeCADGui._cancel_all()

    def add(self, name, command):
        FreeCADGui.addCommand(name, command)
        self.names.append(name)

    def test_a_spec_becomes_a_token_and_the_callback_stays_in_python(self):
        self.add("Test_Dialog", _Dialog())
        result = FreeCADGui._run("Test_Dialog")
        self.assertTrue(result["ok"])
        self.assertEqual(result["dialog"]["title"], "Ask")
        self.assertEqual(result["dialog"]["accept"], "Do it")
        self.assertEqual(result["dialog"]["cancel"], "Nope")
        self.assertNotIn("on_accept", result["dialog"])
        self.assertNotIn("on_accept", json.dumps(result))
        self.assertTrue(result["token"])

    def test_accept_runs_the_callback_once(self):
        self.add("Test_Dialog", _Dialog())
        token = FreeCADGui._run("Test_Dialog")["token"]

        result = FreeCADGui._accept(token, {"N": 3, "Mode": "b"})
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["status"], "got 3 b")
        self.assertEqual(SEEN, [{"N": 3, "Mode": "b"}])

        again = FreeCADGui._accept(token, {"N": 3, "Mode": "b"})
        self.assertFalse(again["ok"])
        self.assertIn("no open dialog", again["error"])
        self.assertEqual(len(SEEN), 1, "the token is single use")

    def test_cancel_drops_the_token_without_running_it(self):
        self.add("Test_Dialog", _Dialog())
        token = FreeCADGui._run("Test_Dialog")["token"]

        self.assertTrue(FreeCADGui._cancel(token)["ok"])
        self.assertFalse(FreeCADGui._accept(token, {})["ok"])
        self.assertEqual(SEEN, [])
        # Cancelling a spent token is harmless.
        self.assertTrue(FreeCADGui._cancel(token)["ok"])

    def test_a_callback_can_return_a_new_dialog(self):
        self.add("Test_Wizard", _Wizard())
        first = FreeCADGui._run("Test_Wizard")
        second = FreeCADGui._accept(first["token"], {})
        self.assertEqual(second["dialog"]["title"], "Two")
        self.assertNotEqual(second["token"], first["token"])

        done = FreeCADGui._accept(second["token"], {"Name": "x"})
        self.assertTrue(done["ok"], done)
        self.assertEqual(done["status"], "done x")

    def test_a_raising_callback_is_isolated_and_spends_the_token(self):
        self.add("Test_Boom", _Raising())
        token = FreeCADGui._run("Test_Boom")["token"]

        result = FreeCADGui._accept(token, {})
        self.assertFalse(result["ok"])
        self.assertIn("ZeroDivisionError", result["error"])
        self.assertFalse(FreeCADGui._accept(token, {})["ok"])

    def test_cancel_all_drops_every_token(self):
        self.add("Test_Dialog", _Dialog())
        first = FreeCADGui._run("Test_Dialog")["token"]
        second = FreeCADGui._run("Test_Dialog")["token"]

        FreeCADGui._cancel_all()
        self.assertFalse(FreeCADGui._accept(first, {})["ok"])
        self.assertFalse(FreeCADGui._accept(second, {})["ok"])


if __name__ == "__main__":
    unittest.main()
