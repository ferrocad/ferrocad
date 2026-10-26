"""App-shell script for the **toy workbench**.

The kernel-free reference edition. It demonstrates the four things a workbench does,
with no geometry kernel behind any of them:

1. **Create objects** — :func:`makeSlab` adds a typed, property-bearing
   ``App::FeaturePython`` inside one named transaction (so it is undoable).
2. **Represent them** — the display provider builds the mesh from the object's
   ``Width``/``Depth``/``Height``/``Placement`` and honours its ``Color`` and
   ``DisplayMode`` properties.
3. **Manipulate them** — two generic mechanisms drive the same document:
   the ``toy_height`` interaction (the Rust ``HeightGrip`` manipulator) and the
   ``transform`` interaction (the Rust ``TransformGizmo``, applied as a placement).
4. **Record it** — every creation and edit is a transaction, so undo/redo and save/
   open work through the engine, not through this script.

The console surface is ``Toy.makeSlab(...)`` (exposed through
``ferrocad_shell.add_namespace``).

The matching Rust half is ``ferrocad_toy``: ``height_grip`` and ``move_gizmo`` are
contributed by the edition's ``main``. Everything that makes them work is generic:
``ferrocad_stage::Manipulator``/``TransformGizmo`` → ``InteractionCommit`` → this
script. Swap this script and the factories for the Part ones and the shell is
unchanged.

The shell imports this module by name (``HostConfig.entry_module``), so it must
re-export every entry point the shell calls.
"""

from __future__ import annotations

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `add_namespace`, `stage_meshes`, `interaction`, ...).
from ferrocad_shell import *  # noqa: F401,F403
from ferrocad_shell import bootstrap as _base_bootstrap

DEFAULT_WIDTH = 40.0
DEFAULT_DEPTH = 40.0
DEFAULT_HEIGHT = 20.0
# RGBA in the 0..1 range; alpha is unused by the toy.
DEFAULT_COLOR = (0.26, 0.55, 0.85, 1.0)

_DISPLAY_MODES = ("Shaded", "ShadedWithEdges", "Wireframe")
_DISPLAY_KEYS = {
    "Shaded": "shaded",
    "ShadedWithEdges": "shaded_edges",
    "Wireframe": "wireframe",
}

# Faces of the box, each counter-clockwise seen from outside. The renderer culls
# back faces, so the winding is the outside.
_FACES = (
    (0, 3, 2, 1),  # bottom (−Z)
    (4, 5, 6, 7),  # top (+Z)
    (0, 1, 5, 4),  # front (−Y)
    (1, 2, 6, 5),  # right (+X)
    (2, 3, 7, 6),  # back (+Y)
    (3, 0, 4, 7),  # left (−X)
)


def _slab_mesh(width: float, depth: float, height: float, placement) -> dict:
    """The world-space tessellation of a slab, placed by `placement`."""
    half_width = width / 2.0
    half_depth = depth / 2.0
    local = [
        (-half_width, -half_depth, 0.0),
        (half_width, -half_depth, 0.0),
        (half_width, half_depth, 0.0),
        (-half_width, half_depth, 0.0),
        (-half_width, -half_depth, height),
        (half_width, -half_depth, height),
        (half_width, half_depth, height),
        (-half_width, half_depth, height),
    ]
    corners = [placement.multVec(FreeCAD.Vector(*corner)) for corner in local]
    positions = []
    triangles = []
    edges = []
    for face in _FACES:
        base = len(positions)
        for index in face:
            corner = corners[index]
            positions.append((corner.x, corner.y, corner.z))
        triangles.extend((base, base + 1, base + 2, base, base + 2, base + 3))
        edges.extend(
            (
                base,
                base + 1,
                base + 1,
                base + 2,
                base + 2,
                base + 3,
                base + 3,
                base,
            )
        )
    return {
        "positions": positions,
        "triangles": triangles,
        # One untagged part: a click selects the whole object (the toy's rule).
        "triangle_parts": [0] * (len(triangles) // 3),
        "edges": edges,
        "segment_parts": [0] * (len(edges) // 2),
        "vertices": [],
        "vertex_parts": [],
        "parts": [],
        # The object's own origin: the triad hugs it and doubles as an origin
        # indicator, and a rotate pivot would turn about it.
        "origin": (placement.Base.x, placement.Base.y, placement.Base.z),
    }


def _display_mesh(obj):
    """The slab mesh for a toy object, or ``None`` for anything else."""
    width = getattr(obj, "Width", None)
    if width is None:
        return None
    try:
        placement = getattr(obj, "Placement", None)
        if placement is None:
            placement = FreeCAD.Placement()
        mesh = _slab_mesh(
            float(width), float(obj.Depth), float(obj.Height), placement
        )
        colors = getattr(obj, "Color", None)
        mesh["color"] = list(colors[0]) if colors else list(DEFAULT_COLOR)
        mode = str(getattr(obj, "DisplayMode", "") or "")
        mesh["display"] = _DISPLAY_KEYS.get(mode, "shaded_edges")
        return mesh
    except Exception:
        return None


def _transaction(obj, name: str, apply):
    """Run `apply` as one named transaction, then recompute."""
    doc = obj.Document
    doc.openTransaction(f"{name} {obj.Name}")
    try:
        apply()
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True}


def _interaction(obj, manipulator, action, element, values):
    """Apply a manipulator commit.

    Two operations reach this workbench:

    * ``toy_height`` / ``set`` — the Rust ``HeightGrip``; ``values[0]`` is the height.
    * ``transform`` / ``apply`` — the generic ``TransformGizmo``; ``values`` are the 16
      row-major elements of the world-space transform and are composed with the
      object's current placement.

    Both are one transaction, so both are undoable.
    """
    if manipulator == "toy_height" and action == "set" and values:
        return _transaction(
            obj, "Height", lambda: setattr(obj, "Height", float(values[0]))
        )
    if manipulator == "transform" and action == "apply" and len(values) == 16:
        delta = FreeCAD.Placement(FreeCAD.Matrix(*values))
        return _transaction(
            obj,
            "Move",
            lambda: setattr(obj, "Placement", delta * obj.Placement),
        )
    return None


FerroCAD.register_display_provider(_display_mesh)
FerroCAD.register_interaction_provider(_interaction)


#: The toy edition's appearance preset (declared, not yet materialised).
#:
#: The toy slab is a mid-blue linear ``(0.26, 0.55, 0.85)``: Rec.709 luminance
#: ``Y = 0.2126*0.26 + 0.7152*0.55 + 0.0722*0.85 = 0.510`` (sRGB ``#8bc4ed``).
#: The shell background is ``#0f1218`` (``Y = 0.006``). The shared gizmo's
#: ``#38bdf8`` is ``Y = 0.440`` at hue 198, i.e. only ``1.14:1`` against the
#: slab and in the same blue/cyan family: it disappears into the object.
#:
#: The preset replaces the stroke with a cool ice-white idle handle and a warm
#: amber active handle, so the gizmo separates by hue *and* by lightness:
#:
#: * inactive ``#e2e8f0``: ``Y = 0.802``, ``1.52:1`` against the slab and
#:   ``15.2:1`` against the background — the lightest ink, so it never mixes
#:   into the blue;
#: * active ``#fbbf24``: ``Y = 0.579`` at hue 43 (a 155° turn from the slab's
#:   ~198), ``11.2:1`` against the background; hover also widens the stroke by
#:   ``hover_scale``, so the state is not carried by colour alone;
#: * axis Z ``#fbbf24`` replaces the blue ``#38bdf8`` (hue 43 at the default
#:   0.65 opacity: ``Y = 0.378`` effective, ``7.7:1`` against the background),
#:   while the conventional red X and green Y stems are unchanged;
#: * grid ``#b8a894`` is a warm neutral (hue 33) at the 0.15 token alpha, so the
#:   ground stays a faint ``Y = 0.066`` and stops echoing the slab's blue.
#:
#: Why not demand WCAG's 3:1 non-text ratio from the stroke alone? Against a
#: mid-luminance object (``Y = 0.510``) and a near-black background
#: (``Y = 0.006``) no single colour can reach 3:1 on both sides: it would need
#: ``Y >= 1.63`` (impossible) or ``Y <= 0.137``, and the dark option is itself
#: only ``3.3:1`` against the background. The shared gizmo **halo** closes that
#: gap instead: its ``#0f172a`` outline (``Y = 0.009``) is ``9.5:1`` against the
#: slab, ``10.7:1`` against the amber stroke and ``14.5:1`` against the ice-white
#: one, so the 1 px boundary carries SC 1.4.11. On the canvas itself the
#: outline is nearly invisible (``1.05:1``), which is fine: the strokes are
#: 11–15:1 there. These are token arithmetic only; the colours were not visually
#: inspected in this environment.
APPEARANCE_PRESETS = {
    "gizmo": {
        "active_color": "#fbbf24",
        "inactive_color": "#e2e8f0",
    },
    "axis": {
        "color_z": "#fbbf24",
    },
    "grid": {
        "color": "#b8a894",
    },
}


def _register_appearance():
    """Declare the toy preset through the GUI facade (idempotent)."""
    for feature, values in APPEARANCE_PRESETS.items():
        FerroCADGui.registerAppearancePreset(feature, values)


_register_appearance()


class _MakeSlabCommand:
    """``Toy_MakeSlab`` — ask for the dimensions, then create the slab."""

    def GetResources(self):
        return {
            "MenuText": "Create Slab",
            "ToolTip": "Add a slab to the document",
            "Accel": "Ctrl+Shift+S",
            "Group": "Toy",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self):
        return {
            "title": "Create Slab",
            "accept": "Create",
            "fields": [
                {
                    "name": "Width",
                    "label": "Width",
                    "kind": "quantity",
                    "value": DEFAULT_WIDTH,
                    "min": 0.1,
                },
                {
                    "name": "Depth",
                    "label": "Depth",
                    "kind": "quantity",
                    "value": DEFAULT_DEPTH,
                    "min": 0.1,
                },
                {
                    "name": "Height",
                    "label": "Height",
                    "kind": "quantity",
                    "value": DEFAULT_HEIGHT,
                    "min": 0.1,
                },
            ],
            "on_accept": _make_from_dialog,
        }


def _make_from_dialog(values):
    """The dialog callback: create the slab the user described.

    ``makeSlab`` owns its transaction, so an accepted dialog is one undo step.
    """
    slab = makeSlab(
        width=float(values.get("Width", DEFAULT_WIDTH)),
        depth=float(values.get("Depth", DEFAULT_DEPTH)),
        height=float(values.get("Height", DEFAULT_HEIGHT)),
    )
    return {"status": f"created {slab.Name}"}


FreeCADGui.addCommand("Toy_MakeSlab", _MakeSlabCommand())


class _PolylineCommand:
    """``Toy_Polyline`` — start the generic polyline tool (transient visuals).

    The tool itself is a Rust contribution (`Contributions::with_tool`); the command
    only names it, which is why a workbench can offer a tool without the shell
    knowing what it draws.
    """

    def GetResources(self):
        return {
            "MenuText": "Polyline",
            "ToolTip": "Draw a polyline in the viewport",
            "Group": "Toy",
            "CmdType": "ForView",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return True

    def Activated(self):
        return {"tool": "toy_polyline"}


FreeCADGui.addCommand("Toy_Polyline", _PolylineCommand())


def _add_slab(doc, width, depth, height, x, y, name):
    """Add the object and its properties; the caller owns the transaction."""
    slab = doc.addObject("App::FeaturePython", name)
    slab.addProperty("App::PropertyLength", "Width", "Toy", "Footprint width")
    slab.addProperty("App::PropertyLength", "Depth", "Toy", "Footprint depth")
    slab.addProperty("App::PropertyLength", "Height", "Toy", "Slab height")
    slab.addProperty("App::PropertyPlacement", "Placement", "Toy", "Placement")
    slab.addProperty(
        "App::PropertyEnumeration", "DisplayMode", "Toy", "How the slab is drawn"
    )
    slab.DisplayMode = list(_DISPLAY_MODES)
    slab.addProperty("App::PropertyColor", "Color", "Toy", "Surface colour")

    slab.Width = float(width)
    slab.Depth = float(depth)
    slab.Height = float(height)
    slab.DisplayMode = "ShadedWithEdges"
    slab.Color = [DEFAULT_COLOR]
    slab.Placement = FreeCAD.Placement(FreeCAD.Vector(float(x), float(y), 0.0))
    return slab


def makeSlab(
    width=DEFAULT_WIDTH,
    depth=DEFAULT_DEPTH,
    height=DEFAULT_HEIGHT,
    x=0.0,
    y=0.0,
    name="Slab",
):
    """Create a toy slab in the active document as one undoable transaction.

    This is the workbench's creation command: it adds a typed object with its
    properties, and the shell picks it up through the display hook on the next sync.
    """
    doc = FreeCAD.ActiveDocument
    if doc is None:
        doc = FreeCAD.newDocument("Unnamed")
    doc.openTransaction("Create Slab")
    try:
        slab = _add_slab(doc, width, depth, height, x, y, name)
        doc.recompute()
        doc.commitTransaction()
    except Exception:
        doc.abortTransaction()
        raise
    return slab


class _Console:
    """The console surface: ``Toy.makeSlab(40, 40, 20)``."""

    makeSlab = staticmethod(makeSlab)


def bootstrap(doc_name: str = "Toy") -> str:
    """The base sample document plus one toy slab, and the ``Toy`` console name."""
    # Materialise the edition preset under the user's persisted values. The
    # settings facades read that store, so every later command sees the preset.
    FerroCADGui.installAppearancePresets()
    payload = _base_bootstrap(doc_name)
    doc = FreeCAD.ActiveDocument
    _add_slab(doc, DEFAULT_WIDTH, DEFAULT_DEPTH, DEFAULT_HEIGHT, 0.0, 0.0, "Slab")
    doc.recompute()
    add_namespace("Toy", _Console)
    return payload
