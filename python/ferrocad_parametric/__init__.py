"""App-shell script for **FerroCAD: Parametric**.

The base app's script (`ferrocad_shell`) is deliberately geometry-free: it knows the
document model and nothing about kernels. This edition links the Part workbench, so
it registers the two **module hooks** Part needs and seeds the demo solid:

* `FerroCAD.register_display_provider` — how a `Part` object is tessellated for the
  viewport;
* `FerroCAD.register_interaction_provider` — what a manipulator commit means (the edge
  dress-up arc's `dress_up` / `fillet` / `chamfer`).

The Rust half of the same contribution (the scrubber itself) is
`ferrocad_part_gui::fillet_manipulator`, which the edition's `main` registers with
the shell. The shell hosts it through the generic `Manipulator` seam, so it never
names Part.

The shell imports this module by name (the edition's `HostConfig.entry_module`), so
it must re-export every entry point the shell calls.
"""

from __future__ import annotations

import FreeCAD
import FerroCAD
import Part  # noqa: F401  (links the kernel; registers Part::Feature/Fillet/Chamfer)

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `stage_meshes`, `interaction`, ...).
from ferrocad_shell import *  # noqa: F401,F403
from ferrocad_shell import bootstrap as _base_bootstrap


def _display_mesh(obj):
    """Tessellate a shape-bearing object (the Part display hook)."""
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return None
    try:
        if shape.isNull():
            return None
        return shape.mesh()
    except Exception:
        return None


def _interaction(obj, manipulator, action, element, values):
    """Apply a manipulator commit: the Part edge dress-up (fillet/chamfer).

    The manipulator names its operation and orders the numbers; this hook owns what
    that means for Part. The feature is named ``<object>_<mode>`` and reused on later
    scrubs. The edit is one named transaction (a single undo step), and the document
    is recomputed before returning, so the caller can re-read :func:`stage_meshes`
    straight away.
    """
    if manipulator != "dress_up":
        return None
    mode = {"fillet": "Fillet", "chamfer": "Chamfer"}.get(action)
    if mode is None:
        return None
    edges = [element] if element else []
    first = float(values[0]) if values else 0.0
    second = float(values[1]) if len(values) > 1 else 0.0

    doc = obj.Document
    feature_name = f"{obj.Name}_{mode}"
    doc.openTransaction(f"{mode} {obj.Name}")
    try:
        feature = doc.getObject(feature_name)
        if feature is None:
            feature = doc.addObject(f"Part::{mode}", feature_name)
        feature.Base = (obj, edges)
        if mode == "Fillet":
            feature.Radius = first
        else:
            feature.Distance = first
            feature.Distance2 = second
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


FerroCAD.register_display_provider(_display_mesh)
FerroCAD.register_interaction_provider(_interaction)


def bootstrap(doc_name: str = "Shell") -> str:
    """The base sample document plus a 20 mm solid for the viewport."""
    payload = _base_bootstrap(doc_name)
    doc = FreeCAD.ActiveDocument
    box = doc.addObject("Part::Feature", "Box")
    box.Shape = Part.makeBox(20.0, 20.0, 20.0)
    doc.recompute()
    return payload
