# A gentle introduction to 3D CAD architecture

*From mathematical curves to GPU silicon and parametric history.*

When you first open a 3D CAD program like FreeCAD, the 3D world on your screen looks
unified and seamless. You can rotate a cylinder, click on its edges, and modify its
size. Under the hood, the software is running a dual-identity system: it constantly
translates pure, abstract mathematics into a visual language the hardware understands.

To use 3D design well — or to architect and script CAD software — it helps to
understand the difference between what a 3D model *is* in memory, how its history is
tracked, and how it is drawn.

Chapters 1–4 are written against **FreeCAD**, because its vocabulary (OCCT, B-Rep,
Coin3D, `Part::TopoShape`) is the one most CAD users and workbench authors already
share. Chapter 5 maps each layer to **FerroCAD**'s crates.

## Chapters

1. [The DNA of 3D geometry](01-the-dna-of-3d-geometry.md) — vectors, NURBS, B-Rep and meshes.
2. [The great translation](02-the-great-translation.md) — tessellation, Coin3D and the GPU pipeline.
3. [Topological naming](03-topological-naming.md) — parametric history and the topological naming problem.
4. [Decoupling the TNS](04-decoupling-the-tns.md) — an external, pluggable naming layer.
5. [How FerroCAD organizes this](05-how-ferrocad-organizes-this.md) — the crates behind each layer.
