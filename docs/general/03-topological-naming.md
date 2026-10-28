# Chapter 3: Time travel and the topological naming system (TNS)

Because B-Rep CAD is **parametric**, a model is not just a static shape — it is a
chronological recipe: sketch a square, extrude it into a box, fillet `Edge4`.

### 3.1 The topological naming problem (TNP)

Every time you change an earlier step, the CAD kernel (OCCT) destroys the old 3D shape
and calculates a brand-new one from scratch.

If you go back to the first step and turn the square into a hexagon, the new solid has
18 edges instead of 12. What used to be `Edge4` might now be `Edge9`. If the fillet step
only remembered "fillet `Edge4`", the fillet jumps to the wrong edge or the model
breaks.

### 3.2 Where the topological naming system lives

The **topological naming system (TNS)** is the genealogical ledger that addresses this.
It assigns persistent, historical names to geometry based on how it was born — "the
edge created by intersecting Face1 and Face2" — rather than volatile index numbers.

- **Invisible to the GPU and the viewport.** TNS lives on the CPU. Meshes, Coin3D and
  OpenGL know nothing about topological history.
- **Global utility vs. namespace reality.** Architecturally, every workbench needs
  TNS: **Part Design** for features, **TechDraw** so 2D dimensions do not detach when
  the 3D model updates, and **Assembly** to keep joints mated. In FreeCAD's C++ source,
  however, the TNS ledger (`ElementMap`) is physically housed inside the **`Part`**
  namespace (`Part::TopoShape`), because `Part::TopoShape` is the foundational wrapper
  around OCCT's `TopoDS_Shape` and its operation history (`BRepTools_History`).
