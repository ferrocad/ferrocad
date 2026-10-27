"""Guard the boundary between the upstream and FerroCAD-only namespaces.

``docs/developer/architecture/api-namespaces.md`` makes the split checkable:

* ``FreeCAD``/``FreeCADGui`` carry the **upstream** surface, name for name;
* ``FerroCAD``/``FerroCADGui`` carry everything FerroCAD adds;
* there are no aliases, so a name lives in exactly one of the two.

The same split covers resource keys: a FerroCAD extension to `GetResources`
carries the `FerroCAD_` prefix (`FerroCAD_Surface`, `FerroCAD_Parent`), and a
bare `Surface`/`Parent` would be a name in the upstream namespace.

This module asserts the boundary in both directions, walks each FerroCAD
``__all__`` so a future extension cannot silently leak upstream, audits the
resource keys of every registered command and of every `mods/*/InitGui.py`, and
then proves a command still round-trips through the shell so the move did not
break the host bridge.
"""

import json
import re
import unittest
from pathlib import Path

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the built-in commands)
import ferrocad_toy  # noqa: F401  (registers the toy commands and hooks)

#: The repository root and the mods the shell loads at boot.
ROOT = Path(__file__).resolve().parents[1]
MODS_DIR = ROOT / "mods"

#: Resource keys that are FerroCAD extensions, and their bare (pre-move) spelling.
EXTENSION_RESOURCE_KEYS = ("FerroCAD_Surface", "FerroCAD_Parent")
BARE_RESOURCE_KEYS = ("Surface", "Parent")

#: A string literal used as a dict key, in either quote style.
DICT_KEY = re.compile(r"""["'](Surface|Parent)["']\s*:""")

#: Names that live only on ``FerroCAD`` (moved from ``FreeCAD`` on 2026-10-22,
#: or added there since).
FERROCAD_ONLY_FREECAD = (
    "Grid",
    "Gizmo",
    "display_mesh",
    "register_display_provider",
    "register_interaction_provider",
    "interaction",
    "set_parameter_file",
    "parameter_file",
)

#: Names that live only on ``FerroCADGui`` (moved from ``FreeCADGui``).
FERROCAD_ONLY_FREECADGUI = (
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
            (FreeCAD, FERROCAD_ONLY_FREECAD),
            (FreeCADGui, FERROCAD_ONLY_FREECADGUI),
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

    def test_every_ferrocad_only_name_resolves_on_the_ferrocad_namespaces(self):
        for namespace, names in (
            (FerroCAD, FERROCAD_ONLY_FREECAD),
            (FerroCADGui, FERROCAD_ONLY_FREECADGUI),
        ):
            for name in names:
                with self.subTest(namespace=namespace.__name__, name=name):
                    self.assertIsNotNone(getattr(namespace, name))

        # ``FerroCAD.Grid`` is the one settings singleton.
        self.assertIs(FerroCAD.Grid, FerroCAD.Grid)

    def test_the_ferrocad_exports_do_not_leak_upstream_and_resolve(self):
        """``__all__`` is the forward-looking guard for the whole boundary.

        The explicit tuples above catch the names we know about today. This test
        instead walks each FerroCAD ``__all__``, so an extension added there
        tomorrow is covered without editing the tuples: it may not appear in
        ``dir(FreeCAD)``/``dir(FreeCADGui)`` (or resolve through a lazy
        ``__getattr__``), and every exported name must resolve.
        """
        for upstream, namespace in (
            (FreeCAD, FerroCAD),
            (FreeCADGui, FerroCADGui),
        ):
            with self.subTest(namespace=namespace.__name__):
                exported = list(namespace.__all__)
                self.assertTrue(
                    exported,
                    "%s.__all__ must name its surface" % namespace.__name__,
                )
                self.assertEqual(
                    len(set(exported)),
                    len(exported),
                    "%s.__all__ repeats a name" % namespace.__name__,
                )
                for name in exported:
                    with self.subTest(namespace=namespace.__name__, name=name):
                        self.assertNotIn(
                            name,
                            dir(upstream),
                            "%s.__all__ exports %r, but %s also names it"
                            % (namespace.__name__, name, upstream.__name__),
                        )
                        # ``dir`` alone would miss a lazy ``__getattr__``.
                        self.assertFalse(
                            hasattr(upstream, name),
                            "%s hasattr(%r) must be False as well"
                            % (upstream.__name__, name),
                        )
                        try:
                            getattr(namespace, name)
                        except AttributeError:
                            self.fail(
                                "%s.__all__ names %r, but it does not resolve"
                                % (namespace.__name__, name)
                            )

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

    def test_registered_commands_namespace_their_extension_resources(self):
        """Every `GetResources()` uses the prefixed keys, never the bare ones.

        The commands come from `ferrocad_shell` and `ferrocad_toy`, so this
        covers the host's own command surface as well as a mod that ships in
        this tree. A raising `GetResources` is treated as `{}`, exactly as the
        facade does, because the shell never sees those keys either.
        """
        self.assertTrue(
            FreeCADGui._COMMANDS,
            "ferrocad_shell/ferrocad_toy registered no commands to audit",
        )
        audited = 0
        annotated = 0
        for name, command in sorted(FreeCADGui._COMMANDS.items()):
            get = getattr(command, "GetResources", None)
            if get is None:
                continue
            try:
                resources = get()
            except Exception:  # noqa: BLE001 - the facade reports and returns {}
                continue
            if not isinstance(resources, dict):
                continue
            audited += 1
            if "FerroCAD_Surface" in resources:
                annotated += 1
            with self.subTest(command=name):
                for bare in BARE_RESOURCE_KEYS:
                    self.assertNotIn(
                        bare,
                        resources,
                        "%s declares the bare resource key %r; use %r"
                        % (name, bare, "FerroCAD_" + bare),
                    )
                for prefixed in EXTENSION_RESOURCE_KEYS:
                    if prefixed in resources:
                        self.assertTrue(
                            isinstance(resources[prefixed], str),
                            "%s: %s must be a string" % (name, prefixed),
                        )
        self.assertGreater(audited, 0, "no command exposed a GetResources() to audit")
        self.assertGreater(
            annotated,
            0,
            "no command declares FerroCAD_Surface; the prefixed surface key is "
            "the extension's only spelling",
        )

    def test_mod_entry_points_never_use_bare_resource_keys(self):
        """The mods are not imported by this suite, so scan their sources.

        `mods/*/InitGui.py` is the only place a mod declares `GetResources`
        metadata, and the loader may not run in every test environment. A bare
        `"Surface"`/`"Parent"` key here would land in the upstream namespace,
        which is exactly the drift this boundary forbids.
        """
        entry_points = sorted(MODS_DIR.glob("*/InitGui.py"))
        self.assertTrue(
            entry_points,
            "no mod entry points found under %s" % MODS_DIR,
        )
        for path in entry_points:
            with self.subTest(mod=path.parent.name):
                source = path.read_text(encoding="utf-8")
                bare = sorted(set(DICT_KEY.findall(source)))
                self.assertEqual(
                    bare,
                    [],
                    "%s declares bare resource key(s) %s; use FerroCAD_Surface/"
                    "FerroCAD_Parent"
                    % (path.relative_to(ROOT), ", ".join(bare)),
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
