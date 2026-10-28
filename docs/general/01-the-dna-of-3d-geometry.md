# Chapter 1: The DNA of 3D geometry (vectors vs. bitmaps in 3D)

The easiest way to understand 3D geometry is to compare it with 2D graphic design. In
the 2D world we divide images into **vector graphics** (Illustrator, SVG) and
**raster/bitmap graphics** (Photoshop, JPEG). 3D software has the same divide.

| 2D design concept | 3D CAD equivalent | What it actually does |
| --- | --- | --- |
| **Bézier curves (math)** | **NURBS** | Stores infinite, smooth mathematical equations for curves and surfaces. |
| **Compound paths / layer masks** | **B-Rep (boundary representation)** | Stitches mathematical surfaces together into a structured, watertight solid part. |
| **Flat JPEG / pixel grid** | **Polygon mesh (STL / OBJ)** | Discards the math, baking the shape into a rigid network of flat triangles. |
| **Live on-screen canvas preview** | **Viewport data (Coin3D)** | Temporarily converts the math into triangles so the monitor can display it. |

### 1.1 NURBS: the 3D vector curves

To build smooth, organic or exact shapes, CAD software uses **NURBS (non-uniform
rational basis splines)**.

- **How it works.** Instead of plotting thousands of individual points, NURBS uses a
  small set of weighted control points and polynomial equations to guide a curve or
  surface — much like pulling on a sheet of digital spandex.
- **Infinite resolution.** Because it is stored as an equation, you can scale a NURBS
  surface up by 1,000,000% and the curve remains smooth.
- **In code.** Inside FreeCAD's geometric kernel (**Open CASCADE / OCCT**), NURBS are
  handled at the C++ level by classes such as `Geom_BSplineCurve` and
  `Geom_BSplineSurface`, and exposed to users through the **Part** and **Curves**
  workbenches.

### 1.2 B-Rep: the structural ledger

NURBS provides the math for an individual sheet or curve, but it does not know what a
"solid object" is. **B-Rep (boundary representation)** is the organizing system that
stitches surfaces together into a solid volume. It combines two layers:

1. **Geometry (the muscle).** The exact mathematical equations of the surfaces
   (NURBS, planes, cylinders).
2. **Topology (the skeleton).** A structural ledger tracking which **faces** meet at
   which **edges**, and which edges terminate at which **vertices**.

Because B-Rep is exact, formats such as **STEP (.stp)** are the usual exchange format
for CNC machining and mechanical engineering.

### 1.3 Polygon meshes: the 3D bitmap

A **mesh** abandons mathematical equations entirely. It approximates a 3D shape with a
dense web of flat polygons, usually triangles.

- **The faceted reality.** In a mesh, a cylinder is not a curve — it is dozens of flat
  rectangular strips arranged in a ring. Zoom in closely and you see the edges.
- **Zero intelligence.** A mesh has no concept of "features". A cylindrical hole in an
  **STL** or **OBJ** file is not a cylinder with a radius; it is an empty space
  surrounded by hundreds of anonymous triangles.
