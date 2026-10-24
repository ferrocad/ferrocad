"""A parametric gear train for the headless Parametric demo.

Run it with the Part-linked edition (needs OCCT and a wgpu adapter)::

    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
      cargo run -p ferrocad_parametric -- \
        --script examples/headless_gear.py --render target/demo/gear.png \
        --stl target/demo/gear.stl --size 1200x900

Each gear is one fused solid: a toothed rim, a raised hub, a central bore and a
ring of lightening holes. The teeth are trapezoidal prisms built from a closed
wire, extruded and rotated around Z; ``multiFuse`` joins them to the disc. The
second gear is placed with an object ``Placement`` at the centre distance and
rotated half a tooth pitch, so the pair meshes. A base plate and two shafts make
the assembly read as a mechanism, not a single primitive.

This exercises the same Part surface the conformance suite covers:
`makeCylinder`, `makePolygon`/`makeFace`, `extrude`, `Shape.rotate`,
`multiFuse`, `cut`, object `Placement`, `ElementMapSize`, and `.FCStd`
persistence.
"""

import math
import os

import FreeCAD as App
import Part

TEETH = 18
ROOT_RADIUS = 38.0
TIP_RADIUS = 50.0
THICKNESS = 10.0
HUB_RADIUS = 14.0
HUB_HEIGHT = 26.0
BORE_RADIUS = 7.0
WEB_HOLE_COUNT = 5
WEB_HOLE_CENTRE = 26.0
WEB_HOLE_RADIUS = 5.0
CENTRE_DISTANCE = 2.0 * (ROOT_RADIUS + TIP_RADIUS) / 2.0

OUT = os.path.abspath("target/demo/gear.FCStd")
os.makedirs(os.path.dirname(OUT), exist_ok=True)


def make_gear():
    """One spur gear, centred on the origin."""
    body = Part.makeCylinder(ROOT_RADIUS, THICKNESS)
    hub = Part.makeCylinder(HUB_RADIUS, HUB_HEIGHT)

    # Trapezoidal teeth, overlapping the disc so the fuse is a solid.
    pitch_arc = 2.0 * math.pi * ROOT_RADIUS / TEETH
    root_half = pitch_arc * 0.28
    tip_half = pitch_arc * 0.14
    teeth = []
    for index in range(TEETH):
        points = [
            App.Vector(ROOT_RADIUS - 2.0, -root_half, 0.0),
            App.Vector(TIP_RADIUS, -tip_half, 0.0),
            App.Vector(TIP_RADIUS, tip_half, 0.0),
            App.Vector(ROOT_RADIUS - 2.0, root_half, 0.0),
        ]
        face = Part.makeFace(Part.makePolygon(points, closed=True))
        tooth = face.extrude(App.Vector(0.0, 0.0, THICKNESS))
        tooth.rotate(
            App.Vector(0.0, 0.0, 0.0),
            App.Vector(0.0, 0.0, 1.0),
            index * 360.0 / TEETH,
        )
        teeth.append(tooth)

    gear = body.multiFuse([hub] + teeth)

    # The central bore and the web lightening holes. The hole cylinders run one
    # millimetre past each face so they cut cleanly instead of leaving a skin.
    gear = gear.cut(Part.makeCylinder(BORE_RADIUS, HUB_HEIGHT + 2.0, App.Vector(0, 0, -1)))
    for index in range(WEB_HOLE_COUNT):
        angle = 2.0 * math.pi * index / WEB_HOLE_COUNT + 0.3
        centre = App.Vector(
            WEB_HOLE_CENTRE * math.cos(angle),
            WEB_HOLE_CENTRE * math.sin(angle),
            -1.0,
        )
        gear = gear.cut(Part.makeCylinder(WEB_HOLE_RADIUS, THICKNESS + 2.0, centre))
    return gear


doc = App.newDocument("GearTrain")

plate = doc.addObject("Part::Feature", "BasePlate")
plate.Shape = Part.makeBox(190.0, 90.0, 6.0, App.Vector(-51.0, -45.0, -6.0))

first = doc.addObject("Part::Feature", "Gear1")
first.Shape = make_gear()

second = doc.addObject("Part::Feature", "Gear2")
second.Shape = make_gear()
second.Placement = App.Placement(
    App.Vector(CENTRE_DISTANCE, 0.0, 0.0),
    App.Rotation(App.Vector(0.0, 0.0, 1.0), 180.0 / TEETH),
)

for name, x in (("Shaft1", 0.0), ("Shaft2", CENTRE_DISTANCE)):
    shaft = doc.addObject("Part::Feature", name)
    shaft.Shape = Part.makeCylinder(6.5, 40.0, App.Vector(x, 0.0, -6.0))

doc.recompute()
doc.saveAs(OUT)

print(
    "headless_gear:",
    f"{len(doc.Objects)} objects,",
    f"{TEETH} teeth each,",
    f"centre distance {CENTRE_DISTANCE:.0f} mm,",
    f"element map {doc.getObject('Gear1').Shape.ElementMapSize},",
    f"saved {OUT}",
    flush=True,
)
