# Create, display and manipulate an object

The toy workbench is the kernel-free reference edition. It shows the four things a workbench does: create an object, give it a visual representation, manipulate it through the generic interaction path, and record the edit as a transaction.

> **Generated page.** `tools/userdocs.py` writes it from
> [`tools/workflows/04_toy_workbench.py`](../../tools/workflows/04_toy_workbench.py).
> Edit the workflow, not this page; `python3 tools/userdocs.py --check`
> fails when the two disagree.

**Requires:** the `ferrocad_toy` app script (it registers on import); no GPU: the mesh is read before it would be drawn.

## 1. Boot the edition

`bootstrap` builds the same sample document the shell opens at startup, then adds one slab and exposes the `Toy` console name. The shell calls exactly this function.

```python
import json
import ferrocad_shell
import ferrocad_toy

payload = json.loads(ferrocad_toy.bootstrap("Toy"))
```

*bootstrap*

```json
{
  "backend": "ferrocad",
  "document": "Toy",
  "version": "0.1.2"
}
```

## 2. The document the shell inspects

`model_tree` is the shell's inspector feed: documents, objects and their type names. It goes through the same public API a workbench uses.

```python
tree = json.loads(ferrocad_shell.model_tree())
```

*model_tree*

```json
[
  {
    "label": "Toy",
    "name": "Toy",
    "objects": [
      {
        "label": "Params",
        "name": "Params",
        "type": "App::FeaturePython"
      },
      {
        "label": "Derived",
        "name": "Derived",
        "type": "App::FeaturePython"
      },
      {
        "label": "Group",
        "name": "Group",
        "type": "App::DocumentObjectGroup"
      },
      {
        "label": "Slab",
        "name": "Slab",
        "type": "App::FeaturePython"
      }
    ]
  }
]
```

## 3. The visual representation

The display provider turns the object's `Width`, `Depth`, `Height` and `Placement` into a mesh, and carries `Color` and `DisplayMode` alongside it. The shell asks for these meshes; it never names the workbench.

```python
meshes = json.loads(ferrocad_shell.stage_meshes())
```

```python
slab_mesh = meshes[0]
positions = slab_mesh["positions"]
summary = {
    "object": slab_mesh["object"],
    "display": slab_mesh["display"],
    "color": slab_mesh["color"],
    "positions (vertices)": len(positions),
    "triangles": len(slab_mesh["triangles"]) // 3,
    "edges": len(slab_mesh["edges"]) // 2,
    "bounds": [
        [min(p[i] for p in positions) for i in range(3)],
        [max(p[i] for p in positions) for i in range(3)],
    ],
}
```

*stage_meshes[0], summarised*

```json
{
  "bounds": [
    [
      -20.0,
      -20.0,
      0.0
    ],
    [
      20.0,
      20.0,
      20.0
    ]
  ],
  "color": [
    0.26,
    0.55,
    0.85,
    1.0
  ],
  "display": "shaded_edges",
  "edges": 24,
  "object": "Slab",
  "positions (vertices)": 24,
  "triangles": 12
}
```

## 4. Create an object

`makeSlab` is the workbench's creation command. It adds the object with its properties and opens one named transaction, so undo treats it as a single step.

```python
slab = ferrocad_toy.makeSlab(width=60, height=30, x=10, name="Wide")
```

| Read back | Value |
| --- | --- |
| Name | Wide |
| Width | 60 mm |
| Height | 30 mm |
| Placement.Base | Vector (10, 0, 0) |
| DisplayMode | ShadedWithEdges |

## 5. Manipulate it through the interaction path

A manipulator in the viewport resolves to a request, and the workbench supplies the edit. The `toy_height` grip writes `Height`; the generic `transform` gizmo composes a world-space matrix with the placement. Both arrive here as one transaction, so both are undoable.

```python
response = json.loads(
    ferrocad_shell.interaction(
        "Wide", "toy_height", "set", "", json.dumps([42.0])
    )
)
```

| Read back | Value |
| --- | --- |
| Response | {"ok": true} |
| Height after the grip | 42 mm |

The transform gizmo sends a row-major 4×4 matrix; the translation lives in the last column (`m03`, `m13`, `m23`).

```python
move = [1, 0, 0, 25, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
response = json.loads(
    ferrocad_shell.interaction(
        "Wide", "transform", "apply", "", json.dumps(move)
    )
)
```

| Read back | Value |
| --- | --- |
| Response | {"ok": true} |
| Placement.Base after the move | Vector (35, 0, 0) |

## 6. Undo the manipulation

Because each interaction is a transaction, the document's own undo stack reverses them one at a time. No workbench-side history is needed.

```python
slab.Document.undo()
slab.Document.recompute()
```

| Read back | Value |
| --- | --- |
| Placement.Base after one undo (the move) | Vector (10, 0, 0) |
| Height after one undo | 42 mm |

```python
slab.Document.undo()
slab.Document.recompute()
```

| Read back | Value |
| --- | --- |
| Height after two undos | 30 mm |

## 7. What the workbench registered

The commands in the list come from `FreeCADGui`, not from the shell. `enabled` is the command's `IsActive` evaluated against the current selection, which is why the edit commands are disabled right now.

```python
commands = json.loads(ferrocad_shell.commands())
```

| Command | Group | Kind | Enabled |
| --- | --- | --- | --- |
| App_Delete | Edit | ForEdit | false |
| App_Move | Edit | ForEdit | false |
| Mods_Settings | Edit | ForEdit | true |
| Toy_MakeSlab | Toy | ForEdit | true |
| Toy_Polyline | Toy | ForView | true |

> The shell never links a workbench. Everything on this page reaches the document through `FreeCAD`, `FreeCADGui` or the module hook the edition registered at import time.

## 8. Close the document

```python
FreeCAD.closeDocument('Toy')
```

| Read back | Value |
| --- | --- |
| Open documents | [] |
