#!/usr/bin/env python3
"""The MVP acceptance demo (``docs/mvp-path.md`` section 2).

An ordinary FreeCAD ``App`` script exercised end to end on the Rust core: typed,
dynamic and enumeration properties; an expression dependency; deterministic
recompute; grouping; a transaction with undo; and save/reload. Nothing here is
FerroCAD-specific, so the same script is intended to run on upstream FreeCAD.

Run it against the Rust-backed facade::

    PYTHONPATH=python python3 examples/mvp_workflow.py
"""

import os
import sys
import tempfile

import FreeCAD as App


def main():
    path = os.path.join(tempfile.gettempdir(), "ferrocad_mvp.FCStd")

    doc = App.newDocument("MVP")

    # --- model: typed + dynamic + enumeration properties ------------------
    params = doc.addObject("App::FeaturePython", "Params")
    params.addProperty("App::PropertyLength", "Length", "Base", "Source length")
    params.addProperty("App::PropertyEnumeration", "Mode")
    params.Mode = ["Fast", "Accurate"]  # enumeration values
    params.Mode = "Accurate"
    params.Length = "10 mm"

    derived = doc.addObject("App::FeaturePython", "Derived")
    derived.addProperty("App::PropertyLength", "Result", "Base", "Computed")
    derived.setExpression("Result", "Params.Length * 2")  # dependency edge
    doc.recompute()
    assert str(derived.Result) == "20 mm", derived.Result
    assert derived.State == ["Up-to-date"], derived.State

    # --- workflow: containment, links, undo -------------------------------
    group = doc.addObject("App::DocumentObjectGroup", "Group")
    group.addObject(derived)
    assert derived in group.Group

    doc.openTransaction("bump length")
    params.Length = "15 mm"
    doc.commitTransaction()
    doc.recompute()
    assert str(derived.Result) == "30 mm", derived.Result
    doc.undo()
    doc.recompute()
    assert str(derived.Result) == "20 mm", derived.Result

    # --- persistence: save + reload ---------------------------------------
    doc.saveAs(path)
    App.closeDocument("MVP")
    doc2 = App.open(path)
    assert str(doc2.getObject("Derived").Result) == "20 mm"
    assert doc2.getObject("Params").Mode == "Accurate"
    App.closeDocument(doc2.Name)

    print("MVP workflow OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
