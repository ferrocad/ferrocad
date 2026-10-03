#!/usr/bin/env python3
"""A Python-only script that loads a document file through the public FreeCAD API.

It exercises the only *file* path this milestone supports: FerroCAD's own
document format (JSON under a `.FCStd` name) via ``Document.saveAs`` /
``FreeCAD.open``. No C++ and no Rust-side scripting is involved — this is a
plain workbench-style script.

Run it against the Rust-backed facade::

    PYTHONPATH=python python3 examples/file_roundtrip.py
"""

import os
import sys
import tempfile

import FreeCAD


def build_document():
    doc = FreeCAD.newDocument("RoundTrip")
    box = doc.addObject("App::FeaturePython", "Box")
    box.addProperty("App::PropertyLength", "Height")
    box.Height = "25 mm"
    box.Label = "A loaded box"
    doc.recompute()
    return doc


def main():
    path = os.path.join(tempfile.gettempdir(), "ferrocad_roundtrip.FCStd")

    doc = build_document()
    doc.saveAs(path)
    FreeCAD.closeDocument(doc.Name)
    print("saved   :", path, "(%d bytes)" % os.path.getsize(path))

    # Load the file back through the public API.
    loaded = FreeCAD.open(path)
    print("loaded  :", loaded.Name, "objects:", [o.Name for o in loaded.Objects])
    box = loaded.getObject("Box")
    print("  label :", box.Label)
    print("  Height:", box.Height)
    FreeCAD.closeDocument(loaded.Name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
