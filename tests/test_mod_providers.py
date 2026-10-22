"""Tests for the mod provider registry (``docs/workbenches/mod-providers.md``).

Two mods now implement the same feature: ``mods/GridOverlay`` draws the native
grid (``"ferrocad"``) and ``mods/GridOverlayCoin`` registers a Coin overlay
(``"coin"``). ``FerroCADGui.registerProvider`` records both against the ``"grid"``
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
import json
import sys
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
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
TOGGLE = "App_ToggleGrid"
SEMANTIC_COMMAND = "GridOverlay_Settings"
COIN_COMMAND = "GridOverlayCoin_Settings"
MODS_COMMAND = "Mods_Settings"


def provider_prefs():
    """The persisted provider choices (``Providers/<feature>``)."""
    return FreeCAD.ParamGet(PREFERENCES).GetGroup("Providers")


def reset_provider_state():
    """Forget applied backends and saved choices, keeping the registrations."""
    FerroCADGui._PROVIDER_APPLIED.clear()
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
        for provider in FerroCADGui.providers(GRID):
            FerroCADGui.unregisterProvider(GRID, provider["backend"])
        reset_provider_state()
        FerroCAD.Grid.reset()
        FreeCADGui._cancel_all()
        close_documents()

    def setUp(self):
        reset_provider_state()
        # ``reset_provider_state`` clears the registry; the mods are already loaded
        # (and cached), so restore their declarations explicitly.
        self.coin.register_provider()
        self.native.register_provider()
        close_documents()
        self.grid = FerroCAD.Grid
        self.grid.reset()
        self.coin.SETTINGS.Clear()
        self.coin.manager.remove_grid()
        self.coin.manager.items.clear()
        coin_suite.COIN._registered.clear()
        self._features = set()

    def tearDown(self):
        for feature in self._features:
            for provider in FerroCADGui.providers(feature):
                FerroCADGui.unregisterProvider(feature, provider["backend"])
        self.coin.manager.remove_grid()
        self.coin.manager.items.clear()
        coin_suite.COIN._registered.clear()
        FreeCADGui._cancel_all()
        close_documents()
        self.grid.reset()
        reset_provider_state()

    # -- the grid toggle over the Coin provider --------------------------
    def switch_to_coin(self, name="Toggle"):
        """Open a document and make Coin the drawn grid; return its overlay."""
        FreeCAD.newDocument(name)
        FreeCAD.setActiveDocument(name)
        FerroCADGui.setActiveProvider(GRID, "coin")
        self.assertTrue(self.grid.enabled, "coin activate turns the one switch on")
        overlay = self.coin.manager.items[name]
        self.assertTrue(overlay.visible)
        self.assertIn(overlay.root, coin_suite.COIN._registered)
        return overlay

    def checked(self, name):
        return next(c for c in FreeCADGui._command_list() if c["id"] == name)[
            "checked"
        ]

    def test_toggling_with_the_coin_provider_unbinds_the_overlay(self):
        overlay = self.switch_to_coin()
        self.assertTrue(self.coin.setting("enabled"))

        # The overlay records the master-switch change the toggle caused.
        original = self.coin.ShaderGrid.set_visible
        calls = []

        def spy(grid, visible):
            calls.append(bool(visible))
            return original(grid, visible)

        with mock.patch.object(self.coin.ShaderGrid, "set_visible", spy):
            result = FreeCADGui._run(TOGGLE)

        self.assertTrue(result["ok"], result)
        self.assertFalse(result["view"]["grid"], "the native ground is hidden")
        self.assertFalse(result["view"]["grid_overlay"], "the overlay stops drawing")
        self.assertTrue(calls, "the recorded overlay was told to hide")
        self.assertTrue(
            all(call is False for call in calls), "set_visible(False) on the overlay"
        )
        self.assertFalse(
            self.coin.setting("enabled"), "coin.setting follows the one switch"
        )
        self.assertFalse(overlay.visible)
        self.assertNotIn(overlay.root, coin_suite.COIN._registered)

        # The shell applies the request and mirrors it; the check mark follows.
        ferrocad_shell.set_view_state(json.dumps({"grid": False}))
        self.assertFalse(self.checked(TOGGLE))

    def test_two_toggles_return_to_the_start_state_and_keep_the_knobs(self):
        overlay = self.switch_to_coin()
        self.coin.save_settings(
            {"color": "#123456", "opacity": 0.4, "base_spacing": 2.5}
        )

        first = FreeCADGui._run(TOGGLE)
        self.assertFalse(first["view"]["grid"])
        self.assertFalse(first["view"]["grid_overlay"])
        self.assertFalse(overlay.visible)
        self.assertFalse(self.grid.enabled)

        second = FreeCADGui._run(TOGGLE)
        self.assertTrue(second["view"]["grid"])
        self.assertTrue(second["view"]["grid_overlay"])
        self.assertTrue(overlay.visible, "the same overlay is re-bound")
        self.assertIn(overlay.root, coin_suite.COIN._registered)
        self.assertTrue(self.grid.enabled)
        self.assertTrue(self.coin.setting("enabled"))

        # Only the master switch moved: the appearance knobs are untouched and
        # were pushed into the rebuilt tree.
        self.assertEqual(self.coin.setting("color"), "#123456")
        self.assertAlmostEqual(self.coin.setting("opacity"), 0.4)
        self.assertAlmostEqual(self.coin.setting("base_spacing"), 2.5)
        self.assertAlmostEqual(overlay.base_spacing.value, 2.5)

    def test_a_broken_coin_refresh_is_reported_and_does_not_raise(self):
        self.switch_to_coin()

        def explode():
            raise RuntimeError("refresh boom")

        stderr = io.StringIO()
        with mock.patch.object(self.coin, "refresh", explode):
            self.coin.register_provider()
            with contextlib.redirect_stderr(stderr):
                result = FreeCADGui._run(TOGGLE)

        self.assertTrue(result["ok"], result)
        self.assertFalse(result["view"]["grid"])
        report = stderr.getvalue()
        self.assertIn("provider 'coin'", report)
        self.assertIn("refresh raised", report)
        self.assertIn("RuntimeError: refresh boom", report)

    # -- selection and persistence ---------------------------------------
    def test_the_highest_priority_wins_and_a_saved_choice_wins(self):
        feature = "demo"
        self._features.add(feature)
        calls = []
        FerroCADGui.registerProvider(
            feature, "low", "Low", priority=1,
            activate=lambda: calls.append("low+"),
        )
        FerroCADGui.registerProvider(
            feature, "high", "High", priority=5,
            activate=lambda: calls.append("high+"),
        )

        self.assertEqual(calls, [], "registration does not run callbacks")
        self.assertEqual(FerroCADGui.activeProvider(feature), "high")
        active = [item["backend"] for item in FerroCADGui.providers(feature) if item["active"]]
        self.assertEqual(active, ["high"], "exactly one provider is active")

        FerroCADGui.setActiveProvider(feature, "low")
        self.assertEqual(FerroCADGui.activeProvider(feature), "low")
        self.assertEqual(provider_prefs().GetString(feature, ""), "low")
        self.assertEqual(calls, ["low+"])

        # Re-registering in the opposite order does not overturn the saved
        # choice: persistence beats priority.
        FerroCADGui.unregisterProvider(feature, "low")
        FerroCADGui.unregisterProvider(feature, "high")
        FerroCADGui.registerProvider(feature, "high", "High", priority=9)
        FerroCADGui.registerProvider(feature, "low", "Low", priority=1)
        self.assertEqual(FerroCADGui.activeProvider(feature), "low")
        self.assertTrue(
            next(item for item in FerroCADGui.providers(feature) if item["backend"] == "low")[
                "active"
            ]
        )

    def test_a_higher_priority_registration_stops_the_outgoing_backend(self):
        feature = "takeover"
        self._features.add(feature)
        calls = []
        FerroCADGui.registerProvider(
            feature, "low", "Low", priority=0,
            activate=lambda: calls.append("low+"),
            deactivate=lambda: calls.append("low-"),
        )
        FerroCADGui.registerProvider(
            feature, "high", "High", priority=10,
            activate=lambda: calls.append("high+"),
        )

        self.assertEqual(
            calls,
            ["low-"],
            "a takeover deactivates the outgoing backend but does not activate the incoming one",
        )
        self.assertEqual(FerroCADGui.activeProvider(feature), "high")

    def test_equal_priority_breaks_the_tie_by_registration_order(self):
        feature = "tie"
        self._features.add(feature)
        FerroCADGui.registerProvider(feature, "first", "First", priority=2)
        FerroCADGui.registerProvider(feature, "second", "Second", priority=2)
        self.assertEqual(FerroCADGui.activeProvider(feature), "first")

    def test_an_unknown_feature_has_no_active_provider(self):
        self.assertIsNone(FerroCADGui.activeProvider("nothing-here"))
        self.assertEqual(FerroCADGui.providers("nothing-here"), [])

    def test_switching_an_unknown_backend_is_a_programmer_error(self):
        self._features.add("guarded")
        FerroCADGui.registerProvider("guarded", "only", "Only")
        with self.assertRaises(ValueError):
            FerroCADGui.setActiveProvider("guarded", "missing")
        with self.assertRaises(ValueError):
            FerroCADGui.setActiveProvider("not-registered", "only")

    # -- the switch contract ---------------------------------------------
    def test_switching_calls_each_callback_exactly_once(self):
        feature = "swap"
        self._features.add(feature)
        a_activate = mock.Mock(return_value=None)
        a_deactivate = mock.Mock(return_value=None)
        b_activate = mock.Mock(return_value=None)
        b_deactivate = mock.Mock(return_value=None)
        FerroCADGui.registerProvider(
            feature, "a", "A", priority=5,
            activate=a_activate, deactivate=a_deactivate,
        )
        FerroCADGui.registerProvider(
            feature, "b", "B", priority=1,
            activate=b_activate, deactivate=b_deactivate,
        )

        FerroCADGui.setActiveProvider(feature, "a")
        a_activate.assert_called_once_with()
        a_deactivate.assert_not_called()
        b_activate.assert_not_called()
        b_deactivate.assert_not_called()

        FerroCADGui.setActiveProvider(feature, "b")
        a_deactivate.assert_called_once_with()
        b_activate.assert_called_once_with()
        self.assertEqual(a_activate.call_count, 1)
        b_deactivate.assert_not_called()

        # Selecting the already-active backend is inert.
        FerroCADGui.setActiveProvider(feature, "b")
        self.assertEqual(a_deactivate.call_count, 1)
        self.assertEqual(b_activate.call_count, 1)

    def test_switching_away_from_the_declared_default_deactivates_it(self):
        feature = "bootstrap"
        self._features.add(feature)
        a_deactivate = mock.Mock(return_value=None)
        b_activate = mock.Mock(return_value=None)
        FerroCADGui.registerProvider(
            feature, "a", "A", priority=10, deactivate=a_deactivate
        )
        FerroCADGui.registerProvider(
            feature, "b", "B", priority=0, activate=b_activate
        )
        self.assertEqual(FerroCADGui.activeProvider(feature), "a")

        FerroCADGui.setActiveProvider(feature, "b")

        a_deactivate.assert_called_once_with()
        b_activate.assert_called_once_with()

    def test_switching_the_grid_backends_adds_and_removes_the_coin_overlay(self):
        FreeCAD.newDocument("Providers")
        FreeCAD.setActiveDocument("Providers")
        self.assertEqual(
            FerroCADGui.activeProvider(GRID), "ferrocad", "the declared default"
        )
        self.assertEqual(
            self.coin.manager.items,
            {},
            "the inactive Coin observer must not build a grid",
        )

        FerroCADGui.setActiveProvider(GRID, "coin")
        self.assertIn("Providers", self.coin.manager.items)
        self.assertTrue(coin_suite.COIN._registered, "the overlay is registered")

        result = FerroCADGui.setActiveProvider(GRID, "ferrocad")
        self.assertEqual(result["previous"], "coin")
        self.assertEqual(self.coin.manager.items, {})
        self.assertEqual(coin_suite.COIN._registered, [], "the Coin overlay is gone")
        self.assertTrue(self.grid.enabled, "native activate enables the native grid")
        self.assertTrue(result["view"]["grid"])

        result = FerroCADGui.setActiveProvider(GRID, "coin")
        self.assertIn("Providers", self.coin.manager.items)
        self.assertTrue(coin_suite.COIN._registered, "the Coin overlay is re-added")
        self.assertTrue(
            self.grid.enabled, "coin activate turns the one grid switch on"
        )
        self.assertTrue(result["view"]["grid"])
        self.assertTrue(result["view"]["grid_overlay"], "the native ground is hidden")

    # -- the inactive command --------------------------------------------
    def test_the_inactive_coin_command_reports_and_changes_nothing(self):
        FreeCAD.newDocument("Inactive")
        FreeCAD.setActiveDocument("Inactive")
        FerroCADGui.setActiveProvider(GRID, "ferrocad")
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
        FerroCADGui.setActiveProvider(GRID, "coin")
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
        FerroCADGui.setActiveProvider(GRID, "ferrocad")

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
        self.assertEqual(FerroCADGui.activeProvider(GRID), "coin")
        self.assertIn("view", accepted, "the switch repaints in one request")
        self.assertTrue(accepted["view"]["grid"])
        self.assertTrue(accepted["view"]["grid_overlay"])
        self.assertIn("Mods", self.coin.manager.items)

        # A fresh dialog (the token was spent) applies the reverse; the value may
        # be the backend id as well as the label.
        run = FreeCADGui._run(MODS_COMMAND)
        accepted = FreeCADGui._accept(run["token"], {GRID: "ferrocad"})
        self.assertTrue(accepted["ok"], accepted)
        self.assertEqual(FerroCADGui.activeProvider(GRID), "ferrocad")
        self.assertTrue(accepted["view"]["grid"])
        self.assertEqual(self.coin.manager.items, {})

    def test_mods_settings_reports_when_nothing_is_registered(self):
        for provider in FerroCADGui.providers(GRID):
            FerroCADGui.unregisterProvider(GRID, provider["backend"])
        try:
            result = FreeCADGui.runCommand(MODS_COMMAND)
            self.assertIn("status", result)
            self.assertIn("no mod providers", result["status"])
            self.assertNotIn("fields", result)
        finally:
            # Restore the two real grid registrations for the remaining tests.
            FerroCADGui.registerProvider(
                GRID, "coin", "Coin (WGSL)", command=COIN_COMMAND, priority=0,
                activate=self.coin.activate, deactivate=self.coin.deactivate,
                refresh=self.coin.refresh,
            )
            FerroCADGui.registerProvider(
                GRID, "ferrocad", "FerroCAD (native)",
                command=SEMANTIC_COMMAND, priority=10,
                activate=self.native.activate, deactivate=self.native.deactivate,
            )
            reset_provider_state()

    # -- failure isolation -------------------------------------------------
    def test_refresh_runs_only_when_asked_never_at_registration_or_switch(self):
        feature = "refreshable"
        self._features.add(feature)
        calls = []

        def refresh():
            calls.append("a")
            return {"view": {"flag": True}, "status": "re-read"}

        FerroCADGui.registerProvider(feature, "a", "A", priority=1, refresh=refresh)
        self.assertEqual(calls, [], "registration does not run refresh")

        FerroCADGui.setActiveProvider(feature, "a")
        self.assertEqual(calls, [], "a switch runs activate, not refresh")

        result = FerroCADGui.refreshProviders(feature)
        self.assertEqual(calls, ["a"])
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["backend"], "a")
        self.assertEqual(result["view"], {"flag": True})
        self.assertEqual(result["status"], "re-read")

        # A feature with no provider (or a backend with no callback) is a no-op,
        # which is what the shell's grid toggle needs on a bare facade.
        empty = FerroCADGui.refreshProviders("nothing-here")
        self.assertTrue(empty["ok"], empty)
        self.assertIsNone(empty["backend"])
        self.assertEqual(empty["view"], {})
        self.assertEqual(empty["errors"], [])

    def test_a_broken_refresh_callback_is_reported_not_raised(self):
        feature = "broken-refresh"
        self._features.add(feature)

        def explode():
            raise RuntimeError("refresh boom")

        FerroCADGui.registerProvider(feature, "a", "A", refresh=explode)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = FerroCADGui.refreshProviders(feature)

        self.assertTrue(result["ok"], result)
        self.assertEqual(result["view"], {})
        self.assertEqual(len(result["errors"]), 1)
        self.assertIn("refresh", result["errors"][0])
        self.assertIn("refresh boom", result["errors"][0])

        report = stderr.getvalue()
        self.assertIn("provider 'a'", report)
        self.assertIn("refresh raised", report)
        self.assertIn("RuntimeError: refresh boom", report)

    def test_a_non_callable_refresh_is_a_programmer_error(self):
        feature = "bad-refresh-registration"
        self._features.add(feature)
        with self.assertRaises(TypeError):
            FerroCADGui.registerProvider(feature, "a", "A", refresh=42)
        self.assertEqual(FerroCADGui.providers(feature), [])

    def test_callback_errors_are_reported_and_do_not_break_the_registry(self):
        feature = "broken"
        self._features.add(feature)
        activated = []

        def explode():
            raise RuntimeError("boom")

        FerroCADGui.registerProvider(
            feature, "a", "A", priority=1, activate=explode, deactivate=explode
        )
        FerroCADGui.registerProvider(
            feature, "b", "B", priority=0,
            activate=lambda: activated.append("b"), deactivate=explode,
        )

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            first = FerroCADGui.setActiveProvider(feature, "a")
            self.assertTrue(first["ok"], first)
            self.assertTrue(first["errors"])
            self.assertIn("activate", first["errors"][0])
            self.assertEqual(FerroCADGui.activeProvider(feature), "a")

            second = FerroCADGui.setActiveProvider(feature, "b")
            self.assertTrue(second["ok"], second)
            self.assertIn("deactivate", second["errors"][0])
            self.assertEqual(FerroCADGui.activeProvider(feature), "b")
            self.assertEqual(
                activated, ["b"], "a failing deactivate does not skip the incoming activate"
            )

        report = stderr.getvalue()
        self.assertIn("provider 'a'", report)
        self.assertIn("activate raised", report)
        self.assertIn("RuntimeError: boom", report)
        self.assertEqual(
            {item["backend"] for item in FerroCADGui.providers(feature)}, {"a", "b"}
        )


class ProviderSkinTests(unittest.TestCase):
    """``registerProvider(skin=...)`` and ``providerSkin``: the shader channel.

    A skin is a material program, not a setting: it reaches the shell through
    :func:`providerSkin` and never appears in the mods dialog's manifest.
    """

    FEATURE = "skinned"
    WGSL = "@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"

    def setUp(self):
        self._features = set()

    def tearDown(self):
        for feature in self._features:
            for provider in FerroCADGui.providers(feature):
                FerroCADGui.unregisterProvider(feature, provider["backend"])
        reset_provider_state()

    def register(self, backend, skin=None, priority=0):
        self._features.add(self.FEATURE)
        return FerroCADGui.registerProvider(
            self.FEATURE, backend, backend.title(), priority=priority, skin=skin
        )

    def test_a_supplied_skin_is_stored_per_backend_and_asked_for_by_the_active_one(self):
        self.register("a", self.WGSL)
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE, "a"), self.WGSL)
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE), self.WGSL)
        self.assertIn("providerSkin", FerroCADGui.__all__)

        # The stored source is trimmed, so a heredoc-style literal is fine.
        self.register("b", "\n  " + self.WGSL + "  \n", priority=5)
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE, "b"), self.WGSL)
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE), self.WGSL)

    def test_no_skin_and_unknown_names_answer_none_without_raising(self):
        self.register("a")
        self.assertIsNone(FerroCADGui.providerSkin(self.FEATURE, "a"))
        self.assertIsNone(FerroCADGui.providerSkin(self.FEATURE))
        self.assertIsNone(FerroCADGui.providerSkin(self.FEATURE, "missing"))
        self.assertIsNone(FerroCADGui.providerSkin("nothing-here"))
        self.assertIsNone(FerroCADGui.providerSkin("nothing-here", "a"))

    def test_a_bad_skin_is_a_programmer_error_and_leaves_the_record_intact(self):
        self.register("a", self.WGSL)

        for bad in (42, b"", ["wgsl"]):
            with self.subTest(skin=bad):
                with self.assertRaises(TypeError):
                    self.register("a", bad)
                self.assertEqual(FerroCADGui.providerSkin(self.FEATURE, "a"), self.WGSL)

        for bad in ("", "   \n\t"):
            with self.subTest(skin=bad):
                with self.assertRaises(ValueError):
                    self.register("a", bad)
                self.assertEqual(
                    FerroCADGui.providerSkin(self.FEATURE, "a"),
                    self.WGSL,
                    "a rejected re-registration keeps the old source",
                )

    def test_the_manifest_does_not_carry_the_skin(self):
        manifest = self.register("a", self.WGSL)
        self.assertEqual(
            set(manifest),
            {"feature", "backend", "label", "command", "priority", "active"},
        )
        self.assertNotIn("skin", manifest)
        self.assertNotIn("skin", FerroCADGui.providers(self.FEATURE)[0])

    def test_reregistering_replaces_the_source_and_omitting_it_clears_it(self):
        self.register("a", self.WGSL)
        self.register("a", "// reloaded")
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE, "a"), "// reloaded")

        self.register("a")
        self.assertIsNone(
            FerroCADGui.providerSkin(self.FEATURE, "a"),
            "a reload without a skin must not keep a stale source",
        )

    def test_switching_the_active_backend_switches_the_source(self):
        self.register("skinned", self.WGSL, priority=0)
        self.register("plain", priority=10)
        self.assertEqual(FerroCADGui.activeProvider(self.FEATURE), "plain")
        self.assertIsNone(FerroCADGui.providerSkin(self.FEATURE))
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE, "skinned"), self.WGSL)

        FerroCADGui.setActiveProvider(self.FEATURE, "skinned")
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE), self.WGSL)
        self.assertIsNone(FerroCADGui.providerSkin(self.FEATURE, "plain"))

        FerroCADGui.setActiveProvider(self.FEATURE, "plain")
        self.assertIsNone(FerroCADGui.providerSkin(self.FEATURE))
        self.assertEqual(FerroCADGui.providerSkin(self.FEATURE, "skinned"), self.WGSL)

    def test_the_shell_bridge_round_trips_and_defaults_to_null(self):
        self.register("a", self.WGSL)
        payload = json.dumps({"feature": self.FEATURE})
        self.assertEqual(
            json.loads(ferrocad_shell.provider_skin(payload)), {"source": self.WGSL}
        )
        self.assertEqual(
            json.loads(ferrocad_shell.provider_skin(json.dumps({"feature": "missing"}))),
            {"source": None},
        )
        self.assertEqual(json.loads(ferrocad_shell.provider_skin("")), {"source": None})
        self.assertEqual(json.loads(ferrocad_shell.provider_skin("{}")), {"source": None})


if __name__ == "__main__":
    unittest.main()
