# Chapter 3: Time travel and the topological naming system (TNS)

Because B-Rep CAD is **parametric**, a model is not just a static shape. It is a
chronological recipe: *Step 1: sketch a square. Step 2: extrude it into a box. Step 3:
fillet (round off) `Edge4`.*

### 3.1 The topological naming problem (TNP)

Every time you edit an earlier step, the CAD kernel (OCCT) destroys the old 3D shape and
builds a brand-new one from scratch.

If you go back to Step 1 and change the square into a hexagon, the new solid has 18
edges instead of 12. The kernel does not hand back anything called `Edge4`; it hands
back topology, and FreeCAD's fallback names are *positional*: it walks the shape and
numbers the edges in traversal order. What used to be `Edge4` might now be `Edge9`. If
Step 3 only remembered "put a fillet on `Edge4`", the fillet jumps to the wrong edge, or
the model breaks completely.

That is the **topological naming problem**: sub-element names are positions in a result,
not identities of a feature.

### 3.2 How the topological naming system helps, and why it is hard

To stop features from jumping around, FreeCAD uses a **topological naming system
(TNS)**. Instead of relying on `Edge4`, it tries to give every face and edge a
**genealogical name** based on how it was created — for example, *"the top edge created
when `SketchLine2` was extruded in `Pad1`"*.

The raw material is a per-operation history. When OCCT performs an extrusion or a
boolean, it can report which old shapes were *modified*, *generated* or *removed* to
make the new shape (`BRepTools_History`, or the `Modified` / `Generated` / `IsDeleted`
calls on the operation builder). FreeCAD's Part module folds that history into a
lookup table, `Data::ElementMap`, that maps a stable name to a sub-shape.

Two details explain why this is engineering rather than bookkeeping:

- **The kernel reports relationships; it never reports names.** "Input face A was
  modified into output face X" is all OCCT says. Every durable name in FreeCAD is
  generated on the FreeCAD side from that relationship.
- **FreeCAD does not use stock OCCT booleans.** Since 2024 the Part module wraps them
  (`FCBRepAlgoAPI_*`) to control fuzzy tolerance, keep the original topology available
  and flatten compound tools. Those choices decide what the history reports as
  *modified*, and therefore what the naming can see.

Even with that history, turning single-step relationships into unbreakable names is one
of the hardest problems in CAD software:

- **Splits and merges.** Cut a groove through the middle of `Edge4` and one edge becomes
  two, both with the same parent history. The naming system must use fallback rules to
  decide which piece keeps which derived name.
- **Vanishing ancestors.** If an edit temporarily removes a face, or merges two coplanar
  faces into one, the chain that named the descendant breaks. The reference has to fail
  cleanly rather than silently point at a different element.
- **Re-derivation.** The map is versioned (`ElementMapVersion`). If the version does not
  match on load, FreeCAD re-derives the map from the shape rather than trusting stale
  names.

Because there is no mathematically perfect way to predict what a human designer *meant*
when a shape changes drastically, topological naming is always a best-effort detective
game, not a magic bullet. Good CAD software aims to fail loudly and locally — a broken
reference you can repair — rather than to move a fillet silently.

FerroCAD takes the same stance in Rust: `ferrocad_lineage` records element references,
per-operation history and forward/reverse name maps, and Part refuses to rebind a name
that is absent from a non-empty map. That does not make the problem disappear; it makes
the failure visible and testable without the kernel in the loop. Chapter 5 shows where
that layer sits, and
[`tns-decoupling.md`](../project/research/tns-decoupling.md) records what is still
open.
