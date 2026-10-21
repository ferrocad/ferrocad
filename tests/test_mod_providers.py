"""Tests for the mod provider registry (``docs/workbenches/mod-providers.md``).

Two mods now implement the same feature: ``mods/GridOverlay`` draws the native
grid (``"ferrocad"``) and ``mods/GridOverlayCoin`` registers a Coin overlay
(``"coin"``). ``FreeCADGui.registerProvider`` records both against the ``"grid"``
feature; exactly one is active, the choice is persisted under
``User parameter:BaseApp/Preferences/Mods``, and switching runs the outgoing
``deactivate`` and incoming ``activate``.

The two grid mods are loaded through the existing suites' loaders so this module
shares their (single) module instance and their stubbed ``pivy.coin``. That keeps
one document observer alive for the whole process; the provider registrations are
removed in ``tearDownClass`` so the later suites still see a registry-free
facade, exactly as they did before this slice.
"""

import contextlib
import importlib
import io
import sys
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import ferrocad_shell  # noqa: F401  (registers Mods_Settings)


def _sibling(name):
    """Return a sibling test module, preferring the copy discovery already loaded.

    Discovery adds ``tests/`` to ``sys.path`` and imports ``test_x``; a direct
    ``python -m unittest tests.test_mod_providers`` imports ``tests.test_x``. Both
    must resolve to **one** module object: the suites install a ``pivy.coin`` stub
    and share it, so a second copy makes ``isinstance`` fail across the two copies.
    """
    for key in (name, "tests." + name):
        if key in sys.modules:
            return sys.modules[key]
    return importlib.import_module("tests." + name)


coin_suite = _sibling("test_grid_overlay_coin")
fork_suite = _sibling("test_grid_overlay_fork")

PREFERENCES = "User parameter:BaseApp/Preferences/Mods"
GRID = "grid"
SEMANTIC_COMMAND = "GridOverlay_Settings"
COIN_COMMAND = "GridOverlayCoin_Settings"
MODS_COMMAND = "Mods_Settings"


def provider_prefs():
    """The persisted provider choices (``Providers/<feature>``)."""
    return FreeCAD.ParamGet(PREFERENCES).GetGroup("Providers")


def reset_provider_state():
    """Forget applied backends and saved choices, keeping the registrations."""
    FreeCADGui._PROVIDER_APPLIED.clear()
    provider_prefs().Clear()


def close_documents():
    for name in list(FreeCAD.listDocuments()):
        FreeCAD.closeDocument(name)


class ModProviderTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        reset_provider_state()
        cls.coin = coin_suite.load_mod()
        cls.native, _ = fork_suite.load_mod()
        # setActiveProvider is the only thing that applies callbacks; start each
        # suite from the declared default (highest priority, no saved choice).
        reset_provider_state()

    @classmethod
    def tearDownClass(cls):
        # Leave the facade as the other suites expect it: commands registered,
        # provider registry empty. Unregistering does not run callbacks; detach
        # the overlays explicitly.
        cls.coin.manager.remove_grid()
        cls.coin.manager.items.clear()
        coin_suite.COIN._registered.clear()
        for provider in FreeCADGui.providers(GRID):
            FreeCADGui.unregisterProvider(GRID, provider["backend"])
        reset_provider_state()
        FreeCAD.Grid.reset()
        FreeCADGui._cancel_all()
        close_documents()

    def setUp(self):
        reset_provider_state()
        # ``reset_provider_state`` clears the registry; the mods are already loaded
        # (and cached), so restore their declarations explicitly.
        self.coin.register_provider()
        self.native.register_provider()
        close_documents()
        self.grid = FreeCAD.Grid
        self.grid.reset()
        self.coin.SETTINGS.Clear()
        self.coin.manager.remove_grid()
        self.coin.manager.items.clear()
        coin_suite.COIN._registered.clear()
        self._features = set()

    def tearDown(self):
        for feature in self._features:
            for provider in FreeCADGui.providers(feature):
                FreeCADGui.unregisterProvider(feature, provider["backend"])
        self.coin.manager.remove_grid()
        self.coin.manager.items.clear()
        coin_suite.COIN._registered.clear()
        FreeCADGui._cancel_all()
        close_documents()
        self.grid.reset()
        reset_provider_state()

    # -- selection and persistence ---------------------------------------
    def test_the_highest_priority_wins_and_a_saved_choice_wins(self):
        feature = "demo"
        self._features.add(feature)
        calls = []
        FreeCADGui.registerProvider(
            feature, "low", "Low", priority=1,
            activate=lambda: calls.append("low+"),
        )
        FreeCADGui.registerProvider(
            feature, "high", "High", priority=5,
            activate=lambda: calls.append("high+"),
        )

        self.assertEqual(calls, [], "registration does not run callbacks")
        self.assertEqual(FreeCADGui.activeProvider(feature), "high")
        active = [item["backend"] for item in FreeCADGui.providers(feature) if item["active"]]
        self.assertEqual(active, ["high"], "exactly one provider is active")

        FreeCADGui.setActiveProvider(feature, "low")
        self.assertEqual(FreeCADGui.activeProvider(feature), "low")
        self.assertEqual(provider_prefs().GetString(feature, ""), "low")
        self.assertEqual(calls, ["low+"])

        # Re-registering in the opposite order does not overturn the saved
        # choice: persistence beats priority.
        FreeCADGui.unregisterProvider(feature, "low")
        FreeCADGui.unregisterProvider(feature, "high")
        FreeCADGui.registerProvider(feature, "high", "High", priority=9)
        FreeCADGui.registerProvider(feature, "low", "Low", priority=1)
        self.assertEqual(FreeCADGui.activeProvider(feature), "low")
        self.assertTrue(
            next(item for item in FreeCADGui.providers(feature) if item["backend"] == "low")[
                "active"
            ]
        )

    def test_a_higher_priority_registration_stops_the_outgoing_backend(self):
        feature = "takeover"
        self._features.add(feature)
        calls = []
        FreeCADGui.registerProvider(
            feature, "low", "Low", priority=0,
            activate=lambda: calls.append("low+"),
            deactivate=lambda: calls.append("low-"),
        )
        FreeCADGui.registerProvider(
            feature, "high", "High", priority=10,
            activate=lambda: calls.append("high+"),
        )

        self.assertEqual(
            calls,
            ["low-"],
            "a takeover deactivates the outgoing backend but does not activate the incoming one",
        )
        self.assertEqual(FreeCADGui.activeProvider(feature), "high")

    def test_equal_priority_breaks_the_tie_by_registration_order(self):
        feature = "tie"
        self._features.add(feature)
        FreeCADGui.registerProvider(feature, "first", "First", priority=2)
        FreeCADGui.registerProvider(feature, "second", "Second", priority=2)
        self.assertEqual(FreeCADGui.activeProvider(feature), "first")

    def test_an_unknown_feature_has_no_active_provider(self):
        self.assertIsNone(FreeCADGui.activeProvider("nothing-here"))
        self.assertEqual(FreeCADGui.providers("nothing-here"), [])

    def test_switching_an_unknown_backend_is_a_programmer_error(self):
        self._features.add("guarded")
        FreeCADGui.registerProvider("guarded", "only", "Only")
        with self.assertRaises(ValueError):
            FreeCADGui.setActiveProvider("guarded", "missing")
        with self.assertRaises(ValueError):
            FreeCADGui.setActiveProvider("not-registered", "only")

    # -- the switch contract ---------------------------------------------
    def test_switching_calls_each_callback_exactly_once(self):
        feature = "swap"
        self._features.add(feature)
        a_activate = mock.Mock(return_value=None)
        a_deactivate = mock.Mock(return_value=None)
        b_activate = mock.Mock(return_value=None)
        b_deactivate = mock.Mock(return_value=None)
        FreeCADGui.registerProvider(
            feature, "a", "A", priority=5,
            activate=a_activate, deactivate=a_deactivate,
        )
        FreeCADGui.registerProvider(
            feature, "b", "B", priority=1,
            activate=b_activate, deactivate=b_deactivate,
        )

        FreeCADGui.setActiveProvider(feature, "a")
        a_activate.assert_called_once_with()
        a_deactivate.assert_not_called()
        b_activate.assert_not_called()
        b_deactivate.assert_not_called()

        FreeCADGui.setActiveProvider(feature, "b")
        a_deactivate.assert_called_once_with()
        b_activate.assert_called_once_with()
        self.assertEqual(a_activate.call_count, 1)
        b_deactivate.assert_not_called()

        # Selecting the already-active backend is inert.
        FreeCADGui.setActiveProvider(feature, "b")
        self.assertEqual(a_deactivate.call_count, 1)
        self.assertEqual(b_activate.call_count, 1)

    def test_switching_away_from_the_declared_default_deactivates_it(self):
        feature = "bootstrap"
        self._features.add(feature)
        a_deactivate = mock.Mock(return_value=None)
        b_activate = mock.Mock(return_value=None)
        FreeCADGui.registerProvider(
            feature, "a", "A", priority=10, deactivate=a_deactivate
        )
        FreeCADGui.registerProvider(
            feature, "b", "B", priority=0, activate=b_activate
        )
        self.assertEqual(FreeCADGui.activeProvider(feature), "a")

        FreeCADGui.setActiveProvider(feature, "b")

        a_deactivate.assert_called_once_with()
        b_activate.assert_called_once_with()

    def test_switching_the_grid_backends_adds_and_removes_the_coin_overlay(self):
        FreeCAD.newDocument("Providers")
        FreeCAD.setActiveDocument("Providers")
        self.assertEqual(
            FreeCADGui.activeProvider(GRID), "ferrocad", "the declared default"
        )
        self.assertEqual(
            self.coin.manager.items,
            {},
            "the inactive Coin observer must not build a grid",
        )

        FreeCADGui.setActiveProvider(GRID, "coin")
        self.assertIn("Providers", self.coin.manager.items)
        self.assertTrue(coin_suite.COIN._registered, "the overlay is registered")

        result = FreeCADGui.setActiveProvider(GRID, "ferrocad")
        self.assertEqual(result["previous"], "coin")
        self.assertEqual(self.coin.manager.items, {})
        self.assertEqual(coin_suite.COIN._registered, [], "the Coin overlay is gone")
        self.assertTrue(self.grid.enabled, "native activate enables the native grid")
        self.assertTrue(result["view"]["grid"])

        result = FreeCADGui.setActiveProvider(GRID, "coin")
        self.assertIn("Providers", self.coin.manager.items)
        self.assertTrue(coin_suite.COIN._registered, "the Coin overlay is re-added")
        self.assertFalse(
            self.grid.enabled, "native deactivate restores the default (hidden)"
        )
        self.assertFalse(result["view"]["grid"])

    # -- the inactive command --------------------------------------------
    def test_the_inactive_coin_command_reports_and_changes_nothing(self):
        FreeCAD.newDocument("Inactive")
        FreeCAD.setActiveDocument("Inactive")
        FreeCADGui.setActiveProvider(GRID, "ferrocad")
        before = self.grid.snapshot()
        items = dict(self.coin.manager.items)

        result = FreeCADGui.runCommand(COIN_COMMAND)

        self.assertIn("status", result)
        self.assertNotIn("fields", result, "no dialog for an inactive backend")
        self.assertIn("not the active", result["status"])
        self.assertIn("ferrocad", result["status"])
        self.assertEqual(self.grid.snapshot(), before)
        self.assertEqual(self.coin.manager.items, items)
        self.assertEqual(coin_suite.COIN._registered, [])

    def test_the_inactive_native_command_reports_and_changes_nothing(self):
        FreeCAD.newDocument("Inactive")
        FreeCAD.setActiveDocument("Inactive")
        FreeCADGui.setActiveProvider(GRID, "coin")
        before = self.grid.snapshot()

        result = FreeCADGui.runCommand(SEMANTIC_COMMAND)

        self.assertIn("status", result)
        self.assertNotIn("fields", result, "no dialog for an inactive backend")
        self.assertIn("not the active", result["status"])
        self.assertIn("coin", result["status"])
        self.assertEqual(self.grid.snapshot(), before)

    # -- the switch dialog ------------------------------------------------
    def test_mods_settings_lists_both_grid_providers_and_switches(self):
        FreeCAD.newDocument("Mods")
        FreeCAD.setActiveDocument("Mods")
        FreeCADGui.setActiveProvider(GRID, "ferrocad")

        run = FreeCADGui._run(MODS_COMMAND)
        self.assertTrue(run["ok"], run)
        self.assertIn("token", run)
        fields = {field["name"]: field for field in run["dialog"]["fields"]}
        self.assertIn(GRID, fields)
        field = fields[GRID]
        self.assertEqual(field["kind"], "enum")
        self.assertEqual(set(field["values"]), {"FerroCAD (native)", "Coin (WGSL)"})
        self.assertEqual(field["value"], "FerroCAD (native)")

        accepted = FreeCADGui._accept(run["token"], {GRID: "Coin (WGSL)"})
        self.assertTrue(accepted["ok"], accepted)
        self.assertEqual(FreeCADGui.activeProvider(GRID), "coin")
        self.assertIn("view", accepted, "the switch repaints in one request")
        self.assertFalse(accepted["view"]["grid"])
        self.assertIn("Mods", self.coin.manager.items)

        # A fresh dialog (the token was spent) applies the reverse; the value may
        # be the backend id as well as the label.
        run = FreeCADGui._run(MODS_COMMAND)
        accepted = FreeCADGui._accept(run["token"], {GRID: "ferrocad"})
        self.assertTrue(accepted["ok"], accepted)
        self.assertEqual(FreeCADGui.activeProvider(GRID), "ferrocad")
        self.assertTrue(accepted["view"]["grid"])
        self.assertEqual(self.coin.manager.items, {})

    def test_mods_settings_reports_when_nothing_is_registered(self):
        for provider in FreeCADGui.providers(GRID):
            FreeCADGui.unregisterProvider(GRID, provider["backend"])
        try:
            result = FreeCADGui.runCommand(MODS_COMMAND)
            self.assertIn("status", result)
            self.assertIn("no mod providers", result["status"])
            self.assertNotIn("fields", result)
        finally:
            # Restore the two real grid registrations for the remaining tests.
            FreeCADGui.registerProvider(
                GRID, "coin", "Coin (WGSL)", command=COIN_COMMAND, priority=0,
                activate=self.coin.activate, deactivate=self.coin.deactivate,
            )
            FreeCADGui.registerProvider(
                GRID, "ferrocad", "FerroCAD (native)",
                command=SEMANTIC_COMMAND, priority=10,
                activate=self.native.activate, deactivate=self.native.deactivate,
            )
            reset_provider_state()

    # -- failure isolation -------------------------------------------------
    def test_callback_errors_are_reported_and_do_not_break_the_registry(self):
        feature = "broken"
        self._features.add(feature)
        activated = []

        def explode():
            raise RuntimeError("boom")

        FreeCADGui.registerProvider(
            feature, "a", "A", priority=1, activate=explode, deactivate=explode
        )
        FreeCADGui.registerProvider(
            feature, "b", "B", priority=0,
            activate=lambda: activated.append("b"), deactivate=explode,
        )

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            first = FreeCADGui.setActiveProvider(feature, "a")
            self.assertTrue(first["ok"], first)
            self.assertTrue(first["errors"])
            self.assertIn("activate", first["errors"][0])
            self.assertEqual(FreeCADGui.activeProvider(feature), "a")

            second = FreeCADGui.setActiveProvider(feature, "b")
            self.assertTrue(second["ok"], second)
            self.assertIn("deactivate", second["errors"][0])
            self.assertEqual(FreeCADGui.activeProvider(feature), "b")
            self.assertEqual(
                activated, ["b"], "a failing deactivate does not skip the incoming activate"
            )

        report = stderr.getvalue()
        self.assertIn("provider 'a'", report)
        self.assertIn("activate raised", report)
        self.assertIn("RuntimeError: boom", report)
        self.assertEqual(
            {item["backend"] for item in FreeCADGui.providers(feature)}, {"a", "b"}
        )


if __name__ == "__main__":
    unittest.main()
