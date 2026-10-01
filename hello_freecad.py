#!/usr/bin/env python3
"""Headless FreeCAD "hello world" — the milestone's acceptance script.

It uses only the public FreeCAD Python API (the document object model), so the
*same* file runs against:

  * upstream FreeCAD's C++ ``App`` module, and
  * the Rust ``freecad-core`` POC that replaces its bindings.

Run against the Rust core::

    ./run.sh

or manually::

    PYTHONPATH=python python3 hello_freecad.py
"""

import sys

import FreeCAD


def main() -> int:
    doc = FreeCAD.newDocument("HelloWorld")

    obj = doc.addObject("App::FeaturePython", "Box")
    obj.addProperty("App::PropertyString", "Description", "Base", "Human readable description")
    obj.Label = "Hello, FreeCAD"
    obj.Description = "Created by the Rust core"

    doc.recompute()

    print("FreeCAD version :", ".".join(FreeCAD.Version()[:3]))
    print("Active document :", FreeCAD.ActiveDocument.Name)
    print("Document label  :", doc.Label)
    print("Object count    :", doc.CountObjects)
    for o in doc.Objects:
        print("  - %s (%s) label=%r" % (o.Name, o.TypeId, o.Label))
    print("obj.Description :", obj.Description)
    print("Properties      :", obj.PropertiesList)
    print("Documents       :", FreeCAD.listDocuments())

    FreeCAD.closeDocument(doc.Name)
    print("Active after close:", FreeCAD.ActiveDocument)
    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
