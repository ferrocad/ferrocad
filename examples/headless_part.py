"""A headless Part demo for `ferrocad_parametric --script`.

Run it with the Part-linked edition (needs OCCT and a wgpu adapter)::

    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
      cargo run -p ferrocad_parametric -- \
        --script examples/headless_part.py --render target/demo/part.png

The three solids are built in world coordinates, so the script does not depend on
the view provider applying an object `Placement`:

* a plain box,
* the same box with a cylindrical bore (a real OCCT boolean),
* the same box with a filleted face edge (a real dress-up).

It also prints the bore's element-map size and saves the document beside the
render. Reopen that file in the shell with `--open`, or render it headlessly:

    cargo run -p ferrocad_parametric -- --open target/demo/headless.FCStd
    cargo run -p ferrocad_parametric -- --open target/demo/headless.FCStd --render target/demo/reopened.png

The `Part` API and these operations are covered by the in-process suites
(`cargo test -p ferrocad_part_py --test part_shape_ops`).
"""

import os

import FreeCAD as App
import Part

OUT = os.path.abspath("target/demo/headless.FCStd")
os.makedirs(os.path.dirname(OUT), exist_ok=True)

doc = App.newDocument("Headless")

# --- geometry ---------------------------------------------------------------
plain = Part.makeBox(20.0, 20.0, 10.0)

bored = plain.cut(Part.makeCylinder(4.0, 10.0, App.Vector(10.0, 10.0, 0.0)))

rounded_box = Part.makeBox(20.0, 20.0, 10.0)
rounded = rounded_box.makeFillet(1.0, rounded_box.Faces[0].Edges)

# --- document objects -------------------------------------------------------
# The shapes are local; the object Placement lays them out. The Parametric display
# hook bakes the placement into the tessellation, which is also what a reopened
# `.FCStd` needs.
for name, shape, x in (
    ("Plain", plain, 0.0),
    ("Bored", bored, 30.0),
    ("Rounded", rounded, 60.0),
):
    feature = doc.addObject("Part::Feature", name)
    feature.Shape = shape
    feature.Placement = App.Placement(App.Vector(x, 0.0, 0.0), App.Rotation())

doc.recompute()
doc.saveAs(OUT)

print(
    "headless_part:",
    f"{len(doc.Objects)} objects,",
    f"bore volume {bored.Volume:.2f},",
    f"element map {bored.ElementMapSize},",
    f"saved {OUT}",
)
