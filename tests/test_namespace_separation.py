"""Guard the boundary between the upstream and FerroCAD-only namespaces.

``docs/architecture/api-namespaces.md`` makes the split checkable:

* ``FreeCAD``/``FreeCADGui`` carry the **upstream** surface, name for name;
* ``FerroCAD``/``FerroCADGui`` carry everything FerroCAD adds;
* there are no aliases, so a name lives in exactly one of the two.

This module asserts that in both directions, and then proves a command still
round-trips through the shell so the move did not break the host bridge.
"""

import json
import unittest

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the built-in commands)
import ferrocad_toy  # noqa: F401  (registers the toy commands and hooks)

#: Names that moved ``FreeCAD`` -> ``FerroCAD``.
MOVED_FROM_FREECAD = (
    "Grid",
    "display_mesh",
    "register_display_provider",
    "register_interaction_provider",
    "interaction",
)

#: Names that moved ``FreeCADGui`` -> ``FerroCADGui``.
MOVED_FROM_FREECADGUI = (
    "registerProvider",
    "unregisterProvider",
    "providers",
    "activeProvider",
    "setActiveProvider",
    "refreshProviders",
    "settings",
    "setSetting",
    "addOverlay",
    "removeOverlay",
    "ViewState",
    "PROVIDER_PREFERENCES",
)

#: The upstream-shaped ``FreeCAD`` surface that must stay put.
UPSTREAM_FREECAD = (
    "newDocument",
    "openDocument",
    "closeDocument",
    "getDocument",
    "listDocuments",
    "activeDocument",
    "ActiveDocument",
    "ParamGet",
    "ParameterGrp",
    "Quantity",
    "Placement",
    "Vector",
    "backend",
)

#: The upstream-shaped ``FreeCADGui`` surface (including the private bridge).
UPSTREAM_FREECADGUI = (
    "addCommand",
    "removeCommand",
    "listCommands",
    "runCommand",
    "Selection",
    "_command_list",
    "_run",
    "_accept",
    "_cancel",
    "_cancel_all",
    "_register_dialog",
)


class NamespaceSeparationTests(unittest.TestCase):
    def test_the_upstream_namespaces_do_not_expose_the_extensions(self):
        for namespace, names in (
            (FreeCAD, MOVED_FROM_FREECAD),
            (FreeCADGui, MOVED_FROM_FREECADGUI),
        ):
            with self.subTest(namespace=namespace.__name__):
                present = sorted(name for name in names if name in dir(namespace))
                self.assertEqual(
                    present,
                    [],
                    "%s still exposes FerroCAD-only names" % namespace.__name__,
                )
                # ``dir`` alone would miss a lazy ``__getattr__``.
                for name in names:
                    self.assertFalse(
                        hasattr(namespace, name),
                        "%s hasattr(%r) must be False as well"
                        % (namespace.__name__, name),
                    )

    def test_every_moved_name_resolves_on_the_ferrocad_namespaces(self):
        for namespace, names in (
            (FerroCAD, MOVED_FROM_FREECAD),
            (FerroCADGui, MOVED_FROM_FREECADGUI),
        ):
            for name in names:
                with self.subTest(namespace=namespace.__name__, name=name):
                    self.assertIsNotNone(getattr(namespace, name))

        # ``FerroCAD.Grid`` is the one settings singleton.
        self.assertIs(FerroCAD.Grid, FerroCAD.Grid)

    def test_the_upstream_shaped_names_still_resolve(self):
        for namespace, names in (
            (FreeCAD, UPSTREAM_FREECAD),
            (FreeCADGui, UPSTREAM_FREECADGUI),
        ):
            for name in names:
                with self.subTest(namespace=namespace.__name__, name=name):
                    try:
                        getattr(namespace, name)
                    except AttributeError:
                        self.fail(
                            "%s lost the upstream-shaped name %r"
                            % (namespace.__name__, name)
                        )

    def test_a_document_command_round_trips(self):
        documents = set(FreeCAD.listDocuments())
        namespace = "_NamespaceSeparation"
        FerroCAD.Grid.reset()
        try:
            payload = json.loads(ferrocad_toy.bootstrap(namespace))
            self.assertEqual(payload["document"], namespace)
            doc = FreeCAD.getDocument(namespace)
            self.assertIsNotNone(doc)

            # A registered command still runs through the bridge, hosts its
            # dialog, and accepts typed values into one transaction.
            before = len(doc.Objects)
            run = json.loads(ferrocad_shell.run_command("Toy_MakeSlab"))
            self.assertTrue(run["ok"], run)
            self.assertIn("token", run)
            accepted = json.loads(
                ferrocad_shell.dialog_accept(
                    json.dumps(
                        {
                            "token": run["token"],
                            "values": {"Width": 10.0, "Depth": 10.0, "Height": 5.0},
                        }
                    )
                )
            )
            self.assertTrue(accepted["ok"], accepted)
            self.assertEqual(len(doc.Objects), before + 1)

            # The grid toggle reads and writes ``FerroCAD.Grid``, and the shell's
            # JSON keeps the neutral ``surface``/``parent`` field names.
            toggled = json.loads(ferrocad_shell.run_command("App_ToggleGrid"))
            self.assertTrue(toggled["ok"], toggled)
            self.assertTrue(toggled["view"]["grid"])
            self.assertTrue(FerroCAD.Grid.enabled)

            commands = {
                entry["id"]: entry for entry in json.loads(ferrocad_shell.commands())
            }
            self.assertEqual(commands["App_ToggleGrid"]["surface"], "grid")
            self.assertEqual(
                commands["View_GridSettings"]["parent"], "App_ToggleGrid"
            )
        finally:
            FreeCADGui._cancel_all()
            for name in list(FreeCAD.listDocuments()):
                if name not in documents:
                    FreeCAD.closeDocument(name)
            FerroCAD.Grid.reset()


if __name__ == "__main__":
    unittest.main()
