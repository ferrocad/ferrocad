# Testing a mod

Status: guide (2026-10-28).

Almost everything a mod does can be driven without a window or a GPU. The
application's own tests do exactly that, by executing `InitGui.py` and calling
the same functions the shell calls.

## Choose an extension set

A test run names an **extension set**: the workbench modules it is **provided**.
`--extension` names what is provided, and the runner installs an import blocker
for everything else, so the set is enforced rather than inferred from whatever
`.so` files happen to be in `python/`.

| Extension set | Provides | What it is |
| --- | --- | --- |
| `base` | no workbench module (`ferrocad` only) | the primary extension, exactly what CI's `headless` job builds. `ferrocad_combined`, `Part`, `Mesh` and `MeshPart` are unimportable. |
| `workbenches` | `Part`, `Mesh`, `MeshPart` (with the `ferrocad_combined` packaging) | every workbench module, for tests that need the kernel. |

Run a mod test against `base`, the way CI will:

```sh
python3 tools/run_tests.py --check                  # static contract check, no build
python3 tools/run_tests.py --extension base         # 41 modules, 673 tests, 8 skipped
python3 tools/run_tests.py --extension workbenches  # 45 modules, 704 tests, 3 skipped
```

`--extension base` makes `import Part` fail even on a developer checkout that has
`ferrocad_combined.abi3.so`, so a test that needs the kernel cannot pass locally
and fail only in CI. `workbenches` runs every suite and needs OCCT; use it only
when you are testing a kernel-backed mod. `ferrocad_combined` is packaging rather
than a workbench, so it is not an `--extension` value: naming `Part`, `Mesh` or
`MeshPart` provides it implicitly.

The runner enforces one invariant: **pass or skip, never error**. A test that
needs something the set does not provide is a failure, not a silent skip.

## The contract

`contracts/extension-sets.json` classifies every test module as `base` or
`workbenches`. A module that silently acquires a workbench dependency changes
that file, so the drift shows up in review. `python3 tools/run_tests.py --check`
verifies the contract names every module exactly once and that its `presets`
match `tools/extensions.py`; it imports nothing and takes about 0.06 s.

For a new test, add the file and run
`python3 tools/run_tests.py --write-contract`. A test that needs `Part`, `Mesh`,
`MeshPart`, or a module that imports one of them at module scope belongs in a
module the contract classifies as `workbenches`; import it directly there. Do not
add a `HAVE_PART` guard or a `skipUnless` decorator: a guard turns the dependency
into a skip, hides it from the contract, and is what made plain
`unittest discover` unreliable in the first place.

The rationale and the edition-side rules are in
[`../developer/testing.md`](../developer/testing.md).

## What needs no GPU

| You can test | How |
| --- | --- |
| import-time registration | load `InitGui.py` by path, then inspect `FreeCADGui.listCommands()` |
| command resources and enablement | `FreeCADGui._command_list()`, `FreeCADGui.listCommands()` |
| running a command | `ferrocad_shell.run_command(id)` or `FreeCADGui.runCommand(id)` |
| dialogs, accept and cancel | `ferrocad_shell.dialog_accept` / `dialog_cancel` |
| provider selection and switching | `registerProvider`, `activeProvider`, `setActiveProvider`, `refreshProviders` |
| selection-dependent `IsActive` | `ferrocad_shell.set_selection(...)` |
| view-dependent `IsChecked` | `ferrocad_shell.set_view_state(...)` |
| display meshes | `FerroCAD.display_mesh(obj)` or `ferrocad_shell.stage_meshes()` |
| interaction commits | `ferrocad_shell.interaction(...)` |
| overlay registration | needs a `pivy.coin` facade; the Coin tests install a stub |
| shader compilation, drawing, the window | **needs a GPU and a viewport** |

## Load the mod by path

Do not depend on `sys.path` order. Load the file directly:

```python
import importlib.util
import pathlib

import FreeCADGui

MOD = pathlib.Path("mods/MinimalBox/InitGui.py").resolve()

def load_mod():
    FreeCADGui.removeCommand("Minimal_AddBox")   # reload-safe
    spec = importlib.util.spec_from_file_location(
        "MinimalBox_InitGui", MOD
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
```

This is the pattern in [`tests/test_grid_overlay_fork.py`](../../tests/test_grid_overlay_fork.py):

```python
import sys

before = set(sys.modules)
FreeCADGui.removeCommand(COMMAND)
spec = importlib.util.spec_from_file_location("GridOverlay_InitGui", MOD_PATH)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
```

It also records the modules the mod imported, so a test can assert that loading
the mod did not pull in Qt or Coin.

If the mod is already registered in this interpreter, `removeCommand` is what
avoids the registry's duplicate-id `ValueError`. A provider re-registration is
safe; a display/interaction provider is not removable, so load that module once
per process.

## A test class

```python
import importlib.util
import json
import pathlib
import unittest

import FreeCAD
import FreeCADGui
import ferrocad_shell

MOD = pathlib.Path("mods/MinimalBox/InitGui.py").resolve()
COMMAND = "Minimal_AddBox"

class MinimalModTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        FreeCADGui.removeCommand(COMMAND)
        spec = importlib.util.spec_from_file_location("MinimalBox_InitGui", MOD)
        cls.mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.mod)

    @classmethod
    def tearDownClass(cls):
        FreeCADGui.removeCommand(COMMAND)

    def setUp(self):
        for name in list(FreeCAD.listDocuments()):
            FreeCAD.closeDocument(name)
        self.doc = FreeCAD.newDocument("Test")

    def tearDown(self):
        FreeCADGui._cancel_all()
        FreeCAD.closeDocument(self.doc.Name)

    def test_the_command_is_registered(self):
        self.assertIn(COMMAND, FreeCADGui.listCommands())

    def test_the_command_creates_a_displayed_box(self):
        result = json.loads(ferrocad_shell.run_command(COMMAND))
        self.assertTrue(result["ok"], result)

        meshes = json.loads(ferrocad_shell.stage_meshes())
        self.assertEqual(meshes[0]["object"], "Box")
        self.assertEqual(len(meshes[0]["triangles"]) // 3, 12)
```

The class loads the module once, so the display provider is registered once.
`tearDownClass` removes the command; the provider stays for the process, which
is the best the facade allows.

## Push the shell's state

The shell owns selection and presentation state. In a test, push them yourself.

```python
import json
import FreeCADGui
import FerroCADGui
import ferrocad_shell
import ferrocad_toy

ferrocad_toy.bootstrap("State")
ferrocad_shell.set_selection(
    json.dumps([{"object": "Slab", "elements": ["Face1"]}])
)
assert FreeCADGui.Selection.getSelection()[0].Name == "Slab"
assert FreeCADGui.Selection.getSelectionEx()[0][1] == ["Face1"]

ferrocad_shell.set_view_state(
    json.dumps({"grid": True, "projection": "ortho"})
)
assert FerroCADGui.ViewState.grid is True
assert FerroCADGui.ViewState.projection == "ortho"
```

- `set_selection` payload: `[{"object": name, "elements": [...]}, ...]`. Objects
  that are no longer in the document are pruned when the snapshot is resolved.
- `set_view_state` payload: any of `grid`, `axis_cross`, `projection`,
  `grid_minor`, `grid_major`, `grid_radius`. Missing keys use defaults
  (`grid=False`, `axis_cross=True`, `projection="persp"`, `grid_minor=10.0`,
  `grid_major=100.0`, `grid_radius=None`).

Clear the selection at the end with `ferrocad_shell.set_selection("[]")` so the
next test does not inherit it.

## Run commands and dialogs

| Call | Returns |
| --- | --- |
| `FreeCADGui.runCommand(id)` | the raw result of `Activated`; raises `KeyError` for an unknown id |
| `FreeCADGui._run(id)` | the shell payload `{ok, status?, value?, view?, tool?, dialog?, token?}`; never raises |
| `ferrocad_shell.run_command(id)` | the shell payload as a JSON string |

```python
import json
import ferrocad_shell

run = json.loads(ferrocad_shell.run_command("View_GridSettings"))
assert "token" in run

accepted = json.loads(ferrocad_shell.dialog_accept(json.dumps({
    "token": run["token"],
    "values": {"enabled": True, "color": "#00ff00"},
})))
assert accepted["ok"], accepted
```

`View_GridSettings` is used here because it is already registered; a mod command
with an `on_accept` behaves identically.

Cancel instead:

```python
import json
import ferrocad_shell

run = json.loads(ferrocad_shell.run_command("View_GridSettings"))
assert json.loads(ferrocad_shell.dialog_cancel(run["token"])) == {"ok": True}
```

`tests/test_grid_overlay_fork.py` uses `FreeCADGui._run` and `FreeCADGui._accept`
directly; those are the private half of the same protocol.

## Providers

[`tests/test_mod_providers.py`](../../tests/test_mod_providers.py) is the
reference. Its pattern:

- load each mod once, then call the module's `register_provider()` in `setUp`
  because the module object is cached and the registry may have been cleared;
- clear the applied-backend map and the persisted choices between cases;
- switch with `setActiveProvider` and assert the callbacks ran exactly once;
- `unregisterProvider` every backend in `tearDown`.

```python
import FreeCAD
import FreeCADGui

FreeCADGui._PROVIDER_APPLIED.clear()
FreeCAD.ParamGet(
    "User parameter:BaseApp/Preferences/Mods"
).GetGroup("Providers").Clear()
```

`_PROVIDER_APPLIED` is private; it is acceptable in tests that need to forget
which backend was applied while keeping the registrations.

```python
import FerroCADGui

FerroCADGui.registerProvider("demo", "a", "A", priority=10)
FerroCADGui.registerProvider("demo", "b", "B", priority=0)
assert FerroCADGui.activeProvider("demo") == "a"

result = FerroCADGui.setActiveProvider("demo", "b")
assert result["previous"] == "a"
assert result["ok"]

for entry in FerroCADGui.providers("demo"):
    FerroCADGui.unregisterProvider("demo", entry["backend"])
```

`tests/test_mod_providers.py` also asserts that callback errors are reported
under `errors` and do not break the registry.

## Display and interaction

```python
import json
import FreeCAD
import FerroCAD
import ferrocad_shell
import ferrocad_toy

ferrocad_toy.bootstrap("Display")
ferrocad_toy.makeSlab(name="Box")

mesh = FerroCAD.display_mesh(FreeCAD.ActiveDocument.getObject("Box"))
assert len(mesh["triangles"]) // 3 == 12

response = json.loads(
    ferrocad_shell.interaction(
        "Slab", "toy_height", "set", "", json.dumps([42.0])
    )
)
assert response["ok"]
```

- `stage_meshes()` needs an active document; it returns `[]` otherwise.
- A mesh reaches the viewport only if a provider claims the object and
  `positions` is not empty.
- `interaction()` returns `{ok: false, error: "no module handles ..."}` when no
  provider claims the request.

## Cleanup checklist

A test that leaves global state behind makes the next test lie. At the end of a
case (or class):

- `FreeCADGui.removeCommand(id)` for every command the test registered.
- `FerroCADGui.unregisterProvider(feature, backend)` for every provider.
- `FreeCADGui._cancel_all()` to drop open dialog tokens.
- `FreeCADGui._PROVIDER_APPLIED.clear()` and clear the `Providers` preference
  group if the test changed the provider selection.
- `FreeCAD.closeDocument(name)` for every document.
- `FerroCAD.Grid.reset()` if the test changed grid settings.
- `ferrocad_shell.set_selection("[]")` if the test pushed a selection.
- `FerroCADGui.removeOverlay(node)` if the test registered an overlay.

You cannot unregister a display or interaction provider. Keep those modules
loaded once, or accept that each re-execution adds another provider.

## What still needs a window

- Overlay extraction needs `pivy.coin` with `addOverlay`/`removeOverlay`.
  [`tests/test_grid_overlay_coin.py`](../../tests/test_grid_overlay_coin.py)
  installs a fake `pivy.coin` and asserts the bridge calls; the real draw is
  manual.
- Shader compilation, the viewport, and anything that measures pixels need a
  GPU and `cargo run -p ferrocad_gpui`.
- The headless facade has `FreeCAD.GuiUp == 0`; `FerroCADGui.ViewState` is still
  writable by `set_view_state`, and `FerroCAD.Grid.snapshot()` reads it only when
  `GuiUp` is truthy.
