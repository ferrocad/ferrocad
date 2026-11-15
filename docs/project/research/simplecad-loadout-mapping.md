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
| `Rotate` (whole objects) | the base `translate_gizmo` declares `GizmoKind::ALL`, so the set already carries a Rotate entry; `Gizmo ▾` narrows to it | `Body` | **wired** (verified by the loadout round trip) |
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

### Slot `3` — Fillet **and** Chamfer

Slot 3 owns both dress-ups. Pressing `3` (or clicking the pill) equips the slot; pressing
it again toggles `3 Fillet` ⇄ `3 Chamfer`, and the bar's label, its glyph and the handle
the factory builds all follow. There is no separate Chamfer button: `Part_Chamfer` stays
registered for Search and for scripts, and its surface is `palette`.

| SimpleCAD | Ours | Status |
| --- | --- | --- |
| `Round` one edge | `DressUpScrubber` + `dress_up/fillet` | **wired** (incl. re-opening a feature) |
| `Round` a **face boundary** or a whole solid | the scrubber arms on one `Edge…` element | **hook**: the provider expands a boundary/solid selection into its edges (`document.operations.commit_rounding` already takes a target set), one handle drives `R` |
| Chamfer | Slot 3's `DressUpKind::Chamfer` → `DressUpMode::Chamfer` (two distances, `D1`/`D2`) → `dress_up/chamfer` → `Part::Chamfer` | **wired** |
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
| `Rotate` whole objects | the base factory's `GizmoKind::ALL` | already served; no factory needed |
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
   `Loft`, `Sweep`, `Union`, `Intersect`, `Slice`, `Subdivide`, `Cleanup`, `Heal`. One
   shared `command → provider → kernel` table with `IsActive` gating; each is a table row,
   and the palette is an acceptable home until C1 lands. **Landed** (§8): thirteen verbs,
   ten pinned by the round trip.
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

## 8. What landed (2026-11-15)

Steps 1–3 of §6 are in, **Python-only** except for one new provider seam in the shell
script. Everything below is pinned by `crates/ferrocad_simplecad/tests/loadout_round_trip.rs`
(8 cases) and by the unchanged suites (`ferrocad_parametric`, `ferrocad_toy`, `base`
718/0/0/8, `workbenches` 881/0/0/3).

### Step 1 — sub-element delete and heal

- **The seam is a provider, not a fork of the command.** `python/FerroCAD/__init__.py`
  gained `register_delete_provider` / `delete_selection(items, remove_objects)`;
  `App_Delete` builds `[(object, sub-elements)]` and offers it to the providers, falling
  back to its own whole-object path when none claims it. A kernel-free build (the toy
  edition) is therefore unchanged, byte for byte: the fallback *is* the old code.
- **The Part implementation is Part-only.** `ferrocad_parametric._delete_subshape`
  removes faces by re-sewing the rest (`Part.makeShell` → `Part.Solid` when it closes),
  merges edges by unifying coplanar faces (`removeSplitter`), refuses a result that is not
  valid, and writes one transaction. On a box, deleting a face leaves an **open surface**
  (5 faces) and one undo restores the solid; deleting the seam of two coplanar faces
  merges them into one.
- **`Part_HealFaces`** is the same path with the open result disallowed. Step 4 upgraded it:
  when the package is on the path it calls `surfaces.edit.remove_faces`, which lets the
  neighbours **grow** until the body closes, so a pocket's floor and four walls heal back
  into the box they were cut from (volume exactly 1000, one undo). The Part-only sewing
  remains the fallback, and that fallback is what refuses a single face of a plain box —
  the earlier "heal cannot grow" limit is gone.

### Step 2 — Offset / Elongate / OffsetFace as declared handles

- `ferrocad_simplecad` registered a `FerroCAD.register_manipulator_provider` and three
  commands (`SimpleCAD_Offset`, `_Elongate`, `_RotateEdge`) that arm the verb; the next
  pick arms the handle, which publishes the **docked** `NumericField` the declarative
  path already builds (no pointer-following input anywhere).
- **The disambiguation, as implemented** (the table in §1 of the module, asserted by the
  test):

| Pick | Manipulator | Why |
| --- | --- | --- |
| planar `FaceN` | `PushPullStem` (compiled, slot 2) | an offset of a plane *is* push/pull; the provider returns **no spec** for it |
| `EdgeN` of a body with faces | `DressUpScrubber` (compiled, slot 3) | a dress-up is the edge edit; the provider declines it |
| **curved** `FaceN` | `simplecad_element` (`offset`/`elongate`) | `push_pull_stem_for` refuses a non-planar face |
| circular `EdgeN` of a **faceless** item | `simplecad_element` (`offset`/`elongate`/`rotate`) | `dress_up_scrubber_for` refuses a faceless item |

  The test asserts exactly this: `manipulator_specs("Box", "Face1")` and
  `("Box", "Edge1")` are empty, the cylinder wall and the drawing rim each carry one
  spec. Two manipulators cannot fight over one pick because the families never share
  one.
- Commits: a face goes to `surfaces.offset.offset_faces` (the *moving* operation) and is
  committed here, because the package's wrapper hands the job to a worker process the
  embedded interpreter cannot spawn; an edge goes to `document.operations.offset`, and
  `elongate` to `document.operations.elongate`. Each is one undo step (asserted for the
  face offset and for the profile elongate).

### Step 3 — Rotate and Chamfer

- **Rotate needed no factory.** The base `translate_gizmo` already declares
  `GizmoKind::ALL`, so the registry emits a Rotate entry and `Gizmo ▾`'s Rotate verb
  narrows the set to rings — the recorded claim in §5 is corrected here. What was
  missing was the **sub-element** case: a declared `ring` handle on a drawing's circular
  edge, committing degrees. `Rotate` of an arc about its own centre is asserted
  (45° moves the start from `(10, 0, 0)` to `(7.07, 7.07, 0)`, length unchanged, undo
  restores).
- **Chamfer** is wired two ways, both conflict-free with the Fillet scrubber:
  - `Part_Chamfer` applies a chamfer to the selected edges through `_apply_dress_up` — the
    same feature the scrubber writes, so the declared grip re-opens it;
  - `_dress_up_target` now answers a **chamfer** as well as a fillet: its band is the one
    face whose *every* edge the feature added, so picking it re-opens the chamfer as a
    chamfer instead of silently converting it to a fillet (which is what the old
    fillet-only answer would have done). Only a **symmetric** chamfer re-opens: one
    declared field cannot carry two distances. **Limit, recorded.**
- A persistent Slot 3 *mode* switch (scrubber as chamfer) is still C2 work: a second
  compiled factory would shadow the fillet by hit priority, and a declared one would
  draw a second grip beside it.

### Step 4 — the selection-verb block

Thirteen verbs are wired as `command → gate → pure function → one transaction`, in one
table in `ferrocad_simplecad`:

| Verb | Function | Default |
| --- | --- | --- |
| `Shell` | `surfaces.offset.shell` | 1.0 mm |
| `Draft` | `surfaces.offset.draft` (axis Z, hinge bottom) | 5° |
| `FillHole` | `surfaces.edit.fill` | — |
| `RemoveRounds` | `surfaces.edit.find_rounds` + `remove_faces` | 5 mm |
| `Unwrap` | `surfaces.edit.unwrap` → a new object beside the part | — |
| `Loft`, `Sweep` | `prepare_profiles` → `form_preview` → `commit_form` | — |
| `Union` | `unite` | — |
| `Intersect` | `intersect` | — |
| `Slice` | `prepare_slicing` → `cut_plane("Main")` → `preview_slicing` → `separate` → `commit_slicing` | 0 mm |
| `Subdivide` | `prepare_subdivision` → `preview_subdivision` → `commit_subdivision` | 3 × 3 |
| `Cleanup`, `Heal` | `cleanup`, `heal` | — |

- **The worker finding is now a rule, not an incident.** Step 2 learned that
  `surfaces.operations` dispatches to a process pool; every verb here calls the *pure*
  module instead, and `Slice` uses `slicing.separate` exactly as the GUI tool does (the
  preview yields `(side, shape)` pairs; the commit wants shapes).
- **The gate is the cheap half of `REQUIREMENTS`** — object count and element kind — and
  it raises `_Refusal`, which `_run_verb` turns into `{ok: false, status}`. The deep
  checks stay where they are (the functions' own `ValueError`s), so a refusal reads as a
  sentence either way and no transaction was opened. Asserted: two faces for `Unwrap`,
  an edge for `Shell`, a face for `Union` — and the document is untouched afterwards.
- **The numeric surface is dense on purpose.** `ToolField` is still C2, so each verb runs
  at a documented default, names the value in its status, and `Activated(value)` accepts
  another (which is how the round trip drives `Shell` at 2 mm).
- **Draft has a real gate the plan did not foresee**: the selected walls must share one
  hinge — one side face alone is refused by the kernel with a sentence about the hinge
  plane, so the command passes the selection through unchanged rather than guessing.

### Step 5 — C4, the face-hosted creation target

This is the one slice of §6 that is **Rust**: the stage learns where a creation tool is
pointing, so the profile tools can draw on a face and the script can divide it.

- **The stage's vocabulary.** `ferrocad_stage::HoverTarget { frame, host, kind }` is the
  placement under the pointer: `ManipulatorFrame` plus the body it came from plus which of
  the three rows of [`creation-loadout.md`] §3.1 resolved it (`TargetKind::{Workplane,
  Face, Edge}`). `CreationHost { entity, element, object }` is the neutral record on
  `InteractionCommit`: the stage writes the entity id and the element (`Face6`), the shell
  writes the document name. A tool reads the placement through the new defaulted
  `Tool::hover`, so a tool that does not care (the polyline) costs nothing.
- **The hover frame is the press frame.** The viewport resolves it from the **snapped**
  point — a planar face first, then an edge's polyline, then the active workplane — and
  the first left press binds it as the session's placement. That is what makes the ghost,
  the preview and the commit one geometry rather than three re-derivations.
- **The ghost is real geometry, at the token alpha.** Each drawing tool gained a third
  fixed slot, `ghost_item(CURSOR_COLOR)`: `ShadedWithEdges`, surface alpha
  `GHOST_SURFACE_ALPHA` (0.35), blended (no depth write), not selectable. Until the anchor
  is placed it holds a closed square in the placement plane
  (`HoverTarget::marker`), which is the honest preview when the drag has no extent yet.
- **The drawing plane follows the face.** `DragFrame::in_plane` builds the drag frame in the
  bound face's `(u, v)` instead of the world plane `drag_axes` would pick, and
  `rectangle_corners_in` does the same for the box. On a **workplane** hover the old
  world-plane rule is untouched: a vertical drag still draws in `XZ`.
- **The commit names its host, and the script imprints.** `on_interaction` resolves
  `host.entity` to the document name before the backend sees the commit; `python::interaction`
  passes it as a sixth value; `FerroCAD.interaction` offers it to providers **by keyword**
  (a provider that predates it gets `TypeError: unexpected keyword argument 'host'`, is
  retried with the five-argument form, and keeps working — only that exact refusal is
  retried). `ferrocad_simplecad._imprint` rebuilds the profile from the commit, calls
  `document.operations.draw_profile` (or `draw_arc` for an open arc), and asserts the host
  gained a face: a profile that divides nothing is a refusal and wrote nothing.
- **What stayed out, and why.**
  - **Edges do not host a drawing commit.** An edge frame names a direction, not a plane; the
    tool draws on the workplane and creates a separate object, which is the honest result
    for "a profile along an edge". Documented as the rule rather than half-implemented.
  - **No primitive placement yet.** `Box`/`Cylinder`/… have no stage tool, so the C4 row's
    `⊕ Add` / `⊖ Cut` boolean commit — which needs a box ghost and a height handle — is C1/C2
    work on top of this seam, not part of it. `_push_pull` remains the boolean hook.
  - **The cue is a fixed 2 mm.** A screen-constant cue needs the camera in the tool; the
    marker's world size is the cheap, honest version.
  - **`ToolField` is still C2.** The tools keep their own numbers; nothing in this slice
    publishes a docked field.

[`creation-loadout.md`]: ../../developer/interaction/creation-loadout.md

### Step 6 — C1, the dock contract and the creation pill

The live window exposed a chrome defect, not a modelling one: every Part/SimpleCAD command
declared `FerroCAD_Surface: "dock"` (and SimpleCAD's drawing tools declared the edition name
`"SimpleCAD"`, an unknown value that fell back to the dock), so the bottom-centre strip became
a twenty-button row that overflowed under the bottom-left loadout bar.

- **Two surfaces were missing.** `Surface` gained `creation` (a creation tool: offered by the
  split pill and Search) and `palette` (registered and searchable, offered by no canvas
  surface). The **fallback for an unannotated or unknown surface moved from `dock` to
  `palette`**, so no mod can overflow a canvas strip again while staying reachable in Search.
- **Every command was re-filed.** Drawing tools (`SimpleCAD_Rectangle|Circle|Polygon|Arc`,
  `Part_Box`, `Part_Polyline`, `Sketch_Polyline`, `Sculpt_MakeCage|MakeSkin`,
  `Toy_MakeSlab|Polyline`) → `creation` with `FerroCAD_Category` (`Sketch`/`Solid`/`Mesh`);
  `SimpleCAD_Loft|Sweep` → `creation`/`Surface`; shape and document verbs → `radial` with
  `FerroCAD_Target` (`body`/`face`/`edge`) and a tone; utilities → `palette`.
- **The pill is one control.** `⊕ <label>` runs the remembered `creation` command (the first
  `Solid` tool when nothing is armed), `▾` opens the flyout, whose rows are the `creation`
  commands grouped by category in the design's order with a hairline between groups. Arming a
  tool remembers it; picking a wheel verb does not change it.
- **Two hands cannot both own the canvas.** The hand column dims to 45 % while a creation tool
  is armed, and the dock dims the same way while a transform or manipulator is pending.
- **The wheel is filtered by the target kind.** `radial_slots` reads the target's element
  (`Face6`/`Edge3`) and offers only the verbs whose `FerroCAD_Target` names that kind, so the
  ring is the design's five-to-eight wedges; `App_Delete` is pinned to the final destructive
  slot. The empty-space fallback is the **creation** row, not the dock globals.
- **Limits, recorded.** The flyout is one flat list with dividers, not the two-tier
  hierarchy; the primitive placements beyond Box (`Cylinder`, `Cone`, `Sphere`) and the
  Box's `⊕ Add`/`⊖ Cut` mode are later slices.

### Step 7 — C1 and C2, the gate, the strip and `PILL_HOLD`

The rest of the Add hand, on the same day:

- **`FerroCAD_Gate` is data.** `CommandInfo.gate` parses `<kind>:<min>[:<max>]`
  (`TopologyGate`, `ElementKind`) and the chrome evaluates it against the selection — not
  the Modify hand's slot, so `Loft` is enabled by two profiles whatever slot is equipped.
  An unmet gate paints the row in `ui::INACTIVE_STRUCTURAL` (`#94A3B8`) with the
  requirement as its right-hand hint, and the pill's arm reports it on a click. Declared
  on `SimpleCAD_Loft|Sweep|Shell|Draft|FillHole|Unwrap|RemoveRounds|Subdivide`,
  `Part_Chamfer|HealFaces` and the booleans.
- **The scope is protected while a tool is armed.** `Select ▾` refuses `SwitchScope` when
  the armed creation tool's gate is unmet, naming the tool and what it needs.
- **C2's strip.** `ToolStatus::fields` + `Tool::set_field` drive one tabular-monospace row
  in the strip; `Tab` cycles, a digit replaces, `Backspace` edits, `Enter` commits, the
  inner `Esc` drops the field. `ferrocad_viewport::set_tool_field` is the write path.
- **The Box tool.** `Part_Box` now returns `{"tool": "box"}` and a new
  `ferrocad_stage::BoxTool` previews a W×L×H box at the workplane (or the hovered face)
  and commits on `Enter` — the zero-mouse path the old dock button had, without the
  button. `makeBox` gained a `z` so the corner is the one the tool placed.
- **`PILL_HOLD`** (1.2 s) accents the pill after a creating commit; it is not a context and
  gates nothing. **Pass-through** is the shell's: a press through the open flyout merges
  `creation_pill` into the report `hover_line` shows, so the viewport's `HoverReport` stays
  pure.

**Measured.** `ferrocad_stage` 303, `ferrocad_viewport` 237, `ferrocad_shell_core` 298;
`chrome_surfaces` 5 cases; a new `ferrocad_parametric` `box_tool_round_trip` (3 cases:
the command arms the tool, a typed width commits a 30 mm box, the zero-mouse `Enter`
places it at the origin); the six `tool_fields` shell cases. `run_tests.py --extension
base` **718/0/0/8** and `--extension workbenches` **881/0/0/3**, unchanged.

### Step 8 — the hierarchical wheel and Slot 3's two dress-ups

Two defects the live window showed, and the fix for each:

- **The wheel was a twenty-wedge circle.** Every SimpleCAD/Part verb was a root wedge, so
  the labels overlapped. A radial verb may now declare `FerroCAD_WheelGroup`
  (`Workplane`, `Booleans`, `Surface`, `Edge`, `Modify`) and the group is **one** wedge:
  `radial_slots` partitions the offered verbs, drops a disabled child, omits an empty
  group, and collapses a one-child group into that child (so the base edition's ring is
  unchanged). `wheel_ring`/`WheelMenu` keep the ring hierarchical — a click (latched),
  a ~140 ms dwell, or a drag past the outer rim (armed) opens the children; the deadzone
  and `Backspace` climb back out, `Esc` closes from any depth — and both rings are clamped
  to `MAX_WEDGES = 8` so a label can never collide. The root is the direct verbs only:
  `Move`, `Rotate`, `Toggle visibility`, the categories, and `Delete` last.
- **`Fillet` and `Chamfer` were two competing controls.** Slot 3 now owns both:
  `DressUpKind { Fillet (default), Chamfer }` lives on the `Loadout`, `3` on an equipped
  Slot 3 toggles it, and `manipulator_for(selection, stage, dress_up)` hands the mode to
  the factory (`fillet_manipulator` maps it to `DressUpMode`). The commit's action is the
  mode, so the document writes a `Part::Fillet` or a `Part::Chamfer` in one transaction.
  Picking a feature's band or one of its edges reports its **kind** in the manipulator
  probe (`ManipulatorProbe::mode`), so re-opening a chamfer syncs Slot 3 to `3 Chamfer`
  (`R`/`D` in the declared grip) instead of rebuilding a fillet over it.

**Measured.** `ferrocad_widgets` **76** (13 wheel cases, 5 of them new: descend, commit a
child, flick past the rim, dwell, the eight-wedge guard); `ferrocad_shell_core` **302**
(`slot_three_toggles_between_fillet_and_chamfer`, `grouped_verbs_share_one_wedge`,
`backspace_dismisses_the_root_ring`); `ferrocad_part_gui` **42**
(`the_factory_builds_the_dress_up_slot_three_names`); `ferrocad_parametric` gained
`the_slot_three_chamfer_mode_drives_the_document`; `chrome_surfaces` **6** cases.

### Step 9 — a stable wheel root, the Add hand's two slots, and profile gates

Three follow-ups from the live window:

- **The wheel's root stopped reflowing.** Every root verb was filtered by the target
  kind, so picking a **face** made `Move` and `Rotate` disappear; and the no-selection
  fallback offered every creation tool flat, which was nine wedges — over the eight-wedge
  cap (a debug build asserted on it). The root is now the *direct* verbs (no
  `FerroCAD_WheelGroup`) on every target: `Move`, `Rotate`, `Toggle visibility`,
  `Workplane ▸`, `Create ▸`, `Modify ▸`, `Delete` — seven, whatever the edition
  registers. The target filters the **children**: `Modify ▸` holds the object/sub-shape
  verbs (bucketed into `Surface ▸`/`Edge ▸`/`Booleans ▸` only when they exceed a ring),
  `Create ▸` holds the creation categories and their tools, `Workplane ▸` the two
  workplane verbs. A category with no applicable child is omitted. The hollow centre now
  paints `◂ Back` beside the open category's name.
- **The Add hand is its own strip, one pill per category.** The dock's single memory
  pill became a **create dock** in the bottom row, right after the Modify bar:
  `5 ⊕ Sketch: Rectangle ▾  6 ⊕ Solid: Box ▾  7 ⊕ Surface: Loft ▾  9 ⊕ Mesh: Mesh Box ▾`,
  one per registered category (a category with no tool is not a pill), permanently — there
  is no create-dock toggle. The globals (`Undo`, `Redo`, `Fit`, Search) are a vertical
  pill on the canvas' **left edge**, so the dock never holds a tool. Each pill
  remembers its category's tool and leads with its key, as the Modify bar does — `5`–`9`
  are the Add hand's, continuing the Modify hand's `0`–`3` so one digit space addresses
  both hands (`4` is left unbound). A key press **selects** the pill and starts the tool
  it names (the pill takes the accent, like the Modify bar's hand) and a press on the pill
  that already holds the armed tool **steps to the next tool of that category**, wrapping
  and skipping a tool the selection cannot use — the slot-with-variants shape Slot 1 has
  for Move ⇄ Rotate. The key never opens the menu. The flyout shows **one category at a
  time** behind a segmented header, with a tab click pointing the pill at the remembered
  tool and a row click binding and running it. The rows carry `1`–`8` in their own column
  and those digits pick a row while the flyout is open (the panel owns the keyboard, as
  the wheel does), so a category is navigable without the pointer; the pill's caret reads
  `^` while its list is showing, and holding `Shift` peeks the equipped pill's list while
  the key is down. The wheel's `Create ▸` commits through the same path, so a flick
  loads the pill and arms the tool at the pointer.
- **A session adds a row; it does not take the dock.** The armed tool's row (its label,
  its `W/L/H` row or point count, its hint, `✗`/`✓`) joins the **bottom-left column,
  directly above the hands' bar**, and only once the tool has begun: a click placed a
  point, or its fields are the input. An armed tool that has placed nothing shows only its
  hover ghost, so arming reads as equipping a slot, not as opening a dialog. The column
  grows upward and the canvas keeps its rectangle (`chrome::globals_dock` owns the left
  column, `chrome::create_dock` the bottom row, `chrome::tool_pill` the row above it).
- **`Loft`/`Sweep` say what they need.** Their gate is `profile:2`, a new
  `ElementKind::Profile` that a body, a face **or** a wire satisfies, and the hint reads
  `select at least 2 profiles` instead of `select at least 2 bodies`.

Also fixed: `the_move_hud_anchors_to_the_gizmo_not_the_pointer` compared a projection
across a re-layout (the hover changes the status line, which can move the canvas) and
failed once the dock grew; it now re-reads the gizmo's projection after the move, which
is the invariant it means to test.

**Measured.** `ferrocad_shell_core` **306** (`the_root_ring_is_stable_and_the_categories_carry_the_context`,
`grouped_verbs_live_under_their_category`,
`each_creation_pill_remembers_its_own_tool`, plus three `CreationDock` unit cases);
`ferrocad_widgets` **76** (13 wheel, incl. the centre-hub `◂ Back`); `ferrocad_shell_api`
**75** (`a_profile_gate_names_profiles_and_accepts_any_closed_reference`);
`ferrocad_simplecad` `chrome_surfaces` **6** (the profile gate and the `Surface` group);
`run_tests.py --extension base` **718/0/0/8** and `--extension workbenches` **881/0/0/3**,
unchanged; `--check-init` OK, `xtask editions --check` green.

### Measured

| Check | Result |
| --- | --- |
| `ferrocad_simplecad` | **10** selection verbs + **9** loadout + **6** drawing + **4** face-hosted + **6** chrome-surface cases |
| `ferrocad_stage` / `ferrocad_viewport` / `ferrocad_shell_api` | **303** / **237** / **75** |
| `ferrocad_parametric` / `ferrocad_toy` / `ferrocad_part_gui` | all green (42 part_gui, 18 toy unit) |
| `ferrocad_shell_core` / `ferrocad_widgets` | **306** / **76** (the wheel hierarchy, the Add hand's slots and Slot 3) |
| `run_tests.py --extension base` / `workbenches` | **718 / 0 / 0 / 8**, **881 / 0 / 0 / 3** |
| `--check-init` (both editions) | `ferrocad: init OK` |
| `xtask editions --check` | green, 14 editions (no dependency change) |

## 9. Open questions

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
