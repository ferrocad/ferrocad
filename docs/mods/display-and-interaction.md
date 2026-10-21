# Display and interaction providers

Status: guide (2026-10-22).

Two module hooks let a mod supply what the shell needs without the shell naming
the mod:

| Hook | Registered with | Asked when |
| --- | --- | --- |
| display | `FreeCAD.register_display_provider(provider)` | the shell syncs the document's meshes |
| interaction | `FreeCAD.register_interaction_provider(provider)` | the user commits a manipulator edit |

Both are registered once, at import time. Both use the same protocol: **return
`None` for a request you do not own**, and the next provider is asked.

```python
FreeCAD.register_display_provider(my_display)
FreeCAD.register_interaction_provider(my_interaction)
```

Registration is idempotent only for the same function object. There is no
unregister, so a process should load a given display/interaction mod once.

## Display

### The provider

```python
def my_display(obj):
    """The display mesh for ``obj``, or ``None`` if this mod does not own it."""
    if not hasattr(obj, "MyGeometry"):
        return None
    return {
        "positions": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        "triangles": [0, 1, 2],
        "triangle_parts": [0],
        "edges": [0, 1, 1, 2, 2, 0],
        "segment_parts": [0, 0, 0],
        "vertices": [],
        "vertex_parts": [],
        "parts": [],
        "color": [0.26, 0.55, 0.85, 1.0],
        "display": "shaded_edges",
    }
```

`FreeCAD.display_mesh(obj)` returns the first non-`None` result. It does **not**
catch exceptions; a provider that raises breaks the mesh sync. Convert your own
geometry in a `try/except` and return `None` on failure (the toy does this).

### The mesh fields

The dict is the `ferrocad.geom.Tessellation` shape plus two presentation fields.

| Field | Shape | Meaning |
| --- | --- | --- |
| `positions` | `[[x, y, z], ...]` | vertex positions used as given; the shell applies no object transform, so bake `Placement` in |
| `normals` | `[[x, y, z], ...]` | optional; used only when its length equals `positions`; otherwise the renderer flat-shades |
| `triangles` | `[i, j, k, ...]` | flat indices, three per triangle |
| `triangle_parts` | `[part_id, ...]` | one part id per triangle; a missing entry means part `0` |
| `edges` | `[i, j, ...]` | flat line indices, two per segment; one run per topological edge |
| `segment_parts` | `[part_id, ...]` | one part id per segment |
| `vertices` | `[position_index, ...]` | the position indices of the topological vertices |
| `vertex_parts` | `[part_id, ...]` | one part id per entry in `vertices` |
| `parts` | `["Face1", "Edge1", ...]` | part id to sub-element name; the id is the index |
| `color` | `[r, g, b, a]` | optional RGBA in linear `0..1`; applied to the surface and the edges |
| `display` | `"shaded"`, `"wireframe"`, `"shaded_edges"` | optional draw mode; absent or unknown means `"shaded_edges"` |

Notes:

- `parts` is what turns a pick into `Face6`/`Edge3`. It is a name table, not a
  per-vertex array; the per-primitive arrays carry the ids.
- The shell flattens `triangle_parts`/`segment_parts`/`vertex_parts` into a
  per-position part id, so a position should belong to one part.
- `vertices`/`vertex_parts` are topology data. The current display path records
  them for picking but does not draw points from them (`show_vertices` is off in
  `DisplaySync`).
- `color` and `display` are presentation only; they never affect the document.

### How the shell consumes it

`ferrocad_shell.stage_meshes()`:

1. reads `FreeCAD.ActiveDocument`;
2. skips an object that a feature consumes through its `Base` link;
3. calls `FreeCAD.display_mesh(obj)`;
4. adds `object` (the internal name), `label`, and `entity` (the object's
   1-based position in `doc.Objects`);
5. returns a JSON list.

A mesh with empty `positions` is skipped when the shell builds the stage. An
empty list means "nothing to show", and the shell keeps its sample scene.

The Rust side (`ferrocad_gpui::display::DisplaySync::build`) turns each entry
into a `ShapeItem`, applies `display` as the draw mode and `color` as the
surface/edge colour, and keeps the `entity -> object` map a pick needs.

```python
import json
import ferrocad_shell
import ferrocad_toy

ferrocad_toy.bootstrap("Display")
mesh = json.loads(ferrocad_shell.stage_meshes())[0]
print(mesh["object"], len(mesh["positions"]), len(mesh["triangles"]) // 3)
```

Output:

```text
Slab 24 12
```

## Interaction

### The provider

```python
def my_interaction(obj, manipulator, action, element, values):
    """Apply a manipulator commit, or return ``None`` if it is not ours."""
    if manipulator != "my_height" or action != "set":
        return None
    doc = obj.Document
    doc.openTransaction("Set height")
    try:
        obj.Height = float(values[0])
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True}
```

| Argument | Meaning |
| --- | --- |
| `obj` | the resolved document object |
| `manipulator` | the manipulator registry id, for example `"toy_height"`, `"transform"`, `"dress_up"` |
| `action` | the operation within it, for example `"set"`, `"apply"`, `"fillet"`, `"chamfer"` |
| `element` | the sub-element name (`"Face6"`, `"Edge3"`) or `""` |
| `values` | the numbers in the manipulator's documented order, as floats |

`FreeCAD.interaction(obj, manipulator, action, element="", values=None)` asks
each registered provider in order and returns the first non-`None` result. The
edit belongs to the provider: it opens the transaction, creates or edits the
feature, and recomputes. The shell only marshals.

### How the shell calls it

`ferrocad_shell.interaction(object_name, manipulator, action, element,
values_json)`:

1. resolves `object_name` in the active document;
2. parses `values_json` as a list of numbers;
3. calls `FreeCAD.interaction(...)`;
4. returns the provider's dict as JSON, or
   `{"ok": false, "error": "no module handles <manipulator> <action>"}` when no
   provider claims it.

```python
import json
import ferrocad_shell
import ferrocad_toy

ferrocad_toy.bootstrap("Display")
print(ferrocad_shell.interaction("Slab", "toy_height", "set", "", json.dumps([42.0])))
```

Output:

```text
{"ok": true}
```

The shell's error paths are also JSON: no active document, no object with that
name, or values that are not numbers.

A provider that raises is **not** isolated here. Return `{"ok": False, "error":
...}` instead of raising.

## Overlays are not display providers

An overlay is a Coin node registered with `FreeCADGui.addOverlay(node)` and
drawn beside the document scene, not a per-object mesh. `addOverlay` extracts
the subtree immediately (the "pull" bridge), so a later in-place edit does not
reach the copy: remove and re-add the node after changing it. `removeOverlay`
keys on Python object identity.

```python
FreeCADGui.addOverlay(root)
FreeCADGui.removeOverlay(root)
```

This path requires a `pivy.coin` facade that provides `addOverlay` and
`removeOverlay`. In a build without it, `addOverlay` raises. A registered overlay
draws regardless of the `grid_overlay` view flag; that flag only hides the native
ground. `mods/GridOverlayCoin` is the complete example.

## Next

- [`providers.md`](providers.md) — the feature-provider registry, which is
  separate from these two per-object hooks.
- [`settings-and-parameters.md`](settings-and-parameters.md) — where a
  provider's appearance settings live.
- [`minimal-mod.md`](minimal-mod.md) — a complete display provider.
