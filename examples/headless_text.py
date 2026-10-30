"""3-D text for the headless Parametric demo: "ferrocad", extruded and raised.

Run it with the Part-linked edition (needs OCCT; no GPU for the STL path)::

    cargo run -p ferrocad_parametric -- \
      --script examples/headless_text.py --stl target/demo/text.stl --size 1200x900

The text is not a kernel concept and this script is deliberately short: one
`Draft.make_shape_string` call turns a font into a compound of planar faces (the
counters are holes, made by `Part::FaceMakerBullseye`), and the rest is Part. A
bare wire extrudes to an open shell, so the faces are what make a solid; each
face is extruded, the glyphs are fused, and the result is raised on a plate.

The font defaults to the OFL fixture vendored for the tests; set
`FERROCAD_FONT` to any `.ttf`/`.otf`/`.ttc`/`.pfb` to draw with another one.
"""

import os

import FreeCAD as App
import Draft
import Part

HEIGHT = 20.0  # cap height, in mm (ScaleToSize is on by default)
DEPTH = 2.0  # extrusion, in mm
FONT = os.environ.get("FERROCAD_FONT") or os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates",
    "ferrocad_text",
    "tests",
    "fixtures",
    "fonts",
    "LiberationMono-Regular.ttf",
)

doc = App.newDocument("Text3D")
string = Draft.make_shape_string("ferrocad", FONT, HEIGHT, 1.0)

# One face per glyph; extrude each, then fuse the disjoint letters.
solids = [face.extrude(App.Vector(0, 0, DEPTH)) for face in string.Shape.Faces]
text = solids[0].multiFuse(solids[1:])

# The compound already sits at the origin with its baseline on y = 0; the round
# letters overshoot the baseline, so the plate uses the bounding box.
bound = text.BoundBox
plate = Part.makeBox(bound.XLength + 4, bound.YLength + 4, 3)
raised = text.copy()
raised.translate(App.Vector(-bound.XMin + 2, -bound.YMin + 2, plate.BoundBox.ZMax))
result = plate.fuse(raised)

# Keep the 3-D result only: the ShapeString's flat faces would duplicate the
# plate's top in the STL.
doc.removeObject(string.Name)
feature = doc.addObject("Part::Feature", "Text3D")
feature.Label = "ferrocad (raised)"
feature.Shape = result
doc.recompute()
