# A gentle introduction to 3D CAD architecture

*From mathematical curves to GPU silicon and parametric history.*

When you first open a 3D CAD program like FreeCAD, the 3D world on your screen looks
unified and seamless. You can rotate a cylinder, click on its edges, and modify its
size. Under the hood, the software is running a dual-identity system: it constantly
translates pure, abstract mathematics into a visual language the hardware understands.

To use 3D design well — or to architect and script CAD software — it helps to
understand the difference between what a 3D model *is* in memory, how its history is
tracked, and how it is drawn.

## The short version

- A CAD part is stored as **formulas** (analytic surfaces and NURBS), trimmed and
  stitched into a **B-Rep** solid. A mesh (STL) is a bag of triangles with no memory of
  how it was made.
- What you see on screen is a **temporary triangle preview** (tessellation). A circle
  that looks slightly jagged is still a perfect circle in the model.
- A parametric model is a **recipe**, not a sculpture. Editing an early step rebuilds
  everything that follows.
- Rebuilding produces new topology, so FreeCAD names elements by **family tree** (the
  topological naming system) rather than by position — and even that can fail, so a
  broken reference should fail loudly rather than move silently.

Chapters 1–4 are written against **FreeCAD**, because its vocabulary (OCCT, B-Rep,
Coin3D, `Data::ComplexGeoData`, `Part::TopoShape`) is the one most CAD users and
workbench authors already share. Chapter 5 maps each layer to **FerroCAD**'s crates.

## Who this is for

Python script writers, UI designers, and new contributors who know 2D vector graphics.
No CAD or geometry-kernel background is assumed: each idea starts from a 2D analogue
(SVG paths, clipping masks, pixels) and only then names the 3D machinery. The chapters
describe how FreeCAD actually works, including its limits, so the mental model does not
have to be unlearned later.

## Chapters

1. [The DNA of 3D geometry](01-the-dna-of-3d-geometry.md) — analytic surfaces, NURBS, B-Rep and meshes.
2. [The great translation](02-the-great-translation.md) — tessellation, Coin3D and the GPU pipeline.
3. [Topological naming](03-topological-naming.md) — parametric history, splits and merges, and the naming problem.
4. [How FreeCAD organizes geometry](04-how-freecad-organizes-geometry.md) — the `App`/`Part` split and why kernels cannot simply be swapped.
5. [How FerroCAD organizes this](05-how-ferrocad-organizes-this.md) — the crates behind each layer.
