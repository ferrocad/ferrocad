# The toy workbench: a complete workbench, without a kernel

Status: reference (2026-10-20). Companion to
[`architecture.md`](../architecture.md) §10, [`gizmo-manipulators.md`](../interaction/gizmo-manipulators.md)
and [`repackaging.md`](../architecture/repackaging.md).

## Why it exists

The interaction registry makes a strong claim: **a workbench contributes its own
objects, representation and manipulations, and the shell, viewport and stage never
name it.** Part proves that claim, but Part also brings OCCT, a tessellator, a feature
graph and a Python module — so a failure does not say *which* layer broke, and the
test cannot run in the fast CI job.

`ferrocad_toy` is the smallest edition that still tells the whole truth. It links **no
geometry module at all**, and it still covers the four things a workbench does:

| Capability | Toy implementation |
| --- | --- |
| **Create** an object | `Toy.makeSlab(...)` adds an `App::FeaturePython` with typed properties, in one transaction |
| **Represent** it | the display provider builds the mesh from the properties and honours `Color` / `DisplayMode` |
| **Manipulate** it | the `HeightGrip` manipulator (resize) **and** the generic `TransformGizmo` (move) |
| **Record** it | creation and every edit are transactions; undo/redo and save/open go through the engine |

If the toy's loop works, the seam is generic. If only Part's works, something
Part-shaped leaked into the shared layers.

## The object model

A toy slab is a plain `App::FeaturePython` with:

| Property | Type | Used for |
| --- | --- | --- |
| `Width`, `Depth`, `Height` | `App::PropertyLength` | the cuboid (default 40 × 40 × 20 mm) |
| `Placement` | `App::PropertyPlacement` | where the slab sits |
| `DisplayMode` | `App::PropertyEnumeration` | `Shaded` / `ShadedWithEdges` / `Wireframe` |
| `Color` | `App::PropertyColor` | the surface colour (one RGBA entry) |

The display provider is the *view*: it tessellates the cuboid from the properties,
places it by `Placement`, and returns the colour and draw mode alongside the mesh. No
kernel is involved, and the shell learns nothing about slabs.

## The loop it exercises

```text
Toy.makeSlab(...) ──▶ document (one transaction) ──▶ stage_meshes ──▶ DisplaySync
                                                                        │
                                                       entity map + Stage
                                                                        │
                                              click: SelectionRef::whole(entity)
                                                                        ▼
                                          Contributions::{manipulator_for, gizmo_for}
                                                                        │
                            HeightGrip (drag)            TransformGizmo (drag)
                                     │                              │
                                     ▼                              ▼
                             InteractionCommit             GizmoCommitEvent
                              "toy_height"/"set"           "transform"/"apply"
                                     │                              │
                                     │                    to_interaction()
                                     └──────────────┬───────────────┘
                                                    ▼
                              display.object_of(target) ─▶ interaction hook
                                                    │
                                FerroCAD.interaction(obj, id, action, "", values)
                                                    ▼
                       Height = …   /   Placement = delta * Placement
                                                    │
                                            recompute ─▶ re-sync
```

Every arrow is the same code the Part edition uses. Only the two endpoints differ.

## Creation and the console

`makeSlab` is the workbench's creation API. It adds the object and its properties,
commits the transaction, and returns the object. The console name is a convenience;
the first-class form is `Toy_MakeSlab`, registered with `FreeCADGui.addCommand` (see
[`actions-and-tools.md`](actions-and-tools.md)):

```python
import ferrocad_toy   # the app script; or use the console name below
Toy.makeSlab(width=60, depth=40, height=25, x=100)   # the console surface
```

The app script exposes `Toy` through `ferrocad_shell.add_namespace`, so the shell's
console can create objects without an import:

```text
>>> Toy.makeSlab(30, 30, 10, 60)
```

The same creation is offered as a command, `Toy_MakeSlab` (registered with
`FreeCADGui.addCommand` in this module's `Init`), so the shell's actions bar shows
**Create Slab** and runs it without knowing what a slab is. The command returns a
**dialog** for Width/Depth/Height; the shell validates it and calls back, and the
callback opens one transaction, so one click is one undo step (see
[`actions-and-tools.md`](actions-and-tools.md) §6 and §16). Its `Ctrl+Shift+S` chord
is bound at the window root, so the same command runs from the keyboard.

The inspector refreshes and the viewport re-syncs through the ordinary observers —
`evaluate` in the shell already does both.

## Manipulation

Two generic mechanisms, one document path:

- **The `HeightGrip` manipulator** (`ferrocad_toy::height_grip`). It claims a
  whole-object selection, anchors at the top-face centroid, draws a screen-invariant
  stem projecting up into empty space (44 px), and drags the height. Its commit is
  `toy_height` / `set` with `[height]`.
- **The `TransformGizmo`** (`ferrocad_toy::move_gizmo`). It sits at the item's centre
  and offers translation. Its commit is converted by
  `GizmoCommitEvent::to_interaction`: `transform` / `apply` with the 16 row-major
  matrix elements. The provider composes it with the current placement, so the object
  moves in world space.

The shell hosts both from the edition's `Contributions`; it never learns what a height
or a transform means.

The **app-level** built-ins come for free: `ferrocad_shell` registers `App_Delete`,
`App_Move` and the checkable `App_ToggleGrid`, so every edition (the toy included)
has them in the actions bar. Delete removes the whole selection as one transaction;
Move opens a ΔX/ΔY/ΔZ dialog (its non-interactive form,
`runCommand("App_Move", dx, dy, dz)`, skips the modal); Grid flips the shell's grid
state through a `view` request (rendering is G5). A Ctrl/Cmd-click in the inspector
builds the multi-selection they act on.

The toy also contributes a **tool**: `Toy_Polyline` returns `{"tool":
"toy_polyline"}`, and the edition registers `ferrocad_stage::PolylineTool` under that
id in its `main`. The tool owns pointer events and draws transient geometry; committing
a polyline to the document is a later slice.

## Document semantics

- **Creation is one transaction.** `makeSlab` opens `"Create Slab"`; `doc.undo()`
  removes the object, `doc.redo()` restores it.
- **Each manipulation is one transaction.** The provider wraps the property write and
  the recompute; undo restores the previous height or placement.
- **Persistence is the engine's.** `doc.saveAs(...)` → `closeDocument` →
  `openDocument(...)` restores the slab with its properties, colour and height.

## What it adds to the contracts

The toy needed four small, generic extensions, all of which Part (or any future
workbench) can use:

| Contract | Addition |
| --- | --- |
| display (`ferrocad_gpui::DisplaySync`) | `StageMesh.color` (RGBA) and `StageMesh.display` (`shaded` / `shaded_edges` / `wireframe`) |
| contributions (`ferrocad_gpui::Contributions`) | `with_gizmo` / `gizmo_for` next to the manipulator factories |
| viewport events | `ViewportEvent::Transform(GizmoCommitEvent)`, plus `take_gizmo_commit` |
| commits (`ferrocad_stage`) | `TRANSFORM_ID` and `GizmoCommitEvent::to_interaction` |
| console | `ferrocad_shell.add_namespace(name, value)` |

None of them names a workbench.

## The pieces

| Path | What |
| --- | --- |
| `crates/ferrocad_toy/src/lib.rs` | `HeightGrip`, the `height_grip` and `move_gizmo` factories, 7 unit tests |
| `crates/ferrocad_toy/src/main.rs` | the edition: `run_edition_with(…, Contributions::with_manipulator(…).with_gizmo(…))` |
| `python/ferrocad_toy/__init__.py` | the app script: `makeSlab`, the display and interaction providers, the `Toy` console name |
| `crates/ferrocad_toy/tests/round_trip.rs` | the end-to-end test: representation, creation + undo/redo, grip, gizmo, save/open |
| `crates/ferrocad_parametric/tests/manipulator_round_trip.rs` | the same loop with the real Part arc and a `Part::Fillet` |

Run it:

```sh
cargo run -p ferrocad_toy        # a window; click the slab, drag the grip or the gizmo
cargo test -p ferrocad_toy       # the kernel-free end-to-end test
```

## Non-goals

The toy is not a full workbench: it has no `Proxy.execute` (the engine has no Python
recompute callback yet), no selection semantics beyond "whole object", and no kernel.
It is a reference implementation of the extension point and a test fixture. When the
extension point changes, it is the first thing to update — if it breaks, the change
was not generic.
