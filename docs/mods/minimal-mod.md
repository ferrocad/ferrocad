# A minimal mod

Status: guide (2026-10-22).

This is a complete mod: one command that creates a box, and one display provider
that draws it. It uses only the current facade and needs no GPU.

## Directory

```text
mods/
  MinimalBox/
    Init.py
    InitGui.py
```

`Init.py` is the non-GUI hook. This mod has nothing to register there:

```python
"""Non-GUI startup hook. This mod registers nothing here."""
```

## `InitGui.py`

```python
"""A minimal FerroCAD mod: one command and one display provider."""

import FreeCAD
import FreeCADGui

COMMAND = "Minimal_AddBox"
SIZE = 10.0

_CORNERS = (
    (-0.5, -0.5, 0.0), (0.5, -0.5, 0.0), (0.5, 0.5, 0.0), (-0.5, 0.5, 0.0),
    (-0.5, -0.5, 1.0), (0.5, -0.5, 1.0), (0.5, 0.5, 1.0), (-0.5, 0.5, 1.0),
)
_FACES = (
    (0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
    (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7),
)


def _box_mesh(obj):
    """Draw an object with a Length property; return None for anything else."""
    length = getattr(obj, "Length", None)
    if length is None:
        return None
    try:
        size = float(length)
        placement = getattr(obj, "Placement", None)
        corners = [
            FreeCAD.Vector(x * size, y * size, z * size)
            for x, y, z in _CORNERS
        ]
        if placement is not None:
            corners = [placement.multVec(corner) for corner in corners]
        positions, triangles, edges = [], [], []
        for face in _FACES:
            base = len(positions)
            for index in face:
                corner = corners[index]
                positions.append((corner.x, corner.y, corner.z))
            triangles.extend((base, base + 1, base + 2, base, base + 2, base + 3))
            edges.extend(
                (base, base + 1, base + 1, base + 2,
                 base + 2, base + 3, base + 3, base)
            )
        return {
            "positions": positions,
            "triangles": triangles,
            "triangle_parts": [0] * (len(triangles) // 3),
            "edges": edges,
            "segment_parts": [0] * (len(edges) // 2),
            "vertices": [],
            "vertex_parts": [],
            "parts": [],
            "color": [0.85, 0.55, 0.20, 1.0],
            "display": "shaded_edges",
        }
    except Exception:
        return None


def _add_box():
    doc = FreeCAD.ActiveDocument
    if doc is None:
        return {"status": "open a document first"}
    doc.openTransaction("Add box")
    try:
        box = doc.addObject("App::FeaturePython", "Box")
        box.addProperty("App::PropertyLength", "Length", "Box", "Edge length")
        box.addProperty("App::PropertyPlacement", "Placement", "Box", "Placement")
        box.Length = SIZE
        box.Placement = FreeCAD.Placement()
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"status": "add box failed: %s" % exc}
    return {"status": "created %s" % box.Name}


class AddBoxCommand:
    def GetResources(self):
        return {
            "MenuText": "Add Minimal Box",
            "ToolTip": "Add a 10 mm box",
            "Group": "Minimal",
            "CmdType": "ForEdit",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self):
        return _add_box()


FreeCADGui.removeCommand(COMMAND)  # make a reload safe
FreeCADGui.addCommand(COMMAND, AddBoxCommand())
FreeCAD.register_display_provider(_box_mesh)
```

## What each piece does

| Piece | Contract |
| --- | --- |
| `AddBoxCommand.GetResources` | the label, tooltip, group and `ForEdit` metadata the actions bar reads |
| `IsActive` | enabled only when a document is open |
| `Activated` | one named transaction; returns `{"status": ...}` |
| `_box_mesh` | returns a mesh for an object with `Length`, `None` for everything else |
| `positions` | 24 entries: six quads, four corners each, already placed by `Placement` |
| `triangles` | 12 triangles (two per face) |
| `edges` | 24 segments (four per face) |
| `triangle_parts` / `segment_parts` | one untagged part (`0`), so a click selects the whole object |
| `color` | RGBA in `0..1`; the shell uses it for the surface and edges |
| `display` | `"shaded_edges"`; `"shaded"` and `"wireframe"` are the other modes |

The provider claims **any** object with a `Length` property. That is deliberate
for a minimal example; a real mod should use a private marker property or a
proxy type so it does not draw someone else's object. In the app's sample
document, `Params` also has a `Length`, so this provider claims it too.

## Run it headlessly

Put the directory at `mods/MinimalBox/`, then:

```python
import importlib.util
import json
import pathlib

import FreeCAD
import FreeCADGui
import ferrocad_shell

path = pathlib.Path("mods/MinimalBox/InitGui.py").resolve()
FreeCADGui.removeCommand("Minimal_AddBox")
spec = importlib.util.spec_from_file_location("MinimalBox_InitGui", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

print("registered:", "Minimal_AddBox" in FreeCADGui.listCommands())

FreeCAD.newDocument("Minimal")
print(json.loads(ferrocad_shell.run_command("Minimal_AddBox")))

mesh = json.loads(ferrocad_shell.stage_meshes())[0]
print(
    mesh["object"],
    len(mesh["positions"]),
    len(mesh["triangles"]) // 3,
    mesh["color"],
    mesh["display"],
)

FreeCAD.closeDocument("Minimal")
FreeCADGui.removeCommand("Minimal_AddBox")
```

Output:

```text
registered: True
{'ok': True, 'status': 'created Box'}
Box 24 12 [0.85, 0.55, 0.2, 1.0] shaded_edges
```

The app loads the same file at boot from any configured mods directory. There is
no registration step and no build step.

## Extend it

- **Ask for the size.** Return a dialog spec from `Activated` with a
  `quantity` field named `Length`, and create the box in `on_accept`
  ([`commands.md`](commands.md)).
- **Pick faces.** Give each face a distinct `triangle_parts` id and fill `parts`
  with names such as `"Face1"`; a pick then reports `Face1`
  ([`display-and-interaction.md`](display-and-interaction.md)).
- **Let the user drag it.** Register an interaction provider for a manipulator
  id and write the new `Length`/`Placement` in one transaction.
- **Add a backend.** If the mod implements a shared capability, declare it with
  `FreeCADGui.registerProvider` instead of keeping a private on/off flag
  ([`providers.md`](providers.md)).

## Next

- [`mod-anatomy.md`](mod-anatomy.md) — the loader, lifecycle and isolation rules.
- [`testing-a-mod.md`](testing-a-mod.md) — turn the run above into a test.
