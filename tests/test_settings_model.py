"""Tests for provider-advertised settings and the ``Settings`` tree.

Covers S0 and S1 of ``docs/tasks/settings-model.md``: a provider declares a
schema at registration, ``FerroCADGui.settings``/``setSetting`` read and write it
(the parameter group is the store), and ``ferrocad_shell`` exposes a synthetic
``Settings/<Feature>`` node whose rows are the property pane's row contract.

The interpreter is shared with the other suites, so every provider record and
parameter group this module touches is removed again in ``tearDown``.
"""

import json
import unittest

import FreeCAD
import FreeCADGui
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the shell commands)

FEATURE = "grid"
PREFERENCES = "User parameter:BaseApp/Preferences/Mods"

NATIVE_SCHEMA = [
    {"name": "enabled", "label": "Enabled", "kind": "bool", "default": True},
    {"name": "color", "label": "Colour", "kind": "color", "default": "#94a3b8"},
    {
        "name": "spacing",
        "label": "Spacing",
        "kind": "quantity",
        "unit": "mm",
        "default": 10.0,
        "min": 0.1,
        "max": 1000.0,
    },
    {
        "name": "opacity",
        "label": "Opacity",
        "kind": "float",
        "default": 1.0,
        "min": 0.0,
        "max": 1.0,
    },
    {
        "name": "count",
        "label": "Count",
        "kind": "int",
        "default": 2,
        "min": 0,
        "max": 10,
    },
    {
        "name": "mode",
        "label": "Mode",
        "kind": "enum",
        "choices": ["Plain", "Fancy"],
        "default": "Plain",
    },
    {"name": "note", "label": "Note", "kind": "text", "default": "hello"},
]

COIN_SCHEMA = [
    {
        "name": "spacing",
        "label": "Spacing",
        "kind": "quantity",
        "unit": "mm",
        "default": 5.0,
    },
    {"name": "glow", "label": "Glow", "kind": "bool", "default": False},
]


def settings_group(feature, backend):
    """The parameter group a provider's values live in."""
    return FreeCAD.ParamGet(f"{PREFERENCES}/{feature}/{backend}")


def provider_prefs():
    return FreeCAD.ParamGet(PREFERENCES).GetGroup("Providers")


def close_documents():
    for name in list(FreeCAD.listDocuments()):
        FreeCAD.closeDocument(name)


class SettingsModelTests(unittest.TestCase):
    def setUp(self):
        self.features = set()
        self.refreshed = []
        close_documents()
        provider_prefs().Clear()
        FerroCADGui._PROVIDER_APPLIED.clear()

    def tearDown(self):
        for feature in self.features:
            for provider in FerroCADGui.providers(feature):
                FerroCADGui.unregisterProvider(feature, provider["backend"])
            FreeCAD.ParamGet(PREFERENCES).GetGroup(feature).Clear()
        provider_prefs().Clear()
        FerroCADGui._PROVIDER_APPLIED.clear()
        FerroCADGui._SETTING_REPORTED.clear()
        FerroCADGui._PROVIDER_REPORTED.clear()
        close_documents()

    # -- helpers -------------------------------------------------------------
    def register(self, backend, label, priority=0, settings=None, feature=FEATURE):
        self.features.add(feature)
        return FerroCADGui.registerProvider(
            feature,
            backend,
            label,
            priority=priority,
            refresh=self.refresh,
            settings=settings,
        )

    def refresh(self):
        self.refreshed.append("refresh")
        return {"view": {"grid": True}, "status": "grid refreshed"}

    def register_both(self):
        self.register("ferrocad", "Native", priority=1, settings=NATIVE_SCHEMA)
        self.register("coin", "Coin (WGSL)", priority=0, settings=COIN_SCHEMA)

    def rows(self, feature=FEATURE, backend=None):
        return {row["name"]: row for row in FerroCADGui.settings(feature, backend)}

    def settings_rows(self, object_name="Settings/Grid"):
        rows = json.loads(ferrocad_shell.properties(object_name))
        return rows[0], {row["name"]: row for row in rows[1:]}

    # -- S0: defaults and row shape -----------------------------------------
    def test_defaults_are_materialised_on_read_and_never_written(self):
        self.register("ferrocad", "Native", settings=NATIVE_SCHEMA)

        rows = FerroCADGui.settings(FEATURE)
        self.assertEqual(
            [row["name"] for row in rows],
            ["enabled", "color", "spacing", "opacity", "count", "mode", "note"],
            "schema order is the row order",
        )
        by_name = self.rows()
        self.assertEqual(by_name["enabled"]["value"], "True")
        self.assertEqual(by_name["color"]["value"], "#94a3b8")
        self.assertEqual(by_name["spacing"]["value"], "10 mm")
        self.assertEqual(by_name["opacity"]["value"], "1.0")
        self.assertEqual(by_name["count"]["value"], "2")
        self.assertEqual(by_name["mode"]["value"], "Plain")
        self.assertEqual(by_name["note"]["value"], "hello")

        spacing = by_name["spacing"]
        self.assertEqual(spacing["label"], "Spacing")
        self.assertEqual(spacing["kind"], "quantity")
        self.assertEqual(spacing["type"], "App::PropertyQuantity")
        self.assertEqual(spacing["unit"], "mm")
        self.assertTrue(spacing["editable"])
        self.assertTrue(by_name["mode"]["editable"])
        self.assertEqual(by_name["mode"]["choices"], ["Plain", "Fancy"])
        self.assertNotIn("unit", by_name["mode"])

        group = settings_group(FEATURE, "ferrocad")
        self.assertEqual(
            group.GetFloat("spacing", -1.0),
            -1.0,
            "reading a default must not create a stored value",
        )
        self.assertEqual(group.GetString("mode", "<missing>"), "<missing>")
        self.assertEqual(self.refreshed, [])

    def test_settings_returns_the_active_provider_and_a_switch_changes_it(self):
        self.register_both()

        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "ferrocad")
        self.assertEqual(self.rows()["spacing"]["value"], "10 mm")
        self.assertIn("enabled", self.rows())

        FerroCADGui.setActiveProvider(FEATURE, "coin")
        coin = self.rows()
        self.assertEqual(coin["spacing"]["value"], "5 mm")
        self.assertIn("glow", coin)
        self.assertNotIn("enabled", coin)

        # The named backend is readable regardless of which one is active.
        self.assertEqual(self.rows(backend="ferrocad")["spacing"]["value"], "10 mm")

    # -- S0: setSetting ------------------------------------------------------
    def test_set_setting_round_trips_each_kind_and_refreshes(self):
        self.register("ferrocad", "Native", settings=NATIVE_SCHEMA)

        cases = [
            ("enabled", "false", "False"),
            ("count", "4", "4"),
            ("opacity", "0.5", "0.5"),
            ("spacing", "5 mm", "5 mm"),
            ("mode", "Fancy", "Fancy"),
            ("color", "#AABBCC", "#aabbcc"),
            ("note", "hi", "hi"),
        ]
        for name, text, expected in cases:
            result = FerroCADGui.setSetting(FEATURE, name, text)
            self.assertTrue(result["ok"], (name, result))
            self.assertEqual(result["value"], expected, name)
            self.assertEqual(result["view"], {"grid": True})
            self.assertEqual(result["backend"], "ferrocad")

        group = settings_group(FEATURE, "ferrocad")
        self.assertFalse(group.GetBool("enabled", True))
        self.assertEqual(group.GetInt("count", 0), 4)
        self.assertEqual(group.GetFloat("opacity", 0.0), 0.5)
        self.assertEqual(group.GetFloat("spacing", 0.0), 5.0)
        self.assertEqual(group.GetString("mode", ""), "Fancy")
        self.assertEqual(group.GetString("color", ""), "#aabbcc")
        self.assertEqual(group.GetString("note", ""), "hi")

        by_name = self.rows()
        self.assertEqual(by_name["spacing"]["value"], "5 mm")
        self.assertEqual(by_name["enabled"]["value"], "False")
        self.assertEqual(
            len(self.refreshed), len(cases), "every successful set refreshes once"
        )

        # A bare number is in the declared unit; a unit string is converted.
        self.assertEqual(
            FerroCADGui.setSetting(FEATURE, "spacing", 7.5)["value"], "7.5 mm"
        )
        self.assertEqual(
            FerroCADGui.setSetting(FEATURE, "spacing", "8")["value"], "8 mm"
        )
        self.assertEqual(
            FerroCADGui.setSetting(FEATURE, "spacing", "0.5 cm")["value"], "5 mm"
        )

    def test_out_of_range_and_enum_miss_change_nothing(self):
        self.register("ferrocad", "Native", settings=NATIVE_SCHEMA)
        self.assertTrue(FerroCADGui.setSetting(FEATURE, "opacity", "0.25")["ok"])
        self.assertTrue(FerroCADGui.setSetting(FEATURE, "spacing", "5 mm")["ok"])
        written = len(self.refreshed)

        too_large = FerroCADGui.setSetting(FEATURE, "opacity", "2.0")
        self.assertFalse(too_large["ok"])
        self.assertIn("at most 1", too_large["error"])

        too_small = FerroCADGui.setSetting(FEATURE, "spacing", "0 mm")
        self.assertFalse(too_small["ok"])
        self.assertIn("at least 0.1", too_small["error"])

        miss = FerroCADGui.setSetting(FEATURE, "mode", "Bogus")
        self.assertFalse(miss["ok"])
        self.assertIn("must be one of", miss["error"])

        self.assertEqual(
            len(self.refreshed), written, "a rejected set does not refresh"
        )
        by_name = self.rows()
        self.assertEqual(by_name["opacity"]["value"], "0.25")
        self.assertEqual(by_name["spacing"]["value"], "5 mm")
        self.assertEqual(by_name["mode"]["value"], "Plain")

    def test_a_provider_without_a_schema_has_no_rows(self):
        self.register("plain", "Plain")

        self.assertEqual(FerroCADGui.settings(FEATURE), [])
        self.assertEqual(FerroCADGui.settings(FEATURE, "plain"), [])
        result = FerroCADGui.setSetting(FEATURE, "anything", "x")
        self.assertFalse(result["ok"])
        self.assertIn("no setting", result["error"])

        self.assertEqual(FerroCADGui.settings("nothing-here"), [])
        self.assertFalse(FerroCADGui.setSetting("nothing-here", "x", "1")["ok"])

    def test_get_and_set_callables_override_the_group(self):
        state = {"level": 3.0}

        def get_level():
            return state["level"]

        def set_level(value):
            state["level"] = value

        self.register(
            "ferrocad",
            "Native",
            settings=[
                {
                    "name": "level",
                    "label": "Level",
                    "kind": "float",
                    "default": 1.0,
                    "min": 0.0,
                    "max": 10.0,
                    "get": get_level,
                    "set": set_level,
                }
            ],
        )

        row = self.rows()["level"]
        self.assertEqual(row["value"], "3.0")
        self.assertEqual(row["status"], "computed")
        self.assertTrue(row["editable"])

        result = FerroCADGui.setSetting(FEATURE, "level", "6")
        self.assertTrue(result["ok"], result)
        self.assertEqual(state["level"], 6.0)
        self.assertEqual(self.rows()["level"]["value"], "6.0")
        self.assertEqual(
            settings_group(FEATURE, "ferrocad").GetFloat("level", -1.0),
            -1.0,
            "a get/set setting must not also write the parameter group",
        )

        rejected = FerroCADGui.setSetting(FEATURE, "level", "11")
        self.assertFalse(rejected["ok"])
        self.assertEqual(state["level"], 6.0)

    def test_a_get_only_setting_is_read_only(self):
        self.register(
            "ferrocad",
            "Native",
            settings=[
                {
                    "name": "level",
                    "label": "Level",
                    "kind": "float",
                    "default": 1.0,
                    "get": lambda: 2.0,
                }
            ],
        )
        row = self.rows()["level"]
        self.assertFalse(row["editable"])
        result = FerroCADGui.setSetting(FEATURE, "level", "3")
        self.assertFalse(result["ok"])
        self.assertIn("read-only", result["error"])

    def test_an_unknown_kind_is_read_only_and_reported_once(self):
        self.register(
            "ferrocad",
            "Native",
            settings=[{"name": "weird", "kind": "gizmo", "default": "x"}],
        )
        row = self.rows()["weird"]
        self.assertEqual(row["kind"], "gizmo")
        self.assertEqual(row["value"], "x")
        self.assertFalse(row["editable"])

        result = FerroCADGui.setSetting(FEATURE, "weird", "y")
        self.assertFalse(result["ok"])
        self.assertIn("unsupported kind", result["error"])

    def test_a_malformed_schema_is_rejected_at_registration(self):
        with self.assertRaises(TypeError):
            self.register("bad", "Bad", settings="not-a-list")
        with self.assertRaises(TypeError):
            self.register("bad", "Bad", settings=[42])
        with self.assertRaises(ValueError):
            self.register("bad", "Bad", settings=[{"kind": "bool"}])
        with self.assertRaises(ValueError):
            self.register("bad", "Bad", settings=[{"name": "x"}])
        with self.assertRaises(ValueError):
            self.register("bad", "Bad", settings=[{"name": "x", "kind": "enum"}])
        with self.assertRaises(ValueError):
            self.register(
                "bad",
                "Bad",
                settings=[
                    {"name": "x", "kind": "bool"},
                    {"name": "x", "kind": "bool"},
                ],
            )
        with self.assertRaises(TypeError):
            self.register(
                "bad", "Bad", settings=[{"name": "x", "kind": "text", "get": 3}]
            )
        self.assertEqual(
            FerroCADGui.providers(FEATURE), [], "a rejected schema registers nothing"
        )

    def test_registering_without_settings_is_unchanged(self):
        self.register("plain", "Plain")
        manifest = FerroCADGui.providers(FEATURE)[0]
        self.assertEqual(
            set(manifest),
            {"feature", "backend", "label", "command", "priority", "active"},
        )

    # -- S1: the Settings tree ----------------------------------------------
    def test_model_tree_prepends_the_settings_root(self):
        tree = json.loads(ferrocad_shell.model_tree())
        self.assertNotIn(
            "Settings",
            [node["name"] for node in tree],
            "with no provider there is nothing to configure and no empty root",
        )

        self.register_both()

        tree = json.loads(ferrocad_shell.model_tree())
        self.assertEqual(tree[0]["name"], "Settings")
        self.assertEqual(tree[0]["label"], "Settings")
        self.assertEqual(
            tree[0]["objects"],
            [
                {
                    "name": "Settings/grid",
                    "label": "Grid",
                    "type": "FerroCAD::Settings",
                }
            ],
            "one node per feature, even with two backends",
        )

        FreeCAD.newDocument("Doc")
        tree = json.loads(ferrocad_shell.model_tree())
        self.assertEqual(tree[0]["name"], "Settings")
        self.assertEqual(tree[1]["name"], "Doc", "documents follow the settings root")

    def test_settings_properties_work_without_a_document_and_switch(self):
        self.register_both()
        close_documents()
        self.assertIsNone(FreeCAD.ActiveDocument)

        selector, settings = self.settings_rows()
        self.assertEqual(selector["name"], "provider")
        self.assertEqual(selector["label"], "Provider")
        self.assertEqual(selector["kind"], "enum")
        self.assertEqual(selector["choices"], ["Native", "Coin (WGSL)"])
        self.assertEqual(selector["value"], "Native")
        self.assertTrue(selector["editable"])
        self.assertEqual(settings["spacing"]["value"], "10 mm")
        self.assertIn("enabled", settings)

        switched = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "provider", "Coin (WGSL)")
        )
        self.assertTrue(switched["ok"], switched)
        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "coin")

        selector, settings = self.settings_rows()
        self.assertEqual(selector["value"], "Coin (WGSL)")
        self.assertEqual(settings["spacing"]["value"], "5 mm")
        self.assertIn("glow", settings)
        self.assertNotIn("enabled", settings)

        # The backend id resolves as well as the label.
        switched = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "provider", "ferrocad")
        )
        self.assertTrue(switched["ok"], switched)
        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "ferrocad")

    def test_settings_set_property_routes_to_the_active_provider(self):
        self.register("ferrocad", "Native", settings=NATIVE_SCHEMA)

        result = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "spacing", "7 mm")
        )
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["value"], "7 mm")
        self.assertEqual(result["view"], {"grid": True})
        self.assertEqual(self.refreshed, ["refresh"])

        _, settings = self.settings_rows()
        self.assertEqual(settings["spacing"]["value"], "7 mm")

        bad = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "spacing", "5000 mm")
        )
        self.assertFalse(bad["ok"])
        self.assertIn("at most 1000", bad["error"])

    def test_unknown_settings_paths_are_reported(self):
        self.register("ferrocad", "Native", settings=NATIVE_SCHEMA)

        self.assertEqual(
            json.loads(ferrocad_shell.properties("Settings/NoSuchFeature")), []
        )
        result = json.loads(
            ferrocad_shell.set_property("Settings/NoSuchFeature", "x", "y")
        )
        self.assertFalse(result["ok"])
        self.assertIn("Settings/NoSuchFeature", result["error"])

        result = json.loads(ferrocad_shell.set_property("Settings/Grid", "nope", "y"))
        self.assertFalse(result["ok"])
        self.assertIn("no setting", result["error"])

        result = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "provider", "nope")
        )
        self.assertFalse(result["ok"])
        self.assertIn("no provider", result["error"])

    def test_document_object_properties_are_unchanged(self):
        self.register("ferrocad", "Native", settings=NATIVE_SCHEMA)
        doc = FreeCAD.newDocument("Doc")
        FreeCAD.setActiveDocument("Doc")
        obj = doc.addObject("App::FeaturePython", "Params")
        obj.addProperty("App::PropertyLength", "Length", "Base", "Source length")
        obj.Length = FreeCAD.Units.Quantity("10 mm")
        doc.recompute()

        rows = json.loads(ferrocad_shell.properties("Params"))
        length = next(row for row in rows if row["name"] == "Length")
        self.assertEqual(length["value"], "10 mm")
        self.assertEqual(length["kind"], "quantity")
        self.assertTrue(length["editable"])

        # A Settings path beside a live document resolves to the settings rows,
        # and the document object still resolves afterwards (no shadowing).
        self.assertEqual(
            json.loads(ferrocad_shell.properties("Settings/Grid"))[0]["name"],
            "provider",
        )
        rows = json.loads(ferrocad_shell.properties("Params"))
        self.assertEqual(
            next(row for row in rows if row["name"] == "Length")["value"], "10 mm"
        )

        edited = json.loads(
            ferrocad_shell.set_property("Params", "Length", "20 mm")
        )
        self.assertTrue(edited["ok"], edited)
        self.assertAlmostEqual(obj.Length.Value, 20.0)

        # An unknown brief is not mistaken for a settings path.
        self.assertEqual(json.loads(ferrocad_shell.properties("Other")), [])

    def test_document_enum_and_colour_rows_carry_widget_metadata(self):
        """The property pane's rich widgets are driven by the row data alone.

        A document enumeration needs its own options for the select box, and a
        single colour must arrive as ``#rrggbb`` for the swatch and its palette.
        """
        doc = FreeCAD.newDocument("Doc")
        FreeCAD.setActiveDocument("Doc")
        obj = doc.addObject("App::FeaturePython", "Params")
        obj.addProperty("App::PropertyEnumeration", "Mode", "Base", "Mode")
        obj.Mode = ["Fast", "Accurate"]
        obj.addProperty("App::PropertyColor", "Color", "Base", "Colour")
        obj.Color = [(0.26, 0.55, 0.85, 0.5)]
        obj.addProperty("App::PropertyColorList", "Diffuse", "Base", "Many colours")
        obj.Diffuse = [(1.0, 0.0, 0.0, 1.0), (0.0, 1.0, 0.0, 1.0)]
        doc.recompute()

        rows = {
            row["name"]: row
            for row in json.loads(ferrocad_shell.properties("Params"))
        }
        self.assertEqual(rows["Mode"]["kind"], "enum")
        self.assertEqual(rows["Mode"]["choices"], ["Fast", "Accurate"])
        self.assertTrue(rows["Mode"]["editable"])

        self.assertEqual(rows["Color"]["kind"], "color")
        self.assertEqual(rows["Color"]["value"], "#428cd9")
        self.assertTrue(rows["Color"]["editable"])

        # A real colour *list* has no single value to swatch, so it stays read-only.
        self.assertEqual(rows["Diffuse"]["kind"], "")
        self.assertFalse(rows["Diffuse"]["editable"])

        # Both widgets commit through the normal property path.
        picked = json.loads(ferrocad_shell.set_property("Params", "Mode", "Accurate"))
        self.assertTrue(picked["ok"], picked)
        self.assertEqual(obj.Mode, "Accurate")

        repainted = json.loads(ferrocad_shell.set_property("Params", "Color", "#ef4444"))
        self.assertTrue(repainted["ok"], repainted)
        self.assertEqual(
            [round(channel, 4) for channel in obj.Color[0]],
            [0.9373, 0.2667, 0.2667, 0.5],
        )

        rejected = json.loads(ferrocad_shell.set_property("Params", "Color", "nope"))
        self.assertFalse(rejected["ok"])
        self.assertIn("invalid colour", rejected["error"])
        # The failed edit left the stored colour (and its alpha) alone.
        self.assertEqual(
            [round(channel, 4) for channel in obj.Color[0]],
            [0.9373, 0.2667, 0.2667, 0.5],
        )

        # A multi-colour list refuses a single-colour edit instead of truncating it.
        refused = json.loads(ferrocad_shell.set_property("Params", "Diffuse", "#123456"))
        self.assertFalse(refused["ok"])
        self.assertIn("single colour", refused["error"])
        self.assertEqual(len(obj.Diffuse), 2)


if __name__ == "__main__":
    unittest.main()
