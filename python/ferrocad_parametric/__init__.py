"""App-shell script for **FerroCAD: Parametric**.

The base app's script (`ferrocad_shell`) is deliberately geometry-free: it knows the
document model and nothing about kernels. This edition links the Part workbench, so
it registers the two **module hooks** Part needs and seeds the demo solid:

* `FreeCAD.register_display_provider` — how a `Part` object is tessellated for the
  viewport;
* `FreeCAD.register_dress_up_provider` — how a fillet/chamfer edit is applied.

The shell imports this module by name (the edition's `HostConfig.entry_module`), so
it must re-export every entry point the shell calls.
"""

from __future__ import annotations

import FreeCAD
import Part  # noqa: F401  (links the kernel; registers Part::Feature/Fillet/Chamfer)

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `stage_meshes`, `dress_up`, ...).
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


def _dress_up(obj, mode, edges, first, second):
    """Create or update a ``Part::Fillet``/``Part::Chamfer`` over `obj`.

    The feature is named ``<object>_<mode>`` and reused on later scrubs. The edit is
    one named transaction (a single undo step), and the document is recomputed before
    returning, so the caller can re-read :func:`stage_meshes` straight away.
    """
    if mode not in ("Fillet", "Chamfer"):
        return None
    doc = obj.Document
    feature_name = f"{obj.Name}_{mode}"
    doc.openTransaction(f"{mode} {obj.Name}")
    try:
        feature = doc.getObject(feature_name)
        if feature is None:
            feature = doc.addObject(f"Part::{mode}", feature_name)
        feature.Base = (obj, edges)
        if mode == "Fillet":
            feature.Radius = float(first)
        else:
            feature.Distance = float(first)
            feature.Distance2 = float(second)
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


FreeCAD.register_display_provider(_display_mesh)
FreeCAD.register_dress_up_provider(_dress_up)


def bootstrap(doc_name: str = "Shell") -> str:
    """The base sample document plus a 20 mm solid for the viewport."""
    payload = _base_bootstrap(doc_name)
    doc = FreeCAD.ActiveDocument
    box = doc.addObject("Part::Feature", "Box")
    box.Shape = Part.makeBox(20.0, 20.0, 20.0)
    doc.recompute()
    return payload
