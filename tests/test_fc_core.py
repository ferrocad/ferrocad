"""Exercises the `ferrocad` PyO3 bindings over `ferrocad_core` (M3b).

Run: PYTHONPATH=python python3 tests/test_fc_core.py
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import ferrocad as fc  # noqa: E402


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


def test_label_and_default_naming():
    doc = fc.newDocument("Naming")
    first = doc.addObject("App::FeaturePython")
    assert first.Name == "FeaturePython"
    second = doc.addObject("Part::Box")
    assert second.Name == "Box"
    third = doc.addObject("App::FeaturePython")
    assert third.Name == "FeaturePython001"

    # Label is independent of Name.
    first.Label = "First thing"
    assert first.Name == "FeaturePython"
    assert first.Label == "First thing"


def test_document_back_reference():
    doc = fc.newDocument("Backref")
    obj = doc.addObject("App::FeaturePython", "Box")
    assert obj.Document is doc
    assert doc.getObject("Box").Document is doc
    assert obj.Document.Name == "Backref"


def test_dynamic_properties():
    doc = fc.newDocument("Props")
    obj = doc.addObject("App::FeaturePython", "Box")
    obj.addProperty("App::PropertyString", "Description", "Base", "")
    assert obj.PropertiesList == ["Description"]
    assert obj.getTypeIdOfProperty("Description") == "App::PropertyString"
    assert obj.Description == ""
    obj.Description = "hello"
    assert obj.getPropertyByName("Description") == "hello"


def test_remove_object():
    doc = fc.newDocument("Remove")
    doc.addObject("App::FeaturePython", "Box")
    assert doc.CountObjects == 1
    doc.removeObject("Box")
    assert doc.CountObjects == 0

    # Removing a missing object raises ValueError.
    raised = False
    try:
        doc.removeObject("Box")
    except ValueError:
        raised = True
    assert raised, "expected ValueError for missing object"


def test_string_hasher():
    h = fc.StringHasher()
    sid = h.getID("A")
    assert sid.Value >= 1
    assert sid.Data == "A"
    assert sid.isSame(h.getID("A")) is True

    # Wrong types raise (matching upstream StringHasher.py semantics).
    for fn, exc in [
        (lambda: fc.StringHasher(0), TypeError),
        (lambda: h.getID(0), ValueError),
        (lambda: h.isSame(0), TypeError),
        (lambda: sid.isSame(0), TypeError),
    ]:
        try:
            fn()
        except exc:
            pass
        else:
            raise AssertionError("expected %s" % exc.__name__)


def test_int_property_value():
    doc = fc.newDocument("Ints")
    obj = doc.addObject("App::FeaturePython", "Box")
    obj.addProperty("App::PropertyInteger", "Integer", "Base", "")
    obj.Integer = 5
    assert obj.getPropertyByName("Integer") == 5.0


if __name__ == "__main__":
    test_quantity()
    test_document_and_expressions()
    test_transactions()
    test_objects_and_lookup()
    test_label_and_default_naming()
    test_document_back_reference()
    test_dynamic_properties()
    test_remove_object()
    test_string_hasher()
    test_int_property_value()
    print("all ferrocad_py tests passed")
