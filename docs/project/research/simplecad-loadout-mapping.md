# SimpleCAD's tools on the two hands: audit, mapping and gap analysis

Status: research (2026-11-15). Measured against `../simplecad` @ `9180c4b` (v0.38.0,
**clean**) and this tree at `8e95ae6`. Companions:
[`../developer/interaction/creation-pill-loadout.md`](../../developer/interaction/creation-pill-loadout.md)
(the two hands, the shared topology, the lock),
[`../developer/interaction/creation-loadout.md`](../../developer/interaction/creation-loadout.md)
(the Add hand's chrome, `CreationTool`/`TopologyGate`/`ToolField`, slices C1–C4),
[`../developer/interaction/visual-language.md`](../../developer/interaction/visual-language.md)
(tokens), [`../developer/interaction/gizmo-manipulators.md`](../../developer/interaction/gizmo-manipulators.md)
(the manipulator contract), and
[`tasks/simplecad-bringup.md`](../tasks/simplecad-bringup.md) (the UI cut this note feeds).

The question: SimpleCAD has ~50 interactive commands. Which of them belong on FerroCAD's
**Add hand** (centre action bar, five categories) and **Modify hand** (bottom-left loadout,
slots `1`–`3`/`0`), what does each anchor to, and what is missing between here and there?
**No Qt/Coin chrome and no pointer-following HUD** is a constraint of the answer, not a
detail: every affordance below is one of the surfaces the tree already has or that
`creation-loadout.md` specifies in place — the action bar, the loadout bar, the in-place
tool strip, the **docked** numeric row, the status bar, the object-verb wheel and the
command palette.

---

## 1. The inventory, as SimpleCAD declares it

`simplecad/gui/commands.py` is the whole surface, in four tables:

| Table | Count | What it is |
| --- | ---: | --- |
| `TOOLS` | 33 | the commands, including `Select` and `Delete` |
| `SURFACE` | 9 | `Imprint`, `OffsetFace`, `Shell`, `Draft`, `HealFaces`, `RemoveRounds`, `FillHole`, `Unwrap`, `Curvature` |
| `SMART` | 4 | `SelectHole`, `SelectLoop`, `SelectFlat`, `SelectSmooth` (one dropdown: `SmartSelectGroup`) |
| `EXTRAS` | 6 | `Isolate`, `Cleanup`, `Heal`, `Settings`, `MCP`, `Appearance` |

They are laid out in five toolbars (`GROUPS`): **Draw** 8, **Modify** 20, **Surface** 8,
**Inspect** 4, **Mesh** 4 — 44 toolbar slots, and **49** ids in the toolbars and menus once
the menu-only `Appearance`/`Cleanup`/`Heal` and the non-slot `Settings`/`MCP` are counted
(the same 49 the workbench host measured in UI-5). Every id is `SimpleCAD_<Key>`;
`SmartSelect` is a *group* command (`GetCommands`, no `Activated`), which is the one shape our
`Gui.addCommand` gained in UI-2.

`simplecad/gui/command_state.py` classifies them by *interaction shape*, which is the part
that matters here:

| Family | Members | Shape |
| --- | --- | --- |
| `DRAW` | Line, Rectangle, Circle, Arc, Arc2Point, Polygon, Sphere | arm, then click N points |
| `PICK` | PushPull, Offset, Elongate, VertexMove, Measure | arm, then click **one** sub-element and drag/click a value |
| `SMART` | `SmartSelect` + its 4 commands | act on the selection at once |
| `SURFACE` | the 8 surface commands | act on the selection, with a preview + a value |
| `ALWAYS` | `Select`, `Settings`, `MCP` | always available |

Three facts from that file drive the mapping below:

1. **`REQUIREMENTS[key]` is already a `TopologyGate`.** One sentence per command states the
   kind and arity it needs (`"Select one flat CAD face, or a single-face object, first."`),
   and `FLAT_ONLY` / `FLAT_OR_ROUND` add the surface-kind filter (`PushPull` flat only;
   `Offset`/`Elongate`/`Draft` flat + cylinder, not free-form). Our `TopologyGate` needs the
   same three pieces: `kind`, `min_arity`, `max_arity`, plus the classify step.
2. **`Draw: Plane` / `Draw: Surface` is a per-tool target switch, not a category.**
   `controller.py` sets `surface_mode` from one combo (`controls.drawing_mode`) and the same
   five drawing tools either create a standalone profile or wrap onto the picked face
   (`ops.draw_surface_curve`). That is exactly our **WP vs Face target host**.
3. **Everything numeric is docked.** `Tab` reaches `width`, `height`, `radius`, `sweep`,
   `bulge`, `sides`, `distance`, `angle`, `rows`/`columns` — a fixed row, never a label
   following the cursor. Our `NumericField` row is the same idea and is already the
   manipulator contract.

The **behavioural reference** for the interactive families is the headless GUI group we
already run (`conformance.py --simplecad-gui`, **78 / 1 / 0 / 0**): `test_dense_drawing`
(drawing on a dense face), `test_elongating`, `test_measuring`, `test_panel_placement` and
`test_push_pull_ui`. Each is a state machine worth translating into a stage-tool test.

### 1.1 What we have to place these on

| Surface | Today |
| --- | --- |
| Add hand | the idle bar's `⊕ Box ▾`, backed by `Part_Box`; no `CreationTool` descriptor, no `TopologyGate`, no `ToolField` strip (`creation-loadout.md` §9 lists all three as missing) |
| Modify hand | slots `1` Move/Rotate, `2` Push/Pull, `3` Fillet, `0` Inspect; `Select ▾` = `Auto ∣ Body ∣ Face ∣ Edge`; `Gizmo ▾` = Translate/Rotate/Universal |
| Hosted today | translate gizmo + `transform/apply`; `push_pull_manipulator` + `push_pull/extrude\|pocket`; `fillet_manipulator` + `dress_up/fillet\|chamfer`; sketch vertex grips + `sketch/drag`; the four SimpleCAD drawing tools |
| Commits | `InteractionCommit { manipulator, action, element, target, values, readout, creates }` → the script's `interaction` provider, which answers JSON |
| Declared handles | a Python edition can declare a handle with `{id, action, element, frame, axis, kind, value, min, max, decimals, label, readout}` (`_manipulator_specs`); `Axis`/`Ring` publish **one** docked numeric field, `Plane` none |
| Deletion | `App_Delete` deletes the **objects** the selection names |

---

## 2. Add hand: the five categories

Category names and the tool rows are `creation-loadout.md` §1's; the SimpleCAD column is
what the audit found. `Gate` uses that document's `TopologyGate`; `Targets` is the anchor
behaviour in that document's vocabulary (`WP` = workplane, `Face` = `+N` on the picked face,
`Edge` = along the edge). Strip fields are `ToolField { label, unit }`.

### Sketch

| SimpleCAD | Add tool | Gate | Targets | Strip fields | Status |
| --- | --- | --- | --- | --- | --- |
| `Line` | `Line` | `SpatialHost` | `WP` → wire or sketch; `Face` → surface line, wraps and splits | last segment `L` | **wired** as `Part_Polyline`/`Sketch_Polyline`; the `SimpleCAD_Line` id is a registration + provider alias |
| `Rectangle` | `Rectangle` | `SpatialHost` | `WP` → face; `Face` → imprint (split) | `W`, `H` | **wired** on `WP`; `Face` is the target-host slice |
| `Circle` | `Circle` | `SpatialHost` | `WP` → face; `Face` → imprint | `R` | **wired** on `WP`; `Face` as above |
| `Polygon` | `Polygon` | `SpatialHost` | `WP` → face; `Face` → imprint | `Sides`, `R` | **wired** on `WP` (sides via a digit today; a `ToolField` is the polish) |
| `Arc` | `Arc` | `SpatialHost` | `WP` → open edge; `Face` → imprint | `R`, `Sweep°` | **wired** on `WP` |
| `Arc2Point` | `Arc2Point` | `SpatialHost` | same | `Chord`, `Bulge` | **rust**: a second arc tool (two points + bulge); the kernel call is the same `Part.Arc` |
| `Point` (Add row, not in SimpleCAD) | `Point` | `SpatialHost` | `WP`, `Face`, `Edge` | — | **hook**: `draw_line`'s degenerate sibling; `Part.Vertex` is green |
| `Project Edge` / `Offset Wire` (Add row) | — | `RequiresSelection { Edge, 1.. }` | from a picked edge | `D` | **hook**: `document.operations.offset` on an edge |

### Solid

| SimpleCAD | Add tool | Gate | Targets | Strip fields | Status |
| --- | --- | --- | --- | --- | --- |
| `Sphere` | `Sphere` | `SpatialHost` | `WP` → solid at the centre; `Face` → boss on `+N` | `R` | **rust** gesture, **hook** object (`Part::Sphere` is green); no hosted placement today |
| — | `Box` | `SpatialHost` | `WP`, `Face` | `W`, `L`, `H` | **wired** as `Part_Box` at the origin; the ghost/host is C4 |
| — | `Cylinder`, `Cone`, `Torus` | `SpatialHost` | `WP`, `Face` | `R`, `H` / `R1`, `R2`, `H` / `R1`, `R2` | **hook** object; **rust** gesture |
| `Loft` | `Prism`'s siblings: `Loft` | `RequiresSelection { Wire ∣ Face, 2.. }` | profiles in order | — | **hook**: `document.operations.prepare_profiles` + `commit_form` |
| `Sweep` | `Sweep` | `RequiresSelection { Wire ∣ Face, 2 }` (profile, then path) | profile along an edge | — | **hook**: same call pair, `mode="Sweep"` |
| `Line` closing a loop | `Prism (Extrude)` | `RequiresSelection { Wire, 1 }` | profile → solid along `+N` | `H` | **wired** as `Part::Extrusion` through the sketch-pad path (`Sketch_Polyline` + `Sketch_Dimension`); a standalone Prism command does not exist yet |

### Surface

| SimpleCAD | Add tool | Gate | Targets | Strip fields | Status |
| --- | --- | --- | --- | --- | --- |
| `Imprint` | the five drawing tools **with a `Face` target** | `RequiresSelection { Face, 1.. }` | `Face` | `Depth` for raise/recess | **rust**: this is C4's `TargetHost`; the kernel path (`draw_surface_curve` → `surfaces.imprint`) is green |
| `OffsetFace` | `Offset Surface` | `RequiresSelection { Face, 1.. }` | `Face` along `+N` (per face, not one plane) | `D` | **hook**: declared axis handle + `document.operations.offset` |
| `Shell` | `Shell` | `RequiresSelection { Face ∣ Body, 1.. }` | the picked faces stay open | `T` | **hook**: `surfaces.operations.shell`; one declared handle for `T` |
| `Draft` | `Blend`'s sibling: `Draft` | `RequiresSelection { Face, 1.. }` | hinge plane + angle | `Angle°` | **hook**: `surfaces.operations.draft` |
| `FillHole` | `Face from Wire`'s sibling: `Fill` | `RequiresSelection { Edge, 1.. }` | rim edges, or a whole open surface | — | **hook**: `surfaces.operations.fill_holes` |
| `Unwrap` | `Unwrap` | `RequiresSelection { Face, 1 }` | flattens beside the part | — | **hook**: `surfaces.operations.unwrap` |
| `RemoveRounds` | — | `RequiresSelection { Body ∣ Face, 1.. }` | searches up to `R` | `R` | **hook**: `surfaces.operations.remove_rounds` |
| `Curvature` | — | `RequiresSelection { Body ∣ Face, 0.. }` | a map, not geometry | — | **hook**: a display provider (`stage_meshes` colour rule), not a tool |

### Text

| SimpleCAD | Add tool | Gate | Targets | Strip fields | Status |
| --- | --- | --- | --- | --- | --- |
| — | `3D Text` | `SpatialHost` | `WP` → solid text; `Face` → emboss on `+N` | `Size`, `Depth`, **`Text`** (a string field — a new `ToolField` kind) | **rust**: the kernel is green (`Part.makeWireString` on the Rust text stack, `ferrocad_text`); no tool, no string field |
| — | `Text along Edge` | `RequiresSelection { Edge, 1 }` | baseline follows the edge | `Size` | **rust** on the same tool |

SimpleCAD has no text command at all, so the Text category is ours to define; the audit's
only input is that a **string** is a new field kind and must not be a pointer-following
entry box.

### Mesh

| SimpleCAD | Add tool | Gate | Targets | Strip fields | Status |
| --- | --- | --- | --- | --- | --- |
| `ConvertMesh` | `Mesh from Shape` | `RequiresSelection { Body ∣ Mesh, 1 }` | tessellates the picked body | `Deflection` | **hook**: `MeshPart` conversion; `ferrocad_mesh` green |
| `MeshSplit`/`MeshStretch`/`MeshRegion` | — | `RequiresSelection { Mesh, 1 }` | split/stretch/region on one mesh | `Plane`, `D`, `Angle` | **hook** for the kernel; **rust** for the plane/axis pick |
| — | `Decimate` | `RequiresSelection { Mesh, 1 }` | — | `Reduction %` | **hook**: `Mesh.decimate` landed in P3 |
| — | `Mesh Box`, `Mesh Sphere` | `SpatialHost` | `WP` | `W`, `H` | **rust**: the mesh module has no primitive builders |

---

## 3. Modify hand: the four slots

### Slot `1` — Move (+ `Select ▾` scope, `Gizmo ▾` kind)

| SimpleCAD | Ours | Scope | Status |
| --- | --- | --- | --- |
| `Move` (whole objects) | translate gizmo + `transform/apply` | `Body` | **wired** |
| `Rotate` (whole objects) | `TransformGizmo::rotate` exists; only the translate factory is registered | `Body` | **hook**: one `rotate_gizmo` factory + `with_gizmo_kinds` |
| `Rotate` (selected lines/arcs/faces) | a declared `Ring` handle at the selection's centroid, commit `rotate/apply` | `Face`/`Edge` | **hook**: `document.operations.commit_rotation`, one provider case |
| `Elongate` | a declared `Axis` handle: the drag *is* the elongation | `Face`/`Edge` | **hook**: `document.operations.elongate` |
| `VertexMove` | a declared `Plane` handle at a vertex | `Vertex` | **hook** on the kernel; **rust** for a Vertex pick scope (`SelectFilter` has no `Vertex`) |
| `Scale`, `Deform` | a declared `Axis`/`Ring` handle, or a command | `Body`/`Face` | **hook**: `scale_objects`, `commit_deformation` |

The two `Select ▾` cases the brief asks about — whole-body transform vs sub-shape move — are
the same slot with a different scope, which is what `Select ▾` already means. No new chrome.

### Slot `2` — Push/Pull (additive `#F59E0B`, subtractive `#EF4444`)

| SimpleCAD | Ours | Status |
| --- | --- | --- |
| `PushPull` a flat face | `PushPullStem` + `push_pull/extrude\|pocket`; sign picks fuse vs cut | **wired** |
| `PushPull` until it meets the opposite face, or **through** it | the stem's range is the kernel's bisection; a *target* face would be a snap candidate | **hook**: snap-to-face candidate + the "up to face" mode |
| `OffsetFace` on a **curved** face | the stem refuses a non-planar face by design | **hook**: declared `Axis` handle + `document.operations.offset` (the stem stays planar-only) |
| `Imprint` raise/recess by a depth | the face-hosted creation target | **rust**: C4 |

Colour rule unchanged: `gizmo::PUSH_PULL_COLOR` `#F59E0B` for `+N`, `gizmo::SUBTRACT_COLOR`
`#EF4444` for `-N` (`visual-language.md` §68–69), and the ghost at
`GHOST_SURFACE_ALPHA = 0.35`.

### Slot `3` — Fillet

| SimpleCAD | Ours | Status |
| --- | --- | --- |
| `Round` one edge | `DressUpScrubber` + `dress_up/fillet` | **wired** (incl. re-opening a feature) |
| `Round` a **face boundary** or a whole solid | the scrubber arms on one `Edge…` element | **hook**: the provider expands a boundary/solid selection into its edges (`document.operations.commit_rounding` already takes a target set), one handle drives `R` |
| Chamfer | `DressUpMode::Chamfer` exists and the provider handles `dress_up/chamfer` | **hook**: one `chamfer_manipulator` factory |
| `RemoveRounds` | — | **hook**: `surfaces.operations.remove_rounds` with an `R` field |

### `Delete` / `Backspace`

| SimpleCAD | Ours | Status |
| --- | --- | --- |
| Delete whole objects | `App_Delete` | **wired** |
| Delete **edges/faces** of a solid, merging coplanar neighbours | the kernel path is green (`delete_selection`, `close_holes` — the P2 `test_edge_deletion` work) | **hook**: a `SimpleCAD_Delete` command that reads `SelectionEx` sub-elements and calls `document.operations.delete_selection`; `App_Delete` deletes the object, not the element |
| `HealFaces` (delete a boss/pocket/round and grow the neighbours closed) | the same call, "grow to close" mode | **hook**: same command, different mode |

This is the one **removal** verb the two hands lack, and it needs **no Rust at all**: the
selection already carries `Face…`/`Edge…` names.

---

## 4. What is not a hand

The audit is explicit about the commands that must **not** be forced onto a slot, because
they are selection verbs or document verbs with a numerical answer:

| Group | Commands | Where they belong |
| --- | --- | --- |
| Selection | `Select`, the 5 smart-selects | `S`/`Esc`, double-click, and the object-verb wheel (wire `SelectHole` etc. to the mod event contract, E1) |
| Inspect | `Measure`, `Curvature`, `Isolate` | slot `0` Inspect; `Curvature` is a display provider, `Isolate` is a view command |
| Block ops | `Union`, `Intersect`, `Slice`, `Subdivide` | commands with a gate and one or two fields; `Union`/`Intersect` on two bodies are `Part_Fuse`/`Part_Common` already |
| Duplicate | `Copy`, `Pattern`, `Mirror`, `Scale` | commands over the selection (`transform(copy=True)`, `mirror`, `pattern`) |
| Repair | `Analytic`, `Cleanup`, `Heal` | document commands (`surfaces.fit`, `document.operations.cleanup`/`heal`); no canvas gesture |
| Document | `Appearance`, `Settings`, `MCP` | properties panel / settings card / a connection toggle |

Forcing any of these into a hand would need a gesture the user does not want to make
("drag a heal"), and `creation-loadout.md` §8 forbids inventing chrome for them. They are
reachable today through the command palette (Ctrl+K) and the wheel, which is the correct
home; the Add hand's tool row is for **making** things.

---

## 5. Gap analysis

### Bucket 1 — works today, end to end (eleven)

`Line` (as `Part_Polyline`/`Sketch_Polyline`), `Rectangle`, `Circle`, `Polygon`, `Arc` on
`WP`, `Box` at the origin, `PushPull` on a planar face, `Round` on one edge, `Move` a whole
body, `Delete` whole objects, plus `Sketch_Dimension`'s pad. Each already has command →
tool/manipulator → commit → provider → kernel.

### Bucket 2 — the kernel is green; only a hook is missing (the bulk)

The SimpleCAD suite at **1039 passed / 6 failed / 2 errored / 7 skipped** exercises every
operation below on our kernel: **87 of its 88 modules** import `document/operations.py`
(and 7 import `surfaces/operations.py`) and drive it through our `Part`. The stage seam to
drive the same operations from a pointer already exists — either a compiled tool, a
**Python-declared handle** with a docked numeric field, or (for non-gesture verbs) a
command whose `IsActive` does the gating. What is missing is a provider case, at most one
`with_tool`/`with_gizmo` line, and a command id.

| Operation | Kernel call | Seam |
| --- | --- | --- |
| `Offset` / `OffsetFace` | `document.operations.offset` | declared `Axis` handle |
| `Elongate` | `document.operations.elongate` | declared `Axis` handle |
| `Shell`, `Draft`, `FillHole`, `Unwrap`, `RemoveRounds` | `surfaces.operations.shell`/`draft`/`fill_holes`/`unwrap`/`remove_rounds` | declared handle for the one scalar; command for the rest |
| `Rotate` sub-shape | `document.operations.commit_rotation` | declared `Ring` handle |
| `Rotate` whole objects | `TransformGizmo::rotate` | one gizmo factory |
| Chamfer | `dress_up/chamfer` | one manipulator factory |
| `VertexMove` | `document.operations.move_vertex` | declared `Plane` handle (**blocked** on a Vertex pick scope) |
| `Loft`, `Sweep` | `document.operations.prepare_profiles`/`commit_form` | command + `IsActive` |
| `Union`, `Intersect`, `Slice`, `Subdivide` | `document.operations.unite`/`intersect`/`commit_slicing`/`commit_subdivision` | command + fields |
| `Copy`, `Pattern`, `Mirror`, `Scale`, `Deform` | `document.operations.transform(copy=True)`/`pattern`/`mirror`/`scale_objects`/`commit_deformation` | command + handle |
| `Analytic`, `Cleanup`, `Heal` | `surfaces.fit`, `document.operations.cleanup`/`heal` | command |
| `Delete`/`HealFaces` on sub-elements | `document.operations.delete_selection` | command reading `SelectionEx` |
| `Measure`, `Isolate`, `Appearance` | `inspection.measuring`, document/display | slot `0` / panel |
| `Mesh from Shape`, `Decimate`, mesh split/stretch/region | `MeshPart`, `Mesh.decimate` | command |

Two of these are the highest-value because they are the *only* missing verbs in their slot:
sub-element **Delete** (slot-less, `Delete` key) and **Offset** (the surface sibling of
Push/Pull, and the one SimpleCAD PICK tool whose whole gesture is "drag a distance").

### Bucket 3 — needs a Rust slice (the chrome, and the gestures)

| Gap | Why it is Rust | Slice |
| --- | --- | --- |
| `CreationTool` / `CreationCategory` descriptors, the flyout, `TopologyGate` evaluation, `INACTIVE_STRUCTURAL` | `Contributions` holds tools by id only | **C1** |
| `ToolField` strip + `Tool::set_field` write-back; a **string** field for `3D Text` | `ToolStatus` is text-only today | **C2** |
| The hover placement frame + 3D ghost on `WP`/`Face`/`Edge` (`ManipulatorFrame`, `GHOST_SURFACE_ALPHA`) | no `TargetHost` | **C4** |
| `Face`-hosted **creation**: the commit must carry the host (object + face) so the provider can split/imprint | `InteractionCommit::creating()` has no target | **C4** |
| `Arc2Point`, `Sphere`/`Cylinder`/`Cone`/`Torus`/`Mesh Box` placement gestures | no stage tool | new tools |
| `Point` tool | trivial, but still a tool | new tool |
| `Vertex` pick scope (`Select ▾`, `PickScope`) for `VertexMove` | `SelectFilter` is `Auto ∣ Body ∣ Face ∣ Edge` | new scope |
| `3D Text` tool + string field | no tool, and the field kind does not exist | C2 + tool |

---

## 6. Recommended slice order

Ranked by the loop it closes per unit of work, not by category order. The first two are
**Python-only** and each closes a direct-modeling loop the product does not have yet.

1. **Sub-element Delete + HealFaces** (`SimpleCAD_Delete`). The `Delete`/`Backspace` verb
   the Modify hand names but does not have: read the sub-elements, call
   `document.operations.delete_selection` / the "grow closed" mode, one transaction, and keep
   the "surface seam" refusal message. Closes the repair loop on imported geometry; zero
   Rust. Pin with
   a round-trip test (a box with a hole face deleted and re-closed) in the edition, mirroring
   the `test_edge_deletion`/`close_holes` cases the suite already proves.
2. **Offset + Elongate + OffsetFace** — the three PICK tools whose gesture is one declared
   axis handle and one docked number. `ferrocad_stage`'s declarative handle already publishes
   the `NumericField`, so this is `_manipulator_specs` + three provider cases. It also gives
   the Modify hand's Slot `2` its curved-face sibling and Slot `1` its sub-shape drag.
3. **Rotate + Chamfer factories** (one line each: a `rotate_gizmo` registered beside the
   triad, a `chamfer_manipulator` beside the fillet). Two of the four Modify slots become
   complete rather than single-mode.
4. **The selection-verb block** — `Shell`, `Draft`, `FillHole`, `Unwrap`, `RemoveRounds`,
   `Loft`, `Sweep`, `Union`, `Intersect`, `Slice`, `Subdivide`, `Analytic`, `Cleanup`,
   `Heal`, `Copy`/`Pattern`/`Mirror`/`Scale`. One shared `command → provider → kernel` table
   with `IsActive` gating; each is a table row, and the palette is an acceptable home until
   C1 lands.
5. **C4 — the face-hosted creation target.** Unlocks `Imprint`, `Draw: Surface` for the
   drawing tools, boss/pocket on a face, and the `Face` column of every Solid row. It is the
   one Rust slice that changes several categories at once.
6. **C1 + C2 — the pill and the strip.** Only now does the Add hand *look* like the design:
   categories, gates with a disabled reason, and the docked fields. Sequencing the chrome
   last is deliberate: the verbs above are usable from the palette without it, and building
   the chrome first would have gated five loops behind one.
7. **Derived gestures**: `Arc2Point`, the primitive placements with a ghost, `VertexMove`
   (with the new pick scope), `3D Text`, the mesh primitives.

The two runs this order must not lose: `--simplecad-gui` (78 / 1 / 0 / 0) as the behaviour
reference, and the SimpleCAD suite (1039 / 6 / 2 / 7) as the kernel gate.

---

## 7. Non-goals

- **No Qt/Coin chrome.** No `QMainWindow` dock, no Coin scene, no Qt toolbar; the
  declarations SimpleCAD makes are data (`Gui.addCommand`, toolbars/menus, `GetResources`)
  and UI-2 already hosts them.
- **No pointer-following HUD.** Every number is in the docked row (`NumericField`) or the
  in-place tool strip; the only pointer-attached object is the ghost geometry itself.
- **No second command registry.** SimpleCAD's ids stay in the `SimpleCAD_` namespace we
  already use; the note does not propose renaming its commands.
- **No new palette entries for their own sake**: a command that a slot or the pill can carry
  should not also get a toolbar row.

## 8. Open questions

1. **`Draw: Surface` vs the `Face` target.** SimpleCAD makes it an explicit combo; the
   two-hand design makes it the hovered target. Keeping both is honest (the combo also
   applies to a preselection), but only one should decide the commit's host.
2. **A string `ToolField`.** `3D Text` is the only tool that needs one; C2 should decide
   whether a field kind carries a string or the tool opens the properties panel instead.
3. **Multi-element Round.** One handle over a boundary is not the same as N handles over N
   edges; the provider can expand the selection, but the *preview* (one fillet or many)
   belongs to the manipulator.
4. **`Select ▾` and vertices.** `VertexMove` is otherwise a one-line handle; adding `Vertex`
   to `SelectFilter` touches pick scope, hit testing and `SelectionRef` filtering.
