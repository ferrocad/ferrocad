# Chapter 1: The DNA of 3D geometry (vectors vs. bitmaps in 3D)

The easiest way to understand 3D CAD geometry is to compare it with 2D graphic design.
In the 2D world we divide images into **vector graphics** (Illustrator, SVG) and
**raster/bitmap graphics** (Photoshop, JPEG). 3D software has the same divide.

| 2D design concept | 3D CAD equivalent | What it actually does |
| --- | --- | --- |
| **Basic shapes and Bézier curves** | **Analytic surfaces and NURBS** | Defines infinite, mathematically smooth surfaces: planes, cylinders, and freeform curves. |
| **Clipping masks and stitched paths** | **B-Rep (boundary representation)** | Trims infinite surfaces along boundary curves and stitches them into a watertight solid. |
| **Flat JPEG / pixel grid** | **Polygon mesh (STL / OBJ)** | Discards the math, baking the shape into a rigid network of flat triangles. |
| **Live on-screen preview** | **Viewport data (Coin3D)** | Temporarily converts the math into triangles so the monitor can display it. |

### 1.1 Mathematical surfaces: from cylinders to NURBS

To build smooth, exact shapes, CAD software does not place thousands of tiny dots.
It uses pure mathematical formulas, the 3D equivalent of vector paths:

- **Analytic shapes (the everyday primitives).** Most mechanical parts are made of
  simple, exact formulas: flat planes, cylinders, cones and spheres. Just as an SVG
  stores a circle as a centre point and a radius ($x^2 + y^2 = r^2$), CAD stores a
  cylinder as an axis and a radius. In OCCT these are described by classes such as
  `gp_Cylinder`, `gp_Plane` and `Geom_CylindricalSurface`.
- **NURBS (the freeform splines).** For organic shapes such as car bodies or ergonomic
  handles, CAD uses **NURBS** (non-uniform rational B-splines). Much like pulling the
  handles of a 2D Bézier curve in Illustrator, NURBS use weighted control points to
  bend a smooth, continuous sheet in 3D space. OCCT names them `Geom_BSplineCurve` and
  `Geom_BSplineSurface`, and the Part workbench exposes them through the
  `Part.BSplineCurve` and `Part.BSplineSurface` classes.
- **Infinite resolution.** Because both are stored as formulas, you can scale a part up
  by 1,000,000% and the curves remain smooth. The formula has no pixels to run out of.

A surface on its own is still just an infinitely thin, untrimmed sheet. It does not
know what a "solid object" is, or where a hole was drilled through it. That is the job
of the next layer.

### 1.2 B-Rep: the tailor's sewing pattern

**B-Rep (boundary representation)** turns raw mathematical sheets into a solid part.
Think of making a tailored suit:

1. **Geometry (the fabric).** The raw mathematical surfaces: planes, cylinders, NURBS
   sheets.
2. **Trimming and topology (the scissors and stitching).** The kernel draws boundary
   curves on each surface to cut out the exact shape — for example a circular hole in a
   flat sheet — and keeps a strict structural ledger of how the pieces connect:

   - **Vertices** are corner points.
   - **Edges** are seam lines; each edge sits on a curve, and some are *seam edges*
     where a closed surface (such as a cylinder) is sewn to itself.
   - **Wires** are chains of edges. A closed wire is the boundary of a face; an open
     wire is just a chain, as in a sketch.
   - **Faces** are trimmed patches of surface.
   - **Shells** are sets of faces joined along their edges. A closed shell bounds a
     **solid**.

3. **Tolerances (the seam allowance).** Where two curved surfaces meet, the exact
   intersection is rarely expressible in clean numbers. B-Rep therefore stores a
   microscopic tolerance on every edge and vertex, so the kernel can agree that the
   surfaces are sealed without demanding an impossible exactness. "Watertight" means
   "closed within the stated tolerance", not "mathematically identical".

In FreeCAD, the Part workbench is the B-Rep side of this story. `Part::TopoShape` wraps
an OCCT `TopoDS_Shape`, and Python sees it as `Part.Shape` with `.Faces`, `.Edges`,
`.Wires`, `.Shells` and `.Solids`. Not every `Shape` is a solid: a sketch is a wire, a
surface patch is a face, and an imported sheet is a shell.

Because B-Rep preserves exact geometry and boundaries, formats such as **STEP (.stp)**
are the usual exchange format for CNC machining and mechanical engineering.

### 1.3 Polygon meshes: the 3D bitmap

A **mesh** abandons mathematical formulas entirely. It approximates a 3D shape with a
dense web of flat polygons, usually triangles.

- **The faceted reality.** In a mesh — an **STL** or **OBJ** file, for example — a
  cylinder is not a curve. It is dozens of flat rectangular strips arranged in a ring.
  Zoom in closely and you see the flat edges.
- **Zero intelligence.** A mesh has no concept of "features". A cylindrical hole in an
  STL file is not a hole with a diameter you can edit; it is an empty space surrounded
  by hundreds of anonymous triangles.

That is the trade: a mesh is simple and universal, and it forgets why the shape looks
the way it does.
