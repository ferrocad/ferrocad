"""Exercises the `fc` PyO3 bindings over `fc-core` (M3b).

Run: PYTHONPATH=python python3 tests/test_fc_core.py
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import fc  # noqa: E402


def test_quantity():
    q = fc.Quantity("2.5 in")
    assert abs(q.value_mm() - 63.5) < 1e-9
    assert abs(q.value_in("mm") - 63.5) < 1e-9
    assert abs(q.value_in("in") - 2.5) < 1e-9


def test_document_and_expressions():
    doc = fc.newDocument("Demo")
    assert doc.Name == "Demo"

    a = doc.addObject("App::Feature", "A")
    b = doc.addObject("App::Feature", "B")
    c = doc.addObject("App::Feature", "C")

    a.setPropertyByName("Width", 10.0)
    b.setPropertyByName("Height", 5.0)
    c.setExpression("Area", "A.Width * B.Height")

    assert doc.recompute() == 1
    assert c.getPropertyByName("Area") == 50.0
    assert c.PropertiesList == ["Area"]
    assert c.TypeId == "App::Feature"


def test_transactions():
    doc = fc.newDocument("Tx")
    a = doc.addObject("App::Feature", "A")
    a.setPropertyByName("Width", 1.0)

    doc.openTransaction()
    a.setPropertyByName("Width", 2.0)
    doc.abortTransaction()
    assert a.getPropertyByName("Width") == 1.0

    doc.openTransaction()
    a.setPropertyByName("Width", 5.0)
    doc.commitTransaction()
    assert a.getPropertyByName("Width") == 5.0

    assert doc.undo() is True
    assert a.getPropertyByName("Width") == 1.0
    assert doc.redo() is True
    assert a.getPropertyByName("Width") == 5.0


def test_objects_and_lookup():
    doc = fc.newDocument("List")
    doc.addObject("App::Feature", "A")
    doc.addObject("App::Feature", "B")
    assert len(doc.Objects) == 2
    assert doc.getObject("B").Name == "B"
    assert doc.getObject("Nope") is None


if __name__ == "__main__":
    test_quantity()
    test_document_and_expressions()
    test_transactions()
    test_objects_and_lookup()
    print("all fc-python tests passed")
